//! The bar UI (Epic E2), matching mockup M1 / M2.1 / M2.2.
//! The UI only *returns commands*; it never calls OS or capture code directly.

mod editor;
pub mod i18n;
mod icons;
mod overlay;
mod settings;

pub use editor::{Editor, EditorResult};
pub use overlay::{Overlay, OverlayResult};
pub use settings::{SettingsOutput, SettingsWindow};

use cb_buffer::{Buffer, BufferItem, ItemId, ItemKind, ItemState, Selection};
use cb_core::{CaptureMode, Command};
use chrono::{DateTime, Local};
use egui::{
    emath::RectAlign, vec2, Align2, Color32, CornerRadius, CursorIcon, FontData, FontDefinitions,
    FontFamily, FontId, Frame, Id, Label, Margin, Response, RichText, Role, Sense, Stroke,
    StrokeKind, TextureHandle, Tooltip, Ui, Vec2, WidgetInfo,
};
use icons::Icon;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

pub mod palette {
    use egui::Color32;
    pub const BAR: Color32 = Color32::from_rgb(0x16, 0x17, 0x1A);
    pub const BORDER: Color32 = Color32::from_rgb(0x2E, 0x30, 0x36);
    pub const CHIP: Color32 = Color32::from_rgb(0x20, 0x21, 0x26);
    pub const SELECTED: Color32 = Color32::from_rgb(0x3A, 0x3C, 0x44);
    pub const TEXT: Color32 = Color32::from_rgb(0xEC, 0xEC, 0xEE);
    pub const MUTED: Color32 = Color32::from_rgb(0xA9, 0xAB, 0xB3);
    pub const PRIMARY: Color32 = Color32::from_rgb(0xF2, 0xF2, 0xF0);
    pub const PRIMARY_TEXT: Color32 = Color32::from_rgb(0x14, 0x14, 0x16);
    pub const RECORD: Color32 = Color32::from_rgb(0xFF, 0x5A, 0x36);
    pub const RECORD_BG: Color32 = Color32::from_rgb(0x3A, 0x1A, 0x14);
    pub const ACCENT: Color32 = Color32::from_rgb(0x3B, 0x82, 0xF6);
    /// Behind a thumbnail that does not fill its slot.
    pub const SLOT_BG: Color32 = Color32::from_rgb(0x0E, 0x0F, 0x11);
}

/// The picture area of a card in the tray, in points.
pub const THUMBNAIL_SIZE: Vec2 = vec2(160.0, 100.0);
/// Seconds each frame of a recording's hover preview stays up.
const PREVIEW_FRAME: f32 = 0.25;

/// Everything the bar needs to draw itself.
#[derive(Debug, Clone, Default)]
pub struct BarState {
    pub mode: CaptureMode,
    pub recording: bool,
    /// The recording is paused: the timer stands still and Pause reads Resume.
    pub paused: bool,
    pub elapsed_secs: u64,
    pub tray_open: bool,
    pub mic_on: bool,
    pub cam_on: bool,
    /// Docked to a left or right edge: stacked, icons only (M2.5).
    pub vertical: bool,
    /// Being dragged within snapping distance of an edge.
    pub snap_preview: bool,
    pub tooltip_side: TooltipSide,
    /// How strongly the bar flashes to get noticed, from 0 (not at all) to 1.
    pub attention: f32,
    /// What new capture file names start with (FR-30).
    pub prefix: String,
    /// A region was captured before: the bar offers to capture it again.
    pub has_last_region: bool,
}

/// Which side of the bar has room for tooltips, reserved by the host.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum TooltipSide {
    #[default]
    Below,
    Above,
    Right,
    Left,
}

/// What the user did this frame.
#[derive(Debug, Default)]
pub struct BarOutput {
    pub commands: Vec<Command>,
    /// The grip was pressed: the host should start an OS window drag (FR-2).
    pub start_drag: bool,
    pub toggle_mic: bool,
    pub toggle_cam: bool,
    pub collapse: bool,
    /// The collapsed pill was clicked: back to the full bar.
    pub expand: bool,
    /// The pointer is on the bar, so a tooltip may be about to open.
    pub hovered: bool,
    pub grip_double_clicked: bool,
    /// Where the bar was drawn, in window coordinates.
    pub top_left: Vec2,
    pub size: Vec2,
}

impl BarOutput {
    /// A bar button other than Buffer was pressed: the tray gets out of the way. Buffer
    /// toggles the tray itself, and moving the bar is not a button.
    pub fn closes_tray(&self) -> bool {
        self.toggle_mic
            || self.toggle_cam
            || self.grip_double_clicked
            || self.commands.iter().any(|c| *c != Command::ToggleBuffer)
    }
}

/// Extra window height a tooltip needs above or below the bar: tooltips cannot leave
/// the window.
pub const TOOLTIP_ROOM: f32 = 44.0;
/// The same beside the vertical bar, where a tooltip's whole width has to fit.
pub const TOOLTIP_ROOM_SIDE: f32 = 296.0;
/// Puts tooltips below the bar's edge rather than on its margin.
const TOOLTIP_GAP: f32 = 14.0;

const H: f32 = 36.0;
const H_IN_GROUP: f32 = 32.0;
const PAD_X: f32 = 10.0;
const GAP: f32 = 8.0;
const CHIP_RADIUS: CornerRadius = CornerRadius::same(10);
const SEMIBOLD: &str = "semibold";

/// Use the OS interface font when there is one, or egui's default otherwise.
/// Bar labels need the semibold family this defines, so call it before the first frame.
pub fn install_fonts(ctx: &egui::Context, system: Option<(Vec<u8>, Vec<u8>)>) {
    let mut fonts = FontDefinitions::default();
    let mut semibold = fonts.families[&FontFamily::Proportional].clone();
    if let Some((regular, bold)) = system {
        fonts
            .font_data
            .insert("ui-regular".into(), Arc::new(FontData::from_owned(regular)));
        fonts
            .font_data
            .insert("ui-semibold".into(), Arc::new(FontData::from_owned(bold)));
        semibold.insert(0, "ui-semibold".into());
        if let Some(family) = fonts.families.get_mut(&FontFamily::Proportional) {
            family.insert(0, "ui-regular".into());
        }
    }
    fonts
        .families
        .insert(FontFamily::Name(SEMIBOLD.into()), semibold);
    ctx.set_fonts(fonts);
}

/// The name of a capture mode in the interface's language.
pub fn mode_name(mode: CaptureMode) -> String {
    match mode {
        CaptureMode::Region => tr!("mode-region"),
        CaptureMode::Window => tr!("mode-window"),
        CaptureMode::Screen => tr!("mode-screen"),
    }
}

/// A default shortcut as shown to the user (FR-28, mockup M3).
pub fn shortcut(key: &str) -> String {
    if cfg!(target_os = "macos") {
        format!("⌥⇧{key}")
    } else {
        format!("Ctrl+Shift+{key}")
    }
}

fn is_light(fill: Color32) -> bool {
    u32::from(fill.r()) + u32::from(fill.g()) + u32::from(fill.b()) > 382
}

/// Tint over a chip while hovered or pressed: it has to show on dark and light fills.
fn state_overlay(fill: Color32, pressed: bool) -> Color32 {
    let alpha = if pressed { 48 } else { 22 };
    if is_light(fill) {
        Color32::from_black_alpha(alpha)
    } else {
        Color32::from_white_alpha(alpha)
    }
}

