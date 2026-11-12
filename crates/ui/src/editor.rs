//! The annotation editor (US-7.1, US-7.2, FR-27): a window over one capture, with arrows,
//! lines, boxes, ellipses, text, highlights, pixelates and a crop kept as layers until
//! saved, and an eraser that takes layers away.

use std::sync::Arc;

use cb_annotate::{
    arrow_parts, corners, curve, default_sizes, mosaic, pixel_rect, shape_at, History, Mosaic,
    Point, Rgba, Shape,
};
use cb_buffer::ItemId;
use egui::{
    pos2, vec2, Align2, CentralPanel, Color32, Context, CursorIcon, FontFamily, FontId, Frame, Key,
    KeyboardShortcut, Margin, Modifiers, Panel, Pos2, Rect, Sense, Stroke, StrokeKind, TextEdit,
    TextureHandle, TextureOptions, Ui, ViewportBuilder, ViewportCommand, ViewportId,
};

use crate::icons::Icon;
use crate::{chip, palette, ChipStyle, Piece, H_IN_GROUP, SEMIBOLD};
use crate::{i18n, tr};

/// What the user decided, which ends the editor.
#[derive(Debug, Clone, PartialEq)]
pub enum EditorResult {
    /// Draw these over the original capture, bottom first. None means back to the original.
    Save(Vec<Shape>),
    /// Nothing changed, or the changes were dropped.
    Cancel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tool {
    Arrow,
    Line,
    Rect,
    Ellipse,
    Pen,
    Text,
    Highlight,
    Pixelate,
    Crop,
    Eraser,
}

impl Tool {
    /// Draws a shape that can be filled.
    fn fills(self) -> bool {
        matches!(self, Tool::Rect | Tool::Ellipse)
    }

    /// Draws with a line, whose width can be picked.
    fn strokes(self) -> bool {
        matches!(
            self,
            Tool::Arrow | Tool::Line | Tool::Rect | Tool::Ellipse | Tool::Pen
        )
    }

    /// Draws a line that can be bent through points.
    fn bends(self) -> bool {
        matches!(self, Tool::Arrow | Tool::Line)
    }
}

/// The line widths offered, as multiples of the one that suits the image, each with how
/// thick it is drawn in its button, in points.
const WIDTHS: [(f32, f32); 5] = [(0.5, 1.5), (1.0, 3.0), (1.5, 4.5), (2.0, 6.0), (3.0, 8.0)];
/// How close to a bend handle a press grabs it, in points on screen.
const HANDLE_REACH: f32 = 9.0;

/// A handle of the line being bent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Handle {
    /// A point the line goes through: 0 is its start, the last one its end.
    Knot(usize),
    /// Half way between knot `i` and the next: dragging it adds a bend there.
    Insert(usize),
}

/// The line or arrow whose bend handles are shown.
#[derive(Debug, Clone, PartialEq)]
struct Bending {
    index: usize,
    /// As it is in the history: once it is not, undo or the eraser took it away.
    shape: Shape,
    /// The handle being dragged, and the shape as dragged so far.
    drag: Option<(Handle, Shape)>,
}

/// Start, bends, end.
fn knots(shape: &Shape) -> Option<Vec<Point>> {
    match shape {
        Shape::Arrow { from, to, via, .. } | Shape::Line { from, to, via, .. } => {
            let mut knots = vec![*from];
            knots.extend_from_slice(via);
            knots.push(*to);
            Some(knots)
        }
        _ => None,
    }
}

/// `shape` with its start, bends and end moved to `knots`.
fn with_knots(shape: &Shape, knots: &[Point]) -> Shape {
    let (from, to) = (knots[0], knots[knots.len() - 1]);
    let via = knots[1..knots.len() - 1].to_vec();
    match shape.clone() {
        Shape::Arrow { color, width, .. } => Shape::Arrow {
            from,
            to,
            color,
            width,
            via,
        },
        Shape::Line { color, width, .. } => Shape::Line {
            from,
            to,
            color,
            width,
            via,
        },
        other => other,
    }
}

/// Where the curve is half way between knot `i` and the next (Catmull-Rom at t = 0.5).
fn midpoint(knots: &[Point], i: usize) -> Point {
    let p0 = knots[i.saturating_sub(1)];
    let (p1, p2) = (knots[i], knots[i + 1]);
    let p3 = knots[(i + 2).min(knots.len() - 1)];
    let at = |a: f32, b: f32, c: f32, d: f32| (9.0 * (b + c) - a - d) / 16.0;
    Point::new(at(p0.x, p1.x, p2.x, p3.x), at(p0.y, p1.y, p2.y, p3.y))
}

/// Each tool with its icon, the message id of its name and the key that picks it.
const TOOLS: [(Tool, Icon, &str, Key); 10] = [
    (Tool::Arrow, Icon::ArrowTool, "tool-arrow", Key::A),
    (Tool::Line, Icon::LineTool, "tool-line", Key::L),
    (Tool::Rect, Icon::BoxTool, "tool-box", Key::R),
    (Tool::Ellipse, Icon::CircleTool, "tool-circle", Key::O),
    (Tool::Pen, Icon::Pencil, "tool-pen", Key::P),
    (Tool::Text, Icon::TextTool, "tool-text", Key::T),
    (Tool::Highlight, Icon::Highlighter, "tool-highlight", Key::H),
    (Tool::Pixelate, Icon::Mosaic, "tool-pixelate", Key::B),
    (Tool::Crop, Icon::Crop, "tool-crop", Key::C),
    (Tool::Eraser, Icon::Eraser, "tool-eraser", Key::E),
];

/// Each colour with the message id of its name.
const COLORS: [(Rgba, &str); 6] = [
    (Rgba([0xFF, 0x3B, 0x30, 0xFF]), "color-red"),
    (Rgba([0xFF, 0xCC, 0x00, 0xFF]), "color-yellow"),
    (Rgba([0x34, 0xC7, 0x59, 0xFF]), "color-green"),
    (Rgba([0x3B, 0x82, 0xF6, 0xFF]), "color-blue"),
    (Rgba([0x11, 0x11, 0x11, 0xFF]), "color-black"),
    (Rgba([0xFF, 0xFF, 0xFF, 0xFF]), "color-white"),
];

/// A highlight lets what is under it show through.
const HIGHLIGHT_ALPHA: u8 = 102;
/// Shorter drags are taken as a slip, not a shape, in image pixels.
const MIN_DRAG: f32 = 3.0;
/// How far from a shape the eraser still takes it, in points on screen.
const ERASER_REACH: f32 = 5.0;
const TOOLBAR_HEIGHT: f32 = 48.0;
const CANVAS_PAD: f32 = 12.0;
/// Until the toolbar has been measured.
const MIN_WIDTH: f32 = 640.0;
/// Between the editing buttons and Cancel / Save.
const TOOLBAR_GAP: f32 = 24.0;
const TOOLBAR_MARGIN: i8 = 10;

fn color32(c: Rgba) -> Color32 {
    let [r, g, b, a] = c.0;
    Color32::from_rgba_unmultiplied(r, g, b, a)
}

/// Where the image is drawn: its top-left corner and points per image pixel.
#[derive(Debug, Clone, Copy, PartialEq)]
struct View {
    origin: Pos2,
    scale: f32,
    /// The image pixel shown at `origin`: not the corner once the image is cropped.
    min: Point,
    /// The size of what is shown, in image pixels.
    size: (f32, f32),
}

