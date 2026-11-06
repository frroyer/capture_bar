//! US-1.2 spike: Windows.Graphics.Capture -> CPU copy -> Media Foundation H.264 MP4.
//! The production pipeline (US-5.1) will replace the CPU copy with GPU frames if
//! the numbers from this spike require it. See docs/spikes/us-1.2-windows.md.

use crate::windows::{ensure_mta, platform_err, resolve_display, MonitorCapture};
use crate::SpikeReport;
use cb_core::{CaptureError, CaptureResult, DisplayId};
use std::path::Path;
use std::sync::OnceLock;
use std::time::{Duration, Instant};
use windows::core::{Result as WinResult, GUID, HSTRING, PWSTR};
use windows::Win32::Foundation::FILETIME;
use windows::Win32::Graphics::Direct3D11::ID3D11Device;
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::CoTaskMemFree;
use windows::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};

const TICKS_PER_SECOND: i64 = 10_000_000; // Media Foundation time unit: 100 ns.
const MF_VERSION: u32 = (MF_SDK_VERSION << 16) | MF_API_VERSION;

fn ensure_mf() -> WinResult<()> {
    static STARTED: OnceLock<WinResult<()>> = OnceLock::new();
    ensure_mta();
    // SAFETY: called once; Media Foundation stays up for the process lifetime.
    STARTED
        .get_or_init(|| unsafe { MFStartup(MF_VERSION, MFSTARTUP_FULL) })
        .clone()
}

fn pack(hi: u32, lo: u32) -> u64 {
    (u64::from(hi) << 32) | u64::from(lo)
}

/// H.264 MP4 writer fed with BGRA rows, through a Media Foundation sink writer.
pub(crate) struct Mp4Writer {
    writer: IMFSinkWriter,
    stream: u32,
    width: u32,
    height: u32,
    fps: u32,
}

impl Mp4Writer {
    /// `width` and `height` must be even: H.264 uses 4:2:0 chroma.
    /// A device gives hardware encoders a D3D11 device to run on.
    pub fn new(
        path: &Path,
        width: u32,
        height: u32,
        fps: u32,
        device: Option<&ID3D11Device>,
    ) -> WinResult<Self> {
        ensure_mf()?;
        // ~0.1 bit per pixel: enough for screen content at 1080p60 (~12 Mbit/s).
        let bitrate = (u64::from(width) * u64::from(height) * u64::from(fps) / 10) as u32;
        // SAFETY: Media Foundation calls below only read arguments valid for each call.
        unsafe {
            let mut attrs = None;
            MFCreateAttributes(&mut attrs, 2)?;
            let attrs: IMFAttributes = attrs.expect("MFCreateAttributes without attributes");
            attrs.SetUINT32(&MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS, 1)?;
            if let Some(device) = device {
                let mut token = 0;
                let mut manager = None;
                MFCreateDXGIDeviceManager(&mut token, &mut manager)?;
                let manager: IMFDXGIDeviceManager =
                    manager.expect("MFCreateDXGIDeviceManager without a manager");
                manager.ResetDevice(device, token)?;
                attrs.SetUnknown(&MF_SINK_WRITER_D3D_MANAGER, &manager)?;
            }
            let url = HSTRING::from(path.as_os_str());
            let writer = MFCreateSinkWriterFromURL(&url, None, &attrs)?;

            let output = MFCreateMediaType()?;
            output.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
            output.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_H264)?;
            output.SetUINT32(&MF_MT_AVG_BITRATE, bitrate)?;
            output.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
            output.SetUINT64(&MF_MT_FRAME_SIZE, pack(width, height))?;
            output.SetUINT64(&MF_MT_FRAME_RATE, pack(fps, 1))?;
            output.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, pack(1, 1))?;
            let stream = writer.AddStream(&output)?;

