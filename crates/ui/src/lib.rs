//! The bar UI (Epic E2), matching mockup M1 / M2.1 / M2.2.
//! The UI only *returns commands*; it never calls OS or capture code directly.

mod icons;
mod overlay;

pub use overlay::{Overlay, OverlayResult};

use cb_buffer::{Buffer, ItemKind, ItemState};
use cb_core::{CaptureMode, Command};
use egui::{
    emath::RectAlign, vec2, Align2, Color32, CornerRadius, CursorIcon, FontData, FontDefinitions,
    FontFamily, FontId, Frame, Id, Margin, Response, RichText, Role, Sense, Stroke, StrokeKind,
    Tooltip, Ui, Vec2, WidgetInfo,
};
use icons::Icon;
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
}

/// Everything the bar needs to draw itself.
#[derive(Debug, Clone, Default)]
pub struct BarState {
    pub mode: CaptureMode,
    pub recording: bool,
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
    icons::paint(ui.painter(), rect.center(), icon, palette::MUTED);
    let grip = grip.on_hover_cursor(CursorIcon::Grab);
    show_tooltip(&grip, "Drag to move. Double-click to dock");
    if grip.drag_started() {
        out.start_drag = true;
    }
    if grip.double_clicked() {
        out.grip_double_clicked = true;
    }
}

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
                    "Expand the bar",
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
                    &[Piece::Icon(Icon::Camera), Piece::Text("Capture")],
                    &ChipStyle::new(PRIMARY, PRIMARY_TEXT),
                    &format!("Capture with the current mode ({capture_key})"),
                )
                .clicked()
                {
                    out.commands.push(Command::Capture);
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
                            Piece::Text("Stop"),
                        ],
                        &ChipStyle::new(RECORD_BG, RECORD),
                        &format!("Stop recording ({record_key})"),
                    )
                    .clicked()
                    {
                        out.commands.push(Command::ToggleRecording);
                    }
                    if chip(
                        ui,
                        &[Piece::Icon(Icon::Pause), Piece::Text("Pause")],
                        &ChipStyle::new(CHIP, TEXT),
                        &format!("Pause recording ({})", shortcut("P")),
                    )
                    .clicked()
                    {
                        out.commands.push(Command::TogglePause);
                    }
                } else if chip(
                    ui,
                    &[
                        Piece::Tinted(Icon::RecordDot, RECORD),
                        Piece::Text("Record"),
                    ],
                    &ChipStyle::new(CHIP, TEXT),
                    &format!("Start recording ({record_key})"),
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
                        Piece::Text("Buffer"),
                        Piece::Badge(&count, badge),
                        Piece::Icon(arrow),
                    ],
                    &ChipStyle::new(CHIP, TEXT),
                    &format!("Show or hide the buffer ({})", shortcut("B")),
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
                    let toggle = |on: bool| {
                        if on {
                            ChipStyle::new(CHIP, TEXT)
                        } else {
                            ChipStyle::new(Color32::TRANSPARENT, MUTED)
                        }
                    };
                    let quiet = ChipStyle::new(Color32::TRANSPARENT, MUTED);
                    if chip(
                        ui,
                        &[Piece::Icon(Icon::Mic)],
                        &toggle(state.mic_on),
                        "Microphone on or off",
                    )
                    .clicked()
                    {
                        out.toggle_mic = true;
                    }
                    if chip(
                        ui,
                        &[Piece::Icon(Icon::Video)],
                        &toggle(state.cam_on),
                        "Webcam on or off",
                    )
                    .clicked()
                    {
                        out.toggle_cam = true;
                    }
                    if chip(ui, &[Piece::Icon(Icon::Sliders)], &quiet, "Settings").clicked() {
                        out.commands.push(Command::OpenSettings);
                    }
                    if chip(ui, &[Piece::Icon(Icon::Minus)], &quiet, "Collapse").clicked() {
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

/// Region / Window / Screen, as one segmented control.
fn mode_switch(ui: &mut Ui, current: CaptureMode, out: &mut BarOutput) {
    use palette::*;
    Frame::new()
        .fill(CHIP)
        .corner_radius(CornerRadius::same(12))
        .inner_margin(Margin::same(2))
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing = vec2(2.0, 2.0);
            for (mode, icon, key) in [
                (CaptureMode::Region, Icon::Region, "1"),
                (CaptureMode::Window, Icon::Window, "2"),
                (CaptureMode::Screen, Icon::Screen, "3"),
            ] {
                let (fill, color) = if current == mode {
                    (SELECTED, Color32::WHITE)
                } else {
                    (Color32::TRANSPARENT, MUTED)
                };
                let style = ChipStyle {
                    fill,
                    color,
                    height: H_IN_GROUP,
                };
                let tip = format!("{}: {} captures now", mode.label(), shortcut(key));
                if chip(
                    ui,
                    &[Piece::Icon(icon), Piece::Text(mode.label())],
                    &style,
                    &tip,
                )
                .clicked()
                {
                    out.commands.push(Command::SetMode(mode));
                }
            }
        });
}