/// One element inside a chip, laid out left to right.
enum Piece<'a> {
    Icon(Icon),
    /// An icon with its own colour.
    Tinted(Icon, Color32),
    Text(&'a str),
    /// A count in a pill.
    Badge(&'a str, Color32),
}

struct ChipStyle {
    fill: Color32,
    color: Color32,
    height: f32,
}

impl ChipStyle {
    const fn new(fill: Color32, color: Color32) -> Self {
        Self {
            fill,
            color,
            height: H,
        }
    }
}

/// A bar button (M2.1). `tooltip` also names it for screen readers.
fn chip(ui: &mut Ui, pieces: &[Piece], style: &ChipStyle, tooltip: &str) -> Response {
    // Stacked chips show their leading icon alone; the tooltip names them.
    let compact = ui.layout().is_vertical();
    let pieces = if compact { &pieces[..1] } else { pieces };
    let label_font = FontId::new(14.0, FontFamily::Name(SEMIBOLD.into()));
    let small_font = FontId::new(11.5, FontFamily::Proportional);

    let galleys: Vec<_> = pieces
        .iter()
        .map(|piece| match piece {
            Piece::Text(t) => Some(ui.painter().layout_no_wrap(
                (*t).to_owned(),
                label_font.clone(),
                style.color,
            )),
            Piece::Badge(t, _) => Some(ui.painter().layout_no_wrap(
                (*t).to_owned(),
                small_font.clone(),
                Color32::WHITE,
            )),
            Piece::Icon(_) | Piece::Tinted(..) => None,
        })
        .collect();
    let widths: Vec<f32> = pieces
        .iter()
        .zip(&galleys)
        .map(|(piece, galley)| {
            let text = galley.as_ref().map_or(0.0, |g| g.size().x);
            match piece {
                Piece::Icon(_) | Piece::Tinted(..) => icons::SIZE,
                Piece::Text(_) => text,
                Piece::Badge(..) => (text + 12.0).max(20.0),
            }
        })
        .collect();
    let content = widths.iter().sum::<f32>() + GAP * (pieces.len() - 1) as f32;
    let width = if compact {
        style.height
    } else {
        (content + PAD_X * 2.0).max(style.height)
    };
    let size = vec2(width, style.height);

    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    response.widget_info(|| WidgetInfo::labeled(Role::Button, true, tooltip));
    focus_ring(ui, &response, CHIP_RADIUS);

    if ui.is_rect_visible(rect) {
        let p = ui.painter();
        p.rect_filled(rect, CHIP_RADIUS, style.fill);
        if response.is_pointer_button_down_on() {
            p.rect_filled(rect, CHIP_RADIUS, state_overlay(style.fill, true));
        } else if response.hovered() {
            p.rect_filled(rect, CHIP_RADIUS, state_overlay(style.fill, false));
        }

        let mut x = rect.center().x - content / 2.0;
        let y = rect.center().y;
        for ((piece, galley), width) in pieces.iter().zip(&galleys).zip(&widths) {
            let center = egui::pos2(x + width / 2.0, y);
            match piece {
                Piece::Icon(icon) => icons::paint(p, center, *icon, style.color),
                Piece::Tinted(icon, color) => icons::paint(p, center, *icon, *color),
                Piece::Badge(_, color) => {
                    let pill = egui::Rect::from_center_size(center, vec2(*width, 20.0));
                    p.rect_filled(pill, CornerRadius::same(10), *color);
                }
                Piece::Text(_) => {}
            }
            if let Some(galley) = galley {
                let pos = center - galley.size() / 2.0;
                p.galley(pos, galley.clone(), style.color);
            }
            x += width + GAP;
        }

        if response.has_focus() {
            p.rect_stroke(
                rect,
                CHIP_RADIUS,
                Stroke::new(2.0, palette::ACCENT),
                StrokeKind::Inside,
            );
        }
    }
    show_tooltip(&response, tooltip);
    response
}

/// A chip that is on or off, told as such to screen readers (NFR-14).
trait OnState {
    fn on_state(self, on: bool, label: &str) -> Self;
}

impl OnState for Response {
    fn on_state(self, on: bool, label: &str) -> Self {
        self.widget_info(|| WidgetInfo::selected(Role::CheckBox, true, on, label));
        self
    }
}

/// The ring around the widget that has the keyboard (NFR-13), drawn over it.
fn focus_ring(ui: &Ui, response: &Response, radius: impl Into<CornerRadius>) {
    if response.has_focus() {
        ui.painter().rect_stroke(
            response.rect.expand(1.0),
            radius,
            Stroke::new(2.0, palette::ACCENT),
            StrokeKind::Outside,
        );
    }
}

fn show_tooltip(response: &Response, text: &str) {
    let side: TooltipSide = response
        .ctx
        .data(|d| d.get_temp(Id::NULL))
        .unwrap_or_default();
    // The fallback stays on the reserved side, flush with the other end of the widget.
    let (align, fallback): (_, &[RectAlign]) = match side {
        TooltipSide::Below => (RectAlign::BOTTOM_START, &[RectAlign::BOTTOM_END]),
        TooltipSide::Above => (RectAlign::TOP_START, &[RectAlign::TOP_END]),
        TooltipSide::Right => (RectAlign::RIGHT_START, &[RectAlign::RIGHT_END]),
        TooltipSide::Left => (RectAlign::LEFT_START, &[RectAlign::LEFT_END]),
    };
    let mut tooltip = Tooltip::for_enabled(response).gap(TOOLTIP_GAP);
    tooltip.popup = tooltip.popup.align(align).align_alternatives(fallback);
    tooltip.show(|ui| ui.label(text));
}

/// A thin divider between groups of controls.
fn divider(ui: &mut Ui) {
    let stroke = Stroke::new(1.0, palette::BORDER);
    if ui.layout().is_vertical() {
        let (rect, _) = ui.allocate_exact_size(vec2(H, 9.0), Sense::hover());
        let inset = rect.width() / 6.0;
        let x = rect.x_range();
        ui.painter()
            .hline((x.min + inset)..=(x.max - inset), rect.center().y, stroke);
    } else {
        let (rect, _) = ui.allocate_exact_size(vec2(9.0, 24.0), Sense::hover());
        ui.painter().vline(rect.center().x, rect.y_range(), stroke);
    }
}

fn fmt_time(secs: u64) -> String {
    format!("{:02}:{:02}", secs / 60, secs % 60)
}

/// The bar's outline colour for the current state.
fn border_color(state: &BarState) -> Color32 {
    if state.snap_preview {
        palette::ACCENT
    } else if state.recording {
        palette::RECORD
    } else {
        palette::BORDER
    }
}

/// A flash over the bar. It takes no room, so the bar does not move while it plays.
fn paint_attention(ui: &Ui, rect: egui::Rect, radius: CornerRadius, level: f32) {
    if level <= 0.0 {
        return;
    }
    let painter = ui.painter();
    painter.rect_filled(rect, radius, palette::ACCENT.gamma_multiply(0.3 * level));
    painter.rect_stroke(
        rect,
        radius,
        Stroke::new(3.0, palette::ACCENT.gamma_multiply(level)),
        StrokeKind::Inside,
    );
}

/// Drag to move, double-click to dock.
fn grip(ui: &mut Ui, vertical: bool, out: &mut BarOutput) {
    let (size, icon) = if vertical {
        (vec2(H, 22.0), Icon::GripWide)
    } else {
        (vec2(22.0, H), Icon::Grip)
    };
    let (rect, grip) = ui.allocate_exact_size(size, Sense::click_and_drag());
    grip.widget_info(|| WidgetInfo::labeled(Role::Button, true, tr!("grip-label")));
    focus_ring(ui, &grip, 4);
    icons::paint(ui.painter(), rect.center(), icon, palette::MUTED);
    let grip = grip.on_hover_cursor(CursorIcon::Grab);
    show_tooltip(&grip, &tr!("grip-tip"));
    if grip.drag_started() {
        out.start_drag = true;
    }
    if grip.double_clicked() {
        out.grip_double_clicked = true;
    }
}

/// Discard the recording (FR-19): the chip or Esc asks first, turning the chip red;
/// a second press within CONFIRM_SECS discards.
fn discard_chip(ui: &mut Ui, out: &mut BarOutput) {
    use palette::*;
    let armed_id = ui.make_persistent_id("discard confirm");
    let now = ui.input(|i| i.time);
    let armed = ui
        .data(|d| d.get_temp::<f64>(armed_id))
        .is_some_and(|at| now - at < CONFIRM_SECS);
    let style = if armed {
        ChipStyle::new(RECORD_BG, RECORD)
    } else {
        ChipStyle::new(CHIP, TEXT)
    };
    let label = tr!("bar-discard");
    let ask = tr!("batch-ask", label = label.as_str());
    let pieces = if armed {
        [Piece::Icon(Icon::Trash), Piece::Text(&ask)]
    } else {
        [Piece::Icon(Icon::Trash), Piece::Text(&label)]
    };
    let tip = if armed {
        tr!("bar-discard-confirm")
    } else {
        tr!("bar-discard-tip")
    };
    let pressed = chip(ui, &pieces, &style, &tip).clicked()
        || ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape));
    if !pressed {
        return;
    }
    if armed {
        ui.data_mut(|d| d.remove::<f64>(armed_id));
        out.commands.push(Command::DiscardRecording);
    } else {
        ui.data_mut(|d| d.insert_temp(armed_id, now));
        ui.ctx().request_repaint_after_secs(CONFIRM_SECS as f32);
    }
}

/// Destructive buttons ask once: the first press turns them red with a question mark,
/// a second press within this many seconds does it. Otherwise they turn back.
const CONFIRM_SECS: f64 = 3.0;