impl View {
    fn to_screen(self, p: Point) -> Pos2 {
        self.origin + vec2(p.x - self.min.x, p.y - self.min.y) * self.scale
    }

    /// Kept on what is shown: a drag past its edge stops at the edge.
    fn to_image(self, p: Pos2) -> Point {
        let v = (p - self.origin) / self.scale;
        Point::new(
            self.min.x + v.x.clamp(0.0, self.size.0),
            self.min.y + v.y.clamp(0.0, self.size.1),
        )
    }

    /// Where what is shown is on screen.
    fn rect(self) -> Rect {
        let max = Point::new(self.min.x + self.size.0, self.min.y + self.size.1);
        Rect::from_min_max(self.origin, self.to_screen(max))
    }
}

/// Text being typed, before it becomes a shape.
struct Typing {
    at: Point,
    text: String,
    focus: bool,
}

pub struct Editor {
    item: ItemId,
    base: Arc<cb_core::Frame>,
    title: String,
    texture: Option<TextureHandle>,
    history: History,
    /// The annotations the capture had when the editor opened.
    saved: Vec<Shape>,
    tool: Tool,
    color: Rgba,
    /// Inside colour of new boxes and ellipses; none leaves them empty.
    fill: Option<Rgba>,
    stroke: f32,
    /// The line width that suits the image; the others are multiples of it.
    base_stroke: f32,
    text_size: f32,
    /// Side of a pixelate block, in image pixels.
    block: f32,
    /// What each pixelate looks like, made from the capture once.
    mosaics: Vec<(Shape, TextureHandle, Mosaic)>,
    drag_from: Option<Point>,
    /// The freehand line being drawn.
    pen: Vec<Point>,
    typing: Option<Typing>,
    /// Closing would lose annotations: ask first.
    confirm_discard: bool,
    focused: bool,
    view: Option<View>,
    /// How wide the toolbar needs the window, measured as it is drawn.
    toolbar_width: f32,
    /// The line or arrow drawn or clicked last, with its handles to bend it.
    bending: Option<Bending>,
}

impl Editor {
    /// `base` is the capture as taken; `annotations` are those saved on it before, which
    /// come back as layers that can be changed or undone down to the original.
    pub fn new(
        item: ItemId,
        base: Arc<cb_core::Frame>,
        title: impl Into<String>,
        annotations: Vec<Shape>,
    ) -> Self {
        let (stroke, text_size) = default_sizes(base.width, base.height);
        Self {
            item,
            base,
            title: title.into(),
            texture: None,
            history: History::with_shapes(annotations.clone()),
            saved: annotations,
            tool: Tool::Arrow,
            color: COLORS[0].0,
            fill: None,
            stroke,
            base_stroke: stroke,
            text_size,
            block: stroke * 4.0,
            mosaics: Vec::new(),
            drag_from: None,
            pen: Vec::new(),
            typing: None,
            confirm_discard: false,
            focused: false,
            view: None,
            toolbar_width: MIN_WIDTH,
            bending: None,
        }
    }

    /// The capture being annotated.
    pub fn item(&self) -> ItemId {
        self.item
    }

    /// The image the annotations go on.
    pub fn base(&self) -> &Arc<cb_core::Frame> {
        &self.base
    }

    /// Bring the window to the front, as when Annotate is asked for again.
    pub fn focus(&mut self) {
        self.focused = false;
    }

    /// Draw it in its own OS window. Call each frame until it returns a result.
    pub fn show(&mut self, ctx: &Context) -> Option<EditorResult> {
        let ppp = ctx.pixels_per_point();
        // One image pixel per screen pixel when the screen has the room.
        let image = vec2(self.base.width as f32, self.base.height as f32) / ppp;
        let wanted = image + vec2(2.0 * CANVAS_PAD, 2.0 * CANVAS_PAD + TOOLBAR_HEIGHT);
        let monitor = ctx
            .input(|i| i.viewport().monitor_size)
            .unwrap_or(vec2(1280.0, 800.0));
        let min = vec2(self.toolbar_width, 280.0);
        let size = wanted.min(monitor * 0.85).max(vec2(min.x, 360.0));
        let builder = ViewportBuilder::default()
            .with_title(&self.title)
            .with_inner_size(size)
            .with_min_inner_size(min)
            .with_active(true);
        ctx.show_viewport_immediate(
            ViewportId::from_hash_of(("annotate", self.item.0)),
            builder,
            |ui, _class| self.ui(ui),
        )
    }

    fn ui(&mut self, ui: &mut Ui) -> Option<EditorResult> {
        let ctx = ui.ctx().clone();
        if !self.focused {
            self.focused = true;
            ctx.send_viewport_cmd(ViewportCommand::Focus);
        }
        let base = &self.base;
        let texture = self
            .texture
            .get_or_insert_with(|| {
                let size = [base.width as usize, base.height as usize];
                let image = egui::ColorImage::from_rgba_unmultiplied(size, &base.rgba);
                ctx.load_texture("annotate base", image, TextureOptions::LINEAR)
            })
            .clone();

        if ctx.input(|i| i.viewport().close_requested()) {
            if !self.changed() {
                return Some(EditorResult::Cancel);
            }
            ctx.send_viewport_cmd(ViewportCommand::CancelClose);
            self.confirm_discard = true;
        }

        // Keys typed into the text field are the text's, not shortcuts.
        let typing = self.typing.is_some();
        let mut result = None;
        Panel::top("annotate tools")
            .exact_size(TOOLBAR_HEIGHT)
            .frame(
                Frame::new()
                    .fill(palette::BAR)
                    .inner_margin(Margin::symmetric(TOOLBAR_MARGIN, 8)),
            )
            .show(ui, |ui| result = self.toolbar(ui));
        // Never narrower than the toolbar: its buttons would overlap.
        if let Some(inner) = ctx.input(|i| i.viewport().inner_rect) {
            if inner.width() + 0.5 < self.toolbar_width {
                let size = vec2(self.toolbar_width, inner.height());
                ctx.send_viewport_cmd(ViewportCommand::InnerSize(size));
            }
        }
        CentralPanel::default()
            .frame(Frame::new().fill(palette::SLOT_BG))
            .show(ui, |ui| self.canvas(ui, &texture));
        if !typing {
            result = result.or_else(|| self.keys(ui));
        }
        result
    }

