//! The annotation editor (US-7.1, FR-27): a window over one capture, with arrows, boxes,
//! text and highlights kept as layers until saved.

use std::sync::Arc;

use cb_annotate::{arrow_parts, corners, default_sizes, History, Point, Rgba, Shape};
use cb_buffer::ItemId;
use egui::{
    pos2, vec2, Align2, CentralPanel, Color32, Context, CursorIcon, FontFamily, FontId, Frame, Key,
    KeyboardShortcut, Margin, Modifiers, Panel, Pos2, Rect, Sense, Stroke, StrokeKind, TextEdit,
    TextureHandle, TextureOptions, Ui, ViewportBuilder, ViewportCommand, ViewportId,
};

use crate::icons::Icon;
use crate::{chip, palette, ChipStyle, Piece, H_IN_GROUP, SEMIBOLD};

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
    Rect,
    Pen,
    Text,
    Highlight,
}

/// Each tool with its icon, its name and the key that picks it.
const TOOLS: [(Tool, Icon, &str, Key); 5] = [
    (Tool::Arrow, Icon::ArrowTool, "Arrow", Key::A),
    (Tool::Rect, Icon::BoxTool, "Box", Key::R),
    (Tool::Pen, Icon::Pencil, "Pen", Key::P),
    (Tool::Text, Icon::TextTool, "Text", Key::T),
    (Tool::Highlight, Icon::Highlighter, "Highlight", Key::H),
];

const COLORS: [(Rgba, &str); 6] = [
    (Rgba([0xFF, 0x3B, 0x30, 0xFF]), "Red"),
    (Rgba([0xFF, 0xCC, 0x00, 0xFF]), "Yellow"),
    (Rgba([0x34, 0xC7, 0x59, 0xFF]), "Green"),
    (Rgba([0x3B, 0x82, 0xF6, 0xFF]), "Blue"),
    (Rgba([0x11, 0x11, 0x11, 0xFF]), "Black"),
    (Rgba([0xFF, 0xFF, 0xFF, 0xFF]), "White"),
];

/// A highlight lets what is under it show through.
const HIGHLIGHT_ALPHA: u8 = 102;
/// Shorter drags are taken as a slip, not a shape, in image pixels.
const MIN_DRAG: f32 = 3.0;
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
    /// The image size, in image pixels.
    size: (f32, f32),
}

impl View {
    fn to_screen(self, p: Point) -> Pos2 {
        self.origin + vec2(p.x, p.y) * self.scale
    }