/// Draw the collapsed pill (M2.3): the count, and the time while recording.
pub fn pill(ui: &mut Ui, state: &BarState, buffer: &Buffer) -> BarOutput {
    use palette::*;
    let mut out = BarOutput::default();
    ui.ctx()
        .data_mut(|d| d.insert_temp(Id::NULL, state.tooltip_side));
    let frame = Frame::new()
        .fill(BAR)
        .stroke(Stroke::new(1.0, border_color(state)))
        .corner_radius(CornerRadius::same(24))
        .inner_margin(Margin::same(5))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing = vec2(4.0, 4.0);
                grip(ui, false, &mut out);
                let count = buffer.len().to_string();
                let badge = if buffer.is_empty() { SELECTED } else { ACCENT };
                let time = fmt_time(state.elapsed_secs);
                let [first, second] = if state.recording {
                    [Piece::Tinted(Icon::RecordDot, RECORD), Piece::Text(&time)]
                } else {
                    [Piece::Icon(Icon::Layers), Piece::Text("Capture Bar")]
                };
                if chip(
                    ui,
                    &[first, second, Piece::Badge(&count, badge)],
                    &ChipStyle::new(Color32::TRANSPARENT, TEXT),
                    &tr!("pill-expand"),
                )
                .clicked()
                {
                    out.expand = true;
                }
            });
        });
    paint_attention(
        ui,
        frame.response.rect,
        CornerRadius::same(24),
        state.attention,
    );
    out.hovered = ui.rect_contains_pointer(frame.response.rect);
    out.top_left = frame.response.rect.min.to_vec2();
    out.size = frame.response.rect.size();
    out
}

/// Draw the full bar (M2.1) or the recording bar (M2.2).
pub fn bar(ui: &mut Ui, state: &BarState, buffer: &Buffer) -> BarOutput {
    use palette::*;
    let mut out = BarOutput::default();
    let vertical = state.vertical;
    ui.ctx()
        .data_mut(|d| d.insert_temp(Id::NULL, state.tooltip_side));

    let frame = Frame::new()
        .fill(BAR)
        .stroke(Stroke::new(1.0, border_color(state)))
        .corner_radius(CornerRadius::same(20))
        .inner_margin(Margin::same(8))
        .show(ui, |ui| {
            let content = |ui: &mut Ui| {
                ui.spacing_mut().item_spacing = vec2(4.0, 4.0);

                grip(ui, vertical, &mut out);

                if !state.recording {
                    mode_switch(ui, state.mode, &mut out);
                    divider(ui);
                }

                let capture_key = shortcut("S");
                if chip(
                    ui,
                    &[Piece::Icon(Icon::Camera), Piece::Text(&tr!("bar-capture"))],
                    &ChipStyle::new(PRIMARY, PRIMARY_TEXT),
                    &tr!("bar-capture-tip", key = capture_key.as_str()),
                )
                .clicked()
                {
                    out.commands.push(Command::Capture);
                }
                if !state.recording && state.mode == CaptureMode::Region && state.has_last_region {
                    let again = chip(
                        ui,
                        &[Piece::Icon(Icon::Repeat)],
                        &ChipStyle::new(CHIP, TEXT),
                        &tr!("bar-repeat-tip", key = shortcut("A")),
                    );
                    if again.clicked() {
                        out.commands.push(Command::RepeatLastRegion);
                    }
                }

                let record_key = shortcut("R");
                if state.recording {
                    let time = fmt_time(state.elapsed_secs);
                    if vertical {
                        // Stacked chips drop their text, but the elapsed time must stay.
                        let (rect, _) = ui.allocate_exact_size(vec2(H, 14.0), Sense::hover());
                        ui.painter().text(
                            rect.center(),
                            Align2::CENTER_CENTER,
                            &time,
                            FontId::new(11.5, FontFamily::Proportional),
                            RECORD,
                        );
                    }
                    if chip(
                        ui,
                        &[
                            Piece::Icon(Icon::Stop),
                            Piece::Text(&time),
                            Piece::Text(&tr!("bar-stop")),
                        ],
                        &ChipStyle::new(RECORD_BG, RECORD),
                        &tr!("bar-stop-tip", key = record_key.as_str()),
                    )
                    .clicked()
                    {
                        out.commands.push(Command::ToggleRecording);
                    }
                    let pause_key = shortcut("P");
                    let (icon, label, tip) = if state.paused {
                        (
                            Icon::Play,
                            tr!("bar-resume"),
                            tr!("bar-resume-tip", key = pause_key.as_str()),
                        )
                    } else {
                        (
                            Icon::Pause,
                            tr!("bar-pause"),
                            tr!("bar-pause-tip", key = pause_key.as_str()),
                        )
                    };
                    if chip(
                        ui,
                        &[Piece::Icon(icon), Piece::Text(&label)],
                        &ChipStyle::new(CHIP, TEXT),
                        &tip,
                    )
                    .clicked()
                    {
                        out.commands.push(Command::TogglePause);
                    }
                    discard_chip(ui, &mut out);
                } else if chip(
                    ui,
                    &[
                        Piece::Tinted(Icon::RecordDot, RECORD),
                        Piece::Text(&tr!("bar-record")),
                    ],
                    &ChipStyle::new(CHIP, TEXT),
                    &tr!("bar-record-tip", key = record_key.as_str()),
                )
                .clicked()
                {
                    out.commands.push(Command::ToggleRecording);
                }

                divider(ui);

                let count = buffer.len().to_string();
                let badge = if buffer.is_empty() { SELECTED } else { ACCENT };
                let arrow = if state.tray_open {
                    Icon::ChevronUp
                } else {
                    Icon::ChevronDown
                };
                let buffer_chip = chip(
                    ui,
                    &[
                        Piece::Icon(Icon::Layers),
                        Piece::Text(&tr!("bar-buffer")),
                        Piece::Badge(&count, badge),
                        Piece::Icon(arrow),
                    ],
                    &ChipStyle::new(CHIP, TEXT),
                    &tr!("bar-buffer-tip", key = shortcut("B")),
                );
                if vertical && !buffer.is_empty() {
                    // The count stays visible as a corner badge on the icon-only chip.
                    let center = buffer_chip.rect.right_top() + vec2(-8.0, 8.0);
                    let painter = ui.painter();
                    painter.circle_filled(center, 8.0, ACCENT);
                    painter.text(
                        center,
                        Align2::CENTER_CENTER,
                        &count,
                        FontId::new(10.0, FontFamily::Proportional),
                        Color32::WHITE,
                    );
                }
                if buffer_chip.clicked() {
                    out.commands.push(Command::ToggleBuffer);
                }

                if !state.recording {
                    divider(ui);
                    // On is red, as the Record button: it will be in the recording.
                    let toggle = |on: bool| {
                        if on {
                            ChipStyle::new(CHIP, RECORD)
                        } else {
                            ChipStyle::new(Color32::TRANSPARENT, MUTED)
                        }
                    };
                    let quiet = ChipStyle::new(Color32::TRANSPARENT, MUTED);
                    if chip(
                        ui,
                        &[Piece::Icon(Icon::Mic)],
                        &toggle(state.mic_on),
                        &tr!("bar-mic-tip"),
                    )
                    .on_state(state.mic_on, &tr!("bar-mic"))
                    .clicked()
                    {
                        out.toggle_mic = true;
                    }
                    if chip(
                        ui,
                        &[Piece::Icon(Icon::Video)],
                        &toggle(state.cam_on),
                        &tr!("bar-cam-tip"),
                    )
                    .on_state(state.cam_on, &tr!("bar-cam"))
                    .clicked()
                    {
                        out.toggle_cam = true;
                    }
                    if chip(
                        ui,
                        &[Piece::Icon(Icon::Sliders)],
                        &quiet,
                        &tr!("bar-settings"),
                    )
                    .clicked()
                    {
                        out.commands.push(Command::OpenSettings);
                    }
                    if chip(
                        ui,
                        &[Piece::Icon(Icon::Minus)],
                        &quiet,
                        &tr!("bar-collapse"),
                    )
                    .clicked()
                    {
                        out.collapse = true;
                    }
                }
            };
            if vertical {
                ui.vertical(content);
            } else {
                ui.horizontal(content);
            }
        });

    paint_attention(
        ui,
        frame.response.rect,
        CornerRadius::same(20),
        state.attention,
    );
    out.hovered = ui.rect_contains_pointer(frame.response.rect);
    out.top_left = frame.response.rect.min.to_vec2();
    out.size = frame.response.rect.size();
    out
}

const MODES: [(CaptureMode, Icon, &str); 3] = [
    (CaptureMode::Region, Icon::Region, "1"),
    (CaptureMode::Window, Icon::Window, "2"),
    (CaptureMode::Screen, Icon::Screen, "3"),
];

