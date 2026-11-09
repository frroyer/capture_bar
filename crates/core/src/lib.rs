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
    /// Stop the recording and throw its file away (FR-19). The bar asks first.
    DiscardRecording,
    RepeatLastRegion,
    ToggleBuffer,
    CopyNewest,
    OpenSettings,
    /// Open the tray with the keyboard in the prefix of new captures (FR-30).
    EditPrefix,
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
    /// Whether the global point is inside.
    pub fn contains(&self, (x, y): (i32, i32)) -> bool {
        let (x, y) = (i64::from(x), i64::from(y));
        x >= i64::from(self.x)
            && y >= i64::from(self.y)
            && x < i64::from(self.x) + i64::from(self.width)
            && y < i64::from(self.y) + i64::from(self.height)
    }

    fn overlap_area(&self, other: &Self) -> i64 {
        let right = |r: &Self| i64::from(r.x) + i64::from(r.width);
        let bottom = |r: &Self| i64::from(r.y) + i64::from(r.height);
        let w = right(self).min(right(other)) - i64::from(self.x.max(other.x));
        let h = bottom(self).min(bottom(other)) - i64::from(self.y.max(other.y));
        w.max(0) * h.max(0)
    }

    pub fn overlaps(&self, other: &Self) -> bool {
        self.overlap_area(other) > 0
    }

    /// The part inside `area`, if any.
    pub fn clamped_to(&self, area: Self) -> Option<Self> {
        let right = (i64::from(self.x) + i64::from(self.width))
            .min(i64::from(area.x) + i64::from(area.width));
        let bottom = (i64::from(self.y) + i64::from(self.height))
            .min(i64::from(area.y) + i64::from(area.height));
        let (x, y) = (self.x.max(area.x), self.y.max(area.y));
        (right > i64::from(x) && bottom > i64::from(y)).then(|| Self {
            x,
            y,
            width: (right - i64::from(x)) as u32,
            height: (bottom - i64::from(y)) as u32,
        })
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

/// The display `rect` overlaps most, or the nearest one.
fn display_for(rect: Rect, displays: &[DisplayInfo]) -> Option<&DisplayInfo> {
    displays.iter().max_by_key(|d| {
        (
            rect.overlap_area(&d.bounds),
            -rect.center_distance(&d.bounds),
        )
    })
}

/// `rect`'s position moved just far enough to be inside `area`. A rectangle larger
/// than the area keeps its top-left corner visible.
fn clamp_into(rect: Rect, area: Rect) -> (i32, i32) {
    let max_x = i64::from(area.x) + (i64::from(area.width) - i64::from(rect.width)).max(0);
    let max_y = i64::from(area.y) + (i64::from(area.height) - i64::from(rect.height)).max(0);
    (
        i64::from(rect.x).clamp(i64::from(area.x), max_x) as i32,
        i64::from(rect.y).clamp(i64::from(area.y), max_y) as i32,
    )
}

/// Where to put `rect` so it can be seen: unchanged when fully visible, otherwise
/// moved just far enough into the display it overlaps most, or the nearest one.
pub fn pull_on_screen(rect: Rect, displays: &[DisplayInfo]) -> (i32, i32) {
    match display_for(rect, displays) {
        Some(display) if !is_fully_visible(rect, displays) => clamp_into(rect, display.bounds),
        _ => (rect.x, rect.y),
    }
}

/// The display under a global point (FR-10).
pub fn display_at(point: (i32, i32), displays: &[DisplayInfo]) -> Option<&DisplayInfo> {
    displays.iter().find(|d| d.bounds.contains(point))
}

/// The topmost window under a global point. `windows` is listed front to back (FR-9).
pub fn window_at(point: (i32, i32), windows: &[WindowInfo]) -> Option<&WindowInfo> {
    windows.iter().find(|w| w.bounds.contains(point))
}

/// The smallest rectangle around all displays.
pub fn desktop_bounds(displays: &[DisplayInfo]) -> Option<Rect> {
    let bounds = || displays.iter().map(|d| d.bounds);
    let right = |r: Rect| i64::from(r.x) + i64::from(r.width);
    let bottom = |r: Rect| i64::from(r.y) + i64::from(r.height);
    let x = bounds().map(|b| b.x).min()?;
    let y = bounds().map(|b| b.y).min()?;
    Some(Rect {
        x,
        y,
        width: (bounds().map(right).max()? - i64::from(x)) as u32,
        height: (bounds().map(bottom).max()? - i64::from(y)) as u32,
    })
}

/// One image of the whole desktop from one frame per display. Where no display
/// reaches, the image is black.
pub fn stitch(parts: &[(Rect, Frame)], desktop: Rect) -> Frame {
    let (width, height) = (desktop.width as usize, desktop.height as usize);
    let mut rgba = [0, 0, 0, 255].repeat(width * height);
    for (bounds, frame) in parts {
        let left = (i64::from(bounds.x) - i64::from(desktop.x)) as usize;
        let top = (i64::from(bounds.y) - i64::from(desktop.y)) as usize;
        let row_bytes = frame.width as usize * 4;
        for (row, line) in frame.rgba.chunks_exact(row_bytes).enumerate() {
            let start = ((top + row) * width + left) * 4;
            rgba[start..start + row_bytes].copy_from_slice(line);
        }
    }
    Frame {
        width: desktop.width,
        height: desktop.height,
        rgba,
        captured_at: SystemTime::now(),
    }
}

/// The part of a desktop image at `rect`, in global pixels. `origin` is where the image
/// starts on the desktop. `None` when `rect` is outside the image.
pub fn crop(desktop: &Frame, origin: (i32, i32), rect: Rect) -> Option<Frame> {
    let area = Rect {
        x: origin.0,
        y: origin.1,
        width: desktop.width,
        height: desktop.height,
    };
    let rect = rect.clamped_to(area)?;
    let left = (i64::from(rect.x) - i64::from(origin.0)) as usize * 4;
    let top = (i64::from(rect.y) - i64::from(origin.1)) as usize;
    let row_bytes = rect.width as usize * 4;
    let mut rgba = Vec::with_capacity(row_bytes * rect.height as usize);
    for line in desktop
        .rgba
        .chunks_exact(desktop.width as usize * 4)
        .skip(top)
        .take(rect.height as usize)
    {
        rgba.extend_from_slice(&line[left..left + row_bytes]);
    }
    Some(Frame {
        width: rect.width,
        height: rect.height,
        rgba,
        captured_at: desktop.captured_at,
    })
}

/// A screen edge the bar can dock to (FR-3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Edge {
    Top,
    Bottom,
    Left,
    Right,
}

