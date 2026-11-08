//! The full-desktop overlay over a frozen image of the screens (M2.4, FR-8, FR-9).

use cb_core::{window_at, CaptureMode, Rect, WindowId, WindowInfo};
use egui::{
    pos2, vec2, Align2, CentralPanel, Color32, ColorImage, Context, CursorIcon, FontFamily, FontId,
    Frame, Key, Painter, Pos2, Sense, Stroke, StrokeKind, TextureHandle, TextureOptions, Ui,
    ViewportBuilder, ViewportCommand, ViewportId,
};

use crate::{palette, tr};

const DIM: Color32 = Color32::from_black_alpha(120);
/// How close to a handle the pointer has to be, in points.
const HANDLE_REACH: f32 = 8.0;
const HANDLE_SIZE: f32 = 8.0;

/// What the user decided, which ends the overlay.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OverlayResult {
    Cancelled,
    Window(WindowId),
    /// A selection in global pixels, to capture or to record.
    Region {
        rect: Rect,
        record: bool,
    },
}

/// Which sides of the selection a handle moves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Edges {
    left: bool,
    right: bool,
    top: bool,
    bottom: bool,
}

const fn edges(left: bool, right: bool, top: bool, bottom: bool) -> Edges {
    Edges {
        left,
        right,
        top,
        bottom,
    }
}

/// The eight handles, with their position as fractions of the selection.
const HANDLES: [(Edges, f32, f32); 8] = [
    (edges(true, false, true, false), 0.0, 0.0),
    (edges(false, false, true, false), 0.5, 0.0),
    (edges(false, true, true, false), 1.0, 0.0),
    (edges(true, false, false, false), 0.0, 0.5),
    (edges(false, true, false, false), 1.0, 0.5),
    (edges(true, false, false, true), 0.0, 1.0),
    (edges(false, false, false, true), 0.5, 1.0),
    (edges(false, true, false, true), 1.0, 1.0),
];

/// What a drag is doing, from where it started.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Grab {
    New((i32, i32)),
    Move { from: (i32, i32), rect: Rect },
    Resize { rect: Rect, edges: Edges },
}

/// A rectangle from two opposite corners, at least one pixel in size.
fn from_corners(a: (i32, i32), b: (i32, i32)) -> Rect {
    Rect {
        x: a.0.min(b.0),
        y: a.1.min(b.1),
        width: a.0.abs_diff(b.0).max(1),
        height: a.1.abs_diff(b.1).max(1),
    }
}

/// The selection `grab` gives with the pointer at `at`. `lock` is ⇧: a new selection
/// is square and a resized one keeps its proportions (FR-8).
fn drag_to(grab: Grab, at: (i32, i32), lock: bool) -> Rect {
    match grab {
        Grab::New(anchor) => {
            if !lock {
                return from_corners(anchor, at);
            }
            let side = anchor.0.abs_diff(at.0).max(anchor.1.abs_diff(at.1)) as i32;
            let corner = (
                anchor.0 + side * (at.0 - anchor.0).signum(),
                anchor.1 + side * (at.1 - anchor.1).signum(),
            );
            from_corners(anchor, corner)
        }
        Grab::Move { from, rect } => Rect {
            x: rect.x + at.0 - from.0,
            y: rect.y + at.1 - from.1,
            ..rect
        },
        Grab::Resize { rect, edges } => {
            let right = rect.x + rect.width as i32;
            let bottom = rect.y + rect.height as i32;
            let x0 = if edges.left { at.0 } else { rect.x };
            let x1 = if edges.right { at.0 } else { right };
            let y0 = if edges.top { at.1 } else { rect.y };
            let y1 = if edges.bottom { at.1 } else { bottom };
            let free = from_corners((x0, y0), (x1, y1));
            if !lock {
                return free;
            }
            let ratio = rect.width as f32 / rect.height.max(1) as f32;
            let horizontal = edges.left || edges.right;
            let (width, height) = if horizontal {
                (
                    free.width,
                    ((free.width as f32 / ratio).round() as u32).max(1),
                )
            } else {
                (
                    ((free.height as f32 * ratio).round() as u32).max(1),
                    free.height,
                )
            };
            // The side or corner opposite the handle stays where it is.
            let x = if edges.left {
                right - width as i32
            } else {
                rect.x
            };
            let y = if edges.top {
                bottom - height as i32
            } else {
                rect.y
            };
            Rect {
                x,
                y,
                width,
                height,
            }
        }
    }
}