    fn keys(&mut self, ui: &mut Ui) -> Option<EditorResult> {
        let shortcut = |modifiers, key| KeyboardShortcut::new(modifiers, key);
        let (redo, undo, save) = ui.input_mut(|i| {
            // Ctrl+Shift+Z first: Ctrl+Z would match it too.
            let redo = i.consume_shortcut(&shortcut(Modifiers::COMMAND | Modifiers::SHIFT, Key::Z))
                || i.consume_shortcut(&shortcut(Modifiers::COMMAND, Key::Y));
            let undo = i.consume_shortcut(&shortcut(Modifiers::COMMAND, Key::Z));
            let save = i.consume_shortcut(&shortcut(Modifiers::COMMAND, Key::S));
            (redo, undo, save)
        });
        if redo {
            self.history.redo();
        }
        if undo {
            self.history.undo();
        }
        if save {
            return Some(self.save());
        }
        let plain = ui.input(|i| i.modifiers.is_none());
        for (tool, _, _, key) in TOOLS {
            if plain && ui.input(|i| i.key_pressed(key)) {
                self.tool = tool;
            }
        }
        let step = ui.input(|i| {
            i32::from(i.key_pressed(Key::CloseBracket)) - i32::from(i.key_pressed(Key::OpenBracket))
        });
        if plain && step != 0 {
            let now = self.width_choice() as i32;
            let next = (now + step).clamp(0, WIDTHS.len() as i32 - 1) as usize;
            self.stroke = self.base_stroke * WIDTHS[next].0;
        }
        // Esc lets go of the line being bent before it closes anything.
        if self.bending.is_some() && ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape)) {
            self.bending = None;
        }
        if ui.input(|i| i.key_pressed(Key::Escape)) {
            if self.confirm_discard || !self.changed() {
                return Some(EditorResult::Cancel);
            }
            self.confirm_discard = true;
        }
        None
    }

    /// Which of `WIDTHS` the current width is, or the nearest.
    fn width_choice(&self) -> usize {
        let ratio = self.stroke / self.base_stroke;
        (0..WIDTHS.len())
            .min_by(|&a, &b| {
                let off = |i: usize| (WIDTHS[i].0 - ratio).abs();
                off(a).total_cmp(&off(b))
            })
            .unwrap_or(1)
    }

    fn save(&mut self) -> EditorResult {
        self.commit_text();
        if self.changed() {
            EditorResult::Save(self.history.shapes().to_vec())
        } else {
            EditorResult::Cancel
        }
    }

    /// Whether saving would change the capture.
    fn changed(&self) -> bool {
        self.history.shapes() != self.saved
            || self
                .typing
                .as_ref()
                .is_some_and(|t| !t.text.trim().is_empty())
    }

    fn toolbar(&mut self, ui: &mut Ui) -> Option<EditorResult> {
        use palette::*;
        let style = |on: bool| ChipStyle {
            fill: if on { SELECTED } else { CHIP },
            color: TEXT,
            height: H_IN_GROUP,
        };
        let mut result = None;
        ui.horizontal_centered(|ui| {
            ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
            if self.confirm_discard {
                ui.label(egui::RichText::new(tr!("editor-discard-question")).color(TEXT));
                if chip(
                    ui,
                    &[Piece::Text(&tr!("editor-discard"))],
                    &style(false),
                    &tr!("editor-discard-tip"),
                )
                .clicked()
                {
                    result = Some(EditorResult::Cancel);
                }
                if chip(
                    ui,
                    &[Piece::Text(&tr!("editor-keep"))],
                    &style(true),
                    &tr!("editor-keep-tip"),
                )
                .clicked()
                {
                    self.confirm_discard = false;
                }
                return;
            }
            let left = ui.min_rect().left();
            for (tool, icon, name, key) in TOOLS {
                let tip = format!("{} ({})", i18n::text(name, None), key.name());
                if chip(ui, &[Piece::Icon(icon)], &style(self.tool == tool), &tip).clicked() {
                    self.commit_text();
                    self.tool = tool;
                }
            }
            ui.add_space(8.0);
            for (color, name) in COLORS {
                if swatch(ui, color, self.color == color, &i18n::text(name, None)).clicked() {
                    self.color = color;
                }
            }
            let fill = ui.add_enabled_ui(self.tool.fills(), |ui| {
                fill_swatch(ui, self.fill, false, &tr!("editor-fill-tip"))
            });
            egui::Popup::menu(&fill.inner).show(|ui| {
                ui.horizontal(|ui| {
                    let none = tr!("editor-fill-none");
                    if fill_swatch(ui, None, self.fill.is_none(), &none).clicked() {
                        self.fill = None;
                    }
                    for (color, name) in COLORS {
                        let name = i18n::text(name, None);
                        let chosen = self.fill == Some(color);
                        if fill_swatch(ui, Some(color), chosen, &name).clicked() {
                            self.fill = Some(color);
                        }
                    }
                });
            });
            let chosen = self.width_choice();
            let width = ui.add_enabled_ui(self.tool.strokes(), |ui| {
                width_swatch(ui, WIDTHS[chosen].1, false, &tr!("editor-width-tip"))
            });
            egui::Popup::menu(&width.inner).show(|ui| {
                ui.horizontal(|ui| {
                    for (i, (times, thick)) in WIDTHS.into_iter().enumerate() {
                        let name = tr!("editor-width", percent = (times * 100.0) as u32);
                        if width_swatch(ui, thick, i == chosen, &name).clicked() {
                            self.stroke = self.base_stroke * times;
                        }
                    }
                });
            });
            ui.add_space(8.0);
            let undo = ui.add_enabled_ui(self.history.can_undo(), |ui| {
                chip(
                    ui,
                    &[Piece::Icon(Icon::Undo)],
                    &style(false),
                    &tr!("editor-undo-tip"),
                )
                .clicked()
            });
            if undo.inner {
                self.history.undo();
            }
            let redo = ui.add_enabled_ui(self.history.can_redo(), |ui| {
                chip(
                    ui,
                    &[Piece::Icon(Icon::Redo)],
                    &style(false),
                    &tr!("editor-redo-tip"),
                )
                .clicked()
            });
            if redo.inner {
                self.history.redo();
            }
            let original = ui.add_enabled_ui(self.history.can_undo(), |ui| {
                let tip = tr!("editor-original-tip");
                let label = tr!("editor-original");
                chip(ui, &[Piece::Text(&label)], &style(false), &tip).clicked()
            });
            if original.inner {
                self.commit_text();
                self.history.undo_all();
            }
            let editing = ui.min_rect().right() - left;
            let actions = ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let primary = ChipStyle {
                    fill: PRIMARY,
                    color: PRIMARY_TEXT,
                    height: H_IN_GROUP,
                };
                if chip(
                    ui,
                    &[Piece::Text(&tr!("editor-save"))],
                    &primary,
                    &tr!("editor-save-tip"),
                )
                .clicked()
                {
                    result = Some(self.save());
                }
                if chip(
                    ui,
                    &[Piece::Text(&tr!("editor-cancel"))],
                    &style(false),
                    &tr!("editor-cancel-tip"),
                )
                .clicked()
                {
                    if self.changed() {
                        self.confirm_discard = true;
                    } else {
                        result = Some(EditorResult::Cancel);
                    }
                }
            });
            let needed = editing
                + TOOLBAR_GAP
                + actions.response.rect.width()
                + 2.0 * f32::from(TOOLBAR_MARGIN);
            self.toolbar_width = needed.ceil().max(MIN_WIDTH);
        });
        result
    }

    fn canvas(&mut self, ui: &mut Ui, texture: &TextureHandle) {
        let ppp = ui.ctx().pixels_per_point();
        let room = ui.max_rect().shrink(CANVAS_PAD);
        let shapes = self.history.shapes().to_vec();
        // Once cropped, only what Save will keep is shown; Undo brings the rest back.
        let (w, h) = (self.base.width, self.base.height);
        let (x0, y0, x1, y1) = match shapes.iter().rev().find_map(|s| match s {
            Shape::Crop { min, max } => Some(pixel_rect(*min, *max, w, h)),
            _ => None,
        }) {
            Some((x0, y0, x1, y1)) if x1 > x0 && y1 > y0 => (x0, y0, x1, y1),
            _ => (0, 0, w, h),
        };
        let size = ((x1 - x0) as f32, (y1 - y0) as f32);
        let scale = (1.0 / ppp)
            .min(room.width() / size.0)
            .min(room.height() / size.1)
            .max(f32::EPSILON);
        let rect = Rect::from_center_size(room.center(), vec2(size.0, size.1) * scale);
        let view = View {
            origin: rect.min,
            scale,
            min: Point::new(x0 as f32, y0 as f32),
            size,
        };
        self.view = Some(view);

        let uv = Rect::from_min_max(
            pos2(x0 as f32 / w as f32, y0 as f32 / h as f32),
            pos2(x1 as f32 / w as f32, y1 as f32 / h as f32),
        );
        ui.painter().image(texture.id(), rect, uv, Color32::WHITE);
        let response = ui.interact(
            rect,
            ui.scope_id().with("annotate canvas"),
            Sense::click_and_drag(),
        );
        let painter = ui.painter_at(rect);
        self.mosaics.retain(|(s, ..)| shapes.contains(s));
        // Kept only while it is still there, as it was.
        self.bending = self
            .bending
            .take()
            .filter(|b| shapes.get(b.index) == Some(&b.shape));
        let dragged = self
            .bending
            .as_ref()
            .and_then(|b| Some((b.index, b.drag.as_ref()?.1.clone())));
        for (i, shape) in shapes.iter().enumerate() {
            match &dragged {
                Some((at, live)) if *at == i => self.paint_layer(ui.ctx(), &painter, view, live),
                _ => self.paint_layer(ui.ctx(), &painter, view, shape),
            }
        }
        if self.tool.bends() && self.bend(&response, &painter, view) {
            return;
        }

        if self.tool == Tool::Pen {
            if response.hovered() {
                ui.ctx().set_cursor_icon(CursorIcon::Crosshair);
            }
            self.freehand(&response, &painter, view);
            return;
        }
        if self.tool == Tool::Eraser {
            self.erase(&response, view);
            return;
        }
        if self.tool == Tool::Text {
            ui.ctx().set_cursor_icon(CursorIcon::Text);
            let was_typing = self.typing.is_some();
            self.text_field(ui, view);
            // A click away from the text finishes it; the next click starts another.
            if response.clicked() && !was_typing {
                if let Some(pos) = response.interact_pointer_pos() {
                    let at = view.to_image(pos);
                    self.typing = Some(Typing {
                        at: Point::new(at.x, (at.y - self.text_size / 2.0).max(0.0)),
                        text: String::new(),
                        focus: true,
                    });
                }
            }
            return;
        }

        if response.hovered() {
            ui.ctx().set_cursor_icon(CursorIcon::Crosshair);
        }
        if response.drag_started() {
            self.drag_from = ui
                .input(|i| i.pointer.press_origin())
                .map(|pos| view.to_image(pos));
        }
        let Some(from) = self.drag_from else {
            return;
        };
        let Some(to) = response
            .interact_pointer_pos()
            .map(|pos| view.to_image(pos))
        else {
            return;
        };
        let shape = self.shape(from, to);
        match &shape {
            Some(Shape::Crop { min, max }) => paint_crop(&painter, view, *min, *max),
            Some(shape) => self.paint_layer(ui.ctx(), &painter, view, shape),
            None => {}
        }
        if response.drag_stopped() {
            self.drag_from = None;
            if let Some(shape) = shape {
                // A new line or arrow shows its handles at once, to bend it.
                if self.tool.bends() {
                    self.bending = Some(Bending {
                        index: self.history.shapes().len(),
                        shape: shape.clone(),
                        drag: None,
                    });
                }
                self.history.add(shape);
            }
        }
    }

    /// The handles of the line being bent: drag a point to move it, the dot half way
    /// between two points to add a bend there; double-click a bend to take it out. A
    /// click on another line or arrow picks it. Returns whether the pointer was used.
    fn bend(&mut self, response: &egui::Response, painter: &egui::Painter, view: View) -> bool {
        let pointer = response.interact_pointer_pos();
        let press = response.ctx.input(|i| i.pointer.press_origin());
        let near = |at: Point, to: Option<Pos2>| {
            to.is_some_and(|p| view.to_screen(at).distance(p) <= HANDLE_REACH)
        };
        if response.clicked() && self.bending.as_ref().is_none_or(|b| b.drag.is_none()) {
            let shapes = self.history.shapes();
            let reach = ERASER_REACH / view.scale;
            let hit = pointer
                .and_then(|p| shape_at(shapes, view.to_image(p), reach))
                .filter(|&i| knots(&shapes[i]).is_some());
            let on_handle = self.bending.as_ref().is_some_and(|b| {
                let k = knots(&b.shape).unwrap_or_default();
                k.iter().any(|p| near(*p, pointer))
            });
            if !on_handle {
                self.bending = hit.map(|index| Bending {
                    index,
                    shape: shapes[index].clone(),
                    drag: None,
                });
            }
        }
        let Some(bending) = &mut self.bending else {
            return false;
        };
        let Some(points) = knots(&bending.shape) else {
            return false;
        };
        if response.double_clicked() {
            let bend = (1..points.len() - 1).find(|&i| near(points[i], pointer));
            if let Some(i) = bend {
                let mut fewer = points.clone();
                fewer.remove(i);
                let shape = with_knots(&bending.shape, &fewer);
                self.history.replace(bending.index, shape.clone());
                bending.shape = shape;
                bending.drag = None;
                return true;
            }
        }
        if response.drag_started() {
            let knot = (0..points.len()).find(|&i| near(points[i], press));
            let insert = (0..points.len() - 1).find(|&i| near(midpoint(&points, i), press));
            bending.drag = knot
                .map(Handle::Knot)
                .or(insert.map(Handle::Insert))
                .map(|handle| (handle, bending.shape.clone()));
        }
        let Some((handle, _)) = bending.drag else {
            paint_handles(painter, view, &points);
            return false;
        };
        if let Some(at) = pointer.map(|p| view.to_image(p)) {
            let mut moved = points.clone();
            match handle {
                Handle::Knot(i) => moved[i] = at,
                Handle::Insert(i) => moved.insert(i + 1, at),
            }
            let live = with_knots(&bending.shape, &moved);
            paint_handles(painter, view, &knots(&live).unwrap_or_default());
            bending.drag = Some((handle, live));
        }
        if response.drag_stopped() {
            if let Some((_, live)) = bending.drag.take() {
                self.history.replace(bending.index, live.clone());
                bending.shape = live;
            }
        }
        true
    }

    /// Follow the pointer while it is held down; the line becomes a shape when let go.
    fn freehand(&mut self, response: &egui::Response, painter: &egui::Painter, view: View) {
        if response.drag_started() {
            self.pen.clear();
            let start = response.ctx.input(|i| i.pointer.press_origin());
            self.pen.extend(start.map(|pos| view.to_image(pos)));
        }
        if let Some(at) = response.interact_pointer_pos().map(|p| view.to_image(p)) {
            // Points closer than a pixel add nothing but size.
            let far = |last: &Point| (at.x - last.x).hypot(at.y - last.y) >= 1.0;
            if response.dragged() && self.pen.last().is_some_and(far) {
                self.pen.push(at);
            }
        }
        let line = Shape::Pen {
            points: self.pen.clone(),
            color: self.color,
            width: self.stroke,
        };
        if self.pen.len() >= 2 {
            paint(painter, view, &line);
        }
        if response.drag_stopped() && std::mem::take(&mut self.pen).len() >= 2 {
            self.history.add(line);
        }
    }

    /// Take away the topmost shape under the pointer while it is pressed; dragging erases
    /// every shape it passes over, each one its own undo step.
    fn erase(&mut self, response: &egui::Response, view: View) {
        if response.hovered() {
            response.ctx.set_cursor_icon(CursorIcon::Crosshair);
        }
        let pressed = response.is_pointer_button_down_on() || response.clicked();
        let Some(pos) = response.interact_pointer_pos().filter(|_| pressed) else {
            return;
        };
        let reach = ERASER_REACH / view.scale;
        if let Some(i) = shape_at(self.history.shapes(), view.to_image(pos), reach) {
            self.history.erase(i);
        }
    }

    /// The shape a drag from `from` to `to` makes with the current tool, if it is big enough.
    fn shape(&self, from: Point, to: Point) -> Option<Shape> {
        let (min, max) = corners(from, to);
        let (w, h) = (max.x - min.x, max.y - min.y);
        let color = self.color;
        match self.tool {
            Tool::Arrow if w.hypot(h) >= MIN_DRAG => Some(Shape::Arrow {
                from,
                to,
                color,
                width: self.stroke,
                via: Vec::new(),
            }),
            Tool::Line if w.hypot(h) >= MIN_DRAG => Some(Shape::Line {
                from,
                to,
                color,
                width: self.stroke,
                via: Vec::new(),
            }),
            Tool::Rect if w >= MIN_DRAG && h >= MIN_DRAG => Some(Shape::Rect {
                min,
                max,
                color,
                width: self.stroke,
                fill: self.fill,
            }),
            Tool::Ellipse if w >= MIN_DRAG && h >= MIN_DRAG => Some(Shape::Ellipse {
                min,
                max,
                color,
                width: self.stroke,
                fill: self.fill,
            }),
            Tool::Highlight if w >= MIN_DRAG && h >= MIN_DRAG => Some(Shape::Highlight {
                min,
                max,
                color: color.with_alpha(HIGHLIGHT_ALPHA),
            }),
            Tool::Pixelate if w >= MIN_DRAG && h >= MIN_DRAG => Some(Shape::Pixelate {
                min,
                max,
                block: self.block,
            }),
            Tool::Crop if w >= MIN_DRAG && h >= MIN_DRAG => Some(Shape::Crop { min, max }),
            _ => None,
        }
    }

    /// Draw one layer; a pixelate shows the blocks it will save, made from the capture.
    fn paint_layer(&mut self, ctx: &Context, painter: &egui::Painter, view: View, shape: &Shape) {
        let Shape::Pixelate { min, max, block } = shape else {
            return paint(painter, view, shape);
        };
        let cached = self.mosaics.iter().position(|(s, ..)| s == shape);
        let i = cached.unwrap_or_else(|| {
            let b = &self.base;
            let m = mosaic(&b.rgba, b.width, b.height, *min, *max, *block);
            let size = [m.cols as usize, m.rows as usize];
            let image = egui::ColorImage::from_rgba_unmultiplied(size, m.colors.as_flattened());
            let texture = ctx.load_texture("pixelate", image, TextureOptions::NEAREST);
            self.mosaics.push((shape.clone(), texture, m));
            self.mosaics.len() - 1
        });
        let (_, texture, m) = &self.mosaics[i];
        if m.cols == 0 || m.rows == 0 {
            return;
        }
        // chisle: shapes under a pixelate are hidden here and blended in when saved.
        let at = |x: u32, y: u32| view.to_screen(Point::new(x as f32, y as f32));
        let covered = Rect::from_min_max(at(m.x0, m.y0), at(m.x1, m.y1));
        let grid = Rect::from_min_max(
            at(m.x0, m.y0),
            at(m.x0 + m.cols * m.block, m.y0 + m.rows * m.block),
        );
        let whole = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
        painter
            .with_clip_rect(covered.intersect(painter.clip_rect()))
            .image(texture.id(), grid, whole, Color32::WHITE);
    }

    /// The field text is typed into, where it will be drawn. Enter or a click away keeps
    /// the text; Esc drops it.
    fn text_field(&mut self, ui: &mut Ui, view: View) {
        let Some(typing) = &mut self.typing else {
            return;
        };
        let pos = view.to_screen(typing.at);
        let font = FontId::new(
            self.text_size * view.scale,
            FontFamily::Name(SEMIBOLD.into()),
        );
        let width = (view.to_screen(Point::new(view.size.0, 0.0)).x - pos.x).max(40.0);
        let height = ui.ctx().fonts_mut(|f| f.row_height(&font));
        let edit = TextEdit::singleline(&mut typing.text)
            .font(font)
            .text_color(color32(self.color))
            .frame(Frame::NONE)
            .margin(Margin::ZERO)
            .desired_width(width)
            .clip_text(false);
        let response = ui.put(Rect::from_min_size(pos, vec2(width, height)), edit);
        let text_width = ui
            .fonts_mut(|f| {
                f.layout_no_wrap(
                    typing.text.clone(),
                    FontId::new(
                        self.text_size * view.scale,
                        FontFamily::Name(SEMIBOLD.into()),
                    ),
                    Color32::WHITE,
                )
            })
            .size()
            .x;
        let outline =
            Rect::from_min_size(pos, vec2(text_width.max(height / 2.0), height)).expand(3.0);
        ui.painter().rect_stroke(
            outline,
            2.0,
            Stroke::new(1.0, palette::ACCENT),
            StrokeKind::Outside,
        );
        if std::mem::take(&mut typing.focus) {
            response.request_focus();
        }
        if response.lost_focus() {
            if ui.input(|i| i.key_pressed(Key::Escape)) {
                self.typing = None;
            } else {
                self.commit_text();
            }
        }
    }

    /// Text being typed becomes a shape, unless it is blank.
    fn commit_text(&mut self) {
        let Some(typing) = self.typing.take() else {
            return;
        };
        if typing.text.trim().is_empty() {
            return;
        }
        self.history.add(Shape::Text {
            at: typing.at,
            text: typing.text,
            color: self.color,
            size: self.text_size,
        });
    }
}

