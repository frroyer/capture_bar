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

/// Keep a window out of every screenshot and recording (FR-6).
///
/// Windows only for now. macOS excludes the bar through the ScreenCaptureKit
/// filter once that backend exists. Linux has no exclusion and must hide the
/// bar while capturing: see `docs/spikes/us-1.2-wayland-bar.md`.
pub fn exclude_from_capture(
    window: &impl raw_window_handle::HasWindowHandle,
) -> cb_core::CaptureResult<()> {
    #[cfg(windows)]
    {
        windows::exclude_from_capture(window)
    }
    #[cfg(not(windows))]
    {
        let _ = window;
        Err(cb_core::CaptureError::NotImplemented(
            "exclude_from_capture",
        ))
    }
}

/// Remove the frame the OS draws around a borderless window, so only what the
/// app paints is visible. macOS uses `ViewportBuilder::with_has_shadow` instead.
pub fn remove_window_frame(
    window: &impl raw_window_handle::HasWindowHandle,
) -> cb_core::CaptureResult<()> {
    #[cfg(windows)]
    {
        windows::remove_window_frame(window)
    }
    #[cfg(not(windows))]
    {
        let _ = window;
        Ok(())
    }
}

/// The OS interface font as `(regular, semibold)` file bytes, when it can be
/// loaded as a file. Not bundled: NFR-15 does not allow the OFL fonts.
pub fn system_ui_fonts() -> Option<(Vec<u8>, Vec<u8>)> {
    #[cfg(windows)]
    {
        windows::system_ui_fonts()
    }
    #[cfg(not(windows))]
    {
        None
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