/// `rect` moved just far enough to be inside `area`, without resizing it if it fits.
fn keep_inside(rect: Rect, area: Rect) -> Rect {
    let fit = |pos: i32, size: u32, start: i32, room: u32| {
        pos.clamp(start, start + room.saturating_sub(size) as i32)
    };
    Rect {
        x: fit(rect.x, rect.width, area.x, area.width),
        y: fit(rect.y, rect.height, area.y, area.height),
        width: rect.width.min(area.width),
        height: rect.height.min(area.height),
    }
}

pub struct Overlay {
    mode: CaptureMode,
    desktop: cb_core::Frame,
    /// The desktop's top-left corner, in global physical pixels.
    origin: (i32, i32),
    windows: Vec<WindowInfo>,
    texture: Option<TextureHandle>,
    focused: bool,
    selection: Option<Rect>,
    grab: Option<Grab>,
    last_region: Option<Rect>,
}

/// A window position to global physical pixels.
fn to_global(pos: Pos2, ppp: f32, origin: (i32, i32)) -> (i32, i32) {
    (
        origin.0 + (pos.x * ppp).round() as i32,
        origin.1 + (pos.y * ppp).round() as i32,
    )
}

/// A global rectangle to window points.
fn to_local(rect: Rect, ppp: f32, origin: (i32, i32)) -> egui::Rect {
    egui::Rect::from_min_size(
        pos2((rect.x - origin.0) as f32, (rect.y - origin.1) as f32) / ppp,
        vec2(rect.width as f32, rect.height as f32) / ppp,
    )
}

/// Darken everything but `hole`.
fn paint_dim(painter: &Painter, screen: egui::Rect, hole: Option<egui::Rect>) {
    let Some(hole) = hole
        .map(|h| h.intersect(screen))
        .filter(|h| h.is_positive())
    else {
        painter.rect_filled(screen, 0.0, DIM);
        return;
    };
    let (s, h) = (screen, hole);
    for band in [
        egui::Rect::from_min_max(s.min, pos2(s.max.x, h.min.y)),
        egui::Rect::from_min_max(pos2(s.min.x, h.max.y), s.max),
        egui::Rect::from_min_max(pos2(s.min.x, h.min.y), pos2(h.min.x, h.max.y)),
        egui::Rect::from_min_max(pos2(h.max.x, h.min.y), pos2(s.max.x, h.max.y)),
    ] {
        painter.rect_filled(band, 0.0, DIM);
    }
}

/// Text on a dark chip, its `anchor` point at `pos`, kept inside `screen`.
fn paint_label(painter: &Painter, screen: egui::Rect, pos: Pos2, anchor: Align2, text: &str) {
    let galley = painter.layout_no_wrap(
        text.to_owned(),
        FontId::new(13.0, FontFamily::Proportional),
        palette::TEXT,
    );
    let size = galley.size() + vec2(16.0, 8.0);
    let rect = anchor.anchor_size(pos, size);
    let rect = rect.translate(vec2(
        (screen.min.x - rect.min.x).max(0.0) + (screen.max.x - rect.max.x).min(0.0),
        (screen.min.y - rect.min.y).max(0.0) + (screen.max.y - rect.max.y).min(0.0),
    ));
    painter.rect_filled(rect, 6.0, palette::BAR);
    painter.galley(rect.min + vec2(8.0, 4.0), galley, palette::TEXT);
}

