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
mod windows_events;
#[cfg(windows)]
mod windows_faces;
#[cfg(windows)]
mod windows_mask;
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

/// The newest webcam picture as opaque RGBA with its number, when newer than picture
/// `seen`, scaled down by a whole step to no less than `min_height` pixels high: the
/// bubble shows it small, and each pixel less is work less on the UI thread.
pub fn camera_frame(seen: Option<u64>, min_height: u32) -> Option<(u64, cb_core::Frame)> {
    #[cfg(windows)]
    {
        let frame = windows_camera::latest().filter(|f| Some(f.seq) != seen)?;
        Some((
            frame.seq,
            shrink_bgra(&frame.bgra, frame.width, frame.height, min_height),
        ))
    }
    #[cfg(not(windows))]
    {
        let _ = (seen, min_height);
        None
    }
}

/// `bgra` as opaque RGBA, every `step`th pixel of every `step`th row, `step` as large
/// as keeps it `min_height` high.
// chisle: nearest pixel, not averaged; the bubble draws it smoothed anyway.
#[cfg_attr(not(windows), allow(dead_code))]
fn shrink_bgra(bgra: &[u8], width: u32, height: u32, min_height: u32) -> cb_core::Frame {
    let step = (height / min_height.max(1)).max(1) as usize;
    let (w, h) = (width as usize / step, height as usize / step);
    let mut rgba = Vec::with_capacity(w * h * 4);
    for y in 0..h {
        let row = &bgra[y * step * width as usize * 4..];
        for x in 0..w {
            let p = &row[x * step * 4..x * step * 4 + 4];
            rgba.extend_from_slice(&[p[2], p[1], p[0], 255]);
        }
    }
    cb_core::Frame {
        width: w as u32,
        height: h as u32,
        rgba,
        captured_at: std::time::SystemTime::now(),
    }
}