            let input = MFCreateMediaType()?;
            input.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
            input.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_RGB32)?;
            input.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
            input.SetUINT64(&MF_MT_FRAME_SIZE, pack(width, height))?;
            input.SetUINT64(&MF_MT_FRAME_RATE, pack(fps, 1))?;
            input.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, pack(1, 1))?;
            // A positive stride means top-down rows, the layout of captured textures.
            input.SetUINT32(&MF_MT_DEFAULT_STRIDE, width * 4)?;
            writer.SetInputMediaType(stream, &input, None)?;

            writer.BeginWriting()?;
            Ok(Self {
                writer,
                stream,
                width,
                height,
                fps,
            })
        }
    }

    /// Queue one BGRA frame at position `index` on the fps timeline.
    /// Blocks when the encoder falls behind, which the spike reports as dropped ticks.
    pub fn write_frame(&self, bgra: &[u8], pitch: usize, index: u64) -> WinResult<()> {
        let row = self.width * 4;
        let size = row * self.height;
        assert!(bgra.len() >= pitch * self.height as usize);
        // SAFETY: the buffer holds `size` bytes while locked and `bgra` covers height rows.
        unsafe {
            let buffer = MFCreateMemoryBuffer(size)?;
            let mut data = std::ptr::null_mut();
            buffer.Lock(&mut data, None, None)?;
            let copied = MFCopyImage(
                data,
                row as i32,
                bgra.as_ptr(),
                pitch as i32,
                row,
                self.height,
            );
            buffer.Unlock()?;
            copied?;
            buffer.SetCurrentLength(size)?;

            let sample = MFCreateSample()?;
            sample.AddBuffer(&buffer)?;
            let fps = i64::from(self.fps);
            sample.SetSampleTime(index as i64 * TICKS_PER_SECOND / fps)?;
            sample.SetSampleDuration(TICKS_PER_SECOND / fps)?;
            self.writer.WriteSample(self.stream, &sample)
        }
    }

    /// The video encoder the sink writer picked, e.g. "Intel Quick Sync Video H.264 Encoder MFT".
    pub fn encoder_name(&self) -> String {
        use windows::core::Interface;
        let Ok(ex) = self.writer.cast::<IMFSinkWriterEx>() else {
            return "unknown".into();
        };
        for index in 0.. {
            let mut category = GUID::zeroed();
            let mut transform = None;
            // SAFETY: out-pointers are valid; an out-of-range index returns an error.
            let found = unsafe {
                ex.GetTransformForStream(self.stream, index, Some(&mut category), &mut transform)
            };
            if found.is_err() {
                break;
            }
            if category == MFT_CATEGORY_VIDEO_ENCODER {
                return transform
                    .and_then(|t: IMFTransform| encoder_friendly_name(&t))
                    .unwrap_or_else(|| "software (Microsoft H264 Encoder MFT)".into());
            }
        }
        "unknown".into()
    }

    pub fn finish(self) -> WinResult<()> {
        // SAFETY: every queued sample stays owned by the writer until it returns.
        unsafe { self.writer.Finalize() }
    }
}

/// Hardware H.264 encoders this machine offers, by friendly name.
fn hardware_h264_encoders() -> Vec<String> {
    let output = MFT_REGISTER_TYPE_INFO {
        guidMajorType: MFMediaType_Video,
        guidSubtype: MFVideoFormat_H264,
    };
    let mut activates = std::ptr::null_mut();
    let mut count = 0;
    // SAFETY: out-pointers are valid; the array and its items are released below.
    unsafe {
        if MFTEnumEx(
            MFT_CATEGORY_VIDEO_ENCODER,
            MFT_ENUM_FLAG_HARDWARE | MFT_ENUM_FLAG_SORTANDFILTER,
            None,
            Some(&output),
            &mut activates,
            &mut count,
        )
        .is_err()
        {
            return Vec::new();
        }
        let names = std::slice::from_raw_parts_mut(activates, count as usize)
            .iter_mut()
            .filter_map(|slot| {
                let activate: IMFActivate = slot.take()?;
                let mut text = PWSTR::null();
                let mut len = 0;
                activate
                    .GetAllocatedString(&MFT_FRIENDLY_NAME_Attribute, &mut text, &mut len)
                    .ok()?;
                let name = text.to_string().ok();
                CoTaskMemFree(Some(text.0 as *const _));
                name
            })
            .collect();
        CoTaskMemFree(Some(activates as *const _));
        names
    }
}