/// Region / Window / Screen: three icons in a track, with a highlight that slides to the
/// chosen one. The tooltips name them; the icons are enough once known.
fn mode_switch(ui: &mut Ui, current: CaptureMode, out: &mut BarOutput) {
    use palette::*;
    const CELL: f32 = H_IN_GROUP;
    const INSET: f32 = 2.0;
    let vertical = ui.layout().is_vertical();
    let along = move |i: f32| {
        if vertical {
            vec2(0.0, i * CELL)
        } else {
            vec2(i * CELL, 0.0)
        }
    };
    let track_size = along(MODES.len() as f32) + along(1.0).yx() + Vec2::splat(INSET * 2.0);
    let (track, _) = ui.allocate_exact_size(track_size, Sense::hover());
    ui.painter()
        .rect_filled(track, CornerRadius::same(12), CHIP);

    let chosen = MODES.iter().position(|(m, ..)| *m == current).unwrap_or(0) as f32;
    let slide =
        ui.ctx()
            .animate_value_with_time(ui.make_persistent_id("mode highlight"), chosen, 0.14);
    let cell_at = |i: f32| {
        egui::Rect::from_min_size(track.min + Vec2::splat(INSET) + along(i), Vec2::splat(CELL))
    };
    ui.painter()
        .rect_filled(cell_at(slide), CornerRadius::same(10), SELECTED);

    for (i, (mode, icon, key)) in MODES.into_iter().enumerate() {
        let rect = cell_at(i as f32);
        let response = ui.interact(rect, ui.make_persistent_id(("mode", key)), Sense::click());
        let tip = tr!("mode-tip", mode = mode_name(mode), key = shortcut(key));
        response
            .widget_info(|| WidgetInfo::selected(Role::RadioButton, true, mode == current, &tip));
        focus_ring(ui, &response, 6);
        let color = if mode == current {
            Color32::WHITE
        } else if response.hovered() {
            TEXT
        } else {
            MUTED
        };
        if response.hovered() && mode != current {
            ui.painter()
                .rect_filled(rect, CornerRadius::same(10), state_overlay(CHIP, false));
        }
        icons::paint(ui.painter(), rect.center(), icon, color);
        show_tooltip(&response, &tip);
        if response.clicked() {
            out.commands.push(Command::SetMode(mode));
        }
    }
}

/// What a button on the capture toast asked for (M2.6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToastAction {
    Annotate,
    Undo,
}

/// A confirmation beside the bar (M2.6). A capture's confirmation offers Annotate and Undo.
pub fn toast(ui: &mut Ui, message: &str, capture: bool) -> Option<ToastAction> {
    use palette::*;
    let mut action = None;
    Frame::new()
        .fill(BAR)
        .stroke(Stroke::new(1.0, BORDER))
        .corner_radius(CornerRadius::same(12))
        .inner_margin(Margin::symmetric(10, 4))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new(message).color(TEXT).size(13.0));
                if capture {
                    let style = ChipStyle {
                        fill: CHIP,
                        color: TEXT,
                        height: H_IN_GROUP,
                    };
                    let label = tr!("toast-annotate");
                    let annotate = [Piece::Icon(Icon::Pen), Piece::Text(&label)];
                    if chip(ui, &annotate, &style, &tr!("toast-annotate-tip")).clicked() {
                        action = Some(ToastAction::Annotate);
                    }
                    let undo = tr!("toast-undo");
                    if chip(ui, &[Piece::Text(&undo)], &style, &tr!("toast-undo-tip")).clicked() {
                        action = Some(ToastAction::Undo);
                    }
                }
            });
        });
    action
}

/// `text` cut to `max` characters, with an ellipsis when cut.
fn shorten(text: &str, max: usize) -> String {
    if text.chars().count() > max {
        text.chars().take(max - 1).chain(['…']).collect()
    } else {
        text.into()
    }
}

/// Where new captures go and what they are called (FR-30), edited from the tray.
pub struct Destination<'a> {
    /// Edited in place.
    pub prefix: &'a mut String,
    pub folder: &'a Path,
    /// The name the next capture will get, shown in the prefix field's tooltip.
    pub next_file: &'a str,
    /// Put the keyboard in the prefix field this frame.
    pub focus: bool,
    /// How many captures in the buffer are named otherwise than the prefix, when the
    /// user should be asked whether to rename them.
    pub rename_offer: Option<usize>,
}

/// The folder chip and prefix field, right-aligned on the tray's title line.
fn destination_line(ui: &mut Ui, destination: &mut Destination, out: &mut TrayOutput) {
    use palette::*;
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        ui.spacing_mut().item_spacing = vec2(6.0, 0.0);
        let edit = ui.add(
            egui::TextEdit::singleline(destination.prefix)
                .char_limit(40)
                .desired_width(120.0)
                .hint_text(tr!("prefix-hint"))
                .text_color(TEXT)
                .background_color(CHIP)
                .margin(Margin::symmetric(8, 5)),
        );
        if destination.focus {
            edit.request_focus();
        }
        if edit.lost_focus() {
            out.prefix_done = true;
        }
        show_tooltip(&edit, &tr!("prefix-tip", next = destination.next_file));
        let name = destination.folder.file_name().map_or_else(
            || destination.folder.display().to_string(),
            |n| n.to_string_lossy().into_owned(),
        );
        let style = ChipStyle {
            fill: CHIP,
            color: TEXT,
            height: 28.0,
        };
        let tip = tr!(
            "folder-tip",
            folder = destination.folder.display().to_string(),
            key = shortcut("L"),
        );
        let label = shorten(&name, 18);
        if chip(
            ui,
            &[Piece::Icon(Icon::Folder), Piece::Text(&label)],
            &style,
            &tip,
        )
        .clicked()
        {
            out.choose_folder = true;
        }
    });
}

/// "Rename the 3 captures to acme_…?" under the title, after the prefix changed.
fn rename_offer(ui: &mut Ui, count: usize, prefix: &str, out: &mut TrayOutput) {
    use palette::*;
    let to = match prefix.trim() {
        "" => tr!("rename-to-timestamp"),
        p => format!("{}_…", shorten(p, 24)),
    };
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing = vec2(6.0, 0.0);
        ui.label(
            RichText::new(tr!("rename-question", count = count, to = to.as_str())).color(TEXT),
        );
        let style = |fill, color| ChipStyle {
            fill,
            color,
            height: 26.0,
        };
        if chip(
            ui,
            &[Piece::Text(&tr!("rename-yes"))],
            &style(PRIMARY, PRIMARY_TEXT),
            &tr!("rename-yes-tip"),
        )
        .clicked()
        {
            out.rename = Some(true);
        }
        let kept = tr!("rename-no");
        if chip(
            ui,
            &[Piece::Text(&kept)],
            &style(CHIP, TEXT),
            &tr!("rename-no-tip"),
        )
        .clicked()
        {
            out.rename = Some(false);
        }
    });
}

/// What the user asked of one capture in the tray (FR-23).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemAction {
    Copy,
    /// Save a copy elsewhere, through a file dialog.
    Save,
    Annotate,
    /// Open a recording in the system's video player.
    Play,
    Delete,
    /// The picture is being dragged: hand the file to the OS drag and drop.
    DragOut,
    /// The picture was clicked, to pick it for a batch action (FR-24).
    Select(SelectHow),
    /// Keep it past the retention period, or let it go again (FR-25).
    TogglePin,
}

/// How a click on a picture changes the selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectHow {
    /// A plain click: this one alone.
    Only,
    /// Ctrl-click, or Cmd-click on macOS.
    Toggle,
    /// Shift-click: a range.
    Range,
}

/// An action on the selected captures, or on all of them when none is (FR-24).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BatchAction {
    CopyAll,
    /// Save copies into a folder the user picks.
    SaveAll,
    /// Take the captures out of the buffer. Their files stay.
    Clear,
    /// One PDF, a page per image (US-6.7).
    ExportPdf,
    /// Delete the selected captures, their files to the Recycle Bin.
    Delete,
    Deselect,
}

/// What the user did in the tray this frame.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct TrayOutput {
    pub item: Option<(ItemId, ItemAction)>,
    pub batch: Option<BatchAction>,
    /// The folder chip was clicked.
    pub choose_folder: bool,
    /// The prefix field lost the keyboard: the prefix is what the user meant.
    pub prefix_done: bool,
    /// The answer to the rename offer: rename, or keep the names.
    pub rename: Option<bool>,
}

