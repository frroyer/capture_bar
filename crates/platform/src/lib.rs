//! OS backends. Exactly one module is compiled per target. Everything above
//! this crate talks only to the traits in `cb-core`.

use cb_core::ScreenSource;

#[cfg(any(windows, test))]
mod audio_mix;
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
mod windows_audio;
#[cfg(windows)]
mod windows_bubble;
#[cfg(windows)]
mod windows_camera;
#[cfg(windows)]
mod windows_mp4;
#[cfg(windows)]
mod windows_record;
#[cfg(windows)]
mod windows_spike;

mod stub;

#[cfg(not(target_os = "linux"))]
pub use tray::Tray;

/// Keeps the tray icon alive. Linux has no icon yet.
#[cfg(target_os = "linux")]
pub struct Tray;

#[cfg(target_os = "linux")]
impl Tray {
    pub fn set_labels(&self, _labels: [&str; 2]) {}
}

/// What the user picked on the tray icon.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayAction {
    ToggleBar,
    Quit,
}

/// Add the tray / menu-bar icon. Call on the thread of the event loop, once it runs.
///
/// Not on Linux yet: the `tray-icon` crate links GTK, which NFR-1 rules out.
/// `labels` name the menu's items: show or hide the bar, and quit.
pub fn create_tray(
    labels: [&str; 2],
    on_action: impl Fn(TrayAction) + Send + Sync + 'static,
) -> cb_core::CaptureResult<Tray> {
    #[cfg(not(target_os = "linux"))]
    {
        tray::create(labels, on_action)
    }
    #[cfg(target_os = "linux")]
    {
        let _ = (labels, on_action);
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

/// Get the first recording to start as fast as later ones (US-5.1). Blocks for a few
/// hundred milliseconds: call it on a worker thread at launch.
pub fn warm_up_recording() {
    #[cfg(windows)]
    windows_record::warm_up();
}

/// The app's icon, the tray's disc: square RGBA pixels and their side. None on Linux,
/// which has no tray icon yet.
pub fn app_icon() -> Option<(Vec<u8>, u32)> {
    #[cfg(not(target_os = "linux"))]
    {
        Some((tray::icon_pixels(), tray::ICON_SIZE))
    }
    #[cfg(target_os = "linux")]
    {
        None
    }
}

/// Give a window a taskbar button, or take it away. The bar has none while it floats,
/// and gets one while minimized, as the way back. Windows only for now: elsewhere the
/// bar collapses to its pill instead. Call from the thread that owns the window.
pub fn show_in_taskbar(
    window: &impl raw_window_handle::HasWindowHandle,
    show: bool,
) -> cb_core::CaptureResult<()> {
    #[cfg(windows)]
    {
        windows::show_in_taskbar(window, show)
    }
    #[cfg(not(windows))]
    {
        let _ = (window, show);
        Err(cb_core::CaptureError::NotImplemented("show_in_taskbar"))
    }
}

/// Brings a minimized window back from any thread. eframe runs no frames while its
/// window is minimized, so shortcuts and the tray icon restore the bar first; their
/// command runs once it is back. Windows only, the one place the bar minimizes.
#[derive(Debug, Clone, Copy)]
pub struct Restorer(#[cfg_attr(not(windows), allow(dead_code))] isize);

impl Restorer {
    /// None where the bar never minimizes.
    pub fn new(window: &impl raw_window_handle::HasWindowHandle) -> Option<Self> {
        #[cfg(windows)]
        {
            windows::window_id(window).map(Self)
        }
        #[cfg(not(windows))]
        {
            let _ = window;
            None
        }
    }

    pub fn restore_if_minimized(self) {
        #[cfg(windows)]
        windows::restore_if_minimized(self.0);
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

/// What the webcam is doing (US-5.4).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CameraState {
    Off,
    Starting,
    Running,
    Failed(String),
}

/// Start or stop the default webcam. Returns at once; [`camera_state`] follows.
/// Windows only for now: elsewhere the camera fails to start.
pub fn set_camera(on: bool) {
    #[cfg(windows)]
    windows_camera::set_on(on);
    #[cfg(not(windows))]
    let _ = on;
}

pub fn camera_state() -> CameraState {
    #[cfg(windows)]
    {
        use windows_camera::State;
        match windows_camera::state() {
            State::Off => CameraState::Off,
            State::Starting => CameraState::Starting,
            State::Running => CameraState::Running,
            State::Failed(e) => CameraState::Failed(e),
        }
    }
    #[cfg(not(windows))]
    CameraState::Off
}

/// The newest webcam picture as RGBA with its number, when newer than picture `seen`.
pub fn camera_frame(seen: Option<u64>) -> Option<(u64, cb_core::Frame)> {
    #[cfg(windows)]
    {
        let frame = windows_camera::latest().filter(|f| Some(f.seq) != seen)?;
        let mut rgba = frame.bgra.clone();
        for pixel in rgba.as_chunks_mut::<4>().0 {
            pixel.swap(0, 2);
            pixel[3] = 255;
        }
        Some((
            frame.seq,
            cb_core::Frame {
                width: frame.width,
                height: frame.height,
                rgba,
                captured_at: std::time::SystemTime::now(),
            },
        ))
    }
    #[cfg(not(windows))]
    {
        let _ = seen;
        None
    }
}

/// Where recordings draw the webcam bubble, in global physical pixels; `None` for no
/// bubble. Read at every video frame, so moving the bubble moves it in the video.
pub fn set_camera_bubble(rect: Option<cb_core::Rect>) {
    #[cfg(windows)]
    windows_camera::set_bubble(rect);
    #[cfg(not(windows))]
    let _ = rect;
}

/// One of this app's own windows, found by its title: for the windows egui opens
/// itself (viewports), whose handles it does not give out. Windows only for now.
pub fn own_window(title: &str) -> Option<OwnWindow> {
    #[cfg(windows)]
    {
        windows::own_window(title).map(OwnWindow)
    }
    #[cfg(not(windows))]
    {
        let _ = title;
        None
    }
}

/// See [`own_window`].
pub struct OwnWindow(#[cfg_attr(not(windows), allow(dead_code))] isize);

impl raw_window_handle::HasWindowHandle for OwnWindow {
    fn window_handle(
        &self,
    ) -> Result<raw_window_handle::WindowHandle<'_>, raw_window_handle::HandleError> {
        #[cfg(windows)]
        {
            let hwnd = std::num::NonZeroIsize::new(self.0)
                .ok_or(raw_window_handle::HandleError::Unavailable)?;
            let raw = raw_window_handle::Win32WindowHandle::new(hwnd);
            // SAFETY: the handle names a window of this process, used on its thread.
            Ok(unsafe { raw_window_handle::WindowHandle::borrow_raw(raw.into()) })
        }
        #[cfg(not(windows))]
        {
            Err(raw_window_handle::HandleError::NotSupported)
        }
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

/// Open `path` in the app the system uses for its type: a recording in the video player.
pub fn open_file(path: &std::path::Path) -> cb_core::CaptureResult<()> {
    #[cfg(windows)]
    {
        windows::open_file(path)
    }
    #[cfg(not(windows))]
    {
        let opener = if cfg!(target_os = "macos") {
            "open"
        } else {
            "xdg-open"
        };
        std::process::Command::new(opener)
            .arg(path)
            .spawn()
            .map(drop)
            .map_err(|e| cb_core::CaptureError::Platform(e.to_string()))
    }
}

/// `count` frames spread evenly over the video at `path`, for a preview, each handed to
/// `each` as it is decoded. Slow: call it from a worker thread.
pub fn video_frames(
    path: &std::path::Path,
    count: u32,
    each: &mut dyn FnMut(cb_core::Frame),
) -> cb_core::CaptureResult<()> {
    #[cfg(windows)]
    {
        windows_mp4::video_frames(path, count, each).map_err(windows::platform_err)
    }
    #[cfg(not(windows))]
    {
        let _ = (path, count, each);
        Err(cb_core::CaptureError::NotImplemented("video_frames"))
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
