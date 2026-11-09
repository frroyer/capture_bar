//! The settings window (US-8.2, FR-29): output, recording and buffer settings. Every
//! change takes effect at once; there is no OK button.

use std::path::Path;

use cb_core::Settings;
use egui::{
    vec2, CentralPanel, Context, Frame, Grid, Margin, RichText, Ui, ViewportBuilder,
    ViewportCommand, ViewportId,
};

use crate::{palette, shortcut, tr};
use cb_core::Language;

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
    /// kept, shown at the bottom. `autostart` is whether the bar starts at login (US-8.3).
    pub fn show(
        &mut self,
        ctx: &Context,
        settings: &mut Settings,
        file: &Path,
        autostart: &mut bool,
    ) -> SettingsOutput {
        let builder = ViewportBuilder::default()
            .with_title(tr!("settings-title"))
            .with_inner_size(vec2(480.0, 410.0))
            .with_resizable(false)
            .with_active(true);
        ctx.show_viewport_immediate(
            ViewportId::from_hash_of("settings"),
            builder,
            |ui, _class| self.ui(ui, settings, file, autostart),
        )
    }

    fn ui(
        &mut self,
        ui: &mut Ui,
        settings: &mut Settings,
        file: &Path,
        autostart: &mut bool,
    ) -> SettingsOutput {
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
                section(ui, &tr!("settings-output"));
                Grid::new("output")
                    .num_columns(2)
                    .spacing([16.0, 10.0])
                    .show(ui, |ui| {
                        ui.label(tr!("settings-folder"));
                        ui.horizontal(|ui| {
                            let shown = settings.folder.display().to_string();
                            ui.add(egui::Label::new(shown).truncate())
                                .on_hover_text(settings.folder.display().to_string());
                            if ui
                                .button(tr!("settings-choose"))
                                .on_hover_text(tr!("settings-choose-tip", key = shortcut("L")))
                                .clicked()
                            {
                                out.choose_folder = true;
                            }
                        });
                        ui.end_row();
                        ui.label(tr!("settings-prefix"));
                        ui.add(
                            egui::TextEdit::singleline(&mut settings.prefix)
                                .char_limit(40)
                                .desired_width(160.0)
                                .hint_text(tr!("prefix-hint")),
                        );
                        ui.end_row();
                    });

                section(ui, &tr!("settings-recording"));
                Grid::new("recording")
                    .num_columns(2)
                    .spacing([16.0, 10.0])
                    .show(ui, |ui| {
                        ui.label(tr!("settings-frame-rate"));
                        ui.horizontal(|ui| {
                            for rate in Settings::FRAME_RATES {
                                ui.selectable_value(
                                    &mut settings.frame_rate,
                                    rate,
                                    tr!("settings-fps", rate = rate),
                                );
                            }
                        });
                        ui.end_row();
                        ui.label(tr!("settings-audio"));
                        ui.horizontal(|ui| {
                            ui.checkbox(&mut settings.system_audio, tr!("settings-system-audio"));
                            ui.checkbox(&mut settings.microphone, tr!("settings-microphone"));
                        });
                        ui.end_row();
                        ui.label(tr!("settings-countdown"));
                        ui.add(
                            egui::Slider::new(
                                &mut settings.countdown_secs,
                                Settings::COUNTDOWN_SECS,
                            )
                            .suffix(" s"),
                        )
                        .on_hover_text(tr!("settings-countdown-tip"));
                        ui.end_row();
                    });

                section(ui, &tr!("settings-general"));
                ui.checkbox(autostart, tr!("settings-autostart"));
                ui.horizontal(|ui| {
                    ui.label(tr!("settings-language"));
                    let name = |language| match language {
                        Language::Auto => tr!("settings-language-auto"),
                        Language::En => "English".to_owned(),
                        Language::Fr => "Français".to_owned(),
                    };
                    egui::ComboBox::from_id_salt("language")
                        .selected_text(name(settings.language))
                        .show_ui(ui, |ui| {
                            for language in [Language::Auto, Language::En, Language::Fr] {
                                ui.selectable_value(
                                    &mut settings.language,
                                    language,
                                    name(language),
                                );
                            }
                        });
                });

                section(ui, &tr!("settings-buffer"));
                ui.horizontal(|ui| {
                    ui.label(tr!("settings-keep"));
                    ui.add(
                        egui::DragValue::new(&mut settings.retention_days)
                            .range(Settings::RETENTION_DAYS)
                            .speed(0.2),
                    );
                    ui.label(tr!("settings-days", count = settings.retention_days));
                })
                .response
                .on_hover_text(tr!("settings-keep-tip"));

                ui.add_space(4.0);
                ui.label(
                    RichText::new(tr!("settings-file", file = file.display().to_string()))
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
