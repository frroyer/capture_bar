//! The demo session setup window (US-10.1; mockup M4 is still to come): title, language,
//! what to record and the microphone. Enter starts, Esc or closing the window cancels.

use cb_core::{CaptureMode, Language};
use egui::{
    vec2, CentralPanel, Context, Frame, Grid, Key, Margin, TextEdit, Ui, ViewportBuilder,
    ViewportCommand, ViewportId,
};

use crate::place::{centred_on, Opening};
use crate::{palette, tr};

/// What the window asked for this frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DemoSetupOutput {
    Open,
    Start,
    Cancel,
}

/// The open setup window, and what it was given.
#[derive(Debug)]
pub struct DemoSetup {
    pub title: String,
    pub language: Language,
    pub mode: CaptureMode,
    pub mic: bool,
    focused: bool,
    opening: Option<Opening>,
}

impl DemoSetup {
    /// Opens with the bar's mode and microphone.
    pub fn new(mode: CaptureMode, mic: bool) -> Self {
        Self {
            title: String::new(),
            language: Language::Auto,
            mode,
            mic,
            focused: false,
            opening: None,
        }
    }

    /// Bring the window to the front, as when it is asked for again.
    pub fn focus(&mut self) {
        self.focused = false;
    }

    /// Draw it in its own OS window, on `monitor` (the bar's).
    pub fn show(
        &mut self,
        ctx: &Context,
        monitor: Option<&cb_core::DisplayInfo>,
    ) -> DemoSetupOutput {
        let size = vec2(420.0, 250.0);
        let opening = *self
            .opening
            .get_or_insert_with(|| Opening::at(monitor.map(|m| centred_on(m, size))));
        let builder = ViewportBuilder::default()
            .with_title(tr!("demo-setup-title"))
            .with_resizable(false)
            .with_active(true);
        let builder = opening.builder(builder, ctx, size);
        ctx.show_viewport_immediate(ViewportId::from_hash_of("demo-setup"), builder, |ui, _| {
            if let Some(opening) = &mut self.opening {
                opening.settle(ui.ctx());
            }
            self.ui(ui)
        })
    }

    fn ui(&mut self, ui: &mut Ui) -> DemoSetupOutput {
        let ctx = ui.ctx().clone();
        let opened = !self.focused;
        if opened {
            self.focused = true;
            ctx.send_viewport_cmd(ViewportCommand::Focus);
        }
        let (close, cancel, enter) = ctx.input(|i| {
            (
                i.viewport().close_requested(),
                i.key_pressed(Key::Escape),
                i.key_pressed(Key::Enter),
            )
        });
        let mut out = if close || cancel {
            DemoSetupOutput::Cancel
        } else if enter {
            DemoSetupOutput::Start
        } else {
            DemoSetupOutput::Open
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
                Grid::new("demo-setup")
                    .num_columns(2)
                    .spacing([12.0, 10.0])
                    .show(ui, |ui| {
                        ui.label(tr!("demo-setup-name"));
                        let title = ui.add(
                            TextEdit::singleline(&mut self.title)
                                .hint_text(tr!("demo-setup-name-hint"))
                                .desired_width(260.0),
                        );
                        if opened {
                            title.request_focus();
                        }
                        ui.end_row();

                        ui.label(tr!("demo-setup-language"));
                        ui.horizontal(|ui| {
                            for (language, name) in [
                                (Language::Auto, tr!("demo-setup-language-auto")),
                                (Language::Fr, "Français".to_owned()),
                                (Language::En, "English".to_owned()),
                            ] {
                                ui.selectable_value(&mut self.language, language, name);
                            }
                        });
                        ui.end_row();

                        ui.label(tr!("demo-setup-area"));
                        ui.horizontal(|ui| {
                            for (mode, name) in [
                                (CaptureMode::Screen, tr!("mode-screen")),
                                (CaptureMode::Window, tr!("mode-window")),
                                (CaptureMode::Region, tr!("mode-region")),
                            ] {
                                ui.selectable_value(&mut self.mode, mode, name);
                            }
                        });
                        ui.end_row();

                        ui.label(tr!("demo-setup-audio"));
                        ui.checkbox(&mut self.mic, tr!("settings-microphone"));
                        ui.end_row();
                    });
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    let start = egui::Button::new(
                        egui::RichText::new(tr!("demo-setup-start")).color(egui::Color32::WHITE),
                    )
                    .fill(palette::ACCENT);
                    if ui.add(start).clicked() {
                        out = DemoSetupOutput::Start;
                    }
                    if ui.button(tr!("demo-setup-cancel")).clicked() {
                        out = DemoSetupOutput::Cancel;
                    }
                });
            });
        out
    }
}
