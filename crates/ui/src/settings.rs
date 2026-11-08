//! The settings window (US-8.2, FR-29): output, recording and buffer settings. Every
//! change takes effect at once; there is no OK button.

use std::path::Path;

use cb_core::Settings;
use egui::{
    vec2, CentralPanel, Context, Frame, Grid, Margin, RichText, Ui, ViewportBuilder,
    ViewportCommand, ViewportId,
};

use crate::{palette, shortcut};

/// What the window asked for this frame.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct SettingsOutput {
    /// The window was closed.
    pub closed: bool,
    /// "Choose…" was clicked: open the folder dialog.
    pub choose_folder: bool,
}

/// The open settings window.
#[derive(Debug, Default)]
pub struct SettingsWindow {
    focused: bool,
}

impl SettingsWindow {
    /// Bring the window to the front, as when Settings is asked for again.
    pub fn focus(&mut self) {
        self.focused = false;
    }

    /// Draw it in its own OS window, editing `settings` in place. `file` is where they are
    /// kept, shown at the bottom.
    pub fn show(&mut self, ctx: &Context, settings: &mut Settings, file: &Path) -> SettingsOutput {
        let builder = ViewportBuilder::default()
            .with_title("Capture Bar settings")
            .with_inner_size(vec2(460.0, 330.0))
            .with_resizable(false)
            .with_active(true);
        ctx.show_viewport_immediate(
            ViewportId::from_hash_of("settings"),
            builder,
            |ui, _class| self.ui(ui, settings, file),
        )
    }

    fn ui(&mut self, ui: &mut Ui, settings: &mut Settings, file: &Path) -> SettingsOutput {
        let ctx = ui.ctx().clone();
        if !self.focused {
            self.focused = true;
            ctx.send_viewport_cmd(ViewportCommand::Focus);
        }
        let mut out = SettingsOutput {
            closed: ctx
                .input(|i| i.viewport().close_requested() || i.key_pressed(egui::Key::Escape)),
            ..Default::default()
        };
        CentralPanel::default()
            .frame(
                Frame::new()
                    .fill(palette::BAR)
                    .inner_margin(Margin::same(18)),
            )
            .show(ui, |ui| {
                ui.style_mut().visuals = egui::Visuals::dark();
                ui.spacing_mut().item_spacing = vec2(10.0, 10.0);
                section(ui, "Output");
                Grid::new("output")
                    .num_columns(2)
                    .spacing([16.0, 10.0])
                    .show(ui, |ui| {
                        ui.label("Folder");
                        ui.horizontal(|ui| {
                            let shown = settings.folder.display().to_string();
                            ui.add(egui::Label::new(shown).truncate())
                                .on_hover_text(settings.folder.display().to_string());
                            if ui
                                .button("Choose…")
                                .on_hover_text(format!(
                                    "Also from the bar's tray ({})",
                                    shortcut("L")
                                ))
                                .clicked()
                            {
                                out.choose_folder = true;
                            }
                        });
                        ui.end_row();
                        ui.label("File name prefix");
                        ui.add(
                            egui::TextEdit::singleline(&mut settings.prefix)
                                .char_limit(40)
                                .desired_width(160.0)
                                .hint_text("no prefix"),
                        );
                        ui.end_row();
                    });

                section(ui, "Recording");
                Grid::new("recording")
                    .num_columns(2)
                    .spacing([16.0, 10.0])
                    .show(ui, |ui| {
                        ui.label("Frame rate");
                        ui.horizontal(|ui| {
                            for rate in Settings::FRAME_RATES {
                                ui.selectable_value(
                                    &mut settings.frame_rate,
                                    rate,
                                    format!("{rate} fps"),
                                );
                            }
                        });
                        ui.end_row();
                        ui.label("Audio");
                        ui.horizontal(|ui| {
                            ui.checkbox(&mut settings.system_audio, "System sound");
                            ui.checkbox(&mut settings.microphone, "Microphone");
                        });
                        ui.end_row();
                    });

                section(ui, "Buffer");
                ui.horizontal(|ui| {
                    ui.label("Keep unpinned captures for");
                    ui.add(
                        egui::DragValue::new(&mut settings.retention_days)
                            .range(Settings::RETENTION_DAYS)
                            .speed(0.2),
                    );
                    ui.label(if settings.retention_days == 1 {
                        "day"
                    } else {
                        "days"
                    });
                })
                .response
                .on_hover_text("Their files stay in the folder");

                ui.add_space(4.0);
                ui.label(
                    RichText::new(format!("Saved in {}", file.display()))
                        .small()
                        .color(palette::MUTED),
                );
            });
        out
    }
}

fn section(ui: &mut Ui, title: &str) {
    ui.label(RichText::new(title).strong().color(palette::TEXT));
}