/// Where recordings draw the webcam bubble, in global physical pixels, and its outline;
/// `None` for no bubble. Read at every video frame, so moving the bubble moves it in the
/// video.
pub fn set_camera_bubble(bubble: Option<(cb_core::Rect, cb_core::BubbleShape)>) {
    #[cfg(windows)]
    windows_camera::set_bubble(bubble);
    #[cfg(not(windows))]
    let _ = bubble;
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

/// Keep a window's size in pixels when it moves onto a monitor of another scale,
/// instead of the OS resizing it mid-drag. Windows only; elsewhere a no-op.
pub fn keep_pixel_size(
    window: &impl raw_window_handle::HasWindowHandle,
) -> cb_core::CaptureResult<()> {
    #[cfg(windows)]
    {
        windows::keep_pixel_size(window)
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

/// The font files installed on this computer (`.ttf`, `.otf`, `.ttc`), for the editor's
/// font setting. Slow: walks the font folders.
pub fn font_files() -> Vec<std::path::PathBuf> {
    let home = std::env::var_os(if cfg!(windows) {
        "LOCALAPPDATA"
    } else {
        "HOME"
    })
    .map(std::path::PathBuf::from);
    let mut dirs: Vec<std::path::PathBuf> = Vec::new();
    if cfg!(windows) {
        let windir = std::env::var_os("WINDIR").unwrap_or_else(|| "C:\\Windows".into());
        dirs.push(std::path::PathBuf::from(windir).join("Fonts"));
        dirs.extend(home.map(|h| h.join("Microsoft\\Windows\\Fonts")));
    } else if cfg!(target_os = "macos") {
        dirs.extend(["/System/Library/Fonts", "/Library/Fonts"].map(Into::into));
        dirs.extend(home.map(|h| h.join("Library/Fonts")));
    } else {
        dirs.extend(["/usr/share/fonts", "/usr/local/share/fonts"].map(Into::into));
        if let Some(h) = home {
            dirs.push(h.join(".local/share/fonts"));
            dirs.push(h.join(".fonts"));
        }
    }
    let mut files = Vec::new();
    while let Some(dir) = dirs.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for path in entries.flatten().map(|e| e.path()) {
            let ext = path
                .extension()
                .and_then(|e| e.to_str())
                .map(str::to_lowercase);
            if path.is_dir() {
                dirs.push(path);
            } else if matches!(ext.as_deref(), Some("ttf" | "otf" | "ttc")) {
                files.push(path);
            }
        }
    }
    files
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

/// The video at `path` from start to end at `fps` frames a second, for a GIF (US-6.7):
/// each tick shows the newest picture before it. `each` gets the frame and how far along
/// the video it is, 0 to 1, and returns false to stop. Slow: call it from a worker thread.
pub fn video_clip(
    path: &std::path::Path,
    fps: u32,
    each: &mut dyn FnMut(cb_core::Frame, f32) -> bool,
) -> cb_core::CaptureResult<()> {
    #[cfg(windows)]
    {
        windows_mp4::video_clip(path, fps, each).map_err(windows::platform_err)
    }
    #[cfg(not(windows))]
    {
        let _ = (path, fps, each);
        Err(cb_core::CaptureError::NotImplemented("video_clip"))
    }
}

/// Blur these parts of the desktop and windows in everything recorded from now on, until
/// set again (US-10.5, DR-20). Windows only for now: elsewhere nothing records yet.
pub fn set_masks(masks: Vec<cb_core::Mask>) {
    #[cfg(windows)]
    windows_mask::set(masks);
    #[cfg(not(windows))]
    let _ = masks;
}

/// Have the next recording write the webcam into its own video at `path`, on the same
/// clock, and leave its bubble out of the screen's (US-10.6, DR-21). The camera must be
/// on ([`set_camera`]); the result's [`cb_core::Recording::webcam`] says what was written.
pub fn set_webcam_track(path: Option<std::path::PathBuf>) {
    #[cfg(windows)]
    windows_record::set_webcam_track(path);
    #[cfg(not(windows))]
    let _ = path;
}

/// The largest face of each second of the video at `path` (`t_ms` from its start, box in
/// its pixels), `None` where nobody shows (US-10.6, DR-23). Slow: from a worker thread.
pub fn face_boxes(
    path: &std::path::Path,
) -> cb_core::CaptureResult<Vec<(u64, Option<cb_core::Rect>)>> {
    #[cfg(windows)]
    {
        windows_faces::face_boxes(path).map_err(windows::platform_err)
    }
    #[cfg(not(windows))]
    {
        let _ = path;
        Err(cb_core::CaptureError::NotImplemented("face_boxes"))
    }
}

/// Play a short 1 kHz beep on its own thread: the clap test's sound (US-10.2). `played`
/// gets when its first sample played, by the sound card's clock, or `None` if it could
/// not play.
pub fn clap_beep(played: Box<dyn FnOnce(Option<std::time::Instant>) + Send>) {
    #[cfg(windows)]
    windows_audio::beep(played);
    #[cfg(not(windows))]
    played(None);
}

/// Logs a demo session's clicks, scrolls, window changes, key combinations and cursor
/// into `events.jsonl` (US-10.4) until dropped. Typed text is never logged (DR-6).
pub struct EventRecorder {
    #[cfg(windows)]
    _inner: windows_events::Recorder,
}

/// Start logging the session's events on its `clock` into `path`, made with its folder
/// on a thread of its own. `None` where the platform cannot log them yet.
pub fn record_events(
    path: std::path::PathBuf,
    clock: cb_core::MediaClock,
) -> Option<EventRecorder> {
    #[cfg(windows)]
    {
        match windows_events::start(path, clock) {
            Ok(inner) => Some(EventRecorder { _inner: inner }),
            Err(error) => {
                tracing::warn!(%error, "event log");
                None
            }
        }
    }
    #[cfg(not(windows))]
    {
        let _ = (path, clock);
        None
    }
}

/// What [`clap_probes`] finds: each frame's time and centre luma, and the sound's levels.
pub type ClapProbes = (Vec<(std::time::Duration, f32)>, Vec<f32>);

/// The clap test's probes (US-10.2) on the recording at `path`: the mean luma (0–255) of
/// the centred `patch`-pixel square of each frame, with its time, and the sound's RMS
/// level (0–1) per `window`. Slow: call it from a worker thread.
pub fn clap_probes(
    path: &std::path::Path,
    patch: usize,
    window: std::time::Duration,
) -> cb_core::CaptureResult<ClapProbes> {
    #[cfg(windows)]
    {
        let luma = windows_mp4::centre_luma(path, patch).map_err(windows::platform_err)?;
        let levels = windows_mp4::audio_levels(path, window).map_err(windows::platform_err)?;
        Ok((luma, levels))
    }
    #[cfg(not(windows))]
    {
        let _ = (path, patch, window);
        Err(cb_core::CaptureError::NotImplemented("clap_probes"))
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

#[cfg(test)]
mod tests {
    #[test]
    fn camera_pictures_shrink_by_whole_steps_to_rgba() {
        // 4×2 BGRA: each pixel's blue is its index.
        let bgra: Vec<u8> = (0..8u8).flat_map(|i| [i, 10, 20, 0]).collect();
        let small = super::shrink_bgra(&bgra, 4, 2, 1);
        assert_eq!((small.width, small.height), (2, 1));
        assert_eq!(small.rgba, [20, 10, 0, 255, 20, 10, 2, 255]);
        let same = super::shrink_bgra(&bgra, 4, 2, 360);
        assert_eq!(
            (same.width, same.height),
            (4, 2),
            "never bigger, never skipped"
        );
    }
}
