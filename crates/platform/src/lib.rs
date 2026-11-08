//! OS backends. Exactly one module is compiled per target. Everything above
//! this crate talks only to the traits in `cb-core`.

use cb_core::ScreenSource;

pub mod autostart;
#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(not(target_os = "linux"))]
mod tray;
#[cfg(windows)]
mod windows;
#[cfg(windows)]
mod windows_spike;

mod stub;

#[cfg(not(target_os = "linux"))]
pub use tray::Tray;

/// Keeps the tray icon alive. Linux has no icon yet.
#[cfg(target_os = "linux")]
pub struct Tray;

/// What the user picked on the tray icon.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayAction {
    ToggleBar,
    Quit,
}

/// Add the tray / menu-bar icon. Call on the thread of the event loop, once it runs.
///
/// Not on Linux yet: the `tray-icon` crate links GTK, which NFR-1 rules out.
pub fn create_tray(
    on_action: impl Fn(TrayAction) + Send + Sync + 'static,
) -> cb_core::CaptureResult<Tray> {
    #[cfg(not(target_os = "linux"))]
    {
        tray::create(on_action)
    }
    #[cfg(target_os = "linux")]
    {
        let _ = on_action;
        Err(cb_core::CaptureError::NotImplemented("tray icon"))
    }
}

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

/// Whether the primary mouse button is held right now, or `None` if this OS cannot
/// tell. The app does not see the release that ends an OS window drag.
pub fn primary_button_down() -> Option<bool> {
    #[cfg(windows)]
    {
        Some(windows::primary_button_down())
    }
    #[cfg(not(windows))]
    {
        None
    }
}

/// Where the cursor is, in global physical pixels, if this OS can tell.
pub fn cursor_position() -> Option<(i32, i32)> {
    #[cfg(windows)]
    {
        windows::cursor_position()
    }
    #[cfg(not(windows))]
    {
        None
    }
}

/// Let the user drag the file at `path` into another app, which receives the file
/// itself (FR-23). Call it while the primary button is held, from the UI thread: the OS
/// runs its own drag loop and returns at the drop. The app never sees that release.
pub fn drag_file_out(path: &std::path::Path) -> cb_core::CaptureResult<()> {
    #[cfg(windows)]
    {
        windows::drag_file_out(path)
    }
    #[cfg(not(windows))]
    {
        let _ = path;
        Err(cb_core::CaptureError::NotImplemented("drag_file_out"))
    }
}

/// Move a file to the Recycle Bin or Trash, where the user can still get it back.
/// Slow on some systems: call it from a worker thread.
pub fn move_to_trash(path: &std::path::Path) -> cb_core::CaptureResult<()> {
    #[cfg(windows)]
    {
        windows::move_to_trash(path)
    }
    #[cfg(not(windows))]
    {
        let _ = path;
        Err(cb_core::CaptureError::NotImplemented("move_to_trash"))
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
