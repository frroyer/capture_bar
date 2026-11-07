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
    OpenSettings,
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

impl Rect {
    fn overlap_area(&self, other: &Self) -> i64 {
        let right = |r: &Self| i64::from(r.x) + i64::from(r.width);
        let bottom = |r: &Self| i64::from(r.y) + i64::from(r.height);
        let w = right(self).min(right(other)) - i64::from(self.x.max(other.x));
        let h = bottom(self).min(bottom(other)) - i64::from(self.y.max(other.y));
        w.max(0) * h.max(0)
    }

    /// Squared distance between centres, to pick the nearest of several rectangles.
    fn center_distance(&self, other: &Self) -> i64 {
        let center = |r: &Self| {
            (
                2 * i64::from(r.x) + i64::from(r.width),
                2 * i64::from(r.y) + i64::from(r.height),
            )
        };
        let (a, b) = (center(self), center(other));
        (a.0 - b.0).pow(2) + (a.1 - b.1).pow(2)
    }
}

/// Identifies a monitor arrangement, so positions can be remembered per layout (FR-2).
pub fn layout_key(displays: &[DisplayInfo]) -> String {
    let mut parts: Vec<_> = displays
        .iter()
        .map(|d| {
            let b = d.bounds;
            (b.x, b.y, b.width, b.height)
        })
        .collect();
    if parts.is_empty() {
        return "unknown".into();
    }
    parts.sort_unstable();
    parts
        .iter()
        .map(|(x, y, w, h)| format!("{x},{y},{w}x{h}"))
        .collect::<Vec<_>>()
        .join(";")
}

/// Whether all of `rect` is on some display. It may span several. True when the
/// displays are unknown, because nothing can be said then.
pub fn is_fully_visible(rect: Rect, displays: &[DisplayInfo]) -> bool {
    let visible: i64 = displays.iter().map(|d| rect.overlap_area(&d.bounds)).sum();
    displays.is_empty() || visible >= i64::from(rect.width) * i64::from(rect.height)
}

/// Where to put `rect` so it can be seen: unchanged when fully visible, otherwise
/// moved just far enough into the display it overlaps most, or the nearest one.
pub fn pull_on_screen(rect: Rect, displays: &[DisplayInfo]) -> (i32, i32) {
    let target = displays.iter().max_by_key(|d| {
        (
            rect.overlap_area(&d.bounds),
            -rect.center_distance(&d.bounds),
        )
    });
    let Some(display) = target.filter(|_| !is_fully_visible(rect, displays)) else {
        return (rect.x, rect.y);
    };
    let d = display.bounds;
    // A rectangle larger than the display keeps its top-left corner visible.
    let max_x = i64::from(d.x) + (i64::from(d.width) - i64::from(rect.width)).max(0);
    let max_y = i64::from(d.y) + (i64::from(d.height) - i64::from(rect.height)).max(0);
    (
        i64::from(rect.x).clamp(i64::from(d.x), max_x) as i32,
        i64::from(rect.y).clamp(i64::from(d.y), max_y) as i32,
    )
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

    fn display(x: i32, y: i32, width: u32, height: u32) -> DisplayInfo {
        DisplayInfo {
            id: DisplayId(0),
            name: String::new(),
            bounds: Rect {
                x,
                y,
                width,
                height,
            },
            scale_factor: 1.0,
        }
    }

    fn bar_at(x: i32, y: i32) -> Rect {
        Rect {
            x,
            y,
            width: 800,
            height: 50,
        }
    }

    #[test]
    fn layout_key_ignores_order_and_sees_changes() {
        let (a, b) = (display(0, 0, 1920, 1080), display(1920, 0, 2560, 1440));
        assert_eq!(
            layout_key(&[a.clone(), b.clone()]),
            layout_key(&[b.clone(), a.clone()])
        );
        assert_ne!(layout_key(std::slice::from_ref(&a)), layout_key(&[a, b]));
        assert_eq!(layout_key(&[]), "unknown");
    }

    #[test]
    fn visible_bar_stays_put_even_across_two_displays() {
        let displays = [display(0, 0, 1920, 1080), display(1920, 0, 1920, 1080)];
        assert_eq!(pull_on_screen(bar_at(100, 100), &displays), (100, 100));
        assert_eq!(pull_on_screen(bar_at(1500, 100), &displays), (1500, 100));
    }

    #[test]
    fn bar_on_a_removed_display_comes_back() {
        let laptop = [display(0, 0, 1920, 1080)];
        assert_eq!(pull_on_screen(bar_at(2500, 300), &laptop), (1120, 300));
        assert_eq!(pull_on_screen(bar_at(-3000, -500), &laptop), (0, 0));
    }

    #[test]
    fn partly_visible_bar_moves_into_the_display_it_overlaps_most() {
        let displays = [display(0, 0, 1920, 1080), display(1920, 0, 1920, 1080)];
        assert_eq!(pull_on_screen(bar_at(100, 1060), &displays), (100, 1030));
        assert_eq!(pull_on_screen(bar_at(3500, 100), &displays), (3040, 100));
    }

    #[test]
    fn spanning_a_gap_between_uneven_displays_is_not_fully_visible() {
        // The second display is shorter, so the corner below it is off every screen.
        let displays = [display(0, 0, 1920, 1080), display(1920, 0, 1280, 720)];
        assert!(!is_fully_visible(bar_at(1500, 700), &displays));
        assert_eq!(pull_on_screen(bar_at(1500, 700), &displays), (1120, 700));
    }

    #[test]
    fn unknown_displays_change_nothing() {
        assert!(is_fully_visible(bar_at(9000, 9000), &[]));
        assert_eq!(pull_on_screen(bar_at(9000, 9000), &[]), (9000, 9000));
    }

    #[test]
    fn oversized_rect_keeps_its_corner_visible() {
        let small = [display(0, 0, 640, 480)];
        assert_eq!(pull_on_screen(bar_at(100, 100), &small), (0, 100));
    }

    #[test]
    fn default_mode_is_region() {
        assert_eq!(CaptureMode::default(), CaptureMode::Region);
        assert_eq!(CaptureMode::Screen.label(), "Screen");
    }
}
