//! The bar UI (Epic E2), matching mockup M1 / M2.1 / M2.2.
//! The UI only *returns commands*; it never calls OS or capture code directly.

mod icons;

use cb_buffer::{Buffer, ItemKind, ItemState};
use cb_core::{CaptureMode, Command};
use egui::{
    vec2, Color32, CornerRadius, CursorIcon, FontData, FontDefinitions, FontFamily, FontId, Frame,
    Margin, Response, RichText, Role, Sense, Stroke, StrokeKind, Tooltip, Ui, Vec2, WidgetInfo,
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
    /// The pointer is on the bar, so a tooltip may be about to open.
    pub hovered: bool,
}

/// Extra window height a tooltip needs below the bar: tooltips cannot leave the window.
pub const TOOLTIP_ROOM: f32 = 44.0;
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
    let size = vec2((content + PAD_X * 2.0).max(style.height), style.height);

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
    Tooltip::for_enabled(response)
        .gap(TOOLTIP_GAP)
        .show(|ui| ui.label(text));
}

/// A thin divider between groups of controls.
fn divider(ui: &mut Ui) {
    let (rect, _) = ui.allocate_exact_size(vec2(9.0, 24.0), Sense::hover());
    ui.painter().vline(
        rect.center().x,
        rect.y_range(),
        Stroke::new(1.0, palette::BORDER),
    );
}

fn fmt_time(secs: u64) -> String {
    format!("{:02}:{:02}", secs / 60, secs % 60)
}

/// Draw the full bar (M2.1) or the recording bar (M2.2).
pub fn bar(ui: &mut Ui, state: &BarState, buffer: &Buffer) -> BarOutput {
    use palette::*;
    let mut out = BarOutput::default();

    let frame = Frame::new()
        .fill(BAR)
        .stroke(Stroke::new(
            1.0,
            if state.recording { RECORD } else { BORDER },
        ))
        .corner_radius(CornerRadius::same(20))
        .inner_margin(Margin::same(8))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;

                // Grip (drag to move).
                let (rect, grip) = ui.allocate_exact_size(vec2(22.0, H), Sense::click_and_drag());
                icons::paint(ui.painter(), rect.center(), Icon::Grip, MUTED);
                let grip = grip.on_hover_cursor(CursorIcon::Grab);
                show_tooltip(&grip, "Drag to move");
                if grip.drag_started() {
                    out.start_drag = true;
                }

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
                if chip(
                    ui,
                    &[
                        Piece::Icon(Icon::Layers),
                        Piece::Text("Buffer"),
                        Piece::Badge(&count, badge),
                        Piece::Icon(arrow),
                    ],
                    &ChipStyle::new(CHIP, TEXT),
                    &format!("Show or hide the buffer ({})", shortcut("B")),
                )
                .clicked()
                {
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
                    if chip(
                        ui,
                        &[Piece::Icon(Icon::Minus)],
                        &quiet,
                        &format!("Collapse ({})", shortcut("Space")),
                    )
                    .clicked()
                    {
                        out.collapse = true;
                    }
                }
            });
        });

    out.hovered = ui.rect_contains_pointer(frame.response.rect);
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
            ui.spacing_mut().item_spacing.x = 2.0;
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

/// The buffer tray (M1, FR-22). Returns commands for item actions later.
pub fn tray(ui: &mut Ui, buffer: &Buffer) {
    use palette::*;
    Frame::new()
        .fill(BAR)
        .stroke(Stroke::new(1.0, BORDER))
        .corner_radius(CornerRadius::same(16))
        .inner_margin(Margin::same(12))
        .show(ui, |ui| {
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

    const TOOLTIP_SLACK: f32 = 4.0;

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

    /// Hover along the whole bar, in a window sized like the app's: a tooltip must
    /// appear for every button and never cover the bar.
    #[test]
    fn tooltips_stay_clear_of_the_bar() {
        // The app uses this font on Windows. Its taller lines are what tooltips must fit.
        let system = std::fs::read("C:/Windows/Fonts/segoeui.ttf")
            .ok()
            .zip(std::fs::read("C:/Windows/Fonts/seguisb.ttf").ok());
        let state = BarState {
            mic_on: true,
            ..Default::default()
        };
        let buffer = Buffer::new();

        let ctx = egui::Context::default();
        install_fonts(&ctx, system.clone());
        let mut bar_size = Vec2::ZERO;
        for _ in 0..2 {
            let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
                bar(ui, &state, &buffer);
                bar_size = ui.min_rect().size();
            });
            output.textures_delta.clear();
        }
        let window = egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            vec2(bar_size.x, bar_size.y + TOOLTIP_ROOM),
        );

        let mut x = 40.0;
        let mut shown = 0;
        while x < bar_size.x - 8.0 {
            let ctx = egui::Context::default();
            install_fonts(&ctx, system.clone());
            for frame in 0..30 {
                let input = egui::RawInput {
                    screen_rect: Some(window),
                    time: Some(f64::from(frame) * 0.1),
                    // Tooltips wait for the pointer to rest, so it moves only once.
                    events: if frame == 0 {
                        vec![egui::Event::PointerMoved(egui::pos2(x, bar_size.y / 2.0))]
                    } else {
                        Vec::new()
                    },
                    ..Default::default()
                };
                let mut output = ctx.run_ui(input, |ui| {
                    bar(ui, &state, &buffer);
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
                // Slack: at fractional display scales the window ends up slightly short,
                // and a tooltip that no longer fits gets moved over the bar.
                assert!(
                    tip.min.y >= bar_size.y - 1.0
                        && window.shrink(TOOLTIP_SLACK).max.y >= tip.max.y
                        && tip.max.x <= window.max.x
                        && tip.min.x >= window.min.x,
                    "hovering x={x}: tooltip {tip:?}, bar {bar_size:?}, window {window:?}"
                );
            }
            x += 30.0;
        }
        assert!(shown > 10, "only {shown} hover positions showed a tooltip");
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
