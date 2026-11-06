//! Capture Bar — entry point. Wires the floating bar (cb-ui), global
//! shortcuts, the capture service and the buffer together through commands.

#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use cb_buffer::{Buffer, ItemId, ItemKind, ItemState};
use cb_capture::{CaptureEvent, CaptureService};
use cb_core::{CaptureMode, CaptureTarget, Command, DisplayId, Rect};
use cb_ui::{palette, BarState};
use eframe::egui;
use global_hotkey::hotkey::{Code, HotKey, Modifiers};
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};

/// Only for the first frame: after that the window follows its content.
const INITIAL_SIZE: [f32; 2] = [900.0, 54.0];

fn main() -> eframe::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();
    tracing::info!(
        backend = cb_platform::backend_name(),
        "starting Capture Bar"
    );

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Capture Bar")
            .with_inner_size(INITIAL_SIZE)
            .with_decorations(false)
            .with_transparent(true)
            .with_has_shadow(false)
            .with_always_on_top()
            .with_resizable(false)
            .with_taskbar(false),
        ..Default::default()
    };

    eframe::run_native(
        "Capture Bar",
        options,
        Box::new(|cc| Ok(Box::new(App::new(cc)))),
    )
}

/// The size of what `add` draws. A panel's own UI always reports the whole panel,
/// so this measures inside a nested scope.
fn content_size(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui)) -> egui::Vec2 {
    ui.scope(add).response.rect.size()
}

/// Default shortcut map (mockup M3). ⌥⇧ on macOS, Ctrl+Shift elsewhere.
fn shortcut_modifiers() -> Modifiers {
    if cfg!(target_os = "macos") {
        Modifiers::ALT | Modifiers::SHIFT
    } else {
        Modifiers::CONTROL | Modifiers::SHIFT
    }
}

fn default_shortcuts() -> Vec<(HotKey, Command)> {
    let m = Some(shortcut_modifiers());
    vec![
        (HotKey::new(m, Code::Space), Command::ToggleBar),
        (HotKey::new(m, Code::KeyS), Command::Capture),
        (HotKey::new(m, Code::KeyR), Command::ToggleRecording),
        (HotKey::new(m, Code::KeyP), Command::TogglePause),
        (
            HotKey::new(m, Code::Digit1),
            Command::CaptureWith(CaptureMode::Region),
        ),
        (
            HotKey::new(m, Code::Digit2),
            Command::CaptureWith(CaptureMode::Window),
        ),
        (
            HotKey::new(m, Code::Digit3),
            Command::CaptureWith(CaptureMode::Screen),
        ),
        (HotKey::new(m, Code::KeyA), Command::RepeatLastRegion),
        (HotKey::new(m, Code::KeyB), Command::ToggleBuffer),
        (HotKey::new(m, Code::KeyC), Command::CopyNewest),
    ]
}

struct App {
    bar: BarState,
    collapsed: bool,
    buffer: Buffer,
    capture: CaptureService,
    pending: HashMap<u64, ItemId>,
    record_started: Option<Instant>,
    toast: Option<(String, Instant)>,
    captures_dir: PathBuf,
    // Kept alive for the lifetime of the app; dropping it unregisters the keys.
    _hotkeys: Option<GlobalHotKeyManager>,
    shortcut_map: HashMap<u32, Command>,
    window_size: egui::Vec2,
    bar_hovered: bool,
}

impl App {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        cb_ui::install_fonts(&cc.egui_ctx, cb_platform::system_ui_fonts());
        if let Err(e) = cb_platform::remove_window_frame(cc) {
            tracing::warn!(error = %e, "window frame stays visible around the bar");
        }

        // Debug builds can opt out, so the bar itself can be screenshotted during development.
        let show_in_capture =
            cfg!(debug_assertions) && std::env::var_os("CB_SHOW_IN_CAPTURE").is_some();
        if !show_in_capture {
            if let Err(e) = cb_platform::exclude_from_capture(cc) {
                // FR-6 is not met on this OS yet: captures will include the bar.
                tracing::warn!(error = %e, "bar cannot be excluded from captures");
            }
        }

