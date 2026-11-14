//! Screen recording on Windows (US-5.1). Frames stay on the GPU: each new capture frame
//! is cropped into a texture of its own, which the H.264 encoder reads directly. The
//! US-1.2 spike's CPU copy dropped 4–5% of frames (docs/spikes/us-1.2-windows.md).

use crate::audio_mix::RATE;
use crate::windows::{platform_err, recording_source, MonitorCapture, RecordingSource};
use crate::windows_audio::Chunk;
use crate::windows_bubble::BubblePainter;
use crate::windows_mp4::Mp4Writer;
use cb_core::{
    create_unique, AudioSources, CaptureError, CaptureResult, CaptureTarget, Frame, Recording,
    StreamHandle,
};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering::Relaxed};
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime};
use windows::Win32::Graphics::Direct3D11::{
    ID3D11Texture2D, D3D11_BIND_RENDER_TARGET, D3D11_BIND_SHADER_RESOURCE, D3D11_BOX,
    D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT,
};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC};

const FIRST_FRAME_TIMEOUT: Duration = Duration::from_secs(1);

/// Where a still taken during the recording goes (US-5.5).
type StillDone = Box<dyn FnOnce(CaptureResult<Frame>) + Send>;

/// Shared by the handle, the video loop and the audio thread.
pub(crate) struct Control {
    pub stop: AtomicBool,
    /// The microphone's level as `f32` bits, set by the audio thread; negative without one.
    pub mic_level: AtomicU32,
    timeline: Mutex<Timeline>,
    /// Stills asked for, taken by the video loop from its newest frame.
    stills: Mutex<Vec<StillDone>>,
}

/// Recorded time: wall time since the first frame, less the pauses. Video slots and
/// audio packets are both placed on it, which keeps them in sync (US-5.3).
struct Timeline {
    /// Time zero; `None` until the first frame is in.
    origin: Option<Instant>,
    paused_at: Option<Instant>,
}

impl Control {
    fn new() -> Self {
        Self {
            stop: AtomicBool::new(false),
            mic_level: AtomicU32::new((-1.0f32).to_bits()),
            timeline: Mutex::new(Timeline {
                origin: None,
                paused_at: None,
            }),
            stills: Mutex::new(Vec::new()),
        }
    }

    fn take_stills(&self) -> Vec<StillDone> {
        std::mem::take(&mut *self.stills.lock().unwrap_or_else(|e| e.into_inner()))
    }

    fn timeline(&self) -> MutexGuard<'_, Timeline> {
        self.timeline.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Time zero is now: the first frame is in. Returns its wall-clock time.
    fn start(&self) -> SystemTime {
        self.timeline().origin = Some(Instant::now());
        SystemTime::now()
    }

    /// See [`StreamHandle::media_time`].
    fn media_time(&self, at: Instant) -> Option<Duration> {
        let timeline = self.timeline();
        media_time(timeline.origin?, timeline.paused_at, at)
    }

    /// The instant of time zero, or `None` while paused.
    fn origin(&self) -> Option<Instant> {
        let timeline = self.timeline();
        timeline.origin.filter(|_| timeline.paused_at.is_none())
    }

    /// The audio frame captured at `at`; `None` before the start or during the current
    /// pause. Negative for a packet captured just before time zero.
    pub fn frames_at(&self, at: Instant) -> Option<i64> {
        let timeline = self.timeline();
        if timeline.paused_at.is_some_and(|paused| at >= paused) {
            return None;
        }
        Some(frames_between(timeline.origin?, at))
    }

    /// The audio frame of now (while paused, of the pause); `None` before the start.
    pub fn frames_now(&self) -> Option<i64> {
        let timeline = self.timeline();
        Some(frames_between(
            timeline.origin?,
            timeline.paused_at.unwrap_or_else(Instant::now),
        ))
    }
}

/// `at` on a timeline that began at `origin` (moved later by each finished pause) and is
/// paused since `paused_at`, if it is.
fn media_time(origin: Instant, paused_at: Option<Instant>, at: Instant) -> Option<Duration> {
    let at = paused_at.map_or(at, |paused| at.min(paused));
    Some(at.saturating_duration_since(origin))
}

fn frames_between(from: Instant, to: Instant) -> i64 {
    let nanos = match to.checked_duration_since(from) {
        Some(after) => after.as_nanos() as i128,
        None => -((from - to).as_nanos() as i128),
    };
    (nanos * i128::from(RATE) / 1_000_000_000) as i64
}

struct Handle(Arc<Control>);