/// A round colour button; the chosen one has a ring.
fn swatch(ui: &mut Ui, color: Rgba, chosen: bool, name: &str) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(vec2(22.0, H_IN_GROUP), Sense::click());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, name));
    let center = rect.center();
    let p = ui.painter();
    if chosen {
        p.circle_stroke(center, 10.0, Stroke::new(2.0, palette::TEXT));
    }
    p.circle(
        center,
        7.0,
        color32(color),
        Stroke::new(1.0, palette::BORDER),
    );
    response.on_hover_text(name)
}

/// The fill colour as a square, so it reads apart from the round line colours; none is
/// crossed out. The chosen one has a ring.
fn fill_swatch(ui: &mut Ui, fill: Option<Rgba>, chosen: bool, name: &str) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(vec2(22.0, H_IN_GROUP), Sense::click());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, ui.is_enabled(), name));
    let square = Rect::from_center_size(rect.center(), vec2(14.0, 14.0));
    let p = ui.painter();
    let dim = |c: Color32| {
        if ui.is_enabled() {
            c
        } else {
            c.gamma_multiply(0.4)
        }
    };
    if chosen {
        p.rect_stroke(
            square.expand(3.0),
            3.0,
            Stroke::new(2.0, palette::TEXT),
            StrokeKind::Outside,
        );
    }
    let border = Stroke::new(1.0, dim(palette::BORDER));
    match fill {
        Some(color) => {
            p.rect(square, 2.0, dim(color32(color)), border, StrokeKind::Inside);
        }
        None => {
            p.rect_stroke(
                square,
                2.0,
                Stroke::new(1.5, dim(palette::MUTED)),
                StrokeKind::Inside,
            );
            p.line_segment(
                [square.left_bottom(), square.right_top()],
                Stroke::new(1.5, dim(palette::RECORD)),
            );
        }
    }
    response.on_hover_text(name)
}