impl Edge {
    /// The bar turns vertical on these edges (M2.5).
    pub fn is_side(self) -> bool {
        matches!(self, Self::Left | Self::Right)
    }
}

/// The closest edge of `rect`'s display, with its distance in pixels.
fn closest_edge(rect: Rect, displays: &[DisplayInfo]) -> Option<(Edge, i64)> {
    let area = display_for(rect, displays)?.work_area;
    let (x, y) = (i64::from(rect.x), i64::from(rect.y));
    let (ax, ay) = (i64::from(area.x), i64::from(area.y));
    let right = ax + i64::from(area.width) - (x + i64::from(rect.width));
    let bottom = ay + i64::from(area.height) - (y + i64::from(rect.height));
    [
        (Edge::Top, y - ay),
        (Edge::Bottom, bottom),
        (Edge::Left, x - ax),
        (Edge::Right, right),
    ]
    .into_iter()
    .map(|(edge, distance)| (edge, distance.abs()))
    .min_by_key(|&(_, distance)| distance)
}

/// The edge a bar dropped at `rect` snaps to, if it is within `distance` pixels of one.
pub fn snap_edge(rect: Rect, displays: &[DisplayInfo], distance: i32) -> Option<Edge> {
    closest_edge(rect, displays)
        .filter(|&(_, d)| d <= i64::from(distance))
        .map(|(edge, _)| edge)
}

/// The edge a double-click on the grip docks to.
pub fn nearest_edge(rect: Rect, displays: &[DisplayInfo]) -> Option<Edge> {
    closest_edge(rect, displays).map(|(edge, _)| edge)
}

/// Where `rect` goes when docked to `edge`, `gap` pixels from it. The other axis
/// keeps its position as far as the display allows.
pub fn docked_position(rect: Rect, edge: Edge, displays: &[DisplayInfo], gap: i32) -> (i32, i32) {
    let Some(display) = display_for(rect, displays) else {
        return (rect.x, rect.y);
    };
    let area = display.work_area;
    let (x, y) = clamp_into(rect, area);
    let far = |start: i32, area_size: u32, size: u32| {
        (i64::from(start) + i64::from(area_size) - i64::from(size) - i64::from(gap)) as i32
    };
    match edge {
        Edge::Top => (x, area.y + gap),
        Edge::Bottom => (x, far(area.y, area.height, rect.height)),
        Edge::Left => (area.x + gap, y),
        Edge::Right => (far(area.x, area.width, rect.width), y),
    }
}