impl Overlay {
    pub fn new(
        mode: CaptureMode,
        desktop: cb_core::Frame,
        origin: (i32, i32),
        windows: Vec<WindowInfo>,
        last_region: Option<Rect>,
    ) -> Self {
        let mut overlay = Self {
            mode,
            desktop,
            origin,
            windows,
            texture: None,
            focused: false,
            selection: None,
            grab: None,
            last_region,
        };
        // A series of captures of the same area: the last region is already there, ready
        // to capture again, move or replace.
        if mode == CaptureMode::Region {
            overlay.selection = last_region.and_then(|r| r.clamped_to(overlay.area()));
        }
        overlay
    }

    pub fn mode(&self) -> CaptureMode {
        self.mode
    }

    /// The pixels of a selection: what was on screen when the overlay opened.
    pub fn crop(&self, rect: Rect) -> Option<cb_core::Frame> {
        cb_core::crop(&self.desktop, self.origin, rect)
    }

    fn area(&self) -> Rect {
        Rect {
            x: self.origin.0,
            y: self.origin.1,
            width: self.desktop.width,
            height: self.desktop.height,
        }
    }

    /// Draw it in its own OS window. Call each frame until it returns a result.
    pub fn show(&mut self, ctx: &Context) -> Option<OverlayResult> {
        let ppp = ctx.pixels_per_point();
        let builder = ViewportBuilder::default()
            .with_title(tr!("overlay-title"))
            .with_position(pos2(self.origin.0 as f32, self.origin.1 as f32) / ppp)
            .with_inner_size(vec2(self.desktop.width as f32, self.desktop.height as f32) / ppp)
            .with_decorations(false)
            .with_always_on_top()
            .with_resizable(false)
            .with_taskbar(false)
            .with_active(true);
        ctx.show_viewport_immediate(
            ViewportId::from_hash_of("capture overlay"),
            builder,
            |ui, _class| self.ui(ui),
        )
    }

    fn ui(&mut self, ui: &mut Ui) -> Option<OverlayResult> {
        let ctx = ui.ctx().clone();
        if !self.focused {
            self.focused = true;
            ctx.send_viewport_cmd(ViewportCommand::Focus);
        }
        let desktop = &self.desktop;
        let texture = self
            .texture
            .get_or_insert_with(|| {
                let size = [desktop.width as usize, desktop.height as usize];
                let image = ColorImage::from_rgba_unmultiplied(size, &desktop.rgba);
                ctx.load_texture("frozen desktop", image, TextureOptions::NEAREST)
            })
            .clone();

        let mut result = None;
        CentralPanel::default()
            .frame(Frame::NONE)
            .show(ui, |ui| result = self.draw(ui, &texture));
        result
    }

    fn draw(&mut self, ui: &mut Ui, texture: &TextureHandle) -> Option<OverlayResult> {
        let screen = ui.max_rect();
        // One image pixel per screen pixel: an `Image` widget would read pixels as points.
        let whole = egui::Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
        ui.painter()
            .image(texture.id(), screen, whole, Color32::WHITE);
        if self.mode == CaptureMode::Region {
            self.draw_region(ui, screen)
        } else {
            self.draw_window(ui, screen)
        }
    }

    fn draw_window(&mut self, ui: &mut Ui, screen: egui::Rect) -> Option<OverlayResult> {
        let ppp = ui.ctx().pixels_per_point();
        let response = ui.interact(screen, ui.scope_id().with("overlay"), Sense::click());
        ui.ctx().set_cursor_icon(CursorIcon::Crosshair);

        let hovered = response
            .hover_pos()
            .and_then(|pos| window_at(to_global(pos, ppp, self.origin), &self.windows));
        let highlight = hovered.map(|w| to_local(w.bounds, ppp, self.origin));

        let painter = ui.painter();
        paint_dim(painter, screen, highlight);
        if let (Some(window), Some(rect)) = (hovered, highlight) {
            painter.rect_stroke(
                rect,
                0.0,
                Stroke::new(2.0, palette::ACCENT),
                StrokeKind::Inside,
            );
            let at = rect.left_top() + vec2(8.0, 8.0);
            paint_label(painter, screen, at, Align2::LEFT_TOP, &window.title);
        }
        paint_hints(
            painter,
            screen,
            &[tr!("hint-click-window"), tr!("hint-cancel")],
        );

        if ui.input(|i| i.key_pressed(Key::Escape)) || response.secondary_clicked() {
            return Some(OverlayResult::Cancelled);
        }
        if response.clicked() {
            return hovered.map(|w| OverlayResult::Window(w.id));
        }
        None
    }