/// A confirmation beside the bar (M2.6). Returns true when Undo was clicked.
pub fn toast(ui: &mut Ui, message: &str, can_undo: bool) -> bool {
    use palette::*;
    let mut undo = false;
    Frame::new()
        .fill(BAR)
        .stroke(Stroke::new(1.0, BORDER))
        .corner_radius(CornerRadius::same(12))
        .inner_margin(Margin::symmetric(10, 4))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new(message).color(TEXT).size(13.0));
                if can_undo {
                    undo = chip(
                        ui,
                        &[Piece::Text("Undo")],
                        &ChipStyle {
                            fill: CHIP,
                            color: TEXT,
                            height: H_IN_GROUP,
                        },
                        "Remove this capture",
                    )
                    .clicked();
                }
            });
        });
    undo
}

/// The buffer tray (M1, FR-22), `width` points wide. Returns commands for item actions later.
pub fn tray(ui: &mut Ui, buffer: &Buffer, width: f32) {
    use palette::*;
    Frame::new()
        .fill(BAR)
        .stroke(Stroke::new(1.0, BORDER))
        .corner_radius(CornerRadius::same(16))
        .inner_margin(Margin::same(12))
        .show(ui, |ui| {
            // Less the margin and the border on both sides.
            ui.set_width((width - 26.0).max(1.0));
            ui.label(
                RichText::new(format!("Buffer · {} captures", buffer.len()))
                    .color(TEXT)
                    .strong(),
            );
            if buffer.is_empty() {
                ui.label(
                    RichText::new(format!(
                        "Empty — press {} or {}. Captures stack up here instantly.",
                        shortcut("S"),
                        shortcut("R")
                    ))
                    .color(MUTED),
                );
                return;
            }
            egui::ScrollArea::horizontal().show(ui, |ui| {
                ui.horizontal(|ui| {
                    for item in buffer.newest_first() {
                        let kind = match item.kind {
                            ItemKind::Image => "PNG".to_string(),
                            ItemKind::Video { duration_ms } => {
                                format!("MP4 {}s", duration_ms / 1000)
                            }
                        };
                        let status = match &item.state {
                            ItemState::Ready => String::new(),
                            ItemState::Encoding { progress_pct } => {
                                format!(" · encoding {progress_pct}%")
                            }
                            ItemState::Failed(e) => format!(" · failed: {e}"),
                        };
                        Frame::new()
                            .fill(CHIP)
                            .corner_radius(CornerRadius::same(8))
                            .inner_margin(Margin::same(8))
                            .show(ui, |ui| {
                                ui.set_min_size(Vec2::new(140.0, 72.0));
                                ui.vertical(|ui| {
                                    ui.label(RichText::new(kind).color(ACCENT).strong());
                                    ui.label(
                                        RichText::new(format!("{}{}", item.mode.label(), status))
                                            .color(TEXT),
                                    );
                                    ui.label(
                                        RichText::new(format!("#{}", item.id.0))
                                            .color(MUTED)
                                            .size(11.0),
                                    );
                                });
                            });
                    }
                });
            });
        });
}

#[cfg(test)]
mod tests {
    use super::*;

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
                    clicked |= toast(ui, "Added to buffer · 1", can_undo);
                    width = ui.min_rect().right() - top.x;
                });
                output.textures_delta.clear();
            }
            clicked
        };
        assert!(click_near_right(true));
        assert!(!click_near_right(false));
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