/// A line width as a short stroke that thick; the chosen one has a ring.
fn width_swatch(ui: &mut Ui, thick: f32, chosen: bool, name: &str) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(vec2(30.0, H_IN_GROUP), Sense::click());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, ui.is_enabled(), name));
    let p = ui.painter();
    let color = if ui.is_enabled() {
        palette::TEXT
    } else {
        palette::TEXT.gamma_multiply(0.4)
    };
    if chosen {
        p.rect_stroke(
            rect.shrink2(vec2(1.0, 4.0)),
            4.0,
            Stroke::new(2.0, palette::TEXT),
            StrokeKind::Inside,
        );
    }
    let c = rect.center();
    p.line_segment(
        [c - vec2(9.0, 0.0), c + vec2(9.0, 0.0)],
        Stroke::new(thick, color),
    );
    response.on_hover_text(name)
}

/// The points a line goes through, and half way between them where a bend can be added.
fn paint_handles(painter: &egui::Painter, view: View, knots: &[Point]) {
    let ring = Stroke::new(1.5, palette::ACCENT);
    for i in 0..knots.len() - 1 {
        let at = view.to_screen(midpoint(knots, i));
        painter.circle(at, 4.0, Color32::from_white_alpha(160), ring);
    }
    for knot in knots {
        painter.circle(view.to_screen(*knot), 6.0, Color32::WHITE, ring);
    }
}

