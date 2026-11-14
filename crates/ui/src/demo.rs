//! The demo session setup window (US-10.1; mockup M4 is still to come): title, language,
//! what to record, the microphone, the webcam's own track (US-10.6) and the masks to
//! blur (US-10.5). Enter starts, Esc or closing the window cancels.

use cb_core::{CaptureMode, Language, Mask};
use egui::{
    vec2, CentralPanel, Context, Frame, Grid, Key, Margin, TextEdit, Ui, ViewportBuilder,
    ViewportCommand, ViewportId,
};

use crate::icons::{self, Icon};
use crate::place::{centred_on, Opening};
use crate::{focus_ring, palette, tr};
use egui::accesskit::Role;
use egui::{
    pos2, Align, Align2, CornerRadius, CursorIcon, FontId, Label, Layout, Response, RichText,
    Sense, Stroke, StrokeKind, UiBuilder, WidgetInfo,
};

/// What the window asked for this frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DemoSetupOutput {
    Open,
    Start,
    Cancel,
    /// Pick an area or a window to mask, with the overlay.
    PickArea,
    PickWindow,
}

/// The open setup window, and what it was given.
#[derive(Debug)]
pub struct DemoSetup {
    pub title: String,
    pub language: Language,
    pub mode: CaptureMode,
    pub mic: bool,
    /// Record the webcam on its own track; off unless asked for (DR-25).
    pub webcam: bool,
    /// What to blur, each with the name shown for it.
    pub masks: Vec<(Mask, String)>,
    focused: bool,
    opening: Option<Opening>,
}

impl DemoSetup {
    /// Opens with the bar's mode and microphone, and the masks of the last session.
    pub fn new(mode: CaptureMode, mic: bool, masks: Vec<(Mask, String)>) -> Self {
        Self {
            title: String::new(),
            language: Language::Auto,
            mode,
            mic,
            webcam: false,
            masks,
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
        let size = vec2(460.0, 480.0);
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

                        ui.label(tr!("demo-setup-webcam"));
                        ui.checkbox(&mut self.webcam, tr!("demo-setup-webcam-track"));
                        ui.end_row();
                    });
                ui.add_space(2.0);
                ui.separator();
                if let Some(pick) = self.masks_ui(ui) {
                    out = pick;
                }
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

    /// The masks (US-10.5): what they do, two ways to add one, and the list, each with
    /// its remove button. Returns the pick asked for.
    fn masks_ui(&mut self, ui: &mut Ui) -> Option<DemoSetupOutput> {
        let mut out = None;
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(tr!("demo-setup-masks"))
                    .strong()
                    .color(palette::TEXT),
            );
            if !self.masks.is_empty() {
                ui.label(RichText::new(self.masks.len().to_string()).color(palette::MUTED));
            }
        });
        ui.label(
            RichText::new(tr!("demo-setup-masks-help"))
                .small()
                .color(palette::MUTED),
        );
        ui.horizontal(|ui| {
            let width = (ui.available_width() - ui.spacing().item_spacing.x) / 2.0;
            let cards = [
                (
                    Icon::Region,
                    tr!("demo-setup-mask-area"),
                    tr!("demo-setup-mask-area-help"),
                    DemoSetupOutput::PickArea,
                ),
                (
                    Icon::Window,
                    tr!("demo-setup-mask-window"),
                    tr!("demo-setup-mask-window-help"),
                    DemoSetupOutput::PickWindow,
                ),
            ];
            for (icon, title, help, pick) in cards {
                if add_card(ui, width, icon, &title, &help).clicked() {
                    out = Some(pick);
                }
            }
        });
        let mut removed = None;
        Frame::new()
            .fill(palette::SLOT_BG)
            .corner_radius(8)
            .inner_margin(Margin::symmetric(6, 6))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                if self.masks.is_empty() {
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        ui.add_space(6.0);
                        ui.label(RichText::new(tr!("demo-setup-masks-none")).color(palette::MUTED));
                    });
                    ui.add_space(4.0);
                    return;
                }
                egui::ScrollArea::vertical()
                    .max_height(96.0)
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        ui.spacing_mut().item_spacing.y = 2.0;
                        for (i, (mask, name)) in self.masks.iter().enumerate() {
                            if mask_row(ui, i, mask, name) {
                                removed = Some(i);
                            }
                        }
                    });
            });
        if let Some(i) = removed {
            self.masks.remove(i);
        }
        out
    }
}

/// A button to add a mask: an icon, what it adds, and how it is picked.
fn add_card(ui: &mut Ui, width: f32, icon: Icon, title: &str, help: &str) -> Response {
    let (rect, response) = ui.allocate_exact_size(vec2(width, 50.0), Sense::click());
    let painter = ui.painter();
    let fill = if response.hovered() {
        palette::SELECTED
    } else {
        palette::CHIP
    };
    painter.rect(
        rect,
        8.0,
        fill,
        Stroke::new(1.0, palette::BORDER),
        StrokeKind::Inside,
    );
    icons::paint(
        painter,
        pos2(rect.left() + 22.0, rect.center().y),
        icon,
        palette::ACCENT,
    );
    let left = rect.left() + 42.0;
    painter.text(
        pos2(left, rect.top() + 9.0),
        Align2::LEFT_TOP,
        title,
        FontId::proportional(13.5),
        palette::TEXT,
    );
    painter.text(
        pos2(left, rect.top() + 28.0),
        Align2::LEFT_TOP,
        help,
        FontId::proportional(11.5),
        palette::MUTED,
    );
    response.widget_info(|| WidgetInfo::labeled(Role::Button, true, title));
    focus_ring(ui, &response, 8);
    response.on_hover_cursor(CursorIcon::PointingHand)
}

/// One mask in the list: its kind, its name, and a button that removes it. Returns
/// whether it was asked to go.
fn mask_row(ui: &mut Ui, index: usize, mask: &Mask, name: &str) -> bool {
    let (row, hover) = ui.allocate_exact_size(vec2(ui.available_width(), 28.0), Sense::hover());
    if hover.hovered() {
        ui.painter().rect_filled(row, 6.0, palette::CHIP);
    }
    let icon = match mask {
        Mask::Area(_) => Icon::Region,
        Mask::Window(_) => Icon::Window,
    };
    icons::paint(
        ui.painter(),
        pos2(row.left() + 16.0, row.center().y),
        icon,
        palette::MUTED,
    );
    let remove_rect =
        egui::Rect::from_center_size(pos2(row.right() - 16.0, row.center().y), vec2(24.0, 24.0));
    let name_rect = egui::Rect::from_min_max(
        pos2(row.left() + 34.0, row.top()),
        pos2(remove_rect.left() - 6.0, row.bottom()),
    );
    ui.scope_builder(
        UiBuilder::new()
            .max_rect(name_rect)
            .layout(Layout::left_to_right(Align::Center)),
        |ui| ui.add(Label::new(RichText::new(name).color(palette::TEXT)).truncate()),
    );
    let label = tr!("demo-setup-mask-remove", name = name.to_owned());
    let remove = ui.interact(
        remove_rect,
        ui.scope_id().with(("remove mask", index)),
        Sense::click(),
    );
    let color = if remove.hovered() {
        ui.painter()
            .rect_filled(remove_rect, CornerRadius::same(5), palette::RECORD_BG);
        palette::RECORD
    } else {
        palette::MUTED
    };
    icons::paint(ui.painter(), remove_rect.center(), Icon::Trash, color);
    remove.widget_info(|| WidgetInfo::labeled(Role::Button, true, &label));
    focus_ring(ui, &remove, 5);
    remove.on_hover_text(&label).clicked()
}