/// What to capture, resolved from the mode (+ selection) by the UI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CaptureTarget {
    Region(Rect),
    Window(WindowId),
    Display(DisplayId),
    /// All displays as one image.
    Desktop,
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
    /// `bounds` without the taskbar or menu bar: where a docked bar belongs.
    pub work_area: Rect,
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
    /// Start recording `target` into an MP4 at `path` (FR-14), or a free name next to it
    /// (see [`create_unique`]). Returns at once: setup runs on the recording's own thread,
    /// and `done` runs there once the file is complete or recording failed.
    fn start_recording(
        &self,
        target: &CaptureTarget,
        fps: u32,
        audio: AudioSources,
        path: &std::path::Path,
        done: Box<dyn FnOnce(CaptureResult<Recording>) + Send>,
    ) -> CaptureResult<Box<dyn StreamHandle>>;
}

/// The sounds a recording takes (FR-17). A source that cannot be opened is left out.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AudioSources {
    pub microphone: bool,
    /// What the computer plays.
    pub system: bool,
}

impl AudioSources {
    pub fn any(self) -> bool {
        self.microphone || self.system
    }
}

/// A finished recording.
#[derive(Debug, Clone)]
pub struct Recording {
    pub path: std::path::PathBuf,
    pub width: u32,
    pub height: u32,
    pub duration: std::time::Duration,
    /// Frame slots written, and the ones skipped because capture or encoding fell behind (NFR-6).
    pub frames: u64,
    pub dropped: u64,
    /// From the start request to the first frame written (US-5.1: at most 300 ms).
    pub start_latency: std::time::Duration,
}

/// A running recording. Dropping it stops it too.
pub trait StreamHandle: Send {
    fn pause(&mut self);
    fn resume(&mut self);
    /// How loud the microphone is, from 0 to 1, falling back over a fraction of a second
    /// after a peak; `None` when the recording has no microphone.
    fn mic_level(&self) -> Option<f32> {
        None
    }
    /// Returns at once; the recording's `done` runs when the file is complete.
    fn stop(self: Box<Self>);
}

