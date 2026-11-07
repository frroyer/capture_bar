//! Capture Bar — entry point. Wires the floating bar (cb-ui), global
//! shortcuts, the capture service and the buffer together through commands.

#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

use cb_buffer::{Buffer, ItemId, ItemKind, ItemState};
use cb_capture::{CaptureEvent, CaptureService};
use cb_core::{CaptureMode, CaptureTarget, Command, DisplayId, Edge, Rect};
use cb_platform::TrayAction;
use cb_ui::{palette, BarState, TooltipSide};
use eframe::egui;
use global_hotkey::hotkey::{Code, HotKey, Modifiers};
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};

/// Only for the first frame: after that the window follows its content.
const INITIAL_SIZE: [f32; 2] = [900.0, 54.0];
const PLACEMENTS_STORAGE_KEY: &str = "bar_placements";
/// FR-3: a drop this close to an edge docks the bar.
const SNAP_DISTANCE: f32 = 24.0;
/// How far a docked bar sits from the edge, in points.
const DOCK_GAP: f32 = 8.0;
const TRAY_WIDTH_BESIDE_BAR: f32 = 640.0;
const TOAST_TIME: Duration = Duration::from_secs(2);
const HOVER_GRACE: Duration = Duration::from_millis(300);
/// Catches monitor changes that do not move the bar.
const DISPLAY_POLL: Duration = Duration::from_secs(2);
/// How long to wait for a requested move before trusting the position again.
const MOVE_TIMEOUT: Duration = Duration::from_secs(1);
/// FR-5: the bar's opacity while the cursor is away.
const FADED_OPACITY: f32 = 0.4;
/// Show, hide and fade animations, in seconds (US-2.4: at most 150 ms).
const FADE_TIME: f32 = 0.15;

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
        // Restored per monitor layout by `App::track_position` instead.
        persist_window: false,
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

/// The point value that egui converts back to exactly `pixel`. Its conversion may
/// truncate, so a plain division can land one pixel short.
fn points_for_pixel(pixel: i32, ppp: f32) -> f32 {
    let nudge = if pixel < 0 { -0.25 } else { 0.25 };
    (pixel as f32 + nudge) / ppp
}