    /// What a press at `pos` starts: a handle, a move, or a new selection.
    fn grab_at(&self, pos: Pos2, ppp: f32) -> Grab {
        let at = to_global(pos, ppp, self.origin);
        let Some(rect) = self.selection else {
            return Grab::New(at);
        };
        let local = to_local(rect, ppp, self.origin);
        let handle = HANDLES.iter().find(|(_, fx, fy)| {
            let center = local.min + local.size() * vec2(*fx, *fy);
            center.distance(pos) <= HANDLE_REACH
        });
        match handle {
            Some((edges, ..)) => Grab::Resize {
                rect,
                edges: *edges,
            },
            None if local.contains(pos) => Grab::Move { from: at, rect },
            None => Grab::New(at),
        }
    }

    fn draw_region(&mut self, ui: &mut Ui, screen: egui::Rect) -> Option<OverlayResult> {
        let ppp = ui.ctx().pixels_per_point();
        let response = ui.interact(
            screen,
            ui.scope_id().with("overlay"),
            Sense::click_and_drag(),
        );
        let lock = ui.input(|i| i.modifiers.shift);

        if response.drag_started() {
            let start = ui.input(|i| i.pointer.press_origin());
            self.grab = start.map(|pos| self.grab_at(pos, ppp));
        }
        if let (Some(grab), Some(pos)) = (self.grab, response.interact_pointer_pos()) {
            let rect = drag_to(grab, to_global(pos, ppp, self.origin), lock);
            let area = self.area();
            self.selection = match grab {
                Grab::Move { .. } => Some(keep_inside(rect, area)),
                _ => rect.clamped_to(area),
            };
        }
        if response.drag_stopped() {
            self.grab = None;
        }
        let hover = response.hover_pos();
        let hovered_window =
            hover.and_then(|pos| window_at(to_global(pos, ppp, self.origin), &self.windows));
        let on_selection = hover.is_some_and(|pos| !matches!(self.grab_at(pos, ppp), Grab::New(_)));
        if response.clicked() && !on_selection {
            // A click without a drag snaps to the window under it (FR-8).
            self.selection = hovered_window.map(|w| w.bounds).or(self.selection);
        }
        if response.double_clicked() && on_selection {
            if let Some(rect) = self.selection {
                return Some(OverlayResult::Region {
                    rect,
                    record: false,
                });
            }
        }

        ui.ctx().set_cursor_icon(match (self.grab, hover) {
            (None, Some(pos)) => match self.grab_at(pos, ppp) {
                Grab::Move { .. } => CursorIcon::Move,
                Grab::Resize { edges, .. } => resize_cursor(edges),
                Grab::New(_) => CursorIcon::Crosshair,
            },
            (Some(Grab::Move { .. }), _) => CursorIcon::Grabbing,
            (Some(Grab::Resize { edges, .. }), _) => resize_cursor(edges),
            _ => CursorIcon::Crosshair,
        });

        let painter = ui.painter().clone();
        let painter = &painter;
        let snap_hint = (self.selection.is_none() && self.grab.is_none())
            .then_some(hovered_window)
            .flatten();
        if let Some(rect) = self.selection {
            let local = to_local(rect, ppp, self.origin);
            paint_dim(painter, screen, Some(local));
            painter.rect_stroke(
                local,
                0.0,
                Stroke::new(1.5, palette::ACCENT),
                StrokeKind::Outside,
            );
            for (_, fx, fy) in HANDLES {
                let center = local.min + local.size() * vec2(fx, fy);
                let handle = egui::Rect::from_center_size(center, vec2(HANDLE_SIZE, HANDLE_SIZE));
                painter.rect_filled(handle, 2.0, Color32::WHITE);
                painter.rect_stroke(
                    handle,
                    2.0,
                    Stroke::new(1.0, palette::ACCENT),
                    StrokeKind::Outside,
                );
            }
            let size = format!("{} × {} px", rect.width, rect.height);
            let at = local.left_top() - vec2(0.0, 8.0);
            paint_label(painter, screen, at, Align2::LEFT_BOTTOM, &size);
            if self.grab.is_none() {
                for (button, label, record) in action_buttons(local, screen) {
                    let id = ui.scope_id().with(("region action", record));
                    let clicked = ui.interact(button, id, Sense::click());
                    let fill = match (record, clicked.hovered()) {
                        (false, false) => palette::PRIMARY,
                        (false, true) => Color32::WHITE,
                        (true, false) => palette::BAR,
                        (true, true) => palette::CHIP,
                    };
                    let color = if record {
                        palette::TEXT
                    } else {
                        palette::PRIMARY_TEXT
                    };
                    ui.painter().rect_filled(button, 8.0, fill);
                    ui.painter().text(
                        button.center(),
                        Align2::CENTER_CENTER,
                        label,
                        FontId::new(13.0, FontFamily::Proportional),
                        color,
                    );
                    if clicked.clicked() {
                        return Some(OverlayResult::Region { rect, record });
                    }
                }
            }
        } else if let Some(window) = snap_hint {
            let local = to_local(window.bounds, ppp, self.origin);
            paint_dim(painter, screen, Some(local));
            painter.rect_stroke(
                local,
                0.0,
                Stroke::new(1.0, palette::ACCENT),
                StrokeKind::Inside,
            );
        } else {
            paint_dim(painter, screen, None);
        }
        paint_hints(
            painter,
            screen,
            &[
                tr!("hint-drag-region"),
                tr!("hint-click-window"),
                tr!("hint-capture"),
                tr!("hint-record"),
                tr!("hint-last-region"),
                tr!("hint-lock-ratio"),
                tr!("hint-cancel"),
            ],
        );

        if ui.input(|i| i.key_pressed(Key::Escape)) || response.secondary_clicked() {
            return Some(OverlayResult::Cancelled);
        }
        if ui.input(|i| i.key_pressed(Key::Space)) {
            if let Some(last) = self.last_region.and_then(|r| r.clamped_to(self.area())) {
                self.selection = Some(last);
            }
        }
        let rect = self.selection?;
        if ui.input(|i| i.key_pressed(Key::Enter)) {
            return Some(OverlayResult::Region {
                rect,
                record: false,
            });
        }
        ui.input(|i| i.key_pressed(Key::R))
            .then_some(OverlayResult::Region { rect, record: true })
    }
}

