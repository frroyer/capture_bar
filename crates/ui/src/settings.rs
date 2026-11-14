//! The settings window (US-8.2, FR-29): output, recording and buffer settings. Every
//! change takes effect at once; there is no OK button.

use std::collections::BTreeMap;
use std::path::Path;

use cb_core::{BubbleShape, Settings, Shortcut, ShortcutConflict};
use egui::{
    vec2, CentralPanel, Context, Frame, Grid, Key, Margin, Modifiers, RichText, ScrollArea, Ui,
    ViewportBuilder, ViewportCommand, ViewportId,
};

use crate::place::{centred_on, Opening};
use crate::{keys_label, palette, shortcut, tr};
use cb_core::Language;

/// What the window asked for this frame.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct SettingsOutput {
    /// The window was closed.
    pub closed: bool,
    /// "Choose…" was clicked: open the folder dialog.
    pub choose_folder: bool,
    /// A shortcut is being recorded: the global shortcuts must let its keys through.
    pub recording_keys: bool,
}

/// The open settings window.
#[derive(Debug, Default)]
pub struct SettingsWindow {
    focused: bool,
    /// The shortcut waiting for its new keys (US-3.2).
    recording: Option<Shortcut>,
    /// Where the window opens: on the bar's monitor.
    opening: Option<Opening>,
}

/// What a key press does to the shortcut being recorded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Recorded {
    /// Not usable as a global shortcut: keep waiting.
    Ignored,
    /// Esc: keep the old keys.
    Cancel,
    /// Backspace or Delete: no shortcut.
    Clear,
    /// The new keys, as `Ctrl+Shift+S`.
    Keys(String),
}

/// The keys of a press, as the settings file keeps them. A global shortcut needs Ctrl, Alt
/// or ⌘, so it never takes plain typing, except the function keys.
pub fn record_key(key: Key, mods: Modifiers) -> Recorded {
    let plain = !(mods.ctrl || mods.alt || mods.shift || mods.mac_cmd);
    match key {
        Key::Escape if plain => return Recorded::Cancel,
        Key::Backspace | Key::Delete if plain => return Recorded::Clear,
        _ => {}
    }
    let name = match key {
        // The names the global-hotkey crate reads differ from egui's for these.
        Key::Equals => "Equal",
        Key::OpenBracket => "BracketLeft",
        Key::CloseBracket => "BracketRight",
        Key::Backtick => "Backquote",
        Key::Tab | Key::Escape => return Recorded::Ignored,
        Key::Space
        | Key::Enter
        | Key::Backspace
        | Key::Insert
        | Key::Delete
        | Key::Home
        | Key::End
        | Key::PageUp
        | Key::PageDown
        | Key::ArrowUp
        | Key::ArrowDown
        | Key::ArrowLeft
        | Key::ArrowRight
        | Key::Comma
        | Key::Minus
        | Key::Period
        | Key::Semicolon
        | Key::Slash
        | Key::Backslash
        | Key::Quote => key.name(),
        _ => {
            let name = key.name();
            let letter_or_digit = name.len() == 1 && name.as_bytes()[0].is_ascii_alphanumeric();
            if !letter_or_digit && !is_function_key(name) {
                return Recorded::Ignored;
            }
            name
        }
    };
    let held = mods.ctrl || mods.alt || mods.mac_cmd;
    if !held && !is_function_key(name) {
        return Recorded::Ignored;
    }
    let mut keys = String::new();
    for (on, token) in [
        (mods.ctrl, "Ctrl+"),
        (mods.alt, "Alt+"),
        (mods.shift, "Shift+"),
        (mods.mac_cmd, "Cmd+"),
    ] {
        if on {
            keys.push_str(token);
        }
    }
    keys.push_str(name);
    Recorded::Keys(keys)
}

/// F1 to F24: the OS shortcut APIs know no more.
fn is_function_key(name: &str) -> bool {
    name.strip_prefix('F')
        .and_then(|n| n.parse::<u8>().ok())
        .is_some_and(|n| (1..=24).contains(&n))
}

