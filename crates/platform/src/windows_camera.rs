//! The webcam on Windows (US-5.4): the first video capture device, read through a Media
//! Foundation source reader on a thread of its own. The newest picture is kept for the
//! bar's bubble and for recordings, which draw it into the video.

use crate::windows::platform_err;
use crate::windows_mp4::ensure_mf;
use cb_core::{CaptureError, CaptureResult, Rect};
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::sync::{Arc, Mutex, MutexGuard};
use windows::core::Result as WinResult;
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::CoTaskMemFree;

/// Wider pictures cost more to convert and upload than a bubble shows.
const MAX_WIDTH: u32 = 1280;

/// One picture: top-down rows of BGRA, `width * 4` bytes each.
pub(crate) struct CamFrame {
    pub width: u32,
    pub height: u32,
    pub bgra: Vec<u8>,
    /// Counts up with each picture: tells a new one from the last one seen.
    pub seq: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum State {
    Off,
    Starting,
    Running,
    Failed(String),
}

struct Camera {
    state: State,
    /// Raised to end the running camera thread.
    stop: Option<Arc<AtomicBool>>,
    latest: Option<Arc<CamFrame>>,
    /// Where recordings draw the bubble, in global physical pixels.
    bubble: Option<Rect>,
}

static CAMERA: Mutex<Camera> = Mutex::new(Camera {
    state: State::Off,
    stop: None,
    latest: None,
    bubble: None,
});

fn lock() -> MutexGuard<'static, Camera> {
    CAMERA.lock().unwrap_or_else(|e| e.into_inner())
}

/// Start or stop the camera. Returns at once: opening a camera can take a second.
pub(crate) fn set_on(on: bool) {
    let mut camera = lock();
    if !on {
        if let Some(stop) = camera.stop.take() {
            stop.store(true, Relaxed);
        }
        camera.state = State::Off;
        camera.latest = None;
        return;
    }
    if matches!(camera.state, State::Starting | State::Running) {
        return;
    }
    let stop = Arc::new(AtomicBool::new(false));
    camera.stop = Some(stop.clone());
    camera.state = State::Starting;
    camera.latest = None;
    std::thread::Builder::new()
        .name("camera".into())
        .spawn(move || {
            let result = run(&stop);
            let mut camera = lock();
            // A newer start owns the state now.
            if camera.stop.as_ref().is_some_and(|s| Arc::ptr_eq(s, &stop)) {
                camera.latest = None;
                camera.state = match result {
                    Err(e) if !stop.load(Relaxed) => State::Failed(e.to_string()),
                    _ => State::Off,
                };
            }
        })
        .expect("spawn camera thread");
}

pub(crate) fn state() -> State {
    lock().state.clone()
}

pub(crate) fn latest() -> Option<Arc<CamFrame>> {
    lock().latest.clone()
}

pub(crate) fn set_bubble(rect: Option<Rect>) {
    lock().bubble = rect;
}

/// Where to draw the camera in recordings, with the picture to draw.
pub(crate) fn bubble() -> Option<(Rect, Arc<CamFrame>)> {
    let camera = lock();
    Some((camera.bubble?, camera.latest.clone()?))
}

fn run(stop: &AtomicBool) -> CaptureResult<()> {
    let reader = open().map_err(|e| match e {
        Some(e) => platform_err(e),
        None => CaptureError::Platform("no camera found".into()),
    })?;
    let stream = MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32;
    let mut seq = 0;
    while !stop.load(Relaxed) {
        let (mut flags, mut sample) = (0u32, None);
        // SAFETY: out-pointers are valid; blocks until the next picture.
        unsafe {
            reader
                .ReadSample(stream, 0, None, Some(&mut flags), None, Some(&mut sample))
                .map_err(platform_err)?;
        }
        if flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 {
            return Err(CaptureError::Platform("the camera stopped".into()));
        }
        let Some(sample) = sample else { continue };
        let frame = to_frame(&reader, &sample, seq).map_err(platform_err)?;
        seq += 1;
        let mut camera = lock();
        if stop.load(Relaxed) {
            break;
        }
        camera.latest = Some(Arc::new(frame));
        camera.state = State::Running;
    }
    Ok(())
}