/// Shade what a crop leaves out.
fn paint_crop(painter: &egui::Painter, view: View, min: Point, max: Point) {
    let full = view.rect();
    let keep = Rect::from_min_max(view.to_screen(min), view.to_screen(max));
    let shade = Color32::from_black_alpha(160);
    for (a, b) in [
        (full.min, pos2(full.max.x, keep.min.y)),
        (pos2(full.min.x, keep.max.y), full.max),
        (pos2(full.min.x, keep.min.y), keep.left_bottom()),
        (keep.right_top(), pos2(full.max.x, keep.max.y)),
    ] {
        painter.rect_filled(Rect::from_min_max(a, b), 0.0, shade);
    }
    let edge = Stroke::new(1.0, Color32::WHITE);
    painter.rect_stroke(keep, 0.0, edge, StrokeKind::Outside);
}

/// Draw `shape` the way [`cb_annotate::flatten`] will. Pixelates and crops are drawn by
/// the editor, which knows the capture.
fn paint(painter: &egui::Painter, view: View, shape: &Shape) {
    match shape {
        Shape::Arrow {
            from,
            to,
            color,
            width,
            via,
        } => {
            let color = color32(*color);
            let parts = arrow_parts(&curve(*from, via, *to), *width);
            paint_polyline(painter, view, &parts.shaft, width * view.scale, color);
            let head = parts.head.iter().map(|p| view.to_screen(*p)).collect();
            painter.add(egui::Shape::convex_polygon(head, color, Stroke::NONE));
        }
        Shape::Line {
            from,
            to,
            color,
            width,
            via,
        } => {
            let line = curve(*from, via, *to);
            paint_polyline(painter, view, &line, width * view.scale, color32(*color));
        }
        Shape::Rect {
            min,
            max,
            color,
            width,
            fill,
        } => {
            let rect = Rect::from_min_max(view.to_screen(*min), view.to_screen(*max));
            if let Some(fill) = fill {
                painter.rect_filled(rect, 0.0, color32(*fill));
            }
            let stroke = Stroke::new(width * view.scale, color32(*color));
            painter.rect_stroke(rect, 0.0, stroke, StrokeKind::Middle);
        }
        Shape::Ellipse {
            min,
            max,
            color,
            width,
            fill,
        } => {
            let rect = Rect::from_min_max(view.to_screen(*min), view.to_screen(*max));
            let radius = rect.size() / 2.0;
            if let Some(fill) = fill {
                painter.add(egui::Shape::ellipse_filled(
                    rect.center(),
                    radius,
                    color32(*fill),
                ));
            }
            let stroke = Stroke::new(width * view.scale, color32(*color));
            painter.add(egui::Shape::ellipse_stroke(rect.center(), radius, stroke));
        }
        Shape::Highlight { min, max, color } => {
            let rect = Rect::from_min_max(view.to_screen(*min), view.to_screen(*max));
            painter.rect_filled(rect, 0.0, color32(*color));
        }
        Shape::Pen {
            points,
            color,
            width,
        } => {
            paint_polyline(painter, view, points, width * view.scale, color32(*color));
        }
        Shape::Text {
            at,
            text,
            color,
            size,
        } => {
            let font = FontId::new(size * view.scale, FontFamily::Name(SEMIBOLD.into()));
            painter.text(
                view.to_screen(*at),
                Align2::LEFT_TOP,
                text,
                font,
                color32(*color),
            );
        }
        Shape::Pixelate { .. } | Shape::Crop { .. } => {}
    }
}