fn action_name(action: Shortcut) -> String {
    match action {
        Shortcut::ToggleBar => tr!("shortcut-toggle-bar"),
        Shortcut::Capture => tr!("shortcut-capture"),
        Shortcut::Record => tr!("shortcut-record"),
        Shortcut::Pause => tr!("shortcut-pause"),
        Shortcut::Region => tr!("shortcut-region"),
        Shortcut::Window => tr!("shortcut-window"),
        Shortcut::Screen => tr!("shortcut-screen"),
        Shortcut::Repeat => tr!("shortcut-repeat"),
        Shortcut::Buffer => tr!("shortcut-buffer"),
        Shortcut::Copy => tr!("shortcut-copy"),
        Shortcut::Prefix => tr!("shortcut-prefix"),
    }
}

/// Why a shortcut is not in effect, for its tooltip.
pub fn conflict_text(conflict: ShortcutConflict) -> String {
    match conflict {
        ShortcutConflict::Taken => tr!("shortcut-taken"),
        ShortcutConflict::SameAs(other) => tr!("shortcut-same-as", action = action_name(other)),
        ShortcutConflict::Invalid => tr!("shortcut-invalid"),
    }
}

impl SettingsWindow {
    /// Bring the window to the front, as when Settings is asked for again.
    pub fn focus(&mut self) {
        self.focused = false;
    }

    /// Draw it in its own OS window, editing `settings` in place. `file` is where they are
    /// kept, shown at the bottom. `autostart` is whether the bar starts at login (US-8.3).
    /// `conflicts` are the shortcuts not in effect, and why. It opens on `monitor`, the
    /// bar's.
    #[allow(clippy::too_many_arguments)] // what the window shows and edits
    pub fn show(
        &mut self,
        ctx: &Context,
        monitor: Option<&cb_core::DisplayInfo>,
        settings: &mut Settings,
        file: &Path,
        autostart: &mut bool,
        conflicts: &BTreeMap<Shortcut, ShortcutConflict>,
    ) -> SettingsOutput {
        let size = vec2(560.0, 640.0);
        let opening = *self
            .opening
            .get_or_insert_with(|| Opening::at(monitor.map(|m| centred_on(m, size))));
        let builder = ViewportBuilder::default()
            .with_title(tr!("settings-title"))
            .with_resizable(false)
            .with_active(true);
        let builder = opening.builder(builder, ctx, size);
        ctx.show_viewport_immediate(
            ViewportId::from_hash_of("settings"),
            builder,
            |ui, _class| {
                if let Some(opening) = &mut self.opening {
                    opening.settle(ui.ctx());
                }
                self.ui(ui, settings, file, autostart, conflicts)
            },
        )
    }