/// The opacity of everything (hide and show), and of the bar on top of that (fade),
/// easing towards where they should be. Requests its own repaints while moving.
fn animated_opacity(ctx: &egui::Context, hidden: bool, cursor_near: bool) -> (f32, f32) {
    let shown = ctx.animate_bool_with_time(egui::Id::unique("bar shown"), !hidden, FADE_TIME);
    let target = if cursor_near { 1.0 } else { FADED_OPACITY };
    let fade = ctx.animate_value_with_time(egui::Id::unique("bar fade"), target, FADE_TIME);
    (shown, fade)
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
    /// FR-4: nothing is drawn and clicks go through. The window stays open, because
    /// eframe stops running a hidden window and it would never hear the shortcut again.
    hidden: bool,
    click_through: bool,
    /// The bar's own opacity this frame, without what opens beside it.
    bar_fade: f32,
    buffer: Buffer,
    capture: CaptureService,
    pending: HashMap<u64, ItemId>,
    record_started: Option<Instant>,
    toast: Option<(String, Instant)>,
    captures_dir: PathBuf,
    // Kept alive for the lifetime of the app; dropping it unregisters the keys.
    _hotkeys: Option<GlobalHotKeyManager>,
    _tray: Option<cb_platform::Tray>,
    /// From global shortcuts and the tray icon.
    commands: mpsc::Receiver<Command>,
    window_size: egui::Vec2,
    bar_hovered: bool,
    hover_seen: Option<Instant>,
    /// The transparent window space reserved for tooltips is open.
    band: bool,
    tray_size: egui::Vec2,
    /// Where the bar was drawn inside the window, and its size, in points.
    bar_offset: egui::Vec2,
    bar_size: egui::Vec2,
    /// The bar's top-left corner in physical pixels. The window follows from it.
    bar_pos: Option<(i32, i32)>,
    dock: Option<Edge>,
    dragging: bool,
    pull_after_drop: bool,
    screen: Arc<dyn cb_core::ScreenSource>,
    displays: Vec<cb_core::DisplayInfo>,
    displays_checked: Option<Instant>,
    layout: Option<String>,
    /// The bar's position and dock, per monitor layout (FR-2, FR-3).
    placements: HashMap<String, (i32, i32, Option<Edge>)>,
    position: Option<(i32, i32)>,
    /// A move this app requested and has not seen land yet: not the user's position.
    moving_to: Option<((i32, i32), Instant)>,
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

        // With a handler set, the crate no longer fills `GlobalHotKeyEvent::receiver()`.
        // Waking the UI here makes shortcuts work even if it is idle.
        let (commands_tx, commands) = mpsc::channel();
        let ctx = cc.egui_ctx.clone();
        let tx = commands_tx.clone();
        GlobalHotKeyEvent::set_event_handler(Some(move |ev: GlobalHotKeyEvent| {
            if ev.state == HotKeyState::Pressed {
                if let Some(cmd) = shortcut_map.get(&ev.id) {
                    let _ = tx.send(cmd.clone());
                    ctx.request_repaint();
                }
            }
        }));

        let ctx = cc.egui_ctx.clone();
        let tray = cb_platform::create_tray(move |action| {
            match action {
                TrayAction::ToggleBar => {
                    let _ = commands_tx.send(Command::ToggleBar);
                }
                TrayAction::Quit => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
            }
            ctx.request_repaint();
        })
        .inspect_err(|e| tracing::warn!(error = %e, "no tray icon"))
        .ok();

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
            hidden: false,
            click_through: false,
            bar_fade: 1.0,
            buffer: Buffer::new(),
            capture: CaptureService::new(source.clone()),
            pending: HashMap::new(),
            record_started: None,
            toast: None,
            captures_dir,
            _hotkeys: hotkeys,
            _tray: tray,
            commands,
            window_size: INITIAL_SIZE.into(),
            bar_hovered: false,
            hover_seen: None,
            band: false,
            tray_size: egui::vec2(TRAY_WIDTH_BESIDE_BAR, 150.0),
            bar_offset: egui::Vec2::ZERO,
            bar_size: INITIAL_SIZE.into(),
            bar_pos: None,
            dock: None,
            dragging: false,
            pull_after_drop: false,
            screen: source,
            displays: Vec::new(),
            displays_checked: None,
            layout: None,
            placements: cc
                .storage
                .and_then(|s| eframe::get_value(s, PLACEMENTS_STORAGE_KEY))
                .unwrap_or_default(),
            position: None,
            moving_to: None,
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
            Command::ToggleBar => self.hidden = !self.hidden,
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
        // Frozen during a drag: changing the band would move the bar under the pointer.
        if !self.dragging {
            self.band = self.bar_hovered && !self.bar.tray_open;
        }
        let toast_on = match &self.toast {
            Some((_, at)) if at.elapsed() >= TOAST_TIME => {
                self.toast = None;
                false
            }
            Some(_) => true,
            None => false,
        };
        let has_extras = self.band || self.bar.tray_open || toast_on;

        ui.spacing_mut().item_spacing = egui::vec2(6.0, 6.0);
        match self.bar.tooltip_side {
            TooltipSide::Below => {
                self.draw_bar(ui, ctx);
                self.draw_extras(ui);
            }
            TooltipSide::Above => {
                self.draw_extras(ui);
                self.draw_bar(ui, ctx);
            }
            TooltipSide::Right => {
                ui.horizontal_top(|ui| {
                    self.draw_bar(ui, ctx);
                    if has_extras {
                        ui.vertical(|ui| self.draw_extras(ui));
                    }
                });
            }
            TooltipSide::Left => {
                ui.horizontal_top(|ui| {
                    if has_extras {
                        ui.vertical(|ui| self.draw_extras(ui));
                    }
                    self.draw_bar(ui, ctx);
                });
            }
        }
    }

    fn draw_bar(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        // Only the bar fades: the tray and the toast stay readable.
        let fade = self.bar_fade;
        let out = ui
            .scope(|ui| {
                ui.multiply_opacity(fade);
                if self.collapsed {
                    cb_ui::pill(ui, &self.bar, &self.buffer)
                } else {
                    cb_ui::bar(ui, &self.bar, &self.buffer)
                }
            })
            .inner;
        self.bar_offset = out.top_left;
        self.bar_size = out.size;
        if out.hovered {
            self.hover_seen = Some(Instant::now());
        }
        // Grace time: when the band opens above the bar, the window moves and the
        // pointer reads as outside until the next mouse event. Without this the band
        // would close again, and keep flipping.
        self.bar_hovered = self.hover_seen.is_some_and(|at| at.elapsed() < HOVER_GRACE);
        if out.start_drag {
            ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
            self.dragging = true;
        }
        if out.grip_double_clicked {
            if let Some(position) = self.bar_pos {
                let ppp = ctx.pixels_per_point();
                let rect = self.bar_rect(position, ppp);
                if let Some(edge) = cb_core::nearest_edge(rect, &self.displays) {
                    self.dock = Some(edge);
                }
            }
        }
        if out.toggle_mic {
            self.bar.mic_on = !self.bar.mic_on;
        }
        if out.toggle_cam {
            self.bar.cam_on = !self.bar.cam_on;
        }
        if out.collapse {
            self.collapsed = true;
        }
        if out.expand {
            self.collapsed = false;
        }
        for cmd in out.commands {
            self.handle(cmd);
        }
    }

    /// What opens on the bar's roomy side: the tray, the toast, the tooltip band.
    fn draw_extras(&mut self, ui: &mut egui::Ui) {
        let beside = matches!(
            self.bar.tooltip_side,
            TooltipSide::Left | TooltipSide::Right
        );
        if self.bar.tray_open {
            let width = if beside || self.collapsed {
                TRAY_WIDTH_BESIDE_BAR
            } else {
                self.bar_size.x
            };
            let buffer = &self.buffer;
            self.tray_size = content_size(ui, |ui| cb_ui::tray(ui, buffer, width));
        }
        if let Some((msg, _)) = &self.toast {
            ui.label(egui::RichText::new(msg).color(palette::MUTED).size(12.0));
        }
        if self.band {
            let room = if beside {
                egui::vec2(cb_ui::TOOLTIP_ROOM_SIDE, 1.0)
            } else {
                egui::vec2(1.0, cb_ui::TOOLTIP_ROOM)
            };
            ui.allocate_exact_size(room, egui::Sense::hover());
        }
    }

    /// Which side of the bar gets the tray, the toast and tooltips. Below when a
    /// floating bar has the room there, so nothing has to move.
    fn roomy_side(&self, ppp: f32) -> TooltipSide {
        match self.dock {
            Some(Edge::Top) => TooltipSide::Below,
            Some(Edge::Bottom) => TooltipSide::Above,
            Some(Edge::Left) => TooltipSide::Right,
            Some(Edge::Right) => TooltipSide::Left,
            None => {
                let extra = if self.bar.tray_open {
                    self.tray_size.y + 6.0
                } else {
                    cb_ui::TOOLTIP_ROOM
                };
                let fits_below = self.bar_pos.is_none_or(|position| {
                    let mut rect = self.bar_rect(position, ppp);
                    rect.height += (extra * ppp).ceil() as u32;
                    cb_core::is_fully_visible(rect, &self.displays)
                });
                if fits_below {
                    TooltipSide::Below
                } else {
                    TooltipSide::Above
                }
            }
        }
    }

    /// Follow the window, the monitors and the end of a drag.
    fn track_position(&mut self, ctx: &egui::Context) {
        let Some(outer) = ctx.input(|i| i.viewport().outer_rect) else {
            return;
        };
        let ppp = ctx.pixels_per_point();
        let position = (
            (outer.min.x * ppp).round() as i32,
            (outer.min.y * ppp).round() as i32,
        );
        let moved = self.position != Some(position);
        self.position = Some(position);
        let bar_on_screen = (
            position.0 + (self.bar_offset.x * ppp).round() as i32,
            position.1 + (self.bar_offset.y * ppp).round() as i32,
        );

        // The OS moves windows off a removed monitor, so the layout is checked before
        // any move is taken as the user's: that move must not overwrite the old layout.
        let poll_due = self
            .displays_checked
            .is_none_or(|at| at.elapsed() >= DISPLAY_POLL);
        if moved || poll_due {
            self.displays_checked = Some(Instant::now());
            match self.screen.displays() {
                Ok(displays) => self.displays = displays,
                Err(e) => tracing::debug!(error = %e, "display list unavailable"),
            }
        }
        let layout = cb_core::layout_key(&self.displays);

        if self
            .moving_to
            .is_some_and(|(to, at)| to == position || at.elapsed() >= MOVE_TIMEOUT)
        {
            self.moving_to = None;
        }

        if self.layout.as_ref() != Some(&layout) {
            let (x, y, dock) = self.placements.get(&layout).copied().unwrap_or((
                bar_on_screen.0,
                bar_on_screen.1,
                self.dock,
            ));
            self.dock = dock;
            self.bar_pos = Some(cb_core::pull_on_screen(
                self.bar_rect((x, y), ppp),
                &self.displays,
            ));
            self.layout = Some(layout);
        } else if moved && self.moving_to.is_none() {
            self.bar_pos = Some(bar_on_screen);
        }

        self.bar.snap_preview = false;
        if self.dragging {
            let snap = self.bar_pos.and_then(|position| {
                let distance = (SNAP_DISTANCE * ppp).round() as i32;
                cb_core::snap_edge(self.bar_rect(position, ppp), &self.displays, distance)
            });
            match cb_platform::primary_button_down() {
                Some(true) => self.bar.snap_preview = snap.is_some(),
                Some(false) => {
                    self.dragging = false;
                    self.dock = snap;
                    self.pull_after_drop = snap.is_none();
                }
                // This OS cannot report the drop, so the bar simply stays where it is put.
                None => self.dragging = false,
            }
        }
        self.bar.vertical = !self.collapsed && self.dock.is_some_and(Edge::is_side);
        if !self.dragging {
            self.bar.tooltip_side = self.roomy_side(ppp);
        }
    }

    /// Rect of the bar itself at `position`, without what opens beside it.
    fn bar_rect(&self, position: (i32, i32), ppp: f32) -> Rect {
        Rect {
            x: position.0,
            y: position.1,
            width: (self.bar_size.x * ppp).ceil() as u32,
            height: (self.bar_size.y * ppp).ceil() as u32,
        }
    }

    /// Fit the window to what was drawn, and place it so the bar lands on `bar_pos`
    /// wherever it sits inside the window.
    fn place_window(&mut self, ctx: &egui::Context, used: egui::Vec2) {
        let size = used.ceil();
        if size != self.window_size {
            self.window_size = size;
            ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(size));
        }

        let Some(mut bar_pos) = self.bar_pos else {
            return;
        };
        if self.dragging {
            return;
        }
        let ppp = ctx.pixels_per_point();
        let rect = self.bar_rect(bar_pos, ppp);
        if let Some(edge) = self.dock {
            let gap = (DOCK_GAP * ppp).round() as i32;
            bar_pos = cb_core::docked_position(rect, edge, &self.displays, gap);
        } else if self.pull_after_drop {
            self.pull_after_drop = false;
            bar_pos = cb_core::pull_on_screen(rect, &self.displays);
        }
        self.bar_pos = Some(bar_pos);
        if let Some(layout) = &self.layout {
            self.placements
                .insert(layout.clone(), (bar_pos.0, bar_pos.1, self.dock));
        }

        let target = (
            bar_pos.0 - (self.bar_offset.x * ppp).round() as i32,
            bar_pos.1 - (self.bar_offset.y * ppp).round() as i32,
        );
        let waiting = self.moving_to.is_some_and(|(to, _)| to == target);
        if self.position != Some(target) && !waiting {
            ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(egui::pos2(
                points_for_pixel(target.0, ppp),
                points_for_pixel(target.1, ppp),
            )));
            self.moving_to = Some((target, Instant::now()));
        }
    }
}