/// A line through `points`, `w` points wide, with round ends and joins as the saved
/// image has them.
fn paint_polyline(painter: &egui::Painter, view: View, points: &[Point], w: f32, color: Color32) {
    let points: Vec<Pos2> = points.iter().map(|p| view.to_screen(*p)).collect();
    for p in &points {
        painter.circle_filled(*p, w / 2.0, color);
    }
    painter.add(egui::Shape::line(points, Stroke::new(w, color)));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_tool_and_colour_has_a_name() {
        let ids = TOOLS.iter().map(|t| t.2).chain(COLORS.iter().map(|c| c.1));
        for id in ids {
            assert_ne!(i18n::text(id, None), id, "no message {id}");
        }
    }

    fn editor() -> Editor {
        let base = cb_core::Frame {
            width: 400,
            height: 200,
            rgba: vec![0; 400 * 200 * 4],
            captured_at: std::time::SystemTime::now(),
        };
        Editor::new(ItemId(1), Arc::new(base), "test", Vec::new())
    }

    /// Runs one frame per entry of `frames` and returns the first answer.
    fn run(editor: &mut Editor, frames: Vec<Vec<egui::Event>>) -> Option<EditorResult> {
        let ctx = Context::default();
        crate::install_fonts(&ctx, None);
        let screen = Rect::from_min_size(Pos2::ZERO, vec2(800.0, 600.0));
        let mut answer = None;
        for events in frames {
            let input = egui::RawInput {
                screen_rect: Some(screen),
                events,
                ..Default::default()
            };
            let mut output = ctx.run_ui(input, |ui| {
                if let Some(now) = editor.ui(ui) {
                    answer.get_or_insert(now);
                }
            });
            output.textures_delta.clear();
        }
        answer
    }

    fn button(pos: Pos2, pressed: bool) -> egui::Event {
        egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        }
    }

    fn key(key: Key, modifiers: Modifiers) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers,
        }
    }

    /// Drags on the canvas between two points of the image.
    fn drag(editor: &mut Editor, from: Point, to: Point) {
        run(editor, vec![vec![]]);
        let view = editor.view.expect("drawn once");
        let (a, b) = (view.to_screen(from), view.to_screen(to));
        let mid = a + (b - a) / 2.0;
        run(
            editor,
            vec![
                vec![egui::Event::PointerMoved(a)],
                vec![button(a, true)],
                vec![egui::Event::PointerMoved(mid)],
                vec![egui::Event::PointerMoved(b)],
                vec![button(b, false)],
                vec![],
            ],
        );
    }

    fn close(a: Point, b: Point) -> bool {
        (a.x - b.x).abs() < 1.0 && (a.y - b.y).abs() < 1.0
    }

    #[test]
    fn a_drag_adds_a_box_in_image_pixels() {
        let mut editor = editor();
        editor.tool = Tool::Rect;
        drag(
            &mut editor,
            Point::new(300.0, 150.0),
            Point::new(100.0, 50.0),
        );
        let [Shape::Rect { min, max, .. }] = editor.history.shapes() else {
            panic!("one box: {:?}", editor.history.shapes());
        };
        assert!(close(*min, Point::new(100.0, 50.0)), "{min:?}");
        assert!(close(*max, Point::new(300.0, 150.0)), "{max:?}");
    }

    #[test]
    fn an_arrow_points_where_the_drag_ended() {
        let mut editor = editor();
        drag(
            &mut editor,
            Point::new(20.0, 20.0),
            Point::new(200.0, 100.0),
        );
        let [Shape::Arrow { from, to, .. }] = editor.history.shapes() else {
            panic!("one arrow");
        };
        assert!(close(*from, Point::new(20.0, 20.0)) && close(*to, Point::new(200.0, 100.0)));
    }

    #[test]
    fn the_pen_keeps_the_path_of_the_drag() {
        let mut editor = editor();
        run(&mut editor, vec![vec![key(Key::P, Modifiers::NONE)]]);
        assert_eq!(editor.tool, Tool::Pen);
        drag(
            &mut editor,
            Point::new(20.0, 20.0),
            Point::new(200.0, 100.0),
        );
        let [Shape::Pen { points, .. }] = editor.history.shapes() else {
            panic!("one line: {:?}", editor.history.shapes());
        };
        assert!(points.len() >= 3, "start, middle, end: {points:?}");
        assert!(close(points[0], Point::new(20.0, 20.0)));
        assert!(close(*points.last().unwrap(), Point::new(200.0, 100.0)));
    }

    #[test]
    fn the_window_is_at_least_as_wide_as_the_toolbar() {
        let mut editor = editor();
        run(&mut editor, vec![vec![]]);
        let narrow = editor.toolbar_width;
        assert!(
            narrow > MIN_WIDTH,
            "ten tools, six colours, a fill, actions: {narrow}"
        );
        editor.history.add(Shape::Highlight {
            min: Point::default(),
            max: Point::new(5.0, 5.0),
            color: COLORS[0].0,
        });
        run(&mut editor, vec![vec![]]);
        assert_eq!(
            editor.toolbar_width, narrow,
            "the same buttons, enabled or not"
        );
    }

    #[test]
    fn o_draws_a_filled_ellipse_and_e_erases_it() {
        let mut editor = editor();
        run(&mut editor, vec![vec![key(Key::O, Modifiers::NONE)]]);
        assert_eq!(editor.tool, Tool::Ellipse);
        editor.fill = Some(COLORS[3].0);
        drag(
            &mut editor,
            Point::new(100.0, 50.0),
            Point::new(300.0, 150.0),
        );
        let [Shape::Ellipse { fill, .. }] = editor.history.shapes() else {
            panic!("one ellipse: {:?}", editor.history.shapes());
        };
        assert_eq!(*fill, Some(COLORS[3].0));

        run(&mut editor, vec![vec![key(Key::E, Modifiers::NONE)]]);
        assert_eq!(editor.tool, Tool::Eraser);
        let away = editor.view.unwrap().to_screen(Point::new(20.0, 20.0));
        let inside = editor.view.unwrap().to_screen(Point::new(200.0, 100.0));
        for at in [away, inside] {
            run(
                &mut editor,
                vec![
                    vec![egui::Event::PointerMoved(at)],
                    vec![button(at, true)],
                    vec![button(at, false)],
                    vec![],
                ],
            );
            if at == away {
                assert_eq!(editor.history.shapes().len(), 1, "a miss erases nothing");
            }
        }
        assert!(
            editor.history.shapes().is_empty(),
            "inside a filled ellipse"
        );
        run(&mut editor, vec![vec![key(Key::Z, Modifiers::COMMAND)]]);
        assert_eq!(editor.history.shapes().len(), 1, "undo brings it back");
    }

    #[test]
    fn brackets_pick_the_width_of_the_next_line() {
        let mut editor = editor();
        let base = editor.stroke;
        run(
            &mut editor,
            vec![vec![key(Key::CloseBracket, Modifiers::NONE)]],
        );
        assert_eq!(editor.stroke, base * 1.5);
        drag(&mut editor, Point::new(20.0, 20.0), Point::new(200.0, 20.0));
        let [Shape::Arrow { width, .. }] = editor.history.shapes() else {
            panic!("one arrow: {:?}", editor.history.shapes());
        };
        assert_eq!(*width, base * 1.5);
        for _ in 0..9 {
            run(
                &mut editor,
                vec![vec![key(Key::OpenBracket, Modifiers::NONE)]],
            );
        }
        assert_eq!(editor.stroke, base * 0.5, "stops at the thinnest");
    }

    #[test]
    fn dragging_the_middle_of_a_new_line_bends_it_and_undo_straightens_it() {
        let mut editor = editor();
        editor.tool = Tool::Line;
        drag(
            &mut editor,
            Point::new(20.0, 100.0),
            Point::new(220.0, 100.0),
        );
        assert!(editor.bending.is_some(), "its handles show");
        drag(
            &mut editor,
            Point::new(120.0, 100.0),
            Point::new(120.0, 40.0),
        );
        let [Shape::Line { via, .. }] = editor.history.shapes() else {
            panic!("still one line: {:?}", editor.history.shapes());
        };
        assert!(
            via.len() == 1 && close(via[0], Point::new(120.0, 40.0)),
            "{via:?}"
        );
        // The end moves too, and Esc lets go of the line without closing the editor.
        drag(
            &mut editor,
            Point::new(220.0, 100.0),
            Point::new(250.0, 150.0),
        );
        let [Shape::Line { to, .. }] = editor.history.shapes() else {
            panic!();
        };
        assert!(close(*to, Point::new(250.0, 150.0)));
        let esc = vec![vec![key(Key::Escape, Modifiers::NONE)]];
        assert_eq!(run(&mut editor, esc), None);
        assert!(editor.bending.is_none());
        editor.history.undo();
        editor.history.undo();
        let [Shape::Line { via, .. }] = editor.history.shapes() else {
            panic!();
        };
        assert!(via.is_empty());
    }

    #[test]
    fn l_draws_a_line_where_dragged() {
        let mut editor = editor();
        run(&mut editor, vec![vec![key(Key::L, Modifiers::NONE)]]);
        drag(
            &mut editor,
            Point::new(20.0, 20.0),
            Point::new(200.0, 100.0),
        );
        let [Shape::Line { from, to, .. }] = editor.history.shapes() else {
            panic!("one line: {:?}", editor.history.shapes());
        };
        assert!(close(*from, Point::new(20.0, 20.0)) && close(*to, Point::new(200.0, 100.0)));
    }

    #[test]
    fn a_click_without_a_drag_draws_nothing() {
        let mut editor = editor();
        editor.tool = Tool::Highlight;
        drag(&mut editor, Point::new(50.0, 50.0), Point::new(51.0, 51.0));
        assert!(editor.history.shapes().is_empty());
    }

    #[test]
    fn shortcuts_undo_redo_and_save() {
        let mut editor = editor();
        drag(
            &mut editor,
            Point::new(20.0, 20.0),
            Point::new(200.0, 100.0),
        );
        run(&mut editor, vec![vec![key(Key::Z, Modifiers::COMMAND)]]);
        assert!(editor.history.shapes().is_empty());
        run(&mut editor, vec![vec![key(Key::Y, Modifiers::COMMAND)]]);
        assert_eq!(editor.history.shapes().len(), 1);
        run(&mut editor, vec![vec![key(Key::Z, Modifiers::COMMAND)]]);
        let redo = Modifiers::COMMAND | Modifiers::SHIFT;
        run(&mut editor, vec![vec![key(Key::Z, redo)]]);
        assert_eq!(editor.history.shapes().len(), 1, "Ctrl+Shift+Z redoes");
        let saved = run(&mut editor, vec![vec![key(Key::S, Modifiers::COMMAND)]]);
        assert_eq!(
            saved,
            Some(EditorResult::Save(editor.history.shapes().to_vec()))
        );
    }

    #[test]
    fn esc_closes_at_once_only_when_nothing_would_be_lost() {
        let mut editor = editor();
        let esc = || vec![vec![key(Key::Escape, Modifiers::NONE)]];
        assert_eq!(run(&mut editor, esc()), Some(EditorResult::Cancel));

        let mut editor = self::editor();
        drag(
            &mut editor,
            Point::new(20.0, 20.0),
            Point::new(200.0, 100.0),
        );
        // The new arrow's bend handles go first.
        assert_eq!(run(&mut editor, esc()), None);
        assert!(editor.bending.is_none() && !editor.confirm_discard);
        assert_eq!(run(&mut editor, esc()), None, "asks first");
        assert!(editor.confirm_discard);
        assert_eq!(run(&mut editor, esc()), Some(EditorResult::Cancel));
    }

    #[test]
    fn saved_annotations_reopen_as_layers_down_to_the_original() {
        let shape = Shape::Highlight {
            min: Point::new(1.0, 1.0),
            max: Point::new(9.0, 9.0),
            color: COLORS[1].0,
        };
        let mut editor = Editor {
            history: History::with_shapes(vec![shape.clone()]),
            saved: vec![shape.clone()],
            ..editor()
        };
        let esc = || vec![vec![key(Key::Escape, Modifiers::NONE)]];
        assert_eq!(
            run(&mut editor, esc()),
            Some(EditorResult::Cancel),
            "unchanged"
        );

        editor.history.undo_all();
        assert!(editor.changed());
        let save = vec![vec![key(Key::S, Modifiers::COMMAND)]];
        assert_eq!(
            run(&mut editor, save.clone()),
            Some(EditorResult::Save(Vec::new())),
            "back to the original"
        );
        editor.history.redo();
        assert_eq!(
            run(&mut editor, save),
            Some(EditorResult::Cancel),
            "as it was"
        );
    }

    #[test]
    fn b_pixelates_and_c_crops_what_is_dragged_over() {
        let mut editor = editor();
        run(&mut editor, vec![vec![key(Key::B, Modifiers::NONE)]]);
        assert_eq!(editor.tool, Tool::Pixelate);
        drag(&mut editor, Point::new(10.0, 10.0), Point::new(110.0, 60.0));
        run(&mut editor, vec![vec![key(Key::C, Modifiers::NONE)]]);
        assert_eq!(editor.tool, Tool::Crop);
        drag(
            &mut editor,
            Point::new(390.0, 190.0),
            Point::new(50.0, 20.0),
        );
        let [Shape::Pixelate { min, max, block }, Shape::Crop { min: c0, max: c1 }] =
            editor.history.shapes()
        else {
            panic!("a pixelate, then a crop: {:?}", editor.history.shapes());
        };
        assert!(close(*min, Point::new(10.0, 10.0)) && close(*max, Point::new(110.0, 60.0)));
        assert!(*block >= 12.0, "blocks big enough to hide text: {block}");
        assert!(close(*c0, Point::new(50.0, 20.0)) && close(*c1, Point::new(390.0, 190.0)));
        assert_eq!(editor.mosaics.len(), 1, "only the layers kept are cached");

        // Only what Save keeps is shown now, and shapes still land in image pixels.
        run(&mut editor, vec![vec![]]);
        let view = editor.view.unwrap();
        assert_eq!(view.size, (340.0, 170.0));
        assert!(close(
            view.to_image(view.rect().min),
            Point::new(50.0, 20.0)
        ));
        editor.tool = Tool::Rect;
        drag(
            &mut editor,
            Point::new(60.0, 30.0),
            Point::new(200.0, 100.0),
        );
        let Some(Shape::Rect { min, max, .. }) = editor.history.shapes().last() else {
            panic!("a box on the cropped image");
        };
        assert!(close(*min, Point::new(60.0, 30.0)) && close(*max, Point::new(200.0, 100.0)));

        editor.history.undo_all();
        run(&mut editor, vec![vec![]]);
        assert_eq!(
            editor.view.unwrap().size,
            (400.0, 200.0),
            "Original uncrops"
        );
    }

    #[test]
    fn typed_text_becomes_a_shape_where_it_was_clicked() {
        let mut editor = editor();
        run(&mut editor, vec![vec![key(Key::T, Modifiers::NONE)]]);
        assert_eq!(editor.tool, Tool::Text);
        let at = editor.view.unwrap().to_screen(Point::new(100.0, 100.0));
        run(
            &mut editor,
            vec![
                vec![egui::Event::PointerMoved(at)],
                vec![button(at, true)],
                vec![button(at, false)],
                vec![],
                vec![egui::Event::Text("Look here".into())],
                vec![key(Key::Enter, Modifiers::NONE)],
                vec![],
            ],
        );
        let [Shape::Text { at, text, .. }] = editor.history.shapes() else {
            panic!("one text: {:?}", editor.history.shapes());
        };
        assert_eq!(text, "Look here");
        assert!((at.x - 100.0).abs() < 1.0, "{at:?}");
        assert!(editor.typing.is_none());
    }
}
