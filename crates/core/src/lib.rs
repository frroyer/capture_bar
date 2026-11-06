//! Shared types, the command bus vocabulary and the traits every platform
//! backend implements. No OS-specific code lives here (see CLAUDE.md).

use serde::{Deserialize, Serialize};
use std::time::SystemTime;

/// What the next Capture / Record acts on (FR-7).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum CaptureMode {
    #[default]
    Region,
    Window,
    Screen,
}

impl CaptureMode {
    pub fn label(self) -> &'static str {
        match self {
            CaptureMode::Region => "Region",
            CaptureMode::Window => "Window",
            CaptureMode::Screen => "Screen",
        }
    }
}

/// Every user intent goes through the command bus as one of these.
/// UI buttons and global shortcuts produce the same commands (US-3.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    ToggleBar,
    SetMode(CaptureMode),
    Capture,
    CaptureWith(CaptureMode),
    ToggleRecording,
    TogglePause,
    RepeatLastRegion,
    ToggleBuffer,
    CopyNewest,
    Cancel,
}

/// A rectangle in physical pixels, in global desktop coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

/// What to capture, resolved from the mode (+ selection) by the UI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CaptureTarget {
    Region(Rect),
    Window(WindowId),
    Display(DisplayId),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct WindowId(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct DisplayId(pub u64);

#[derive(Debug, Clone)]
pub struct DisplayInfo {
    pub id: DisplayId,
    pub name: String,
    pub bounds: Rect,
    pub scale_factor: f32,
}

#[derive(Debug, Clone)]
pub struct WindowInfo {
    pub id: WindowId,
    pub title: String,
    pub app_name: String,
    pub bounds: Rect,
}

/// One captured frame: tightly packed RGBA8, `width * 4` bytes per row.
#[derive(Debug, Clone)]
pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    pub captured_at: SystemTime,
}

#[derive(Debug, thiserror::Error)]
pub enum CaptureError {
    #[error("screen recording permission not granted")]
    PermissionDenied,
    #[error("target no longer exists")]
    TargetGone,
    #[error("not implemented on this platform yet: {0}")]
    NotImplemented(&'static str),
    #[error("platform error: {0}")]
    Platform(String),
}

pub type CaptureResult<T> = Result<T, CaptureError>;

/// Implemented once per OS in `cb-platform` (US-1.1, US-1.2).
pub trait ScreenSource: Send + Sync {
    fn displays(&self) -> CaptureResult<Vec<DisplayInfo>>;
    fn windows(&self) -> CaptureResult<Vec<WindowInfo>>;
    /// Grab a single still (FR-11). Must not block the UI thread for long.
    fn grab(&self, target: &CaptureTarget) -> CaptureResult<Frame>;
    /// Start a frame stream for recording (FR-14). Frames go to `sink`.
    fn start_stream(
        &self,
        target: &CaptureTarget,
        fps: u32,
        sink: Box<dyn FrameSink>,
    ) -> CaptureResult<Box<dyn StreamHandle>>;
}

pub trait FrameSink: Send {
    fn push(&mut self, frame: Frame);
}

pub trait StreamHandle: Send {
    fn pause(&mut self);
    fn resume(&mut self);
    fn stop(self: Box<Self>);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_mode_is_region() {
        assert_eq!(CaptureMode::default(), CaptureMode::Region);
        assert_eq!(CaptureMode::Screen.label(), "Screen");
    }
}