    fn ui(
        &mut self,
        ui: &mut Ui,
        settings: &mut Settings,
        file: &Path,
        autostart: &mut bool,
        conflicts: &BTreeMap<Shortcut, ShortcutConflict>,
    ) -> SettingsOutput {
        let ctx = ui.ctx().clone();
        if !self.focused {
            self.focused = true;
            ctx.send_viewport_cmd(ViewportCommand::Focus);
        }
        // A key press while recording goes to the shortcut, not to the window.
        if let Some(action) = self.recording {
            let presses = ctx.input_mut(|i| {
                let presses: Vec<_> = i
                    .events
                    .iter()
                    .filter_map(|e| match e {
                        egui::Event::Key {
                            key,
                            pressed: true,
                            modifiers,
                            ..
                        } => Some(record_key(*key, *modifiers)),
                        _ => None,
                    })
                    .collect();
                i.events.retain(|e| !matches!(e, egui::Event::Key { .. }));
                presses
            });
            for press in presses {
                match press {
                    Recorded::Ignored => continue,
                    Recorded::Cancel => {}
                    Recorded::Clear => settings.set_keys(action, String::new()),
                    Recorded::Keys(keys) => settings.set_keys(action, keys),
                }
                self.recording = None;
                break;
            }
        }
        let mut out = SettingsOutput {
            closed: ctx.input(|i| {
                i.viewport().close_requested()
                    || (self.recording.is_none() && i.key_pressed(egui::Key::Escape))
            }),
            ..Default::default()
        };
        CentralPanel::default()
            .frame(
                Frame::new()
                    .fill(palette::BAR)
                    .inner_margin(Margin::same(18)),
            )
            .show(ui, |ui| {
                ScrollArea::vertical().show(ui, |ui| {
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
                                    .on_hover_text(tr!(
                                        "settings-choose-tip",
                                        key = shortcut(Shortcut::Prefix)
                                    ))
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
                                ui.checkbox(
                                    &mut settings.system_audio,
                                    tr!("settings-system-audio"),
                                );
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
                            ui.label(tr!("settings-webcam"));
                            ui.horizontal(|ui| {
                                for (shape, name) in [
                                    (BubbleShape::Circle, tr!("settings-webcam-circle")),
                                    (BubbleShape::Square, tr!("settings-webcam-square")),
                                    (BubbleShape::Portrait, tr!("settings-webcam-portrait")),
                                ] {
                                    ui.selectable_value(&mut settings.bubble_shape, shape, name);
                                }
                            });
                            ui.end_row();
                        });

                    section(ui, &tr!("settings-general"));
                    ui.checkbox(autostart, tr!("settings-autostart"));
                    ui.checkbox(&mut settings.fade_when_away, tr!("settings-fade"));
                    ui.checkbox(&mut settings.tooltips, tr!("settings-tooltips"));
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

                    section(ui, "Takebox+");
                    ui.horizontal(|ui| {
                        ui.label(tr!("settings-plus-key"));
                        ui.add(
                            egui::TextEdit::singleline(&mut settings.plus_key)
                                .password(true)
                                .desired_width(180.0),
                        );
                        let state = if settings.plus() {
                            tr!("settings-plus-on")
                        } else {
                            tr!("settings-plus-off")
                        };
                        ui.label(RichText::new(state).small().color(palette::MUTED));
                    });

                    self.shortcuts(ui, settings, conflicts);

                    ui.add_space(4.0);
                    ui.label(
                        RichText::new(tr!("settings-file", file = file.display().to_string()))
                            .small()
                            .color(palette::MUTED),
                    );
                })
            });
        out.recording_keys = self.recording.is_some();
        out
    }

    /// Each action with its keys; a click records new ones (US-3.2).
    fn shortcuts(
        &mut self,
        ui: &mut Ui,
        settings: &mut Settings,
        conflicts: &BTreeMap<Shortcut, ShortcutConflict>,
    ) {
        ui.horizontal(|ui| {
            section(ui, &tr!("settings-shortcuts"));
            let changed = !settings.shortcuts.is_empty();
            if ui
                .add_enabled(changed, egui::Button::new(tr!("settings-shortcuts-reset")))
                .clicked()
            {
                settings.shortcuts.clear();
                self.recording = None;
            }
        });
        Grid::new("shortcuts")
            .num_columns(4)
            .spacing([12.0, 8.0])
            .show(ui, |ui| {
                for (i, action) in Shortcut::ALL.into_iter().enumerate() {
                    ui.label(action_name(action));
                    let recording = self.recording == Some(action);
                    let conflict = conflicts.get(&action).copied();
                    let text = if recording {
                        RichText::new(tr!("settings-shortcut-press")).color(palette::ACCENT)
                    } else {
                        let label = keys_label(&settings.keys(action));
                        match conflict {
                            Some(_) => RichText::new(format!("⚠ {label}")).color(palette::WARNING),
                            None => RichText::new(label),
                        }
                    };
                    let tip = match conflict {
                        Some(c) if !recording => {
                            format!("{}\n{}", conflict_text(c), tr!("settings-shortcut-tip"))
                        }
                        _ => tr!("settings-shortcut-tip"),
                    };
                    let button = ui
                        .add(
                            egui::Button::new(text)
                                .selected(recording)
                                .min_size(vec2(130.0, 0.0)),
                        )
                        .on_hover_text(tip);
                    if button.clicked() {
                        self.recording = (!recording).then_some(action);
                    }
                    if i % 2 == 1 {
                        ui.end_row();
                    }
                }
            });
        let warnings = conflicts.len();
        if warnings > 0 {
            ui.label(
                RichText::new(tr!("settings-shortcuts-conflicts", count = warnings))
                    .color(palette::WARNING),
            );
        }
    }
}

fn section(ui: &mut Ui, title: &str) {
    ui.label(RichText::new(title).strong().color(palette::TEXT));
}