/// Where the Capture and Record buttons go for a selection at `selection`: under its
/// right end, or inside it when the screen ends there. Each with its label and whether
/// it records.
fn action_buttons(selection: egui::Rect, screen: egui::Rect) -> [(egui::Rect, String, bool); 2] {
    const SIZE: egui::Vec2 = vec2(96.0, 30.0);
    const GAP: f32 = 6.0;
    let below = selection.bottom() + 10.0;
    let top = if below + SIZE.y <= screen.bottom() {
        below
    } else {
        (selection.bottom() - SIZE.y - 10.0).max(screen.top())
    };
    let right = selection
        .right()
        .clamp(screen.left() + 2.0 * SIZE.x + GAP, screen.right());
    let capture = egui::Rect::from_min_size(pos2(right - SIZE.x, top), SIZE);
    let record = capture.translate(vec2(-(SIZE.x + GAP), 0.0));
    [
        (record, tr!("overlay-record"), true),
        (capture, tr!("overlay-capture"), false),
    ]
}

fn resize_cursor(edges: Edges) -> CursorIcon {
    match (edges.left || edges.right, edges.top || edges.bottom) {
        (true, false) => CursorIcon::ResizeHorizontal,
        (false, true) => CursorIcon::ResizeVertical,
        _ if edges.left == edges.top => CursorIcon::ResizeNwSe,
        _ => CursorIcon::ResizeNeSw,
    }
}

