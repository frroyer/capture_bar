//! Screen recording on Windows (US-5.1). Frames stay on the GPU: each new capture frame
//! is cropped into a texture of its own, which the H.264 encoder reads directly. The
//! US-1.2 spike's CPU copy dropped 4–5% of frames (docs/spikes/us-1.2-windows.md).

use crate::windows::{platform_err, recording_source, MonitorCapture};
use crate::windows_mp4::Mp4Writer;
use cb_core::{create_unique, CaptureError, CaptureResult, CaptureTarget, Recording, StreamHandle};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::sync::Arc;
use std::time::{Duration, Instant};
use windows::Win32::Graphics::Direct3D11::{
    ID3D11Texture2D, D3D11_BIND_RENDER_TARGET, D3D11_BIND_SHADER_RESOURCE, D3D11_BOX,
    D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT,
};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC};

const FIRST_FRAME_TIMEOUT: Duration = Duration::from_secs(1);

#[derive(Default)]
struct Control {
    stop: AtomicBool,
    paused: AtomicBool,
}

struct Handle(Arc<Control>);

impl StreamHandle for Handle {
    fn pause(&mut self) {
        self.0.paused.store(true, Relaxed);
    }
    fn resume(&mut self) {
        self.0.paused.store(false, Relaxed);
    }
    fn stop(self: Box<Self>) {}
}

impl Drop for Handle {
    fn drop(&mut self) {
        self.0.stop.store(true, Relaxed);
    }
}

pub(crate) fn start(
    target: CaptureTarget,
    fps: u32,
    path: PathBuf,
    done: Box<dyn FnOnce(CaptureResult<Recording>) + Send>,
) -> Box<dyn StreamHandle> {
    let control = Arc::new(Control::default());
    let shared = control.clone();
    std::thread::Builder::new()
        .name("recording".into())
        .spawn(move || done(record(&target, fps.max(1), path, &shared)))
        .expect("spawn recording thread");
    Box::new(Handle(control))
}

/// Pay the one-off costs of a first recording ahead of time: the GPU device, the
/// primary monitor's capture item, Media Foundation and the hardware encoder
/// (~400 ms on a cold process, against the 300 ms start budget of US-5.1).
pub(crate) fn warm_up() {
    let started = Instant::now();
    let ready = crate::windows::shared_gpu().is_ok()
        && crate::windows_mp4::ensure_mf().is_ok()
        && crate::windows::resolve_display(cb_core::DisplayId(0))
            .is_ok_and(|monitor| crate::windows::capture_item(monitor).is_ok());
    // Loading the hardware encoder is the slowest part: open one on a scratch file.
    let scratch = std::env::temp_dir().join(format!("capture-bar-{}.mp4", std::process::id()));
    let encoder = crate::windows::shared_gpu()
        .and_then(|gpu| Mp4Writer::new(&scratch, 64, 64, 30, Some(&gpu.device)))
        .map(|mp4| mp4.encoder_name());
    let _ = std::fs::remove_file(&scratch);
    tracing::debug!(
        ready,
        ?encoder,
        ms = started.elapsed().as_millis() as u64,
        "recording warmed up"
    );
}

fn io_err(e: std::io::Error) -> CaptureError {
    CaptureError::Platform(e.to_string())
}

/// A copy of `area` of `from` in a new texture, which the encoder can keep reading
/// while the capture pool reuses `from`.
fn copy_area(
    capture: &MonitorCapture,
    from: &ID3D11Texture2D,
    area: &D3D11_BOX,
) -> CaptureResult<ID3D11Texture2D> {
    let desc = D3D11_TEXTURE2D_DESC {
        Width: area.right - area.left,
        Height: area.bottom - area.top,
        MipLevels: 1,
        ArraySize: 1,
        Format: DXGI_FORMAT_B8G8R8A8_UNORM,
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        Usage: D3D11_USAGE_DEFAULT,
        // The encoder's colour converter reads it as a shader input.
        BindFlags: (D3D11_BIND_RENDER_TARGET.0 | D3D11_BIND_SHADER_RESOURCE.0) as u32,
        CPUAccessFlags: 0,
        MiscFlags: 0,
    };
    let mut texture: Option<ID3D11Texture2D> = None;
    // SAFETY: `desc` and the out-pointer are valid for the call; `area` lies inside `from`
    // and both textures share the device and format.
    // chisle: a texture per new frame, no pool; add one if profiling shows the allocations.
    unsafe {
        capture
            .device
            .CreateTexture2D(&desc, None, Some(&mut texture))
            .map_err(platform_err)?;
        let texture = texture.expect("CreateTexture2D succeeded without a texture");
        capture
            .context
            .CopySubresourceRegion(&texture, 0, 0, 0, 0, from, 0, Some(area));
        Ok(texture)
    }
}