        let mut shortcut_map = HashMap::new();
        let hotkeys = match GlobalHotKeyManager::new() {
            Ok(manager) => {
                for (hk, cmd) in default_shortcuts() {
                    match manager.register(hk) {
                        Ok(()) => {
                            shortcut_map.insert(hk.id(), cmd);
                        }
                        // US-3.2: surface conflicts in the UI instead of the log.
                        Err(e) => tracing::warn!(?cmd, error = %e, "shortcut unavailable"),
                    }
                }
                Some(manager)
            }
            Err(e) => {
                // Wayland without the GlobalShortcuts portal lands here (US-3.1).
                tracing::warn!(error = %e, "global shortcuts unavailable");
                None
            }
        };

        // Wake the UI whenever a shortcut fires, even if it is idle.
        let ctx = cc.egui_ctx.clone();
        GlobalHotKeyEvent::set_event_handler(Some(move |_ev: GlobalHotKeyEvent| {
            ctx.request_repaint()
        }));

        let captures_dir = std::env::var_os("HOME")
            .or_else(|| std::env::var_os("USERPROFILE"))
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."))
            .join("Captures");

        let source: Arc<dyn cb_core::ScreenSource> = Arc::from(cb_platform::screen_source());

        Self {
            bar: BarState {
                mic_on: true,
                ..Default::default()
            },
            collapsed: false,
            buffer: Buffer::new(),
            capture: CaptureService::new(source),
            pending: HashMap::new(),
            record_started: None,
            toast: None,
            captures_dir,
            _hotkeys: hotkeys,
            shortcut_map,
            window_size: INITIAL_SIZE.into(),
            bar_hovered: false,
        }
    }

    fn flash(&mut self, msg: impl Into<String>) {
        self.toast = Some((msg.into(), Instant::now()));
    }

    fn target_for(&self, mode: CaptureMode) -> CaptureTarget {
        // TODO(US-4.2 / US-4.3): open the region overlay or window picker first.
        match mode {
            CaptureMode::Region => CaptureTarget::Region(Rect {
                x: 0,
                y: 0,
                width: 800,
                height: 600,
            }),
            CaptureMode::Window => CaptureTarget::Display(DisplayId(0)),
            CaptureMode::Screen => CaptureTarget::Display(DisplayId(0)),
        }
    }

    fn handle(&mut self, cmd: Command) {
        match cmd {
            Command::SetMode(mode) => self.bar.mode = mode,
            Command::Capture => self.capture_still(self.bar.mode),
            Command::CaptureWith(mode) => {
                self.bar.mode = mode;
                self.capture_still(mode);
            }
            Command::ToggleRecording => self.toggle_recording(),
            Command::TogglePause => self.flash("Pause: not implemented yet (US-5.2)"),
            Command::ToggleBuffer => self.bar.tray_open = !self.bar.tray_open,
            Command::ToggleBar => self.collapsed = !self.collapsed,
            Command::RepeatLastRegion => self.capture_still(CaptureMode::Region),
            Command::CopyNewest => self.flash("Copy: not implemented yet (US-4.4)"),
            Command::OpenSettings => self.flash("Settings: not implemented yet (US-8.2)"),
            Command::Cancel => {}
        }
    }

    /// FR-20: the item appears in the buffer immediately; the frame arrives later.
    fn capture_still(&mut self, mode: CaptureMode) {
        let item = self.buffer.push(ItemKind::Image, mode);
        if let Some(i) = self.buffer.get_mut(item) {
            i.state = ItemState::Encoding { progress_pct: 0 };
        }
        let req = self.capture.request_still(self.target_for(mode));
        self.pending.insert(req, item);
        self.flash(format!("Added to buffer · {}", self.buffer.len()));
    }

    fn toggle_recording(&mut self) {
        match self.record_started.take() {
            None => {
                self.record_started = Some(Instant::now());
                self.bar.recording = true;
            }
            Some(start) => {
                self.bar.recording = false;
                let ms = start.elapsed().as_millis() as u64;
                let id = self
                    .buffer
                    .push(ItemKind::Video { duration_ms: ms }, self.bar.mode);
                if let Some(i) = self.buffer.get_mut(id) {
                    i.state =
                        ItemState::Failed("recording backend not implemented (US-5.1)".into());
                }
            }
        }
    }

    fn poll_captures(&mut self) {
        let events: Vec<_> = self.capture.poll().collect();
        for ev in events {
            match ev {
                CaptureEvent::Still { request_id, frame } => {
                    let Some(item) = self.pending.remove(&request_id) else {
                        continue;
                    };
                    let _ = std::fs::create_dir_all(&self.captures_dir);
                    let path = self.captures_dir.join(format!("capture-{}.png", item.0));
                    let result = std::fs::File::create(&path)
                        .map_err(cb_encode::EncodeError::from)
                        .and_then(|f| cb_encode::write_png(&frame, std::io::BufWriter::new(f)));
                    if let Some(i) = self.buffer.get_mut(item) {
                        match result {
                            Ok(()) => {
                                i.state = ItemState::Ready;
                                i.path = Some(path);
                            }
                            Err(e) => i.state = ItemState::Failed(e.to_string()),
                        }
                    }
                }
                CaptureEvent::Failed { request_id, error } => {
                    if let Some(item) = self.pending.remove(&request_id) {
                        if let Some(i) = self.buffer.get_mut(item) {
                            i.state = ItemState::Failed(error.to_string());
                        }
                    }
                }
            }
        }
    }

    fn draw(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        if self.collapsed {
            self.bar_hovered = false;
            // M2.3 collapsed pill.
            let r = ui.add(
                egui::Button::new(
                    egui::RichText::new(format!("Capture Bar · {}", self.buffer.len()))
                        .color(palette::TEXT),
                )
                .fill(palette::BAR)
                .corner_radius(egui::CornerRadius::same(24))
                .min_size(egui::vec2(240.0, 48.0)),
            );
            if r.clicked() {
                self.handle(Command::ToggleBar);
            }
            return;
        }

        let out = cb_ui::bar(ui, &self.bar, &self.buffer);
        self.bar_hovered = out.hovered;
        if out.start_drag {
            ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
        }
        if out.toggle_mic {
            self.bar.mic_on = !self.bar.mic_on;
        }
        if out.toggle_cam {
            self.bar.cam_on = !self.bar.cam_on;
        }
        if out.collapse {
            self.handle(Command::ToggleBar);
        }
        for cmd in out.commands {
            self.handle(cmd);
        }

        if self.bar.tray_open {
            ui.add_space(6.0);
            cb_ui::tray(ui, &self.buffer);
        }

        if let Some((msg, at)) = &self.toast {
            if at.elapsed() < Duration::from_secs(2) {
                ui.label(egui::RichText::new(msg).color(palette::MUTED).size(12.0));
            } else {
                self.toast = None;
            }
        }
    }

    /// Keep the window exactly as large as what is drawn, so nothing but the bar shows.
    fn fit_window(&mut self, ctx: &egui::Context, used: egui::Vec2) {
        // The open tray gives tooltips enough room already.
        let tooltip_room = if self.bar_hovered && !self.bar.tray_open {
            cb_ui::TOOLTIP_ROOM
        } else {
            0.0
        };
        let size = (used + egui::vec2(0.0, tooltip_room)).ceil();
        if size != self.window_size {
            self.window_size = size;
            ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(size));
        }
    }
}

