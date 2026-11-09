//! US-1.2 spike: Windows.Graphics.Capture -> CPU copy -> Media Foundation H.264 MP4.
//! Kept to compare against the GPU path recordings use (US-5.1).
//! See docs/spikes/us-1.2-windows.md.

use crate::windows::{platform_err, resolve_display, MonitorCapture};
use crate::windows_mp4::{decode_check, hardware_h264_encoders, pack, Mp4Writer};
use crate::SpikeReport;
use cb_core::{CaptureError, CaptureResult, DisplayId};
use std::path::Path;
use std::time::{Duration, Instant};
use windows::Win32::Foundation::FILETIME;
use windows::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};

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
    let mp4 = Mp4Writer::new(path, width, height, fps, Some(&capture.device), false)
        .map_err(platform_err)?;
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