fn record(
    target: &CaptureTarget,
    fps: u32,
    path: PathBuf,
    control: &Control,
) -> CaptureResult<Recording> {
    let asked = Instant::now();
    let (capture, (x, y, w, h)) = recording_source(target)?;
    // H.264 needs even dimensions; a trailing odd row or column is dropped.
    let (width, height) = (w & !1, h & !1);
    if width == 0 || height == 0 {
        return Err(CaptureError::Platform("too small to record".into()));
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(io_err)?;
    }
    let (_, path) = create_unique(&path).map_err(io_err)?;
    let mp4 =
        Mp4Writer::new(&path, width, height, fps, Some(&capture.device)).map_err(platform_err)?;
    let area = D3D11_BOX {
        left: x,
        top: y,
        front: 0,
        right: x + width,
        bottom: y + height,
        back: 1,
    };

    let deadline = Instant::now() + FIRST_FRAME_TIMEOUT;
    let mut last = loop {
        if let Some(frame) = capture.latest_texture().map_err(platform_err)? {
            break copy_area(&capture, &frame, &area)?;
        }
        if Instant::now() > deadline {
            return Err(CaptureError::Platform("no frame within 1 s".into()));
        }
        std::thread::sleep(Duration::from_millis(1));
    };
    let start_latency = asked.elapsed();
    tracing::info!(
        latency_ms = start_latency.as_millis() as u64,
        encoder = %mp4.encoder_name(),
        width,
        height,
        fps,
        "recording started"
    );

    // Frame slots on a fixed fps timeline; the newest image fills each one, repeated
    // while the screen is still. A slot's time in the file is fixed, so a late one is
    // still written and the next ones catch up. Only falling behind by more than
    // MAX_LAG_SLOTS (encoder overload, not scheduler jitter) skips slots: those are drops.
    const MAX_LAG_SLOTS: u64 = 4;
    let slot_time = |slot: u64| Duration::from_secs(slot) / fps;
    let mut origin = Instant::now();
    let (mut slots, mut dropped) = (0u64, 0u64);
    let mut failure = None;
    while !control.stop.load(Relaxed) {
        if control.paused.load(Relaxed) {
            let paused_at = Instant::now();
            while control.paused.load(Relaxed) && !control.stop.load(Relaxed) {
                std::thread::sleep(Duration::from_millis(10));
            }
            // The pause leaves no gap in the video.
            origin += paused_at.elapsed();
            continue;
        }
        let due = origin + slot_time(slots);
        let now = Instant::now();
        if now < due {
            std::thread::sleep(due - now);
        } else if now - due >= slot_time(MAX_LAG_SLOTS) {
            dropped += 1;
            slots += 1;
            continue;
        }
        let written = capture
            .latest_texture()
            .map_err(platform_err)
            .and_then(|frame| match frame {
                Some(frame) => copy_area(&capture, &frame, &area),
                None => Ok(last.clone()),
            })
            .and_then(|frame| {
                mp4.write_texture(&frame, slots).map_err(platform_err)?;
                Ok(frame)
            });
        match written {
            Ok(frame) => last = frame,
            Err(e) => {
                failure = Some(e);
                break;
            }
        }
        slots += 1;
    }
    let finished = mp4.finish().map_err(platform_err);
    if let Some(e) = failure {
        return Err(e);
    }
    finished?;

    let recording = Recording {
        path,
        width,
        height,
        duration: slot_time(slots),
        frames: slots - dropped,
        dropped,
        start_latency,
    };
    tracing::info!(
        seconds = recording.duration.as_secs_f32(),
        frames = recording.frames,
        dropped = recording.dropped,
        "recording finished"
    );
    Ok(recording)
}