impl StreamHandle for Handle {
    fn pause(&mut self) {
        self.0.timeline().paused_at.get_or_insert_with(Instant::now);
    }
    fn resume(&mut self) {
        let mut timeline = self.0.timeline();
        if let (Some(paused), Some(origin)) = (timeline.paused_at.take(), &mut timeline.origin) {
            // The pause leaves no gap in the video or the sound.
            *origin += paused.elapsed();
        }
    }
    fn mic_level(&self) -> Option<f32> {
        Some(f32::from_bits(self.0.mic_level.load(Relaxed))).filter(|level| *level >= 0.0)
    }
    fn media_time(&self, at: Instant) -> Option<Duration> {
        self.0.media_time(at)
    }
    fn still(&self, done: StillDone) {
        self.0
            .stills
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(done);
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
    audio: AudioSources,
    path: PathBuf,
    done: Box<dyn FnOnce(CaptureResult<Recording>) + Send>,
) -> Box<dyn StreamHandle> {
    let control = Arc::new(Control::new());
    let shared = control.clone();
    std::thread::Builder::new()
        .name("recording".into())
        .spawn(move || done(record(&target, fps.max(1), audio, path, shared)))
        .expect("spawn recording thread");
    Box::new(Handle(control))
}

/// Pay the one-off costs of a first recording ahead of time: the GPU device, the
/// primary monitor's capture item, Media Foundation, the hardware encoder and the AAC
/// encoder (~400 ms on a cold process, against the 300 ms start budget of US-5.1).
pub(crate) fn warm_up() {
    let started = Instant::now();
    let ready = crate::windows::shared_gpu().is_ok()
        && crate::windows_mp4::ensure_mf().is_ok()
        && crate::windows::resolve_display(cb_core::DisplayId(0))
            .is_ok_and(|monitor| crate::windows::capture_item(monitor).is_ok());
    // Loading the hardware encoder is the slowest part: open one on a scratch file.
    let scratch = std::env::temp_dir().join(format!("takebox-{}.mp4", std::process::id()));
    let encoder = crate::windows::shared_gpu()
        .and_then(|gpu| Mp4Writer::new(&scratch, 64, 64, 30, Some(&gpu.device), true))
        .map(|mp4| mp4.encoder_name());
    let _ = std::fs::remove_file(&scratch);
    tracing::debug!(
        ready,
        ?encoder,
        ms = started.elapsed().as_millis() as u64,
        "recording warmed up"
    );
}

/// All of a `width` x `height` texture.
fn whole(width: u32, height: u32) -> D3D11_BOX {
    D3D11_BOX {
        left: 0,
        top: 0,
        front: 0,
        right: width,
        bottom: height,
        back: 1,
    }
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

/// Hand `screen`, the newest frame (`width` x `height`), to each still asked for. Only
/// the GPU copy happens here; the colour conversion runs on a thread of its own, so the
/// video loop keeps its pace.
fn serve_stills(
    capture: &MonitorCapture,
    screen: &ID3D11Texture2D,
    (width, height): (u32, u32),
    stills: Vec<StillDone>,
) {
    if stills.is_empty() {
        return;
    }
    let captured_at = std::time::SystemTime::now();
    let bgra = capture
        .read_bgra(screen, width, height)
        .map_err(|e| platform_err(e).to_string());
    std::thread::spawn(move || {
        let frame = bgra.map(|bgra| {
            let (w, h) = (width as usize, height as usize);
            Frame {
                width,
                height,
                rgba: crate::windows::bgra_to_rgba(&bgra, w * 4, 0, 0, w, h),
                captured_at,
            }
        });
        for done in stills {
            done(frame.clone().map_err(CaptureError::Platform));
        }
    });
}

fn record(
    target: &CaptureTarget,
    fps: u32,
    audio: AudioSources,
    path: PathBuf,
    control: Arc<Control>,
) -> CaptureResult<Recording> {
    let asked = Instant::now();
    // However this returns, the audio thread ends too.
    struct StopOnReturn(Arc<Control>);
    impl Drop for StopOnReturn {
        fn drop(&mut self) {
            self.0.stop.store(true, Relaxed);
        }
    }
    let _stop = StopOnReturn(control.clone());
    // The sound devices open while the screen capture starts.
    let sound = audio
        .any()
        .then(|| crate::windows_audio::start(audio, control.clone()));
    let RecordingSource {
        capture,
        area: (x, y, w, h),
        origin,
    } = recording_source(target)?;
    // The area's top-left corner on the desktop: where the bubble is drawn from.
    let area_origin = (origin.0 + x as i32, origin.1 + y as i32);
    let mut bubble_painter = None;
    // H.264 needs even dimensions; a trailing odd row or column is dropped.
    let (width, height) = (w & !1, h & !1);
    if width == 0 || height == 0 {
        return Err(CaptureError::Platform("too small to record".into()));
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(io_err)?;
    }
    let (_, path) = create_unique(&path).map_err(io_err)?;
    let mp4 = Mp4Writer::new(
        &path,
        width,
        height,
        fps,
        Some(&capture.device),
        sound.is_some(),
    )
    .map_err(platform_err)?;
    let area = D3D11_BOX {
        left: x,
        top: y,
        front: 0,
        right: x + width,
        bottom: y + height,
        back: 1,
    };

    let deadline = Instant::now() + FIRST_FRAME_TIMEOUT;
    // The newest screen picture, without the bubble: repeated while the screen is
    // still, with the camera drawn afresh on each copy.
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
        sound = sound.is_some(),
        "recording started"
    );

    // Frame slots on a fixed fps timeline; the newest image fills each one, repeated
    // while the screen is still. A slot's time in the file is fixed, so a late one is
    // still written and the next ones catch up. Only falling behind by more than
    // MAX_LAG_SLOTS (encoder overload, not scheduler jitter) skips slots: those are drops.
    const MAX_LAG_SLOTS: u64 = 4;
    let slot_time = |slot: u64| Duration::from_secs(slot) / fps;
    let started_at = control.start();
    let (mut slots, mut dropped) = (0u64, 0u64);
    let mut failure = None;
    let write_sound = |chunks: &Receiver<Chunk>| {
        chunks
            .try_iter()
            .try_for_each(|(first, pcm)| mp4.write_audio(&pcm, first))
            .map_err(platform_err)
    };
    while !control.stop.load(Relaxed) {
        // Paused too: the screen is still there to capture.
        serve_stills(&capture, &last, (width, height), control.take_stills());
        if let Some(Err(e)) = sound.as_ref().map(|s| write_sound(&s.chunks)) {
            failure = Some(e);
            break;
        }
        let Some(origin) = control.origin() else {
            std::thread::sleep(Duration::from_millis(10));
            continue;
        };
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
            .and_then(|screen| {
                let out = match crate::windows_camera::bubble() {
                    Some((bubble, shape, picture)) => {
                        let out = copy_area(&capture, &screen, &whole(width, height))?;
                        let painter = match &mut bubble_painter {
                            Some(painter) => painter,
                            None => bubble_painter
                                .insert(BubblePainter::new(&capture.device).map_err(platform_err)?),
                        };
                        let place = cb_core::Rect {
                            x: bubble.x - area_origin.0,
                            y: bubble.y - area_origin.1,
                            ..bubble
                        };
                        painter
                            .paint(
                                &capture.device,
                                &capture.context,
                                &out,
                                place,
                                shape.corner(),
                                &picture,
                            )
                            .map_err(platform_err)?;
                        out
                    }
                    None => screen.clone(),
                };
                mp4.write_texture(&out, slots).map_err(platform_err)?;
                Ok(screen)
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
    // Already raised by the handle, unless writing failed: either way the audio thread
    // hands over its last chunk and ends.
    control.stop.store(true, Relaxed);
    if let Some(sound) = sound {
        if let Err(e) = write_sound(&sound.join()) {
            failure.get_or_insert(e);
        }
    }
    serve_stills(&capture, &last, (width, height), control.take_stills());
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
        started_at,
    };
    tracing::info!(
        seconds = recording.duration.as_secs_f32(),
        frames = recording.frames,
        dropped = recording.dropped,
        "recording finished"
    );
    Ok(recording)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Audio packets land where the video is: nothing before the first frame, nothing
    /// during a pause, and no jump in time across it.
    #[test]
    fn a_pause_leaves_no_gap_in_the_timeline() {
        let control = Arc::new(Control::new());
        assert_eq!(control.frames_now(), None);
        assert_eq!(control.frames_at(Instant::now()), None);
        control.start();
        let mut handle = Handle(control.clone());
        std::thread::sleep(Duration::from_millis(50));
        handle.pause();
        let paused = control.frames_now().unwrap();
        assert!(paused >= i64::from(RATE) / 20, "{paused}");
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(control.frames_now(), Some(paused));
        assert_eq!(control.frames_at(Instant::now()), None);
        handle.resume();
        let resumed = control.frames_now().unwrap();
        assert!(
            resumed - paused < i64::from(RATE) / 100,
            "{resumed} vs {paused}"
        );
    }

    /// The media clock (DR-2) agrees with the audio timeline: it stands still during a
    /// pause and goes on from there.
    #[test]
    fn media_time_leaves_out_pauses() {
        let t0 = Instant::now();
        let ms = Duration::from_millis;
        assert_eq!(media_time(t0, None, t0 + ms(40)), Some(ms(40)));
        assert_eq!(media_time(t0, None, t0), Some(ms(0)));
        // Paused at 100 ms: later instants stay at 100.
        assert_eq!(
            media_time(t0, Some(t0 + ms(100)), t0 + ms(400)),
            Some(ms(100))
        );
        assert_eq!(
            media_time(t0, Some(t0 + ms(100)), t0 + ms(60)),
            Some(ms(60))
        );
        // Resumed after a 300 ms pause: the origin moved by 300.
        assert_eq!(media_time(t0 + ms(300), None, t0 + ms(450)), Some(ms(150)));

        let control = Control::new();
        assert_eq!(control.media_time(Instant::now()), None);
        let wall = control.start();
        assert!(SystemTime::now().duration_since(wall).unwrap() < ms(100));
        assert!(control.media_time(Instant::now()).unwrap() < ms(100));
    }
}
