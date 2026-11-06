//! OS backends. Exactly one module is compiled per target. Everything above
//! this crate talks only to the traits in `cb-core`.

use cb_core::ScreenSource;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(windows)]
mod windows;
#[cfg(windows)]
mod windows_spike;

mod stub;

/// Numbers from one US-1.2 spike recording, logged against NFR-6.
#[derive(Debug)]
pub struct SpikeReport {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub target_frames: u64,
    pub written_frames: u64,
    /// Written frames that carried a new image from the OS; the rest repeat the last one.
    pub fresh_frames: u64,
    pub dropped_frames: u64,
    pub elapsed: std::time::Duration,
    pub finalize: std::time::Duration,
    /// This process only, 100% = every logical CPU busy. GPU encoding is not included.
    pub cpu_percent_of_machine: f32,
    pub cores: usize,
    pub encoder: String,
    /// Hardware H.264 encoders the machine offers, used or not.
    pub hardware_encoders: Vec<String>,
    pub file_bytes: u64,
    pub decoded_frames: u64,
    pub decoded_duration: std::time::Duration,
    pub last_frame_mean_luma: f32,
}

/// Record a display to an MP4 and read it back (US-1.2).
pub fn spike_record(
    path: &std::path::Path,
    display: cb_core::DisplayId,
    seconds: u32,
    fps: u32,
) -> cb_core::CaptureResult<SpikeReport> {
    #[cfg(windows)]
    {
        windows_spike::record(path, display, seconds, fps)
    }
    #[cfg(not(windows))]
    {
        let _ = (path, display, seconds, fps);
        Err(cb_core::CaptureError::NotImplemented("spike_record"))
    }
}

/// The screen source for the current OS.
pub fn screen_source() -> Box<dyn ScreenSource> {
    #[cfg(target_os = "macos")]
    {
        Box::new(macos::MacScreenSource::new())
    }
    #[cfg(windows)]
    {
        Box::new(windows::WinScreenSource::new())
    }
    #[cfg(target_os = "linux")]
    {
        Box::new(linux::LinuxScreenSource::new())
    }
    #[cfg(not(any(target_os = "macos", windows, target_os = "linux")))]
    {
        Box::new(stub::StubScreenSource)
    }
}

/// Human-readable backend name, shown in logs and the About box.
pub fn backend_name() -> &'static str {
    #[cfg(target_os = "macos")]
    {
        "macOS · ScreenCaptureKit (todo)"
    }
    #[cfg(windows)]
    {
        "Windows · Graphics.Capture"
    }
    #[cfg(target_os = "linux")]
    {
        linux::backend_name()
    }
    #[cfg(not(any(target_os = "macos", windows, target_os = "linux")))]
    {
        "unsupported"
    }
}