/// The key hints, as a row of chips along the top of the screen.
fn paint_hints(painter: &Painter, screen: egui::Rect, hints: &[String]) {
    let font = FontId::new(13.0, FontFamily::Proportional);
    let galleys: Vec<_> = hints
        .iter()
        .map(|hint| painter.layout_no_wrap(hint.clone(), font.clone(), palette::TEXT))
        .collect();
    let (pad, gap) = (vec2(16.0, 8.0), 6.0);
    let total: f32 = galleys
        .iter()
        .map(|g| g.size().x + pad.x + gap)
        .sum::<f32>()
        - gap;
    let mut x = screen.center().x - total / 2.0;
    let y = screen.top() + 16.0;
    for galley in galleys {
        let rect = egui::Rect::from_min_size(pos2(x, y), galley.size() + pad);
        painter.rect_filled(rect, 6.0, palette::BAR);
        x += rect.width() + gap;
        painter.galley(rect.min + pad / 2.0, galley, palette::TEXT);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: i32, y: i32, width: u32, height: u32) -> Rect {
        Rect {
            x,
            y,
            width,
            height,
        }
    }

    fn region_overlay(last: Option<Rect>) -> Overlay {
        let desktop = cb_core::Frame {
            width: 800,
            height: 600,
            rgba: vec![0; 800 * 600 * 4],
            captured_at: std::time::SystemTime::now(),
        };
        Overlay::new(CaptureMode::Region, desktop, (0, 0), Vec::new(), last)
    }

    /// Clicks at `pos` (`clicks` times) and returns what the overlay answered.
    fn click_overlay(overlay: &mut Overlay, pos: Pos2, clicks: usize) -> Option<OverlayResult> {
        let ctx = Context::default();
        let screen = egui::Rect::from_min_size(Pos2::ZERO, vec2(800.0, 600.0));
        let button = |pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        let mut frames = vec![vec![], vec![egui::Event::PointerMoved(pos)]];
        for _ in 0..clicks {
            frames.push(vec![button(true)]);
            frames.push(vec![button(false)]);
        }
        frames.push(vec![]);
        let mut answer = None;
        for events in frames {
            let input = egui::RawInput {
                screen_rect: Some(screen),
                events,
                ..Default::default()
            };
            let mut output = ctx.run_ui(input, |ui| {
                if let Some(now) = overlay.ui(ui) {
                    answer.get_or_insert(now);
                }
            });
            output.textures_delta.clear();
        }
        answer
    }

    #[test]
    fn the_last_region_is_offered_and_one_click_captures_it() {
        let last = rect(100, 100, 300, 200);
        let screen = egui::Rect::from_min_size(Pos2::ZERO, vec2(800.0, 600.0));
        let local = to_local(last, 1.0, (0, 0));
        let [(record, ..), (capture, ..)] = action_buttons(local, screen);

        let mut overlay = region_overlay(Some(last));
        assert_eq!(overlay.selection, Some(last), "preselected");
        let answer = click_overlay(&mut overlay, capture.center(), 1);
        assert!(
            matches!(answer, Some(OverlayResult::Region { rect, record: false }) if rect == last)
        );
        let answer = click_overlay(&mut region_overlay(Some(last)), record.center(), 1);
        assert!(matches!(
            answer,
            Some(OverlayResult::Region { record: true, .. })
        ));
        let inside = click_overlay(&mut region_overlay(Some(last)), local.center(), 2);
        assert!(
            matches!(inside, Some(OverlayResult::Region { rect, record: false }) if rect == last)
        );

        assert_eq!(region_overlay(None).selection, None);
        let off_screen = rect(5000, 5000, 10, 10);
        assert_eq!(region_overlay(Some(off_screen)).selection, None);
    }

    #[test]
    fn the_buttons_stay_on_screen() {
        let screen = egui::Rect::from_min_size(Pos2::ZERO, vec2(800.0, 600.0));
        for selection in [
            egui::Rect::from_min_size(pos2(0.0, 0.0), vec2(800.0, 600.0)),
            egui::Rect::from_min_size(pos2(0.0, 500.0), vec2(40.0, 100.0)),
        ] {
            for (button, ..) in action_buttons(selection, screen) {
                assert!(screen.contains_rect(button), "{button:?} for {selection:?}");
            }
        }
    }

    #[test]
    fn a_new_selection_works_in_every_direction() {
        let grab = Grab::New((100, 100));
        assert_eq!(drag_to(grab, (150, 180), false), rect(100, 100, 50, 80));
        assert_eq!(drag_to(grab, (50, 20), false), rect(50, 20, 50, 80));
        assert_eq!(drag_to(grab, (100, 100), false), rect(100, 100, 1, 1));
    }

    #[test]
    fn shift_makes_a_new_selection_square_in_the_drag_direction() {
        let grab = Grab::New((100, 100));
        assert_eq!(drag_to(grab, (150, 180), true), rect(100, 100, 80, 80));
        assert_eq!(drag_to(grab, (50, 20), true), rect(20, 20, 80, 80));
    }

    #[test]
    fn handles_move_only_their_sides_and_can_cross_over() {
        let start = rect(100, 100, 200, 100);
        let resize = |handle: usize, at| {
            let grab = Grab::Resize {
                rect: start,
                edges: HANDLES[handle].0,
            };
            drag_to(grab, at, false)
        };
        assert_eq!(resize(0, (50, 80)), rect(50, 80, 250, 120), "top left");
        assert_eq!(resize(4, (350, 999)), rect(100, 100, 250, 100), "right");
        assert_eq!(resize(6, (999, 250)), rect(100, 100, 200, 150), "bottom");
        assert_eq!(
            resize(4, (50, 0)),
            rect(50, 100, 50, 100),
            "right past left"
        );
    }

    #[test]
    fn shift_keeps_proportions_and_the_opposite_corner_when_resizing() {
        let start = rect(100, 100, 200, 100);
        let locked = |handle: usize, at| {
            let grab = Grab::Resize {
                rect: start,
                edges: HANDLES[handle].0,
            };
            drag_to(grab, at, true)
        };
        assert_eq!(
            locked(7, (500, 110)),
            rect(100, 100, 400, 200),
            "bottom right"
        );
        assert_eq!(
            locked(0, (0, 90)),
            rect(0, 50, 300, 150),
            "top left, corner fixed"
        );
        assert_eq!(locked(6, (0, 300)), rect(100, 100, 400, 200), "bottom");
    }

    #[test]
    fn moving_keeps_the_size_and_stays_on_the_desktop() {
        let area = rect(-100, 0, 1000, 500);
        let grab = Grab::Move {
            from: (10, 10),
            rect: rect(0, 0, 200, 100),
        };
        let moved = drag_to(grab, (30, 50), false);
        assert_eq!(moved, rect(20, 40, 200, 100));
        assert_eq!(keep_inside(moved, area), moved);
        let too_far = drag_to(grab, (5000, 5000), false);
        assert_eq!(keep_inside(too_far, area), rect(700, 400, 200, 100));
        let off_left = drag_to(grab, (-5000, 10), false);
        assert_eq!(keep_inside(off_left, area), rect(-100, 0, 200, 100));
    }

    #[test]
    fn global_and_local_agree_at_fractional_scales_and_negative_origins() {
        for ppp in [1.0, 1.25, 1.5, 2.0] {
            for origin in [(0, 0), (-1920, -200)] {
                let rect = Rect {
                    x: origin.0 + 300,
                    y: origin.1 + 40,
                    width: 800,
                    height: 600,
                };
                let local = to_local(rect, ppp, origin);
                assert_eq!(to_global(local.min, ppp, origin), (rect.x, rect.y));
                assert_eq!(
                    to_global(local.max, ppp, origin),
                    (rect.x + 800, rect.y + 600)
                );
            }
        }
    }
}