/// Hardware MFTs are asynchronous and carry a device URL; the software one is neither.
fn encoder_friendly_name(transform: &IMFTransform) -> Option<String> {
    // SAFETY: the returned strings are copied, then freed with the COM allocator.
    unsafe {
        let attrs = transform.GetAttributes().ok()?;
        let read = |key: &GUID| {
            let mut text = PWSTR::null();
            let mut len = 0;
            attrs.GetAllocatedString(key, &mut text, &mut len).ok()?;
            let value = text.to_string().ok();
            CoTaskMemFree(Some(text.0 as *const _));
            value
        };
        let is_async = attrs.GetUINT32(&MF_TRANSFORM_ASYNC).unwrap_or(0) != 0;
        let url = read(&MFT_ENUM_HARDWARE_URL_Attribute);
        let name = read(&MFT_FRIENDLY_NAME_Attribute);
        (is_async || url.is_some()).then(|| name.unwrap_or_else(|| "hardware (unnamed)".into()))
    }
}

/// What a decoder reads back from an MP4: proof the file is valid and not black.
#[derive(Debug)]
pub(crate) struct Mp4Check {
    pub frames: u64,
    pub last_timestamp: Duration,
    pub last_frame_mean_luma: f32,
    /// First and last luma rows of the last frame, averaged: orientation checks.
    pub last_frame_top_luma: f32,
    pub last_frame_bottom_luma: f32,
}

pub(crate) fn decode_check(path: &Path, width: u32, height: u32) -> WinResult<Mp4Check> {
    ensure_mf()?;
    let stream = MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32;
    // SAFETY: Media Foundation calls below only read arguments valid for each call;
    // the locked slice is only used before Unlock.
    unsafe {
        let reader = MFCreateSourceReaderFromURL(&HSTRING::from(path.as_os_str()), None)?;
        let nv12 = MFCreateMediaType()?;
        nv12.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
        nv12.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_NV12)?;
        reader.SetCurrentMediaType(stream, None, &nv12)?;
        let pitch = reader
            .GetCurrentMediaType(stream)?
            .GetUINT32(&MF_MT_DEFAULT_STRIDE)
            .unwrap_or(width) as usize;

        let mut check = Mp4Check {
            frames: 0,
            last_timestamp: Duration::ZERO,
            last_frame_mean_luma: 0.0,
            last_frame_top_luma: 0.0,
            last_frame_bottom_luma: 0.0,
        };
        let mean =
            |rows: &[u8]| rows.iter().map(|&y| f32::from(y)).sum::<f32>() / rows.len() as f32;
        loop {
            let mut flags = 0u32;
            let mut timestamp = 0i64;
            let mut sample = None;
            reader.ReadSample(
                stream,
                0,
                None,
                Some(&mut flags),
                Some(&mut timestamp),
                Some(&mut sample),
            )?;
            if flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 {
                break;
            }
            let Some(sample): Option<IMFSample> = sample else {
                continue;
            };
            check.frames += 1;
            check.last_timestamp = Duration::from_nanos(timestamp as u64 * 100);

            let buffer = sample.ConvertToContiguousBuffer()?;
            let mut data = std::ptr::null_mut();
            let mut len = 0;
            buffer.Lock(&mut data, None, Some(&mut len))?;
            let bytes = std::slice::from_raw_parts(data as *const u8, len as usize);
            let (w, h) = (width as usize, height as usize);
            if bytes.len() >= pitch * h {
                check.last_frame_top_luma = mean(&bytes[..w]);
                check.last_frame_bottom_luma = mean(&bytes[pitch * (h - 1)..pitch * (h - 1) + w]);
                // Every 16th row is plenty for a black-frame check.
                let sum: f32 = (0..h)
                    .step_by(16)
                    .map(|y| mean(&bytes[pitch * y..pitch * y + w]))
                    .sum();
                check.last_frame_mean_luma = sum / h.div_ceil(16) as f32;
            }
            buffer.Unlock()?;
        }
        Ok(check)
    }
}

/// CPU time used by this process so far, across all threads.
fn process_cpu_time() -> Duration {
    let as_duration = |t: FILETIME| {
        let ticks = pack(t.dwHighDateTime, t.dwLowDateTime);
        Duration::from_nanos(ticks * 100)
    };
    let (mut a, mut b, mut kernel, mut user) = Default::default();
    // SAFETY: out-pointers are valid; on failure the zero defaults stay.
    let _ = unsafe { GetProcessTimes(GetCurrentProcess(), &mut a, &mut b, &mut kernel, &mut user) };
    as_duration(kernel) + as_duration(user)
}