impl eframe::App for App {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        [0.0, 0.0, 0.0, 0.0] // transparent window: only the bar is visible
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = &ui.ctx().clone();
        // Global shortcuts -> commands.
        while let Ok(ev) = GlobalHotKeyEvent::receiver().try_recv() {
            if ev.state == HotKeyState::Pressed {
                if let Some(cmd) = self.shortcut_map.get(&ev.id).cloned() {
                    self.handle(cmd);
                }
            }
        }
        self.poll_captures();

        if let Some(start) = self.record_started {
            self.bar.elapsed_secs = start.elapsed().as_secs();
        }

        let mut used = self.window_size;
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE)
            .show(ui, |ui| {
                used = content_size(ui, |ui| self.draw(ui, ctx));
            });
        self.fit_window(ctx, used);

        // Keep the timer and toast fresh without spinning the CPU (NFR-7).
        ctx.request_repaint_after(Duration::from_millis(250));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The window follows this measurement, so it must not report the panel's size.
    #[test]
    fn content_size_inside_a_panel_is_the_content() {
        let ctx = egui::Context::default();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1000.0, 500.0),
            )),
            ..Default::default()
        };
        let mut size = egui::Vec2::ZERO;
        let mut output = ctx.run_ui(input, |ui| {
            egui::CentralPanel::default()
                .frame(egui::Frame::NONE)
                .show(ui, |ui| {
                    size = content_size(ui, |ui| {
                        ui.allocate_exact_size(egui::vec2(240.0, 48.0), egui::Sense::hover());
                    });
                });
        });
        output.textures_delta.clear();
        assert_eq!(size, egui::vec2(240.0, 48.0));
    }
}