/// One capture in the tray: its picture, then its name, details and actions.
fn tray_card(
    ui: &mut Ui,
    item: &BufferItem,
    thumbnail: Option<&TextureHandle>,
    preview: &[TextureHandle],
    selected: bool,
) -> Option<ItemAction> {
    use palette::*;
    let kind = match item.kind {
        ItemKind::Image => "PNG".to_string(),
        ItemKind::Video { duration_ms } => format!("MP4 {}s", duration_ms / 1000),
    };
    let time = DateTime::<Local>::from(item.created_at).format("%H:%M:%S");
    let details = format!("{kind} · {} · {time}", mode_name(item.mode));
    let name = item.path.as_deref().and_then(Path::file_name).map_or_else(
        || format!("#{}", item.id.0),
        |name| name.to_string_lossy().into_owned(),
    );
    let (status, status_color) = match &item.state {
        ItemState::Ready => (String::new(), MUTED),
        ItemState::Encoding { .. } => (tr!("card-saving"), MUTED),
        ItemState::Failed(_) => (tr!("card-failed"), RECORD),
    };
    let hover = match &item.state {
        ItemState::Failed(error) => error.clone(),
        _ => item
            .path
            .as_deref()
            .map_or(name.clone(), |path| path.display().to_string()),
    };

    // Copy, save and drag need the file; the rest work at any time.
    let has_file = item.state == ItemState::Ready && item.path.is_some();
    let mut action = None;
    // The same border either way, so selecting does not move anything.
    let border = if selected { ACCENT } else { CHIP };
    Frame::new()
        .fill(CHIP)
        .stroke(Stroke::new(2.0, border))
        .corner_radius(CornerRadius::same(8))
        .inner_margin(Margin::same(4))
        .show(ui, |ui| {
            ui.set_width(THUMBNAIL_SIZE.x);
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing = vec2(0.0, 2.0);
                let sense = if has_file {
                    Sense::click_and_drag()
                } else {
                    Sense::click()
                };
                let (slot, response) = ui.allocate_exact_size(THUMBNAIL_SIZE, sense);
                response.widget_info(|| {
                    WidgetInfo::selected(Role::Button, true, selected, format!("{name}, {details}"))
                });
                if response.drag_started() {
                    action = Some(ItemAction::DragOut);
                }
                if response.clicked() {
                    let modifiers = ui.input(|i| i.modifiers);
                    let how = if modifiers.shift {
                        SelectHow::Range
                    } else if modifiers.command {
                        SelectHow::Toggle
                    } else {
                        SelectHow::Only
                    };
                    action = Some(ItemAction::Select(how));
                }
                let response = if has_file {
                    response.on_hover_cursor(egui::CursorIcon::Grab)
                } else {
                    response
                };
                ui.painter()
                    .rect_filled(slot, CornerRadius::same(4), SLOT_BG);
                // A recording plays its preview frames while the cursor is on it.
                let texture = match preview.len() {
                    0 => thumbnail,
                    n if response.hovered() => {
                        let time = ui.input(|i| i.time);
                        ui.ctx()
                            .request_repaint_after(std::time::Duration::from_secs_f32(
                                PREVIEW_FRAME,
                            ));
                        preview.get((time / f64::from(PREVIEW_FRAME)) as usize % n)
                    }
                    _ => thumbnail.or(preview.first()),
                };
                if let Some(texture) = texture {
                    let size = texture.size_vec2();
                    let fit = (slot.width() / size.x).min(slot.height() / size.y).min(1.0);
                    let picture = egui::Rect::from_center_size(slot.center(), size * fit);
                    egui::Image::new(texture)
                        .corner_radius(CornerRadius::same(4))
                        .paint_at(ui, picture);
                }
                focus_ring(ui, &response, 4);
                // On top of the picture, so it takes the clicks there.
                let pin_rect = egui::Rect::from_min_size(
                    slot.right_top() + vec2(-26.0, 4.0),
                    vec2(22.0, 22.0),
                );
                let pin = ui.interact(
                    pin_rect,
                    ui.make_persistent_id(("tray pin", item.id)),
                    Sense::click(),
                );
                pin.widget_info(|| {
                    WidgetInfo::selected(Role::CheckBox, has_file, item.pinned, tr!("card-pin"))
                });
                let pin_seen = response.hovered() || pin.hovered() || pin.has_focus();
                if has_file && (item.pinned || pin_seen) {
                    focus_ring(ui, &pin, 11);
                    let color = if item.pinned { ACCENT } else { TEXT };
                    ui.painter().circle_filled(pin_rect.center(), 11.0, BAR);
                    icons::paint(ui.painter(), pin_rect.center(), Icon::Pin, color);
                    let tip = if item.pinned {
                        tr!("card-unpin-tip")
                    } else {
                        tr!("card-pin-tip")
                    };
                    if pin.on_hover_text(tip).clicked() {
                        action = Some(ItemAction::TogglePin);
                    }
                }
                if !status.is_empty() {
                    ui.painter().text(
                        slot.center(),
                        Align2::CENTER_CENTER,
                        &status,
                        FontId::new(12.0, FontFamily::Proportional),
                        status_color,
                    );
                }
                let hover = if has_file {
                    format!("{hover}\n{}", tr!("card-drag-tip"))
                } else {
                    hover
                };
                let hover = format!("{hover}\n{}", tr!("card-select-tip", key = command_key()));
                response.on_hover_text(hover);
                ui.add(Label::new(RichText::new(name).color(TEXT).size(12.0)).truncate());
                ui.add(Label::new(RichText::new(details).color(MUTED).size(11.0)).truncate());
                ui.add_space(2.0);
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing = vec2(4.0, 0.0);
                    let style = ChipStyle {
                        fill: BAR,
                        color: TEXT,
                        height: 26.0,
                    };
                    let buttons = [
                        (Icon::Copy, tr!("card-copy"), ItemAction::Copy, has_file),
                        (Icon::Save, tr!("card-save"), ItemAction::Save, has_file),
                        match item.kind {
                            ItemKind::Image => (
                                Icon::Pen,
                                tr!("card-annotate"),
                                ItemAction::Annotate,
                                has_file,
                            ),
                            ItemKind::Video { .. } => {
                                (Icon::Play, tr!("card-play"), ItemAction::Play, has_file)
                            }
                        },
                        (Icon::Trash, tr!("card-delete"), ItemAction::Delete, true),
                    ];
                    for (icon, tooltip, what, enabled) in buttons {
                        let clicked = ui
                            .add_enabled_ui(enabled, |ui| {
                                chip(ui, &[Piece::Icon(icon)], &style, &tooltip).clicked()
                            })
                            .inner;
                        if clicked {
                            action = Some(what);
                        }
                    }
                });
            });
        });
    action
}

/// The name of the key that adds one item to a selection.
fn command_key() -> &'static str {
    if cfg!(target_os = "macos") {
        "Cmd"
    } else {
        "Ctrl"
    }
}

/// The batch actions, named for what they act on (FR-24). Delete only appears for a
/// selection: one click must not send every capture to the Recycle Bin.
fn batch_row(ui: &mut Ui, selected: usize) -> Option<BatchAction> {
    use palette::*;
    let style = ChipStyle {
        fill: CHIP,
        color: TEXT,
        height: 28.0,
    };
    let count = selected;
    let mut buttons = vec![
        (
            tr!("batch-copy", count = count),
            tr!("batch-copy-tip", count = count),
            BatchAction::CopyAll,
        ),
        (
            tr!("batch-save", count = count),
            tr!("batch-save-tip", count = count),
            BatchAction::SaveAll,
        ),
        (
            "PDF…".to_string(),
            tr!("batch-pdf-tip", count = count),
            BatchAction::ExportPdf,
        ),
        (
            tr!("batch-clear", count = count),
            tr!("batch-clear-tip", count = count),
            BatchAction::Clear,
        ),
    ];
    if selected > 0 {
        buttons.push((
            tr!("batch-delete", count = count),
            tr!("batch-delete-tip", count = count),
            BatchAction::Delete,
        ));
        buttons.push((
            tr!("batch-deselect"),
            tr!("batch-deselect-tip"),
            BatchAction::Deselect,
        ));
    }
    // Clear and Delete ask once (CONFIRM_SECS).
    let armed_id = ui.make_persistent_id("batch confirm");
    let now = ui.input(|i| i.time);
    let armed: Option<(BatchAction, f64)> = ui
        .data(|d| d.get_temp(armed_id))
        .filter(|&(_, at): &(BatchAction, f64)| now - at < CONFIRM_SECS);
    let mut arm = armed;
    let mut action = None;
    let confirm = tr!("batch-confirm");
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing = vec2(6.0, 0.0);
        for (label, tooltip, what) in &buttons {
            let asks = matches!(what, BatchAction::Clear | BatchAction::Delete);
            let asking = armed.is_some_and(|(a, _)| a == *what);
            let (label, tooltip, style) = if asking {
                let red = ChipStyle {
                    fill: RECORD_BG,
                    color: RECORD,
                    height: style.height,
                };
                (
                    tr!("batch-ask", label = label.as_str()),
                    confirm.as_str(),
                    red,
                )
            } else {
                (label.clone(), tooltip.as_str(), ChipStyle { ..style })
            };
            if chip(ui, &[Piece::Text(&label)], &style, tooltip).clicked() {
                if asks && !asking {
                    arm = Some((*what, now));
                    ui.ctx().request_repaint_after_secs(CONFIRM_SECS as f32);
                } else {
                    arm = None;
                    action = Some(*what);
                }
            }
        }
    });
    ui.data_mut(|d| match arm {
        Some(armed) => {
            d.insert_temp(armed_id, armed);
        }
        None => d.remove::<(BatchAction, f64)>(armed_id),
    });
    action
}

