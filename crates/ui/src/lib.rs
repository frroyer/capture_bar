//! The bar UI (Epic E2), matching mockup M1 / M2.1 / M2.2.
//! The UI only *returns commands*; it never calls OS or capture code directly.

use cb_buffer::{Buffer, ItemKind, ItemState};
use cb_core::{CaptureMode, Command};
use egui::{Color32, CornerRadius, Frame, Margin, RichText, Sense, Stroke, Ui, Vec2};

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
}

const H: f32 = 38.0;

fn chip(text: impl Into<String>, fill: Color32, color: Color32) -> egui::Button<'static> {
    egui::Button::new(RichText::new(text.into()).color(color).size(13.0))
        .fill(fill)
        .stroke(Stroke::NONE)
        .corner_radius(CornerRadius::same(10))
        .min_size(Vec2::new(0.0, H))
}

fn fmt_time(secs: u64) -> String {
    format!("{:02}:{:02}", secs / 60, secs % 60)
}

/// Draw the full bar (M2.1) or the recording bar (M2.2).
pub fn bar(ui: &mut Ui, state: &BarState, buffer: &Buffer) -> BarOutput {
    use palette::*;
    let mut out = BarOutput::default();

    Frame::new()
        .fill(BAR)
        .stroke(Stroke::new(
            1.0,
            if state.recording { RECORD } else { BORDER },
        ))
        .corner_radius(CornerRadius::same(16))
        .inner_margin(Margin::same(6))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;

                // Grip (drag to move).
                let grip = ui
                    .add(
                        egui::Label::new(RichText::new("::").color(MUTED).size(16.0))
                            .sense(Sense::click_and_drag()),
                    )
                    .on_hover_text("Drag to move");
                if grip.drag_started() {
                    out.start_drag = true;
                }

                if !state.recording {
                    // Mode switch.
                    for mode in [
                        CaptureMode::Region,
                        CaptureMode::Window,
                        CaptureMode::Screen,
                    ] {
                        let selected = state.mode == mode;
                        let (fill, color) = if selected {
                            (SELECTED, Color32::WHITE)
                        } else {
                            (CHIP, MUTED)
                        };
                        if ui.add(chip(mode.label(), fill, color)).clicked() {
                            out.commands.push(Command::SetMode(mode));
                        }
                    }
                    ui.separator();
                }

                if ui
                    .add(chip("Capture  ⌥⇧S", PRIMARY, PRIMARY_TEXT))
                    .on_hover_text("Capture with the current mode")
                    .clicked()
                {
                    out.commands.push(Command::Capture);
                }

                if state.recording {
                    let label = format!("■ {}  Stop", fmt_time(state.elapsed_secs));
                    if ui.add(chip(label, RECORD_BG, RECORD)).clicked() {
                        out.commands.push(Command::ToggleRecording);
                    }
                    if ui.add(chip("Pause", CHIP, TEXT)).clicked() {
                        out.commands.push(Command::TogglePause);
                    }
                } else if ui.add(chip("● Record  ⌥⇧R", CHIP, TEXT)).clicked() {
                    out.commands.push(Command::ToggleRecording);
                }

                ui.separator();

                let arrow = if state.tray_open { "▲" } else { "▼" };
                let buf_label = format!("Buffer  {}  {}", buffer.len(), arrow);
                if ui.add(chip(buf_label, CHIP, TEXT)).clicked() {
                    out.commands.push(Command::ToggleBuffer);
                }

                if !state.recording {
                    ui.separator();
                    let mic = if state.mic_on { TEXT } else { MUTED };
                    if ui.add(chip("Mic", CHIP, mic)).clicked() {
                        out.toggle_mic = true;
                    }
                    let cam = if state.cam_on { TEXT } else { MUTED };
                    if ui.add(chip("Cam", CHIP, cam)).clicked() {
                        out.toggle_cam = true;
                    }
                    if ui
                        .add(chip("–", CHIP, MUTED))
                        .on_hover_text("Collapse")
                        .clicked()
                    {
                        out.collapse = true;
                    }
                }
            });
        });

    out
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
                    RichText::new("Empty — press ⌥⇧S or ⌥⇧R. Captures stack up here instantly.")
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