impl eframe::App for App {
    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        eframe::set_value(storage, PLACEMENTS_STORAGE_KEY, &self.placements);
    }

    /// The bar has no quit button yet, so a killed process should lose little.
    /// The file is only written when something changed.
    fn auto_save_interval(&self) -> Duration {
        Duration::from_secs(5)
    }

    fn persist_egui_memory(&self) -> bool {
        false
    }

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        [0.0, 0.0, 0.0, 0.0] // transparent window: only the bar is visible
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = &ui.ctx().clone();
        while let Ok(cmd) = self.commands.try_recv() {
            self.handle(cmd);
        }
        self.poll_captures();
        self.track_position(ctx);

        if let Some(start) = self.record_started {
            self.bar.elapsed_secs = start.elapsed().as_secs();
        }

        if self.click_through != self.hidden {
            self.click_through = self.hidden;
            ctx.send_viewport_cmd(egui::ViewportCommand::MousePassthrough(self.hidden));
        }
        let cursor_near =
            self.dragging || self.bar_hovered || ctx.input(|i| i.pointer.hover_pos().is_some());
        let (shown, fade) = animated_opacity(ctx, self.hidden, cursor_near);
        self.bar_fade = fade;

        let mut used = self.window_size;
        if shown > 0.0 {
            egui::CentralPanel::default()
                .frame(egui::Frame::NONE)
                .show(ui, |ui| {
                    ui.multiply_opacity(shown);
                    used = content_size(ui, |ui| self.draw(ui, ctx));
                });
        }
        self.place_window(ctx, used);

        // Keep the timer and toast fresh without spinning the CPU (NFR-7). A drag polls
        // faster, to show the snap preview and catch the drop.
        let wake = if self.dragging { 16 } else { 250 };
        ctx.request_repaint_after(Duration::from_millis(wake));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn points_for_pixel_round_trips_at_fractional_scales() {
        for ppp in [1.0, 1.25, 1.5, 1.75, 2.0, 2.25] {
            for pixel in [-3840, -1, 0, 1, 100, 1439, 1920, 3840, 5119] {
                let back = points_for_pixel(pixel, ppp) * ppp;
                assert_eq!(back as i32, pixel, "truncating at {ppp}");
                assert_eq!(back.round() as i32, pixel, "rounding at {ppp}");
            }
        }
    }

    /// Frames at 60 Hz until `FADE_TIME` is over, for `animated_opacity`.
    fn run_opacity(ctx: &egui::Context, time: &mut f64, hidden: bool, near: bool) -> (f32, f32) {
        let mut result = (0.0, 0.0);
        for _ in 0..=(f64::from(FADE_TIME) * 60.0).ceil() as u32 {
            *time += 1.0 / 60.0;
            let input = egui::RawInput {
                time: Some(*time),
                predicted_dt: 1.0 / 60.0,
                ..Default::default()
            };
            let mut output = ctx.run_ui(input, |ui| {
                result = animated_opacity(ui.ctx(), hidden, near);
            });
            output.textures_delta.clear();
        }
        result
    }

    #[test]
    fn bar_fades_to_40_percent_and_back_within_150_ms() {
        let ctx = egui::Context::default();
        let mut time = 0.0;
        assert_eq!(run_opacity(&ctx, &mut time, false, true), (1.0, 1.0));
        let (_, away) = run_opacity(&ctx, &mut time, false, false);
        assert!((away - 0.4).abs() < 1e-3, "away at {away}");
        let (_, back) = run_opacity(&ctx, &mut time, false, true);
        assert!((back - 1.0).abs() < 1e-3, "back at {back}");
    }

    #[test]
    fn hiding_and_showing_finish_within_150_ms() {
        let ctx = egui::Context::default();
        let mut time = 0.0;
        run_opacity(&ctx, &mut time, false, true);
        assert_eq!(run_opacity(&ctx, &mut time, true, true).0, 0.0);
        assert_eq!(run_opacity(&ctx, &mut time, false, true).0, 1.0);
    }

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