/// Create `path`, or `<stem>-2.<ext>`, `-3`… when it exists: a file is never overwritten (FR-30).
pub fn create_unique(
    path: &std::path::Path,
) -> std::io::Result<(std::fs::File, std::path::PathBuf)> {
    use std::io::ErrorKind;
    let stem = path.file_stem().unwrap_or_default().to_string_lossy();
    let extension = path.extension().unwrap_or_default().to_string_lossy();
    for n in 1u32.. {
        let candidate = match n {
            1 => path.to_path_buf(),
            n => path.with_file_name(format!("{stem}-{n}.{extension}")),
        };
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&candidate)
        {
            Err(e) if e.kind() == ErrorKind::AlreadyExists => continue,
            opened => return opened.map(|file| (file, candidate)),
        }
    }
    unreachable!("every name from -2 to -{} exists", u32::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn display(x: i32, y: i32, width: u32, height: u32) -> DisplayInfo {
        let bounds = Rect {
            x,
            y,
            width,
            height,
        };
        DisplayInfo {
            id: DisplayId(0),
            name: String::new(),
            bounds,
            work_area: bounds,
            scale_factor: 1.0,
        }
    }

    /// A 1920x1080 display with a 40 px taskbar along the bottom.
    fn with_taskbar() -> DisplayInfo {
        let mut d = display(0, 0, 1920, 1080);
        d.work_area.height = 1040;
        d
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
    fn snaps_only_within_the_distance() {
        let d = [display(0, 0, 1920, 1080)];
        assert_eq!(snap_edge(bar_at(500, 24), &d, 24), Some(Edge::Top));
        assert_eq!(snap_edge(bar_at(500, 25), &d, 24), None);
        assert_eq!(snap_edge(bar_at(10, 500), &d, 24), Some(Edge::Left));
        assert_eq!(snap_edge(bar_at(1110, 500), &d, 24), Some(Edge::Right));
        assert_eq!(snap_edge(bar_at(500, 1020), &d, 24), Some(Edge::Bottom));
        assert_eq!(snap_edge(bar_at(500, 500), &[], 24), None);
    }

    #[test]
    fn slightly_past_an_edge_still_snaps() {
        let d = [display(0, 0, 1920, 1080)];
        assert_eq!(snap_edge(bar_at(-20, 500), &d, 24), Some(Edge::Left));
    }

    #[test]
    fn taskbar_counts_as_the_bottom_edge() {
        let d = [with_taskbar()];
        assert_eq!(snap_edge(bar_at(500, 980), &d, 24), Some(Edge::Bottom));
        assert_eq!(
            docked_position(bar_at(500, 980), Edge::Bottom, &d, 8),
            (500, 982)
        );
    }

    #[test]
    fn nearest_edge_works_from_anywhere() {
        let d = [display(0, 0, 1920, 1080)];
        assert_eq!(nearest_edge(bar_at(500, 100), &d), Some(Edge::Top));
        assert_eq!(nearest_edge(bar_at(500, 900), &d), Some(Edge::Bottom));
        assert_eq!(nearest_edge(bar_at(50, 500), &d), Some(Edge::Left));
        assert_eq!(nearest_edge(bar_at(500, 500), &[]), None);
    }

    #[test]
    fn docking_keeps_the_other_axis_on_screen() {
        let d = [display(0, 0, 1920, 1080)];
        let vertical = Rect {
            x: 1800,
            y: 900,
            width: 54,
            height: 500,
        };
        assert_eq!(docked_position(vertical, Edge::Right, &d, 8), (1858, 580));
        assert_eq!(docked_position(vertical, Edge::Left, &d, 8), (8, 580));
        assert_eq!(
            docked_position(bar_at(1500, 300), Edge::Top, &d, 8),
            (1120, 8)
        );
    }

    #[test]
    fn docks_on_the_display_the_bar_is_on() {
        let d = [display(0, 0, 1920, 1080), display(1920, 0, 2560, 1440)];
        assert_eq!(
            docked_position(bar_at(2500, 700), Edge::Bottom, &d, 8),
            (2500, 1382)
        );
    }

    #[test]
    fn only_side_edges_turn_the_bar_vertical() {
        assert!(Edge::Left.is_side() && Edge::Right.is_side());
        assert!(!Edge::Top.is_side() && !Edge::Bottom.is_side());
    }

    fn window(id: u64, x: i32, y: i32) -> WindowInfo {
        WindowInfo {
            id: WindowId(id),
            title: String::new(),
            app_name: String::new(),
            bounds: Rect {
                x,
                y,
                width: 100,
                height: 100,
            },
        }
    }

    #[test]
    fn the_front_window_wins_and_edges_are_exact() {
        let windows = [window(1, 50, 50), window(2, 0, 0)];
        let at = |p| window_at(p, &windows).map(|w| w.id.0);
        assert_eq!(at((60, 60)), Some(1), "both hit: front first");
        assert_eq!(at((10, 10)), Some(2));
        assert_eq!(at((149, 149)), Some(1));
        assert_eq!(at((150, 150)), None, "right and bottom edges are outside");
    }

    #[test]
    fn finds_the_display_under_a_point_left_of_the_primary() {
        let d = [display(0, 0, 1920, 1080), display(-1280, 0, 1280, 720)];
        assert_eq!(display_at((-1, 10), &d).map(|d| d.bounds.x), Some(-1280));
        assert_eq!(display_at((1919, 1079), &d).map(|d| d.bounds.x), Some(0));
        assert!(display_at((-1, 800), &d).is_none(), "below the small one");
    }

    #[test]
    fn stitching_places_each_display_and_fills_gaps_with_black() {
        let d = [display(0, 0, 2, 2), display(-1, 1, 1, 1)];
        let desktop = desktop_bounds(&d).unwrap();
        assert_eq!((desktop.x, desktop.y), (-1, 0));
        assert_eq!((desktop.width, desktop.height), (3, 2));
        let frame = |w: u32, h: u32, v: u8| Frame {
            width: w,
            height: h,
            rgba: [v, v, v, 255].repeat((w * h) as usize),
            captured_at: SystemTime::now(),
        };
        let parts = [
            (d[0].bounds, frame(2, 2, 200)),
            (d[1].bounds, frame(1, 1, 100)),
        ];
        let out = stitch(&parts, desktop);
        let red: Vec<u8> = out.rgba.as_chunks::<4>().0.iter().map(|p| p[0]).collect();
        assert_eq!(red, [0, 200, 200, 100, 200, 200]);
        assert!(out.rgba.as_chunks::<4>().0.iter().all(|p| p[3] == 255));
        assert!(desktop_bounds(&[]).is_none());
    }

    #[test]
    fn crop_takes_the_right_pixels_with_a_negative_origin_and_clamps() {
        // 3x2 image at (-1, 10), each pixel's red value being its index.
        let desktop = Frame {
            width: 3,
            height: 2,
            rgba: (0..6u8).flat_map(|i| [i, 0, 0, 255]).collect(),
            captured_at: SystemTime::now(),
        };
        let rect = |x, y, width, height| Rect {
            x,
            y,
            width,
            height,
        };
        let red = |f: Frame| f.rgba.iter().step_by(4).copied().collect::<Vec<u8>>();
        let middle = crop(&desktop, (-1, 10), rect(0, 10, 2, 2)).unwrap();
        assert_eq!((middle.width, middle.height), (2, 2));
        assert_eq!(red(middle), [1, 2, 4, 5]);
        let clamped = crop(&desktop, (-1, 10), rect(1, 11, 50, 50)).unwrap();
        assert_eq!((clamped.width, clamped.height), (1, 1));
        assert_eq!(red(clamped), [5]);
        assert!(crop(&desktop, (-1, 10), rect(500, 500, 2, 2)).is_none());
    }

    #[test]
    fn default_mode_is_region() {
        assert_eq!(CaptureMode::default(), CaptureMode::Region);
        assert_eq!(CaptureMode::Screen.label(), "Screen");
    }
}

/// What the settings panel changes (FR-29, US-8.2). Kept in a TOML file; fields missing
/// from an older file take their default, so a file only ever gains keys.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// The schema the file was written with. Raised when a key changes meaning.
    pub version: u32,
    /// Where new captures go (FR-30).
    pub folder: std::path::PathBuf,
    /// What new capture file names start with (FR-30).
    pub prefix: String,
    /// How long an unpinned capture stays in the buffer (FR-25).
    pub retention_days: u32,
    /// Frames per second of recordings (FR-18): 30 or 60.
    pub frame_rate: u32,
    /// Record what the computer plays.
    pub system_audio: bool,
    /// Record the microphone. The bar's mic toggle is the same setting.
    pub microphone: bool,
    /// Seconds counted down on screen before a recording starts; 0 starts at once.
    pub countdown_secs: u32,
    /// The language of the interface (NFR-14).
    pub language: Language,
}