/// A reader on the first camera, giving RGB32 at the largest size up to [`MAX_WIDTH`].
/// `Err(None)` when there is no camera.
fn open() -> Result<IMFSourceReader, Option<windows::core::Error>> {
    ensure_mf()?;
    let stream = MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32;
    // SAFETY: Media Foundation calls below only read arguments valid for each call;
    // the device array and its entries are released here.
    unsafe {
        let mut attrs = None;
        MFCreateAttributes(&mut attrs, 1)?;
        let attrs: IMFAttributes = attrs.expect("MFCreateAttributes without attributes");
        attrs.SetGUID(
            &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE,
            &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_GUID,
        )?;
        let (mut devices, mut count) = (std::ptr::null_mut(), 0);
        MFEnumDeviceSources(&attrs, &mut devices, &mut count)?;
        let mut all: Vec<IMFActivate> = std::slice::from_raw_parts_mut(devices, count as usize)
            .iter_mut()
            .filter_map(Option::take)
            .collect();
        CoTaskMemFree(Some(devices as *const _));
        if all.is_empty() {
            return Err(None);
        }
        let source: IMFMediaSource = all.swap_remove(0).ActivateObject()?;

        let mut options = None;
        MFCreateAttributes(&mut options, 1)?;
        let options: IMFAttributes = options.expect("MFCreateAttributes without attributes");
        // Lets the reader convert the camera's YUV to RGB.
        options.SetUINT32(&MF_SOURCE_READER_ENABLE_VIDEO_PROCESSING, 1)?;
        let reader = MFCreateSourceReaderFromMediaSource(&source, &options)?;

        // The camera's own sizes: the largest that fits, at video frame rates.
        let mut best: Option<(u32, IMFMediaType)> = None;
        for index in 0.. {
            let Ok(native) = reader.GetNativeMediaType(stream, index) else {
                break;
            };
            let width = (native.GetUINT64(&MF_MT_FRAME_SIZE).unwrap_or(0) >> 32) as u32;
            let rate = native.GetUINT64(&MF_MT_FRAME_RATE).unwrap_or(0);
            let fps = (rate >> 32) as f32 / (rate & 0xffff_ffff).max(1) as f32;
            if width <= MAX_WIDTH && fps >= 24.0 && best.as_ref().is_none_or(|(w, _)| width > *w) {
                best = Some((width, native));
            }
        }
        if let Some((_, native)) = best {
            reader.SetCurrentMediaType(stream, None, &native)?;
        }
        let rgb = MFCreateMediaType()?;
        rgb.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
        rgb.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_RGB32)?;
        reader.SetCurrentMediaType(stream, None, &rgb)?;
        Ok(reader)
    }
}

/// The picture in `sample` as top-down BGRA.
fn to_frame(reader: &IMFSourceReader, sample: &IMFSample, seq: u64) -> WinResult<CamFrame> {
    let stream = MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32;
    // SAFETY: the locked slice is only read before Unlock.
    unsafe {
        let kind = reader.GetCurrentMediaType(stream)?;
        let size = kind.GetUINT64(&MF_MT_FRAME_SIZE)?;
        let (width, height) = ((size >> 32) as u32, (size & 0xffff_ffff) as u32);
        // Negative means bottom-up rows, as RGB32 from the video processor often is.
        let stride = kind
            .GetUINT32(&MF_MT_DEFAULT_STRIDE)
            .map_or(width as i32 * 4, |s| s as i32);
        let pitch = stride.unsigned_abs() as usize;
        let row = width as usize * 4;
        let buffer = sample.ConvertToContiguousBuffer()?;
        let (mut data, mut len) = (std::ptr::null_mut(), 0);
        buffer.Lock(&mut data, None, Some(&mut len))?;
        let bytes = std::slice::from_raw_parts(data as *const u8, len as usize);
        let mut bgra = Vec::with_capacity(row * height as usize);
        if pitch >= row && bytes.len() >= pitch * height as usize {
            for y in 0..height as usize {
                let from = if stride < 0 {
                    height as usize - 1 - y
                } else {
                    y
                } * pitch;
                bgra.extend_from_slice(&bytes[from..from + row]);
            }
        }
        buffer.Unlock()?;
        if bgra.is_empty() {
            return Err(windows::core::Error::from(
                windows::Win32::Foundation::E_FAIL,
            ));
        }
        Ok(CamFrame {
            width,
            height,
            bgra,
            seq,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    #[ignore = "needs a camera"]
    fn the_camera_gives_pictures_and_stops() {
        set_on(true);
        let deadline = Instant::now() + Duration::from_secs(10);
        let first = loop {
            if let Some(frame) = latest() {
                break frame.seq;
            }
            assert!(
                !matches!(state(), State::Failed(_)),
                "camera failed: {:?}",
                state()
            );
            assert!(Instant::now() < deadline, "no picture within 10 s");
            std::thread::sleep(Duration::from_millis(20));
        };
        // Pictures keep coming; a later one has had time to expose.
        std::thread::sleep(Duration::from_secs(2));
        let frame = latest().expect("still running");
        assert!(frame.seq > first, "new pictures keep coming");
        let brightest = frame
            .bgra
            .as_chunks::<4>()
            .0
            .iter()
            .flat_map(|p| &p[..3])
            .max();
        eprintln!(
            "camera picture {} x {}, #{}, brightest {brightest:?}",
            frame.width, frame.height, frame.seq
        );
        assert_eq!(state(), State::Running);
        assert_eq!(frame.bgra.len(), (frame.width * frame.height * 4) as usize);
        set_on(false);
        assert_eq!(state(), State::Off);
        assert!(latest().is_none());
    }
}