/// The buffer tray (M1, FR-22), `width` points wide. Cards in `selection` are highlighted.
/// A recording with `previews` frames plays them while the cursor is on its picture.
pub fn tray(
    ui: &mut Ui,
    buffer: &Buffer,
    selection: &Selection,
    thumbnails: &HashMap<ItemId, TextureHandle>,
    previews: &HashMap<ItemId, Vec<TextureHandle>>,
    mut destination: Destination,
    width: f32,
) -> TrayOutput {
    use palette::*;
    let mut out = TrayOutput::default();
    Frame::new()
        .fill(BAR)
        .stroke(Stroke::new(1.0, BORDER))
        .corner_radius(CornerRadius::same(16))
        .inner_margin(Margin::same(12))
        .show(ui, |ui| {
            // Less the margin and the border on both sides.
            ui.set_width((width - 26.0).max(1.0));
            let title = match selection.len() {
                0 => tr!("tray-title", count = buffer.len()),
                n => tr!("tray-title-selected", count = buffer.len(), selected = n),
            };
            ui.horizontal(|ui| {
                ui.label(RichText::new(title).color(TEXT).strong());
                destination_line(ui, &mut destination, &mut out);
            });
            if let Some(count) = destination.rename_offer {
                rename_offer(ui, count, destination.prefix, &mut out);
            }
            if !buffer.is_empty() {
                out.batch = batch_row(ui, selection.len());
            }
            if buffer.is_empty() {
                ui.label(
                    RichText::new(tr!(
                        "tray-empty",
                        capture = shortcut("S"),
                        record = shortcut("R"),
                    ))
                    .color(MUTED),
                );
                return;
            }
            egui::ScrollArea::horizontal().show(ui, |ui| {
                ui.horizontal(|ui| {
                    for item in buffer.newest_first() {
                        let selected = selection.contains(item.id);
                        let thumbnail = thumbnails.get(&item.id);
                        let preview = previews.get(&item.id).map_or(&[][..], Vec::as_slice);
                        if let Some(what) = tray_card(ui, item, thumbnail, preview, selected) {
                            out.item = Some((item.id, what));
                        }
                    }
                });
            });
        });
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::Key;

    /// The tray with an empty prefix and no rename offer.
    fn tray_plain(
        ui: &mut Ui,
        buffer: &Buffer,
        selection: &Selection,
        thumbnails: &HashMap<ItemId, TextureHandle>,
        width: f32,
    ) -> TrayOutput {
        let mut prefix = String::new();
        let destination = Destination {
            prefix: &mut prefix,
            folder: Path::new("C:/Captures"),
            next_file: "x.png",
            focus: false,
            rename_offer: None,
        };
        tray(
            ui,
            buffer,
            selection,
            thumbnails,
            &HashMap::new(),
            destination,
            width,
        )
    }

    #[test]
    fn overlay_contrasts_with_fill() {
        let on_light = state_overlay(palette::PRIMARY, false);
        let on_dark = state_overlay(palette::CHIP, false);
        let on_clear = state_overlay(Color32::TRANSPARENT, false);
        assert_eq!(on_light.r(), 0, "light chips darken");
        assert!(on_dark.r() > 0, "dark chips lighten");
        assert!(on_clear.r() > 0, "transparent chips sit on the dark bar");
        assert!(state_overlay(palette::CHIP, true).a() > on_dark.a());
    }

    #[test]
    fn shortcut_uses_platform_modifiers() {
        let expected = if cfg!(target_os = "macos") {
            "⌥⇧S"
        } else {
            "Ctrl+Shift+S"
        };
        assert_eq!(shortcut("S"), expected);
    }

    fn bar_size(state: &BarState, window_width: f32) -> Vec2 {
        let ctx = egui::Context::default();
        install_fonts(&ctx, None);
        let buffer = Buffer::new();
        let mut size = Vec2::ZERO;
        for _ in 0..2 {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    vec2(window_width, 200.0),
                )),
                ..Default::default()
            };
            let mut output = ctx.run_ui(input, |ui| {
                bar(ui, state, &buffer);
                size = ui.min_rect().size();
            });
            output.textures_delta.clear();
        }
        size
    }

    /// Run `pill` on a clean context, clicking at `click` if given.
    fn run_pill(click: Option<egui::Pos2>) -> BarOutput {
        let ctx = egui::Context::default();
        install_fonts(&ctx, None);
        let buffer = Buffer::new();
        let mut out = BarOutput::default();
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, vec2(600.0, 200.0));
        let frames: Vec<Vec<egui::Event>> = match click {
            None => vec![Vec::new(), Vec::new()],
            Some(pos) => {
                let button = |pressed| egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                };
                vec![
                    Vec::new(),
                    vec![egui::Event::PointerMoved(pos)],
                    vec![button(true)],
                    vec![button(false)],
                ]
            }
        };
        let mut expanded = false;
        for events in frames {
            let input = egui::RawInput {
                screen_rect: Some(screen),
                events,
                ..Default::default()
            };
            let mut output = ctx.run_ui(input, |ui| {
                out = pill(ui, &BarState::default(), &buffer);
                expanded |= out.expand;
            });
            output.textures_delta.clear();
        }
        out.expand = expanded;
        out
    }

    #[test]
    fn toast_reports_a_click_on_undo_only_when_it_has_one() {
        let click_near_right = |can_undo: bool| {
            let ctx = egui::Context::default();
            install_fonts(&ctx, None);
            let mut clicked = false;
            let mut width = 0.0;
            let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, vec2(600.0, 100.0));
            for frame in 0..4 {
                let pos = egui::pos2(width - 24.0, 18.0);
                let button = |pressed| egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                };
                let events = match frame {
                    1 => vec![egui::Event::PointerMoved(pos)],
                    2 => vec![button(true)],
                    3 => vec![button(false)],
                    _ => Vec::new(),
                };
                let input = egui::RawInput {
                    screen_rect: Some(screen),
                    events,
                    ..Default::default()
                };
                let mut output = ctx.run_ui(input, |ui| {
                    let top = ui.next_widget_position();
                    clicked |=
                        toast(ui, "Added to buffer · 1", can_undo) == Some(ToastAction::Undo);
                    width = ui.min_rect().right() - top.x;
                });
                output.textures_delta.clear();
            }
            clicked
        };
        assert!(click_near_right(true));
        assert!(!click_near_right(false));
    }

    /// The tray's meshes and texts, run until its layout has settled.
    fn tray_shapes(buffer: &Buffer, thumbnails: &HashMap<ItemId, TextureHandle>) -> Vec<String> {
        let ctx = egui::Context::default();
        install_fonts(&ctx, None);
        let mut seen = Vec::new();
        for _ in 0..3 {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    vec2(900.0, 400.0),
                )),
                ..Default::default()
            };
            let mut output = ctx.run_ui(input, |ui| {
                tray_plain(ui, buffer, &Selection::default(), thumbnails, 800.0);
            });
            output.textures_delta.clear();
            seen = output
                .shapes
                .iter()
                .filter_map(|clipped| match &clipped.shape {
                    egui::Shape::Text(text) => Some(text.galley.text().to_owned()),
                    shape if shape.texture_id() != egui::TextureId::default() => {
                        Some(format!("{:?}", shape.texture_id()))
                    }
                    _ => None,
                })
                .collect();
        }
        seen
    }

    #[test]
    fn tray_shows_thumbnails_newest_first_with_the_count() {
        let ctx = egui::Context::default();
        let texture = |name: &str| {
            let image = egui::ColorImage::new([2, 2], vec![Color32::RED; 4]);
            ctx.load_texture(name, image, egui::TextureOptions::LINEAR)
        };
        let mut buffer = Buffer::new();
        let older = buffer.push(ItemKind::Image, CaptureMode::Region);
        let newer = buffer.push(ItemKind::Image, CaptureMode::Screen);
        let without = buffer.push(ItemKind::Image, CaptureMode::Window);
        buffer.get_mut(without).unwrap().state = ItemState::Encoding { progress_pct: 0 };
        let (old_pic, new_pic) = (texture("older"), texture("newer"));
        let thumbnails = HashMap::from([(older, old_pic.clone()), (newer, new_pic.clone())]);

        let shapes = tray_shapes(&buffer, &thumbnails);
        let at = |wanted: String| shapes.iter().position(|s| *s == wanted).unwrap();
        assert!(shapes[0].contains("3 captures"), "{shapes:?}");
        assert!(at("Saving…".into()) < at(format!("{:?}", new_pic.id())));
        assert!(at(format!("{:?}", new_pic.id())) < at(format!("{:?}", old_pic.id())));
    }

    /// Runs the tray with the keyboard in the prefix, types `typed`, then presses `key`.
    /// Returns the prefix, whether it was reported done, and the texts drawn.
    /// The recording bar's commands for one frame per `(time, esc pressed)`.
    fn recording_bar_frames(frames: &[(f64, bool)]) -> Vec<Vec<Command>> {
        let ctx = egui::Context::default();
        install_fonts(&ctx, None);
        let state = BarState {
            recording: true,
            ..Default::default()
        };
        let esc = |pressed| egui::Event::Key {
            key: Key::Escape,
            physical_key: None,
            pressed,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        };
        frames
            .iter()
            .map(|&(time, pressed)| {
                let input = egui::RawInput {
                    time: Some(time),
                    events: if pressed {
                        vec![esc(true), esc(false)]
                    } else {
                        vec![]
                    },
                    ..Default::default()
                };
                let mut commands = Vec::new();
                let mut output = ctx.run_ui(input, |ui| {
                    commands = bar(ui, &state, &Buffer::new()).commands;
                });
                output.textures_delta.clear();
                commands
            })
            .collect()
    }

    /// FR-19 / US-5.2 AC: Esc asks before discarding; a second Esc discards.
    #[test]
    fn esc_asks_before_discarding_a_recording() {
        let discarded = |frames: &[(f64, bool)]| {
            recording_bar_frames(frames)
                .iter()
                .position(|c| c.contains(&Command::DiscardRecording))
        };
        assert_eq!(discarded(&[(0.0, true), (0.1, false)]), None, "one Esc");
        assert_eq!(
            discarded(&[(0.0, true), (0.5, false), (1.0, true)]),
            Some(2),
            "second Esc"
        );
        assert_eq!(
            discarded(&[(0.0, true), (5.0, false), (5.1, true)]),
            None,
            "the question expired, so the late Esc only asks again"
        );
    }

    fn type_in_tray(
        prefix: &str,
        typed: &str,
        key: Key,
        offer: Option<usize>,
    ) -> (String, TrayOutput, Vec<String>) {
        let ctx = egui::Context::default();
        install_fonts(&ctx, None);
        let mut prefix = prefix.to_owned();
        let mut out = TrayOutput::default();
        let mut texts = Vec::new();
        let press = |pressed| egui::Event::Key {
            key,
            physical_key: None,
            pressed,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        };
        let frames = [
            vec![],
            vec![egui::Event::Text(typed.into())],
            vec![press(true), press(false)],
            vec![],
        ];
        for (n, events) in frames.into_iter().enumerate() {
            let input = egui::RawInput {
                events,
                ..Default::default()
            };
            let mut output = ctx.run_ui(input, |ui| {
                let destination = Destination {
                    prefix: &mut prefix,
                    folder: Path::new("C:/Users/me/Captures"),
                    next_file: "acme_x.png",
                    focus: n == 0,
                    rename_offer: offer,
                };
                let now = tray(
                    ui,
                    &Buffer::new(),
                    &Selection::default(),
                    &HashMap::new(),
                    &HashMap::new(),
                    destination,
                    800.0,
                );
                out.prefix_done |= now.prefix_done;
            });
            output.textures_delta.clear();
            texts = output
                .shapes
                .iter()
                .filter_map(|c| match &c.shape {
                    egui::Shape::Text(t) => Some(t.galley.text().to_owned()),
                    _ => None,
                })
                .collect();
        }
        (prefix, out, texts)
    }

    #[test]
    fn the_prefix_is_typed_in_the_tray_and_enter_finishes_it() {
        let (prefix, out, texts) = type_in_tray("", "acme", Key::Enter, None);
        assert_eq!(prefix, "acme");
        assert!(out.prefix_done);
        assert!(
            texts.iter().any(|t| t == "Captures"),
            "the folder by its name: {texts:?}"
        );
        let (_, out, _) = type_in_tray("a", "b", Key::Tab, None);
        assert!(out.prefix_done, "Tab leaves the field too");
    }

    #[test]
    fn the_rename_offer_names_the_count_and_the_new_prefix() {
        let (_, _, texts) = type_in_tray("acme", "", Key::Enter, Some(3));
        assert!(
            texts
                .iter()
                .any(|t| t == "Rename the 3 captures in the buffer to acme_…?"),
            "{texts:?}"
        );
        let (_, _, texts) = type_in_tray("", "", Key::Enter, Some(1));
        assert!(texts
            .iter()
            .any(|t| t.contains("1 capture in the buffer to the timestamp alone")));
        let (_, _, texts) = type_in_tray("acme", "", Key::Enter, None);
        assert!(!texts.iter().any(|t| t.starts_with("Rename the")));
    }

    #[test]
    fn rename_and_keep_answer_the_offer() {
        let mut buffer = Buffer::new();
        buffer.push(ItemKind::Image, CaptureMode::Region);
        let answer = |label: &str| {
            let ctx = egui::Context::default();
            install_fonts(&ctx, None);
            let mut target = egui::Rect::NOTHING;
            let mut answer = None;
            for frame in 0..6 {
                let pos = target.center();
                let button = |pressed| egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                };
                let events = match frame {
                    3 => vec![egui::Event::PointerMoved(pos)],
                    4 => vec![button(true), button(false)],
                    _ => vec![],
                };
                let input = egui::RawInput {
                    events,
                    ..Default::default()
                };
                let mut prefix = "acme".to_string();
                let mut output = ctx.run_ui(input, |ui| {
                    let destination = Destination {
                        prefix: &mut prefix,
                        folder: Path::new("C:/Captures"),
                        next_file: "x.png",
                        focus: false,
                        rename_offer: Some(1),
                    };
                    let thumbnails = HashMap::new();
                    let out = tray(
                        ui,
                        &buffer,
                        &Selection::default(),
                        &thumbnails,
                        &HashMap::new(),
                        destination,
                        800.0,
                    );
                    answer = answer.or(out.rename);
                });
                output.textures_delta.clear();
                for c in &output.shapes {
                    if let egui::Shape::Text(t) = &c.shape {
                        if t.galley.text() == label {
                            target = t.visual_bounding_rect();
                        }
                    }
                }
            }
            answer
        };
        assert_eq!(answer("Rename"), Some(true));
        assert_eq!(answer("Keep names"), Some(false));
    }

    #[test]
    fn long_names_are_shortened() {
        assert_eq!(shorten("acme", 16), "acme");
        let long = shorten("a very long client name indeed", 16);
        assert_eq!(long.chars().count(), 16);
        assert!(long.ends_with('…'));
    }

    #[test]
    fn empty_tray_explains_itself() {
        let shapes = tray_shapes(&Buffer::new(), &HashMap::new());
        assert!(shapes.iter().any(|s| s.starts_with("Empty")), "{shapes:?}");
    }

    #[test]
    fn pill_is_much_smaller_than_the_bar() {
        let pill = run_pill(None).size;
        let full = bar_size(&BarState::default(), 2000.0);
        assert!(pill.x < full.x / 2.0, "pill {pill:?}, bar {full:?}");
        assert!(pill.y <= 48.0, "pill {pill:?}");
    }

    #[test]
    fn clicking_the_pill_expands_and_its_grip_does_not() {
        let size = run_pill(None).size;
        let on_chip = egui::pos2(size.x / 2.0 + 10.0, size.y / 2.0);
        let on_grip = egui::pos2(14.0, size.y / 2.0);
        assert!(run_pill(Some(on_chip)).expand);
        assert!(!run_pill(Some(on_grip)).expand);
    }

    #[test]
    fn bar_reports_when_the_pointer_is_on_it() {
        let hovered_at = |pos: egui::Pos2| {
            let ctx = egui::Context::default();
            install_fonts(&ctx, None);
            let buffer = Buffer::new();
            let mut hovered = false;
            for _ in 0..2 {
                let input = egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        vec2(2000.0, 200.0),
                    )),
                    events: vec![egui::Event::PointerMoved(pos)],
                    ..Default::default()
                };
                let mut output = ctx.run_ui(input, |ui| {
                    hovered = bar(ui, &BarState::default(), &buffer).hovered;
                });
                output.textures_delta.clear();
            }
            hovered
        };
        assert!(hovered_at(egui::pos2(100.0, 20.0)));
        assert!(!hovered_at(egui::pos2(100.0, 150.0)));
    }

    /// Hover along the whole bar, in a window with the room the app reserves on `side`:
    /// a tooltip must appear for every button and never cover the bar.
    fn assert_tooltips_clear(side: TooltipSide, vertical: bool) {
        // The app uses this font on Windows. Its taller lines are what tooltips must fit.
        let system = std::fs::read("C:/Windows/Fonts/segoeui.ttf")
            .ok()
            .zip(std::fs::read("C:/Windows/Fonts/seguisb.ttf").ok());
        let state = BarState {
            mic_on: true,
            vertical,
            tooltip_side: side,
            ..Default::default()
        };
        let buffer = Buffer::new();

        let ctx = egui::Context::default();
        install_fonts(&ctx, system.clone());
        let mut size = Vec2::ZERO;
        for _ in 0..2 {
            let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
                size = bar(ui, &state, &buffer).size;
            });
            output.textures_delta.clear();
        }

        // Slack: at fractional display scales the window ends up slightly short,
        // and a tooltip that no longer fits gets moved over the bar.
        let slack = 4.0;
        let (room, offset) = match side {
            TooltipSide::Below => (vec2(0.0, TOOLTIP_ROOM), Vec2::ZERO),
            TooltipSide::Above => (vec2(0.0, TOOLTIP_ROOM), vec2(0.0, TOOLTIP_ROOM)),
            TooltipSide::Right => (vec2(TOOLTIP_ROOM_SIDE, 0.0), Vec2::ZERO),
            TooltipSide::Left => (vec2(TOOLTIP_ROOM_SIDE, 0.0), vec2(TOOLTIP_ROOM_SIDE, 0.0)),
        };
        let window = egui::Rect::from_min_size(egui::Pos2::ZERO, size + room);
        let bar_rect = egui::Rect::from_min_size(offset.to_pos2(), size);
        let mut roomy = window;
        match side {
            TooltipSide::Below => roomy.max.y -= slack,
            TooltipSide::Above => roomy.min.y += slack,
            TooltipSide::Right => roomy.max.x -= slack,
            TooltipSide::Left => roomy.min.x += slack,
        }

        let mut shown = 0;
        let mut along = 12.0;
        let length = if vertical { size.y } else { size.x };
        while along < length - 8.0 {
            let pointer = if vertical {
                bar_rect.min + vec2(size.x / 2.0, along)
            } else {
                bar_rect.min + vec2(along, size.y / 2.0)
            };
            let ctx = egui::Context::default();
            install_fonts(&ctx, system.clone());
            for frame in 0..30 {
                let input = egui::RawInput {
                    screen_rect: Some(window),
                    time: Some(f64::from(frame) * 0.1),
                    // Tooltips wait for the pointer to rest, so it moves only once.
                    events: if frame == 0 {
                        vec![egui::Event::PointerMoved(pointer)]
                    } else {
                        Vec::new()
                    },
                    ..Default::default()
                };
                let mut output = ctx.run_ui(input, |ui| {
                    ui.scope_builder(egui::UiBuilder::new().max_rect(bar_rect), |ui| {
                        bar(ui, &state, &buffer);
                    });
                });
                output.textures_delta.clear();
            }
            let tooltips: Vec<_> = ctx.memory(|m| {
                m.layer_ids()
                    .filter(|layer| layer.order == egui::Order::Tooltip)
                    .filter_map(|layer| m.area_rect(layer.id))
                    .collect()
            });
            for tip in &tooltips {
                shown += 1;
                assert!(
                    !tip.intersects(bar_rect) && roomy.contains_rect(*tip),
                    "{side:?}, hovering {pointer:?}: tooltip {tip:?}, bar {bar_rect:?}, \
                     usable window {roomy:?}"
                );
            }
            along += if vertical { 20.0 } else { 30.0 };
        }
        assert!(
            shown > 8,
            "{side:?}: only {shown} hover positions showed a tooltip"
        );
    }

    #[test]
    fn tooltips_stay_clear_of_the_bar_below() {
        assert_tooltips_clear(TooltipSide::Below, false);
    }

    #[test]
    fn tooltips_stay_clear_of_the_bar_above() {
        assert_tooltips_clear(TooltipSide::Above, false);
    }

    #[test]
    fn tooltips_stay_clear_of_the_vertical_bar_on_its_right() {
        assert_tooltips_clear(TooltipSide::Right, true);
    }

    #[test]
    fn tooltips_stay_clear_of_the_vertical_bar_on_its_left() {
        assert_tooltips_clear(TooltipSide::Left, true);
    }

    #[test]
    fn vertical_bar_is_one_narrow_column() {
        for recording in [false, true] {
            let state = BarState {
                recording,
                vertical: true,
                ..Default::default()
            };
            let size = bar_size(&state, 300.0);
            assert!(size.x < 60.0, "vertical bar should be narrow: {size:?}");
            assert!(
                size.y > size.x * 2.0,
                "vertical bar should be tall: {size:?}"
            );
        }
    }

    #[test]
    fn the_flash_does_not_resize_the_bar() {
        let flashing = BarState {
            attention: 1.0,
            ..Default::default()
        };
        assert_eq!(
            bar_size(&flashing, 2000.0),
            bar_size(&BarState::default(), 2000.0)
        );
    }

    /// Clicks the bar at `pos` and returns its commands.
    fn click_bar(state: &BarState, pos: egui::Pos2) -> Vec<Command> {
        let ctx = egui::Context::default();
        install_fonts(&ctx, None);
        let buffer = Buffer::new();
        let mut commands = Vec::new();
        let button = |pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        for events in [
            vec![],
            vec![egui::Event::PointerMoved(pos)],
            vec![button(true)],
            vec![button(false)],
        ] {
            let input = egui::RawInput {
                events,
                ..Default::default()
            };
            let mut output = ctx.run_ui(input, |ui| {
                commands.extend(bar(ui, state, &buffer).commands);
            });
            output.textures_delta.clear();
        }
        commands
    }

    #[test]
    fn bar_buttons_but_buffer_close_the_tray() {
        let with = |commands: Vec<Command>| BarOutput {
            commands,
            ..Default::default()
        };
        assert!(with(vec![Command::Capture]).closes_tray());
        assert!(with(vec![Command::SetMode(CaptureMode::Screen)]).closes_tray());
        assert!(!with(vec![Command::ToggleBuffer]).closes_tray());
        assert!(!with(Vec::new()).closes_tray());
        let mic = BarOutput {
            toggle_mic: true,
            ..Default::default()
        };
        assert!(mic.closes_tray());
        let drag = BarOutput {
            start_drag: true,
            ..Default::default()
        };
        assert!(!drag.closes_tray(), "moving the bar keeps the tray");
    }

    #[test]
    fn mode_icons_switch_mode_without_labels() {
        let state = BarState::default();
        // Margin 8, grip 22, spacing 4, track inset 2, then 32-point cells; centred on the row.
        let cell = |i: f32| egui::pos2(8.0 + 22.0 + 4.0 + 2.0 + 16.0 + 32.0 * i, 8.0 + 18.0);
        for (i, mode) in [
            CaptureMode::Region,
            CaptureMode::Window,
            CaptureMode::Screen,
        ]
        .into_iter()
        .enumerate()
        {
            assert_eq!(
                click_bar(&state, cell(i as f32)),
                vec![Command::SetMode(mode)]
            );
        }
        let shapes = texts_of(|ui| {
            bar(ui, &state, &Buffer::new());
        });
        for label in ["Region", "Window", "Screen"] {
            assert!(
                !shapes.iter().any(|s| s == label),
                "{label} is drawn: {shapes:?}"
            );
        }
    }

    #[test]
    fn the_same_region_button_needs_region_mode_and_a_region() {
        let with = |mode, has_last_region| {
            let state = BarState {
                mode,
                has_last_region,
                ..Default::default()
            };
            bar_size(&state, 2000.0).x
        };
        let plain = with(CaptureMode::Region, false);
        assert!(with(CaptureMode::Region, true) > plain + 30.0);
        assert_eq!(with(CaptureMode::Window, true), plain);
    }

    /// The texts and textures `draw` paints, once its layout has settled.
    fn texts_of(mut draw: impl FnMut(&mut Ui)) -> Vec<String> {
        let ctx = egui::Context::default();
        install_fonts(&ctx, None);
        let mut seen = Vec::new();
        for _ in 0..3 {
            let mut output = ctx.run_ui(egui::RawInput::default(), &mut draw);
            output.textures_delta.clear();
            seen = output
                .shapes
                .iter()
                .filter_map(|clipped| match &clipped.shape {
                    egui::Shape::Text(text) => Some(text.galley.text().to_owned()),
                    _ => None,
                })
                .collect();
        }
        seen
    }

    /// The window is sized to the bar, so the bar must not size itself to the window.
    #[test]
    fn bar_size_ignores_window_size() {
        for recording in [false, true] {
            let state = BarState {
                recording,
                ..Default::default()
            };
            let narrow = bar_size(&state, 300.0);
            assert!(narrow.x > 300.0, "bar is {narrow:?}");
            assert!(narrow.y < 60.0, "bar should be one row: {narrow:?}");
            assert_eq!(narrow, bar_size(&state, 2000.0));
        }
    }
}