/// The language of the interface (NFR-14).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Language {
    /// The system's language when there is a translation for it, else English.
    #[default]
    Auto,
    En,
    Fr,
}

impl Settings {
    pub const VERSION: u32 = 1;
    pub const FRAME_RATES: [u32; 2] = [30, 60];
    pub const RETENTION_DAYS: std::ops::RangeInclusive<u32> = 1..=365;
    pub const COUNTDOWN_SECS: std::ops::RangeInclusive<u32> = 0..=10;

    /// `~/Captures`, or `./Captures` when there is no home folder.
    pub fn default_folder() -> std::path::PathBuf {
        std::env::var_os("HOME")
            .or_else(|| std::env::var_os("USERPROFILE"))
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| ".".into())
            .join("Captures")
    }

    /// Values a hand-edited file may have put out of range, brought back in.
    pub fn sanitized(mut self) -> Self {
        let days = Self::RETENTION_DAYS;
        self.retention_days = self.retention_days.clamp(*days.start(), *days.end());
        self.countdown_secs = self.countdown_secs.min(*Self::COUNTDOWN_SECS.end());
        if !Self::FRAME_RATES.contains(&self.frame_rate) {
            self.frame_rate = Self::FRAME_RATES[0];
        }
        self.version = Self::VERSION;
        self
    }

    pub fn retention(&self) -> std::time::Duration {
        std::time::Duration::from_secs(u64::from(self.retention_days) * 24 * 60 * 60)
    }
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            version: Self::VERSION,
            folder: Self::default_folder(),
            prefix: "capture".into(),
            retention_days: 7,
            frame_rate: 30,
            system_audio: true,
            microphone: true,
            countdown_secs: 3,
            language: Language::Auto,
        }
    }
}