/// Record `seconds` of `display` to an MP4 on a fixed fps timeline, then decode it back.
pub(crate) fn record(
    path: &Path,
    display: DisplayId,
    seconds: u32,
    fps: u32,
) -> CaptureResult<SpikeReport> {
    let monitor = resolve_display(display)?;
    let capture = MonitorCapture::start(monitor).map_err(platform_err)?;
    capture.wait_first_frame()?;
    // H.264 needs even dimensions; a trailing odd row or column is dropped.
    let (width, height) = (capture.width & !1, capture.height & !1);
    let mp4 =
        Mp4Writer::new(path, width, height, fps, Some(&capture.device)).map_err(platform_err)?;
    let encoder = mp4.encoder_name();
    let hardware_encoders = hardware_h264_encoders();

    let target_frames = u64::from(seconds) * u64::from(fps);
    let period = Duration::from_secs(1) / fps;
    let cpu_start = process_cpu_time();
    let start = Instant::now();
    let (mut written, mut fresh, mut dropped) = (0u64, 0u64, 0u64);

    for tick in 0..target_frames {
        let due = start + period * tick as u32;
        let now = Instant::now();
        if now < due {
            std::thread::sleep(due - now);
        } else if now - due >= period {
            // More than a whole frame late: this slot is lost.
            dropped += 1;
            continue;
        }
        if capture.copy_latest().map_err(platform_err)? {
            fresh += 1;
        }
        capture
            .with_pixels(|bgra, pitch| mp4.write_frame(bgra, pitch, tick))
            .map_err(platform_err)?
            .map_err(platform_err)?;
        written += 1;
    }

    let elapsed = start.elapsed();
    let cpu = process_cpu_time() - cpu_start;
    let finalize_start = Instant::now();
    mp4.finish().map_err(platform_err)?;
    let finalize = finalize_start.elapsed();

    let check = decode_check(path, width, height).map_err(platform_err)?;
    let file_bytes = std::fs::metadata(path)
        .map_err(|e| CaptureError::Platform(e.to_string()))?
        .len();
    let cores = std::thread::available_parallelism().map_or(1, |n| n.get());

    Ok(SpikeReport {
        width,
        height,
        fps,
        target_frames,
        written_frames: written,
        fresh_frames: fresh,
        dropped_frames: dropped,
        elapsed,
        finalize,
        cpu_percent_of_machine: cpu.as_secs_f32() / elapsed.as_secs_f32() / cores as f32 * 100.0,
        cores,
        encoder,
        hardware_encoders,
        file_bytes,
        decoded_frames: check.frames,
        decoded_duration: check.last_timestamp,
        last_frame_mean_luma: check.last_frame_mean_luma,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Encode a white-over-black pattern, decode it, and check frame count,
    /// timing and that the image is not upside down. Needs no screen.
    #[test]
    fn mp4_round_trip_keeps_frames_and_orientation() {
        let (w, h, fps) = (640u32, 480u32, 30u32);
        let dir = std::env::temp_dir().join(format!("cb-spike-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("round-trip.mp4");

        let mp4 = match Mp4Writer::new(&path, w, h, fps, None) {
            Ok(mp4) => mp4,
            Err(e) => {
                // Some Windows Server images ship no H.264 encoder.
                eprintln!("skipped: no H.264 encoder on this machine: {e}");
                return;
            }
        };
        let mut bgra = vec![255u8; (w * h * 4) as usize];
        bgra[(w * 4 * h / 2) as usize..].fill(0);
        for i in 0..30 {
            mp4.write_frame(&bgra, (w * 4) as usize, i).unwrap();
        }
        mp4.finish().unwrap();

        let check = decode_check(&path, w, h).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();

        assert_eq!(check.frames, 30);
        assert!(
            check.last_timestamp >= Duration::from_millis(900),
            "{check:?}"
        );
        assert!(check.last_frame_top_luma > 200.0, "{check:?}");
        assert!(check.last_frame_bottom_luma < 50.0, "{check:?}");
    }
}