    /// Kept on the image: a drag past its edge stops at the edge.
    fn to_image(self, p: Pos2) -> Point {
        let v = (p - self.origin) / self.scale;
        Point::new(v.x.clamp(0.0, self.size.0), v.y.clamp(0.0, self.size.1))
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
    stroke: f32,
    text_size: f32,
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
            stroke,
            text_size,
            drag_from: None,
            pen: Vec::new(),
            typing: None,
            confirm_discard: false,
            focused: false,
            view: None,
            toolbar_width: MIN_WIDTH,
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
        if ui.input(|i| i.key_pressed(Key::Escape)) {
            if self.confirm_discard || !self.changed() {
                return Some(EditorResult::Cancel);
            }
            self.confirm_discard = true;
        }
        None
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
                ui.label(egui::RichText::new("Discard your changes?").color(TEXT));
                if chip(
                    ui,
                    &[Piece::Text("Discard")],
                    &style(false),
                    "Close without saving",
                )
                .clicked()
                {
                    result = Some(EditorResult::Cancel);
                }
                if chip(
                    ui,
                    &[Piece::Text("Keep editing")],
                    &style(true),
                    "Back to the annotations",
                )
                .clicked()
                {
                    self.confirm_discard = false;
                }
                return;
            }
            let left = ui.min_rect().left();
            for (tool, icon, name, key) in TOOLS {
                let tip = format!("{name} ({})", key.name());
                if chip(ui, &[Piece::Icon(icon)], &style(self.tool == tool), &tip).clicked() {
                    self.commit_text();
                    self.tool = tool;
                }
            }
            ui.add_space(8.0);
            for (color, name) in COLORS {
                if swatch(ui, color, self.color == color, name).clicked() {
                    self.color = color;
                }
            }
            ui.add_space(8.0);
            let undo = ui.add_enabled_ui(self.history.can_undo(), |ui| {
                chip(
                    ui,
                    &[Piece::Icon(Icon::Undo)],
                    &style(false),
                    "Undo (Ctrl+Z)",
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
                    "Redo (Ctrl+Y)",
                )
                .clicked()
            });
            if redo.inner {
                self.history.redo();
            }
            let original = ui.add_enabled_ui(self.history.can_undo(), |ui| {
                let tip = "Back to the capture as taken; Redo brings the annotations back";
                chip(ui, &[Piece::Text("Original")], &style(false), tip).clicked()
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
                    &[Piece::Text("Save")],
                    &primary,
                    "Save into the capture (Ctrl+S)",
                )
                .clicked()
                {
                    result = Some(self.save());
                }
                if chip(
                    ui,
                    &[Piece::Text("Cancel")],
                    &style(false),
                    "Close without saving (Esc)",
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
        let size = (self.base.width as f32, self.base.height as f32);
        let scale = (1.0 / ppp)
            .min(room.width() / size.0)
            .min(room.height() / size.1)
            .max(f32::EPSILON);
        let rect = Rect::from_center_size(room.center(), vec2(size.0, size.1) * scale);
        let view = View {
            origin: rect.min,
            scale,
            size,
        };
        self.view = Some(view);

        let whole = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
        ui.painter()
            .image(texture.id(), rect, whole, Color32::WHITE);
        let response = ui.interact(
            rect,
            ui.scope_id().with("annotate canvas"),
            Sense::click_and_drag(),
        );
        let painter = ui.painter_at(rect);
        for shape in self.history.shapes() {
            paint(&painter, view, shape);
        }

        if self.tool == Tool::Pen {
            if response.hovered() {
                ui.ctx().set_cursor_icon(CursorIcon::Crosshair);
            }
            self.freehand(&response, &painter, view);
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
        if let Some(shape) = &shape {
            paint(&painter, view, shape);
        }
        if response.drag_stopped() {
            self.drag_from = None;
            if let Some(shape) = shape {
                self.history.add(shape);
            }
        }
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
            }),
            Tool::Rect if w >= MIN_DRAG && h >= MIN_DRAG => Some(Shape::Rect {
                min,
                max,
                color,
                width: self.stroke,
            }),
            Tool::Highlight if w >= MIN_DRAG && h >= MIN_DRAG => Some(Shape::Highlight {
                min,
                max,
                color: color.with_alpha(HIGHLIGHT_ALPHA),
            }),
            _ => None,
        }
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

/// Draw `shape` the way [`cb_annotate::flatten`] will.
fn paint(painter: &egui::Painter, view: View, shape: &Shape) {
    match shape {
        Shape::Arrow {
            from,
            to,
            color,
            width,
        } => {
            let color = color32(*color);
            let parts = arrow_parts(*from, *to, *width);
            let w = width * view.scale;
            let start = view.to_screen(*from);
            painter.line_segment(
                [start, view.to_screen(parts.shaft_end)],
                Stroke::new(w, color),
            );
            painter.circle_filled(start, w / 2.0, color);
            let head = parts.head.iter().map(|p| view.to_screen(*p)).collect();
            painter.add(egui::Shape::convex_polygon(head, color, Stroke::NONE));
        }
        Shape::Rect {
            min,
            max,
            color,
            width,
        } => {
            let rect = Rect::from_min_max(view.to_screen(*min), view.to_screen(*max));
            let stroke = Stroke::new(width * view.scale, color32(*color));
            painter.rect_stroke(rect, 0.0, stroke, StrokeKind::Middle);
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
            let color = color32(*color);
            let w = width * view.scale;
            let points: Vec<Pos2> = points.iter().map(|p| view.to_screen(*p)).collect();
            // Round ends and joins, as the saved image has them.
            for p in &points {
                painter.circle_filled(*p, w / 2.0, color);
            }
            painter.add(egui::Shape::line(points, Stroke::new(w, color)));
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
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
            "five tools, six colours, actions: {narrow}"
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
