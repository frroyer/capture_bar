//! Takebox — entry point. Wires the floating bar (cb-ui), global
//! shortcuts, the capture service and the buffer together through commands.

#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod config;
mod demo;
mod hotkeys;
mod output;
mod peek_video;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant, SystemTime};

use cb_buffer::{index, Buffer, BufferItem, ItemId, ItemKind, ItemState, Selection};
use cb_capture::{CaptureEvent, CaptureService};
use cb_core::{
    AudioSources, CaptureMode, CaptureResult, CaptureTarget, Command, DisplayId, Edge, Frame,
    Recording, Rect, Settings, StreamHandle,
};
use cb_platform::TrayAction;
use cb_ui::{
    tr, BarState, BatchAction, Editor, EditorResult, ItemAction, Overlay, OverlayResult, Peek,
    PeekAction, SelectHow, SettingsWindow, ToastAction, TooltipSide,
};
use eframe::egui;
use global_hotkey::{GlobalHotKeyEvent, HotKeyState};

/// Only for the first frame: after that the window follows its content.
const INITIAL_SIZE: [f32; 2] = [900.0, 54.0];
const PLACEMENTS_STORAGE_KEY: &str = "bar_placements";
const MODE_STORAGE_KEY: &str = "capture_mode";
const REGION_STORAGE_KEY: &str = "last_region";
const BUBBLE_STORAGE_KEY: &str = "camera_bubble";
/// Where the prefix and folder were kept before the settings file: read once, to move them.
const PREFIX_STORAGE_KEY: &str = "file_prefix";
const FOLDER_STORAGE_KEY: &str = "captures_dir";
/// FR-3: a drop this close to an edge docks the bar.
const SNAP_DISTANCE: f32 = 24.0;
/// How far a docked bar sits from the edge, in points.
const DOCK_GAP: f32 = 8.0;
const TRAY_WIDTH_BESIDE_BAR: f32 = 640.0;
/// How often captures past the retention period are looked for (FR-25).
const SWEEP_EVERY: Duration = Duration::from_secs(60 * 60);
/// The tray folds away once the cursor has been off the bar's window this long.
const TRAY_IDLE: Duration = Duration::from_secs(2);

/// Whether an open tray should fold now. `seen` is when the cursor was last on the window,
/// or when the tray was closed: the wait starts again each time it opens.
fn tray_idle(open: bool, cursor_in: bool, seen: &mut Instant, now: Instant) -> bool {
    if !open || cursor_in {
        *seen = now;
        return false;
    }
    now.duration_since(*seen) >= TRAY_IDLE
}

/// The buffer the last run left (FR-25), and whether its index was damaged. A damaged
/// index is kept beside as `.bad`, never overwritten, so nothing in it is lost for good.
fn load_buffer(path: &Path) -> (Buffer, bool) {
    match index::read(path) {
        Ok(items) => (Buffer::restore(items), false),
        Err(error) => {
            tracing::error!(%error, ?path, "buffer index unreadable");
            if let Err(e) = std::fs::rename(path, path.with_extension("ron.bad")) {
                tracing::warn!(error = %e, "damaged index left in place");
            }
            (Buffer::new(), true)
        }
    }
}
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
/// The flash at launch, so the bar can be found before it fades.
const ATTENTION_TIME: Duration = Duration::from_secs(3);
const ATTENTION_PULSES: f32 = 3.0;

/// The app's name for the OS: window title and eframe storage folder.
const APP_ID: &str = "Takebox";

fn main() -> eframe::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();
    // `takebox clap-report <folder>`: measure a demo session's clap test again (US-10.2).
    let args: Vec<String> = std::env::args().collect();
    if let [_, command, dir] = args.as_slice() {
        if command == "clap-report" {
            println!("{}", output::clap_message(Path::new(dir)));
            return Ok(());
        }
    }
    tracing::info!(backend = cb_platform::backend_name(), "starting Takebox");
    config::migrate_legacy();
    if let Err(e) = cb_platform::autostart::migrate_legacy() {
        tracing::warn!("could not move the login entry from Capture Bar: {e}");
    }

    let mut viewport = egui::ViewportBuilder::default();
    // Shown on the taskbar button while the bar is minimized.
    if let Some((rgba, side)) = cb_platform::app_icon() {
        viewport = viewport.with_icon(egui::IconData {
            rgba,
            width: side,
            height: side,
        });
    }
    let options = eframe::NativeOptions {
        viewport: viewport
            .with_title(APP_ID)
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

    eframe::run_native(APP_ID, options, Box::new(|cc| Ok(Box::new(App::new(cc)))))
}

/// The tray menu's items in the interface's language.
fn tray_labels() -> [String; 2] {
    [tr!("tray-menu-toggle"), tr!("tray-menu-quit")]
}

/// The size of what `add` draws. A panel's own UI always reports the whole panel,
/// so this measures inside a nested scope.
fn content_size(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui)) -> egui::Vec2 {
    ui.scope(add).response.rect.size()
}

use cb_ui::points_for_pixel;

/// Ready one of the windows egui opens for the bar (overlay, countdown, camera bubble)
/// to show: it loses the thin frame Windows 11 draws around borderless windows, and
/// stays out of captures when `exclude`. Whether that is done: the window exists only
/// once drawn, and is kept hidden until then. Nothing to do on other systems.
fn dress_window(title: &str, exclude: bool) -> bool {
    if !cfg!(windows) {
        return true;
    }
    let Some(window) = cb_platform::own_window(title) else {
        return false;
    };
    let _ = cb_platform::remove_window_frame(&window);
    if exclude && !show_in_capture() {
        let _ = cb_platform::exclude_from_capture(&window);
    }
    true
}

/// Whether to ask for `wanted` as the window size. Asking once is not enough: on Windows the
/// end of a drag puts back the size the window had when the drag began, which would leave a
/// bar undocked from a side clipped until its size changed again.
fn needs_resize(
    wanted: egui::Vec2,
    requested: egui::Vec2,
    actual: Option<egui::Vec2>,
    dragging: bool,
) -> bool {
    // Within a point: the size comes back from whole pixels.
    let off = |size: egui::Vec2| (size - wanted).abs().max_elem() >= 1.0;
    wanted != requested || (!dragging && actual.is_some_and(off))
}

/// The opacity of everything (hide and show), and of the bar on top of that (fade),
/// easing towards where they should be. Requests its own repaints while moving.
fn animated_opacity(ctx: &egui::Context, hidden: bool, cursor_near: bool) -> (f32, f32) {
    let shown = ctx.animate_bool_with_time(egui::Id::unique("bar shown"), !hidden, FADE_TIME);
    let target = if cursor_near { 1.0 } else { FADED_OPACITY };
    let fade = ctx.animate_value_with_time(egui::Id::unique("bar fade"), target, FADE_TIME);
    (shown, fade)
}

/// How strongly the bar flashes this long after launch: smooth pulses from and back
/// to 0, then 0 for good.
fn attention_level(elapsed: Duration) -> f32 {
    if elapsed >= ATTENTION_TIME {
        return 0.0;
    }
    let progress = elapsed.as_secs_f32() / ATTENTION_TIME.as_secs_f32();
    (progress * ATTENTION_PULSES * std::f32::consts::PI)
        .sin()
        .powi(2)
}

/// FR-7: the mode chosen in an earlier session.
fn saved_mode(storage: Option<&dyn eframe::Storage>) -> CaptureMode {
    storage
        .and_then(|s| eframe::get_value(s, MODE_STORAGE_KEY))
        .unwrap_or_default()
}

/// The command a hotkey event asks for. A shortcut fires once, on the press.
fn shortcut_command(map: &HashMap<u32, Command>, ev: &GlobalHotKeyEvent) -> Option<Command> {
    (ev.state == HotKeyState::Pressed)
        .then(|| map.get(&ev.id).cloned())
        .flatten()
}

/// A short message beside the bar, for `TOAST_TIME` (FR-12).
struct Toast {
    message: String,
    shown_at: Instant,
    /// The capture this toast confirms, which Annotate opens and Undo removes.
    capture: Option<ItemId>,
}

impl Toast {
    fn new(message: impl Into<String>, capture: Option<ItemId>) -> Self {
        Self {
            message: message.into(),
            shown_at: Instant::now(),
            capture,
        }
    }
}

/// A recording's place in the buffer and how it ended.
type Recorded = (ItemId, CaptureResult<Recording>);
/// A still taken from a running recording (US-5.5), for its buffer item.
type Still = (ItemId, CaptureResult<Frame>);

/// Where the bar is on its way to or from the taskbar (Windows). It has a taskbar
/// button only while minimized.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum Taskbar {
    /// Floating, with no taskbar button.
    #[default]
    Off,
    /// The collapse button was pressed: minimize on the next frame.
    Asked,
    /// Minimized, with a taskbar button to click. eframe runs no frames meanwhile, so
    /// the next frame means the bar is back.
    In,
}

/// How long the bar shows a marker just put down (US-10.3).
const MARKER_SHOWN: Duration = Duration::from_millis(1500);

/// The clap test's period (US-10.2): `CB_CLAP_SECS=10` claps every 10 s through each
/// demo session, to measure how the tracks keep in sync. Off when unset or 0.
fn clap_every() -> Option<Duration> {
    let secs: u64 = std::env::var("CB_CLAP_SECS").ok()?.trim().parse().ok()?;
    (secs > 0).then(|| Duration::from_secs(secs))
}

/// A square at the centre of what a demo session records, black but for a flash at each
/// clap, which also beeps (US-10.2). It is in the video: that is its point.
struct Clap {
    every: Duration,
    next: Instant,
    lit_until: Instant,
    /// Global physical pixels.
    center: (i32, i32),
    dressed: bool,
    shown: bool,
    /// When each beep really played, from the sound card, sent from its thread.
    played: (mpsc::Sender<Instant>, mpsc::Receiver<Instant>),
}

/// The blue outline around a repeated region once it is taken.
struct Highlight {
    /// Global physical pixels.
    region: Rect,
    since: Instant,
    dressed: bool,
    shown: bool,
}

impl Highlight {
    /// Fully lit for this long, then fading out over as long again.
    const HOLD: Duration = Duration::from_millis(350);

    /// 1 while held, then down to 0; `None` once gone.
    fn level(&self, now: Instant) -> Option<f32> {
        let t = now.saturating_duration_since(self.since).as_secs_f32() / Self::HOLD.as_secs_f32();
        (t < 2.0).then(|| (2.0 - t).min(1.0))
    }
}

/// The font file at `path`, if it reads as a font; `None` for an empty path.
fn read_font(path: &str) -> Option<Vec<u8>> {
    if path.is_empty() {
        return None;
    }
    match std::fs::read(path) {
        Ok(bytes) if cb_annotate::font_name(&bytes).is_some() => Some(bytes),
        Ok(_) => {
            tracing::warn!(
                path,
                "editor font is not a font: the interface font is used"
            );
            None
        }
        Err(e) => {
            tracing::warn!(path, error = %e, "editor font not read: the interface font is used");
            None
        }
    }
}

/// What saved annotations are written in: the editor font, else the interface's
/// semibold, else the built-in one.
fn annotation_font(editor: Option<&[u8]>, system: Option<&(Vec<u8>, Vec<u8>)>) -> Arc<[u8]> {
    match (editor, system) {
        (Some(font), _) => font.into(),
        (None, Some((_, semibold))) => semibold.as_slice().into(),
        (None, None) => cb_annotate::fallback_font().into(),
    }
}

/// Debug builds only: `CB_SHOW_IN_CAPTURE` lets screenshots include the app's windows.
fn show_in_capture() -> bool {
    cfg!(debug_assertions) && std::env::var_os("CB_SHOW_IN_CAPTURE").is_some()
}

/// The webcam and its bubble (US-5.4), on while the bar's cam toggle is.
#[derive(Default)]
struct Camera {
    /// What `cb_platform::set_camera` was last told.
    on: bool,
    bubble: cb_ui::CameraBubble,
    /// The last picture handed to the bubble.
    seen: Option<u64>,
    /// Where the bubble first opens: top-left corner, global physical pixels.
    start: Option<cb_core::Rect>,
    /// Its window has lost the OS frame and is kept out of captures.
    dressed: bool,
}

/// Seconds shown on screen before a recording starts (Settings: countdown).
struct Countdown {
    mode: CaptureMode,
    target: CaptureTarget,
    /// Where it shows: the centre of what will be recorded, in global physical pixels.
    center: (i32, i32),
    ends: Instant,
    total: Duration,
    /// Its window has lost the OS frame and is kept out of captures.
    dressed: bool,
    /// Its window is in place and showing.
    shown: bool,
}

struct ActiveRecording {
    handle: Box<dyn StreamHandle>,
    /// Its place in the buffer, there from the start.
    item: ItemId,
    clock: Stopwatch,
}

/// Recorded time: runs from the start, stands still while paused (US-5.2). The video
/// leaves no gap for a pause either, so the two agree.
#[derive(Debug, Clone, Copy)]
struct Stopwatch {
    /// Time banked before the current run.
    banked: Duration,
    /// When the current run began; none while paused.
    running_since: Option<Instant>,
}

impl Stopwatch {
    fn start(now: Instant) -> Self {
        Self {
            banked: Duration::ZERO,
            running_since: Some(now),
        }
    }

    fn paused(&self) -> bool {
        self.running_since.is_none()
    }

    fn elapsed(&self, now: Instant) -> Duration {
        self.banked
            + self
                .running_since
                .map_or(Duration::ZERO, |since| now - since)
    }

    fn pause(&mut self, now: Instant) {
        self.banked = self.elapsed(now);
        self.running_since = None;
    }

    fn resume(&mut self, now: Instant) {
        self.running_since.get_or_insert(now);
    }
}

struct App {
    bar: BarState,
    collapsed: bool,
    taskbar: Taskbar,
    /// FR-4: nothing is drawn and clicks go through. The window stays open, because
    /// eframe stops running a hidden window and it would never hear the shortcut again.
    hidden: bool,
    click_through: bool,
    /// The bar's own opacity this frame, without what opens beside it.
    bar_fade: f32,
    /// When the first frame was drawn.
    launched_at: Option<Instant>,
    buffer: Buffer,
    /// Gets each new state of the buffer, to keep its index on disk (FR-25).
    index: mpsc::Sender<Vec<BufferItem>>,
    /// The buffer generation last sent to `index`.
    indexed: u64,
    /// When expired captures were last taken out of the buffer.
    swept_at: Option<Instant>,
    /// When the cursor was last on the window while the tray was open (see `tray_idle`).
    tray_seen: Instant,
    /// What Copy all, Save all and Clear act on; none means all (FR-24).
    selection: Selection,
    /// What the tray shows for each capture (FR-22).
    thumbnails: HashMap<ItemId, egui::TextureHandle>,
    /// The frames a recording's card plays while the cursor is on it.
    previews: HashMap<ItemId, Vec<egui::TextureHandle>>,
    /// Recordings with a GIF on its way, and how far it is: `None` while it waits its
    /// turn (US-6.2).
    exports: HashMap<ItemId, Option<u8>>,
    capture: CaptureService,
    pending: HashMap<u64, ItemId>,
    /// The desktop grab an overlay is waiting for, and that overlay's mode.
    overlay_request: Option<(u64, CaptureMode)>,
    overlay: Option<Overlay>,
    /// The overlay's window has lost its frame and may show.
    overlay_dressed: bool,
    /// The annotation editor, open on one capture at a time (US-7.1).
    editor: Option<Editor>,
    /// The capture whose image is being read to open the editor.
    editor_loading: Option<ItemId>,
    /// Quick Look on one capture of the buffer, full size.
    peek: Option<Peek>,
    /// The capture whose image is being read for Quick Look.
    peek_loading: Option<ItemId>,
    /// The recording Quick Look plays.
    peek_video: Option<peek_video::PeekVideo>,
    /// Annotations saved before their capture's file was written: they go on once it is.
    annotation_waiting: Option<(ItemId, Arc<Frame>, Vec<cb_annotate::Shape>)>,
    /// What annotation text is written in: the font Settings picked, else the bar's
    /// semibold, as the editor shows it.
    annotation_font: Arc<[u8]>,
    /// The OS interface fonts, kept to set the fonts again when the editor's changes.
    system_fonts: Option<(Vec<u8>, Vec<u8>)>,
    /// The fonts Settings offers for the editor, as name and file, once found.
    font_list: Option<Vec<(String, String)>>,
    font_list_rx: Option<mpsc::Receiver<Vec<(String, String)>>>,
    /// The editor font being read, off the UI thread: its file and its bytes.
    editor_font_rx: Option<mpsc::Receiver<(String, Option<Vec<u8>>)>>,
    /// Where annotated captures are kept as taken, out of sight (US-7.1).
    originals_dir: PathBuf,
    last_region: Option<Rect>,
    /// The running recording (US-5.1).
    recording: Option<ActiveRecording>,
    countdown: Option<Countdown>,
    camera: Camera,
    /// Where the camera bubble last was and its size, global physical pixels: it
    /// reopens there.
    bubble_rect: Option<Rect>,
    /// Finished recordings, sent from their own threads.
    recorded: (mpsc::Sender<Recorded>, mpsc::Receiver<Recorded>),
    stills: (mpsc::Sender<Still>, mpsc::Receiver<Still>),
    /// Wakes the UI when a recording finishes.
    repaint: egui::Context,
    /// The overlay picks what to record rather than what to capture.
    overlay_records: bool,
    toast: Option<Toast>,
    /// Where new captures go (FR-30). Their prefix is `bar.prefix`.
    captures_dir: PathBuf,
    /// The settings without the ones above and `bar.prefix` / `bar.mic_on`, which are kept
    /// where the bar uses them (see `App::settings`).
    prefs: Settings,
    /// What was last sent to `settings_writer`.
    saved_settings: Settings,
    /// The settings file (US-8.2).
    settings_file: PathBuf,
    settings_writer: mpsc::Sender<Settings>,
    settings_window: Option<SettingsWindow>,
    /// The demo session setup, while open (US-10.1).
    demo_setup: Option<cb_ui::DemoSetup>,
    /// The demo session being recorded, or about to be once what to record is picked.
    demo: Option<demo::DemoSession>,
    /// The marker just put down, and when: the bar shows it a moment (US-10.3).
    marker: Option<(cb_core::MarkerKind, Instant)>,
    /// The clap test, while a demo session records with `CB_CLAP_SECS` set (US-10.2).
    clap: Option<Clap>,
    /// The repeated capture whose region lights up once it is taken, and the region.
    highlight_after: Option<(ItemId, Rect)>,
    highlight: Option<Highlight>,
    /// Give the bar's window the keyboard on the next frame.
    focus_bar: bool,
    /// Whether the bar starts at login, as the OS had it when Settings opened (US-8.3).
    autostart: bool,
    /// The answer of the thread changing it.
    autostart_done: Option<mpsc::Receiver<std::io::Result<()>>>,
    /// Put the keyboard in the prefix field on the next frame.
    focus_prefix: bool,
    /// The prefix the captures in the buffer were last offered to be renamed to.
    named_prefix: String,
    /// Ask whether to rename the buffer's captures to the current prefix (FR-30).
    rename_offer: bool,
    /// The answer of the folder dialog, which runs on its own thread: `None` if cancelled.
    folder_choice: Option<mpsc::Receiver<Option<PathBuf>>>,
    /// "Choose…" was clicked: the dialog opens once the frame's window handle is at hand.
    folder_requested: bool,
    /// What the tray asked of a capture this frame (FR-23), done once the frame's window
    /// handle is at hand.
    item_action: Option<(ItemId, ItemAction)>,
    /// What the tray asked of the selection this frame, done like `item_action`.
    batch_action: Option<BatchAction>,
    /// An OS drag loop took the button release: egui is given one on the next frame.
    release_pointer: bool,
    output: mpsc::Sender<output::Job>,
    output_done: mpsc::Receiver<output::Done>,
    /// What Copy puts on the clipboard (M3).
    newest_frame: Option<(ItemId, Arc<Frame>)>,
    /// Global shortcuts; dropping them unregisters the keys. `None` when the OS gives none.
    hotkeys: Option<hotkeys::Hotkeys>,
    /// The shortcuts not in effect, and why (US-3.2).
    shortcut_conflicts: std::collections::BTreeMap<cb_core::Shortcut, cb_core::ShortcutConflict>,
    /// Shortcuts are off while Settings records new keys.
    shortcuts_suspended: bool,
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
        let settings_file = config::path();
        let loaded = config::load(&settings_file, |settings| {
            let Some(storage) = cc.storage else {
                return;
            };
            if let Some(folder) = eframe::get_value(storage, FOLDER_STORAGE_KEY) {
                settings.folder = folder;
            }
            if let Some(prefix) = eframe::get_value(storage, PREFIX_STORAGE_KEY) {
                settings.prefix = prefix;
            }
        });
        tracing::info!(file = %settings_file.display(), "settings");
        cb_ui::i18n::set_language(loaded.settings().language);
        let system_fonts = cb_platform::system_ui_fonts();
        // Read before the first frame, as the interface fonts are.
        let editor_font = read_font(&loaded.settings().editor_font);
        let annotation_font = annotation_font(editor_font.as_deref(), system_fonts.as_ref());
        cb_ui::install_fonts_with(&cc.egui_ctx, system_fonts.clone(), editor_font);
        cb_ui::set_editor_palette(loaded.settings().editor_palette());
        if let Err(e) = cb_platform::remove_window_frame(cc) {
            tracing::warn!(error = %e, "window frame stays visible around the bar");
        }

        // Debug builds can opt out, so the bar itself can be screenshotted during development.
        let show_in_capture = show_in_capture();
        if !show_in_capture {
            if let Err(e) = cb_platform::exclude_from_capture(cc) {
                // FR-6 is not met on this OS yet: captures will include the bar.
                tracing::warn!(error = %e, "bar cannot be excluded from captures");
            }
        }

        cb_ui::set_shortcuts(loaded.settings());
        cb_ui::set_tooltips(loaded.settings().tooltips);
        let mut hotkeys = hotkeys::Hotkeys::new()
            // Wayland without the GlobalShortcuts portal lands here (US-3.1).
            .inspect_err(|e| tracing::warn!(error = %e, "global shortcuts unavailable"))
            .ok();
        let shortcut_conflicts = hotkeys
            .as_mut()
            .map(|h| h.apply(loaded.settings(), false))
            .unwrap_or_default();
        let shortcut_map = hotkeys.as_ref().map(hotkeys::Hotkeys::fired);
        let shortcuts_work = shortcut_map
            .as_ref()
            .is_some_and(|m| !m.lock().unwrap_or_else(|e| e.into_inner()).is_empty());

        // With a handler set, the crate no longer fills `GlobalHotKeyEvent::receiver()`.
        // Waking the UI here makes shortcuts work even if it is idle.
        let (commands_tx, commands) = mpsc::channel();
        // Minimized to the taskbar, the bar runs no frames: bring it back first.
        let restorer = cb_platform::Restorer::new(cc);
        let restore = move || {
            if let Some(restorer) = restorer {
                restorer.restore_if_minimized();
            }
        };
        let ctx = cc.egui_ctx.clone();
        let tx = commands_tx.clone();
        let toast = if !shortcuts_work {
            Some(Toast::new(tr!("toast-no-shortcuts"), None))
        } else if shortcut_conflicts
            .values()
            .any(|c| *c == cb_core::ShortcutConflict::Taken)
        {
            Some(Toast::new(tr!("toast-shortcuts-taken"), None))
        } else {
            None
        };
        GlobalHotKeyEvent::set_event_handler(Some(move |ev: GlobalHotKeyEvent| {
            let Some(map) = &shortcut_map else {
                return;
            };
            let cmd = shortcut_command(&map.lock().unwrap_or_else(|e| e.into_inner()), &ev);
            if let Some(cmd) = cmd {
                tracing::debug!(?cmd, "shortcut");
                restore();
                let _ = tx.send(cmd);
                ctx.request_repaint();
            }
        }));

        let ctx = cc.egui_ctx.clone();
        let labels = tray_labels();
        let tray = cb_platform::create_tray(labels.each_ref().map(String::as_str), move |action| {
            restore();
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

        let toast = toast.or_else(|| match &loaded {
            config::Loaded::Damaged(_, error) => {
                tracing::warn!(%error, "settings file not read");
                Some(Toast::new(tr!("toast-settings-damaged"), None))
            }
            _ => None,
        });
        let settings_writer = config::spawn_writer(settings_file.clone(), |error| {
            tracing::warn!(%error, "settings not written");
        });
        // A new file is written at once, so the moved values are not lost.
        let saved_settings = match &loaded {
            config::Loaded::New(_) => Settings {
                version: 0,
                ..Default::default()
            },
            _ => loaded.settings().clone(),
        };
        let prefs = loaded.settings().clone();
        let captures_dir = prefs.folder.clone();
        let default_dir = Settings::default_folder();
        // Where the default folder is, whichever folder captures go to now (FR-25).
        let index_path = default_dir.join(".buffer").join(index::INDEX_FILE);
        let originals_dir = default_dir.join(".buffer").join("originals");
        let (buffer, damaged) = load_buffer(&index_path);
        let toast = toast.or_else(|| damaged.then(|| Toast::new(tr!("toast-index-damaged"), None)));
        let index = index::spawn_writer(index_path, |error| {
            tracing::warn!(%error, "buffer index not written");
        });
        let prefix = prefs.prefix.clone();

        let source: Arc<dyn cb_core::ScreenSource> = Arc::from(cb_platform::screen_source());
        let ctx = cc.egui_ctx.clone();
        let (output, output_done) = output::spawn(move || ctx.request_repaint());
        for item in buffer.newest_first() {
            if let Some(path) = item.path.clone() {
                let _ = output.send(match item.kind {
                    ItemKind::Image => output::Job::Thumbnail {
                        item: item.id,
                        path,
                    },
                    ItemKind::Video { .. } => output::Job::Preview {
                        item: item.id,
                        path,
                    },
                });
            }
        }
        let ctx = cc.egui_ctx.clone();
        let capture = CaptureService::new(source.clone(), move || ctx.request_repaint());
        // The first recording would otherwise take ~650 ms to start, over US-5.1's 300 ms.
        std::thread::Builder::new()
            .name("recording warm-up".into())
            .spawn(cb_platform::warm_up_recording)
            .expect("spawn recording warm-up thread");

        Self {
            bar: BarState {
                mic_on: prefs.microphone,
                mode: saved_mode(cc.storage),
                prefix: prefix.clone(),
                ..Default::default()
            },
            collapsed: false,
            taskbar: Taskbar::Off,
            hidden: false,
            click_through: false,
            bar_fade: 1.0,
            launched_at: None,
            indexed: buffer.generation(),
            buffer,
            index,
            swept_at: None,
            tray_seen: Instant::now(),
            selection: Selection::default(),
            thumbnails: HashMap::new(),
            previews: HashMap::new(),
            exports: HashMap::new(),
            capture,
            pending: HashMap::new(),
            overlay_request: None,
            overlay: None,
            overlay_dressed: false,
            editor: None,
            editor_loading: None,
            peek: None,
            peek_loading: None,
            peek_video: None,
            annotation_waiting: None,
            annotation_font,
            system_fonts,
            font_list: None,
            font_list_rx: None,
            editor_font_rx: None,
            originals_dir,
            last_region: cc
                .storage
                .and_then(|s| eframe::get_value(s, REGION_STORAGE_KEY)),
            recording: None,
            countdown: None,
            camera: Camera::default(),
            bubble_rect: cc
                .storage
                .and_then(|s| eframe::get_value(s, BUBBLE_STORAGE_KEY)),
            recorded: mpsc::channel(),
            stills: mpsc::channel(),
            repaint: cc.egui_ctx.clone(),
            overlay_records: false,
            toast,
            captures_dir,
            prefs,
            saved_settings,
            settings_file,
            settings_writer,
            settings_window: None,
            demo_setup: None,
            demo: None,
            marker: None,
            clap: None,
            highlight_after: None,
            highlight: None,
            focus_bar: false,
            autostart: false,
            autostart_done: None,
            focus_prefix: false,
            named_prefix: prefix,
            rename_offer: false,
            folder_choice: None,
            folder_requested: false,
            item_action: None,
            batch_action: None,
            release_pointer: false,
            output,
            output_done,
            newest_frame: None,
            hotkeys,
            shortcut_conflicts,
            shortcuts_suspended: false,
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
        self.toast = Some(Toast::new(msg, None));
    }

    /// FR-10: the monitor under the cursor, or the primary one if that is unknown.
    fn screen_target(&self) -> CaptureTarget {
        let display = cb_platform::cursor_position()
            .and_then(|cursor| cb_core::display_at(cursor, &self.displays));
        CaptureTarget::Display(display.map_or(DisplayId(0), |d| d.id))
    }

    /// Whether Screen mode asks which monitor: with several, when the pointer is on the
    /// bar, so it says nothing about the screen wanted. A shortcut pressed with the
    /// pointer elsewhere takes the monitor under it.
    fn pick_screen(&self) -> bool {
        if self.displays.len() < 2 {
            return false;
        }
        let ppp = self.repaint.pixels_per_point();
        match (self.bar_pos, cb_platform::cursor_position()) {
            (Some(bar), Some(cursor)) => self.bar_rect(bar, ppp).contains(cursor),
            _ => true,
        }
    }

    /// Capture now, or open the overlay to choose what. Asking again while it is open
    /// closes it: the way out if the overlay did not get the keyboard for Esc.
    fn begin_capture(&mut self, mode: CaptureMode) {
        self.overlay_records = false;
        if self.overlay.take().is_some() || self.overlay_request.is_some() {
            return;
        }
        if self.recording.is_some() {
            self.capture_while_recording(mode);
            return;
        }
        match mode {
            CaptureMode::Screen if !self.pick_screen() => {
                self.capture_still(mode, self.screen_target());
            }
            _ => {
                let request = self.capture.request_still(CaptureTarget::Desktop);
                self.overlay_request = Some((request, mode));
            }
        }
    }

    fn open_overlay(&mut self, mode: CaptureMode, desktop: Frame) {
        let displays = self.screen.displays().unwrap_or_default();
        let origin =
            cb_core::desktop_bounds(&displays).map_or((0, 0), |bounds| (bounds.x, bounds.y));
        let windows = self.screen.windows().unwrap_or_else(|e| {
            tracing::warn!(error = %e, "window list unavailable");
            Vec::new()
        });
        self.overlay = Some(Overlay::new(
            mode,
            desktop,
            origin,
            windows,
            displays,
            self.last_region,
            self.overlay_records,
        ));
        self.overlay_dressed = false;
    }

    fn show_overlay(&mut self, ctx: &egui::Context) {
        let Some(overlay) = &mut self.overlay else {
            return;
        };
        let mode = overlay.mode();
        let result = overlay.show(ctx, self.overlay_dressed);
        // Once its window exists it loses the frame Windows 11 draws round borderless
        // windows, before it shows.
        self.overlay_dressed = self.overlay_dressed || dress_window(&tr!("overlay-title"), false);
        match result {
            None => {}
            Some(OverlayResult::Display(id)) => {
                let bounds = self.displays.iter().find(|d| d.id == id).map(|d| d.bounds);
                let frame = bounds.and_then(|rect| overlay.crop(rect));
                self.overlay = None;
                if self.overlay_records {
                    self.start_recording(mode, CaptureTarget::Display(id));
                } else if let Some(frame) = frame {
                    // What the overlay showed, as for a region.
                    let item = self.add_capture(mode);
                    self.store(item, frame);
                } else {
                    self.capture_still(mode, CaptureTarget::Display(id));
                }
            }
            Some(OverlayResult::Cancelled) => self.overlay = None,
            Some(OverlayResult::Window(id)) => {
                self.overlay = None;
                if self.overlay_records {
                    self.start_recording(mode, CaptureTarget::Window(id));
                } else {
                    self.capture_still(mode, CaptureTarget::Window(id));
                }
            }
            Some(OverlayResult::Region { rect, record }) => {
                let frame = overlay.crop(rect);
                self.overlay = None;
                self.last_region = Some(rect);
                match frame {
                    _ if record || self.overlay_records => {
                        self.start_recording(mode, CaptureTarget::Region(rect));
                    }
                    Some(frame) => {
                        let item = self.add_capture(mode);
                        self.store(item, frame);
                    }
                    None => self.flash(tr!("toast-region-off-screen")),
                }
            }
        }
    }

    fn handle(&mut self, cmd: Command) {
        match cmd {
            Command::SetMode(mode) => self.bar.mode = mode,
            Command::Capture => self.begin_capture(self.bar.mode),
            Command::CaptureWith(mode) => {
                // While recording it takes a still of the recording; the mode stays.
                if self.recording.is_none() {
                    self.bar.mode = mode;
                }
                self.begin_capture(mode);
            }
            Command::ToggleRecording => self.toggle_recording(),
            Command::TogglePause => self.toggle_pause(),
            Command::DiscardRecording => self.discard_recording(),
            Command::ToggleBuffer => self.bar.tray_open = !self.bar.tray_open,
            Command::ToggleBar if self.taskbar != Taskbar::Off => {
                // Minimized: the shortcut brings the bar back, as its taskbar button does.
                self.repaint
                    .send_viewport_cmd(egui::ViewportCommand::Minimized(false));
                self.hidden = false;
                self.focus_bar = true;
            }
            Command::ToggleBar => {
                self.hidden = !self.hidden;
                // Shown from a shortcut, the bar takes the keyboard, so Tab reaches its
                // controls (NFR-13).
                self.focus_bar = !self.hidden;
            }
            Command::RepeatLastRegion => match self.last_region {
                Some(rect) if self.overlay.is_none() => {
                    self.capture_still(CaptureMode::Region, CaptureTarget::Region(rect));
                    // While recording, the still comes from the video: no highlight in it.
                    if self.prefs.repeat_highlight && self.recording.is_none() {
                        self.highlight_after = self.buffer.newest().map(|i| (i.id, rect));
                    }
                }
                _ => self.begin_capture(CaptureMode::Region),
            },
            Command::CopyNewest => self.copy_newest(),
            Command::OpenSettings => {
                self.hidden = false;
                if self.settings_window.is_none() {
                    self.autostart = cb_platform::autostart::enabled();
                }
                self.find_fonts();
                self.settings_window
                    .get_or_insert_with(SettingsWindow::default)
                    .focus();
            }
            Command::EditPrefix => {
                // From a shortcut, the bar may be hidden and another app has the keyboard.
                self.hidden = false;
                self.collapsed = false;
                self.bar.tray_open = true;
                self.focus_prefix = true;
            }
            Command::ToggleDemo => self.toggle_demo(),
            Command::Mark(kind) => self.mark(kind),
            Command::Cancel => {}
        }
    }

    /// Open the demo session setup, or stop the session being recorded (US-10.1).
    fn toggle_demo(&mut self) {
        if !self.prefs.plus() {
            return self.flash(tr!("toast-demo-needs-plus"));
        }
        match &self.demo {
            Some(d) if d.item.is_some() || self.countdown.is_some() => {
                return self.toggle_recording();
            }
            // Waiting for the overlay: Esc there cancels.
            Some(_) => return,
            None => {}
        }
        if self.recording.is_some() || self.countdown.is_some() {
            return self.flash(tr!("toast-demo-busy"));
        }
        self.hidden = false;
        let (mode, mic) = (self.bar.mode, self.bar.mic_on);
        self.demo_setup
            .get_or_insert_with(|| cb_ui::DemoSetup::new(mode, mic))
            .focus();
    }

    /// Put a marker down in the demo session being recorded, on its clock (US-10.3).
    fn mark(&mut self, kind: cb_core::MarkerKind) {
        let (Some(recording), Some(demo)) = (&self.recording, &mut self.demo) else {
            return;
        };
        if demo.item != Some(recording.item) {
            return;
        }
        let now = Instant::now();
        if let Some(t) = recording.handle.media_time(now) {
            demo.mark(kind, t.as_millis() as u64, SystemTime::now());
            self.marker = Some((kind, now));
        }
    }

    /// Draw the blue outline of a repeated capture while it lasts.
    fn show_highlight(&mut self, ctx: &egui::Context) {
        let Some(highlight) = &mut self.highlight else {
            return;
        };
        // Its clock starts once the window shows: the moment it takes to place it is
        // not lost from the effect.
        if !highlight.shown {
            highlight.since = Instant::now();
        }
        let Some(level) = highlight.level(Instant::now()) else {
            self.highlight = None;
            return;
        };
        let region = highlight.region;
        let center = (
            region.x + region.width as i32 / 2,
            region.y + region.height as i32 / 2,
        );
        let scale = cb_core::display_at(center, &self.displays)
            .map_or_else(|| ctx.pixels_per_point(), |d| d.scale_factor);
        cb_ui::show_highlight(
            ctx,
            region,
            scale,
            level,
            highlight.dressed,
            &mut highlight.shown,
        );
        highlight.dressed =
            highlight.dressed || dress_window(&tr!("highlight-title"), !show_in_capture());
        ctx.request_repaint();
    }

    /// Clap when due, logging the clap on the session's clock, and draw the square.
    fn show_clap(&mut self, ctx: &egui::Context) {
        let running = match (&self.recording, &self.demo) {
            (Some(recording), Some(demo)) => demo.item == Some(recording.item),
            _ => false,
        };
        if !running {
            self.clap = None;
        }
        let (Some(clap), Some(recording), Some(demo)) =
            (&mut self.clap, &self.recording, &mut self.demo)
        else {
            return;
        };
        let now = Instant::now();
        let ms = |t: Duration| t.as_millis() as u64;
        for played in clap.played.1.try_iter() {
            if let Some(t) = recording.handle.media_time(played) {
                demo.claps.beeps.push(ms(t));
            }
        }
        if recording.clock.paused() {
            clap.next = now + clap.every;
        } else if now >= clap.next && clap.shown {
            if let Some(t) = recording.handle.media_time(now) {
                demo.claps.claps.push(ms(t));
                let (played, repaint) = (clap.played.0.clone(), self.repaint.clone());
                cb_platform::clap_beep(Box::new(move |at| {
                    if let Some(at) = at {
                        let _ = played.send(at);
                        repaint.request_repaint();
                    }
                }));
                clap.lit_until = now + Duration::from_millis(100);
                clap.next = now + clap.every;
            }
        }
        let lit = now < clap.lit_until;
        let scale = cb_core::display_at(clap.center, &self.displays)
            .map_or_else(|| ctx.pixels_per_point(), |d| d.scale_factor);
        cb_ui::show_clap(ctx, clap.center, scale, lit, clap.dressed, &mut clap.shown);
        // Not kept out of captures: the video must show it.
        clap.dressed = clap.dressed || dress_window(&tr!("clap-title"), false);
        let wake = if lit {
            clap.lit_until - now
        } else {
            clap.next.saturating_duration_since(now)
        };
        ctx.request_repaint_after(wake);
    }

    fn show_demo_setup(&mut self, ctx: &egui::Context) {
        let monitor = self.bar_display();
        let Some(setup) = &mut self.demo_setup else {
            return;
        };
        match setup.show(ctx, monitor.as_ref()) {
            cb_ui::DemoSetupOutput::Open => {}
            cb_ui::DemoSetupOutput::Cancel => self.demo_setup = None,
            cb_ui::DemoSetupOutput::Start => {
                let setup = self.demo_setup.take().expect("checked above");
                if self.recording.is_some() || self.countdown.is_some() {
                    return self.flash(tr!("toast-demo-busy"));
                }
                self.bar.mode = setup.mode;
                self.bar.mic_on = setup.mic;
                self.demo = Some(demo::DemoSession::new(
                    &setup.title,
                    setup.language,
                    &self.captures_dir,
                    SystemTime::now(),
                ));
                self.toggle_recording();
            }
        }
    }

    /// FR-20: the item appears in the buffer immediately; the frame arrives later.
    /// A still of what is being recorded, from the recording's own frames (US-5.5): no
    /// second capture session, no overlay over the video, no frame lost.
    fn capture_while_recording(&mut self, mode: CaptureMode) {
        let item = self.add_capture(mode);
        let Some(recording) = &self.recording else {
            return;
        };
        // On the session's clock, as asked: the still is the frame of this moment.
        if let Some(demo) = self
            .demo
            .as_mut()
            .filter(|d| d.item == Some(recording.item))
        {
            if let Some(t) = recording.handle.media_time(Instant::now()) {
                demo.still_times.insert(item, t.as_millis() as u64);
            }
        }
        let (sender, repaint) = (self.stills.0.clone(), self.repaint.clone());
        recording.handle.still(Box::new(move |result| {
            let _ = sender.send((item, result));
            repaint.request_repaint();
        }));
    }

    fn poll_stills(&mut self) {
        while let Ok((item, result)) = self.stills.1.try_recv() {
            match result {
                Ok(frame) => self.store(item, frame),
                Err(error) => {
                    if let Some(i) = self.buffer.get_mut(item) {
                        i.state = ItemState::Failed(error.to_string());
                        self.flash(tr!("toast-capture-failed", error = error.to_string()));
                    }
                }
            }
        }
    }

    fn capture_still(&mut self, mode: CaptureMode, target: CaptureTarget) {
        if self.recording.is_some() {
            return self.capture_while_recording(mode);
        }
        let item = self.add_capture(mode);
        let req = self.capture.request_still(target);
        self.pending.insert(req, item);
    }

    fn add_capture(&mut self, mode: CaptureMode) -> ItemId {
        let item = self.buffer.push(ItemKind::Image, mode);
        if let Some(i) = self.buffer.get_mut(item) {
            i.state = ItemState::Encoding { progress_pct: 0 };
        }
        let message = tr!("toast-added", count = self.buffer.len());
        self.toast = Some(Toast::new(message, Some(item)));
        item
    }

    fn copy_newest(&mut self) {
        // After a restart the newest capture is only on disk.
        let newest_file = || {
            let newest = self.buffer.newest()?;
            (newest.state == ItemState::Ready).then(|| newest.path.clone())?
        };
        match &self.newest_frame {
            Some((_, frame)) => {
                let _ = self.output.send(output::Job::Copy(frame.clone()));
                self.flash(tr!("toast-copied"));
            }
            None => match newest_file() {
                Some(path) => {
                    let _ = self.output.send(output::Job::CopyFile(path));
                }
                None => self.flash(tr!("toast-nothing-to-copy")),
            },
        }
    }

    /// Take captures past the retention period out of the buffer, hourly. Pinned ones
    /// stay, and every file stays on disk: only the buffer forgets them (FR-25).
    fn sweep(&mut self) {
        if self.swept_at.is_some_and(|at| at.elapsed() < SWEEP_EVERY) {
            return;
        }
        self.swept_at = Some(Instant::now());
        let expired = self
            .buffer
            .expired(SystemTime::now(), self.prefs.retention());
        if !expired.is_empty() {
            tracing::info!(
                count = expired.len(),
                "captures past retention left the buffer"
            );
        }
        for item in expired {
            self.forget(item);
        }
    }

    /// The captures whose file would take another name with the current prefix (FR-30).
    /// Captures still being written keep the name they were given.
    fn to_rename(&self) -> Vec<(ItemId, PathBuf)> {
        self.buffer
            .newest_first()
            .filter(|i| i.state == ItemState::Ready)
            .filter_map(|i| Some((i.id, i.path.clone()?)))
            .filter(|(_, path)| {
                path.file_name()
                    .and_then(|n| output::renamed(&n.to_string_lossy(), &self.bar.prefix))
                    .is_some()
            })
            .collect()
    }

    /// Send the buffer to the index writer if it changed since the last time.
    fn save_index(&mut self) {
        if self.buffer.generation() != self.indexed {
            self.indexed = self.buffer.generation();
            let _ = self.index.send(self.buffer.snapshot());
        }
    }

    /// Drop a capture from the buffer and memory. Returns its file, if it got to disk.
    fn forget(&mut self, item: ItemId) -> Option<PathBuf> {
        self.thumbnails.remove(&item);
        self.previews.remove(&item);
        self.selection.remove(item);
        if self
            .newest_frame
            .as_ref()
            .is_some_and(|(id, _)| *id == item)
        {
            self.newest_frame = None;
        }
        let removed = self.buffer.remove(item)?;
        if let Some(original) = removed.original {
            let _ = self.output.send(output::Job::Remove(original));
        }
        removed.path
    }

    /// Take a capture back: out of the buffer, and off the disk if it got there.
    fn undo(&mut self, item: ItemId) {
        self.toast = None;
        if let Some(path) = self.forget(item) {
            if let Err(e) = std::fs::remove_file(&path) {
                tracing::warn!(error = %e, ?path, "undone capture stays on disk");
            }
        }
    }

    /// Delete from the tray: unlike Undo, the file goes to the Recycle Bin, as it may be
    /// an older capture the user still wants back.
    fn delete(&mut self, item: ItemId) {
        match self.forget(item) {
            Some(path) => {
                let _ = self.output.send(output::Job::Trash(vec![path]));
            }
            // Not written yet: `poll_output` removes the file when it lands.
            None => self.flash(tr!("toast-deleted")),
        }
    }

    /// Run what the tray asked of `item` (FR-23).
    fn item_action(
        &mut self,
        ctx: &egui::Context,
        frame: &eframe::Frame,
        item: ItemId,
        action: ItemAction,
    ) {
        let Some(found) = self.buffer.get(item) else {
            return;
        };
        let path = found.path.clone();
        match (action, path) {
            (ItemAction::Select(how), _) => match how {
                SelectHow::Only => self.selection.click(item),
                SelectHow::Toggle => self.selection.toggle(item),
                SelectHow::Range => {
                    let order: Vec<_> = self.buffer.newest_first().map(|i| i.id).collect();
                    self.selection.extend_to(item, &order);
                }
            },
            (ItemAction::Delete, _) => self.delete(item),
            (ItemAction::TogglePin, _) => {
                if let Some(found) = self.buffer.get_mut(item) {
                    found.pinned = !found.pinned;
                }
            }
            (ItemAction::Annotate, _) => self.annotate(item),
            (ItemAction::Peek, _) => self.peek(ctx, item),
            // A recording goes on the clipboard as a file, to paste in a folder or a chat.
            (ItemAction::Copy, Some(path)) if matches!(found.kind, ItemKind::Video { .. }) => {
                let _ = self.output.send(output::Job::CopyFiles(vec![path]));
            }
            (ItemAction::Copy, Some(path)) => {
                let _ = self.output.send(output::Job::CopyFile(path));
            }
            (ItemAction::Play, Some(path)) => {
                let _ = self.output.send(output::Job::Play(path));
            }
            (ItemAction::ExportGif, Some(from)) => {
                if self.exports.insert(item, None).is_none() {
                    let _ = self.output.send(output::Job::ExportGif { item, from });
                }
            }
            (ItemAction::Save, Some(path)) => self.save_copy(frame, path),
            (ItemAction::DragOut, Some(path)) => {
                // Blocks this thread until the drop, as every OS drag loop does.
                if let Err(e) = cb_platform::drag_file_out(&path) {
                    tracing::warn!(error = %e, ?path, "drag out");
                    self.flash(tr!("toast-drag-failed", error = e.to_string()));
                }
                self.release_pointer = true;
            }
            // The tray only offers these once the file is on disk.
            (
                ItemAction::Copy
                | ItemAction::Save
                | ItemAction::DragOut
                | ItemAction::Play
                | ItemAction::ExportGif,
                None,
            ) => {}
        }
    }

    /// Run a batch action on the selection, or on everything when nothing is selected (FR-24).
    fn batch_action(&mut self, frame: &eframe::Frame, action: BatchAction) {
        let targets = self.buffer.targets(&self.selection);
        // Copy and save need the file; a capture still being written is left out.
        let files: Vec<PathBuf> = targets
            .iter()
            .filter_map(|&id| self.buffer.get(id))
            .filter(|i| i.state == ItemState::Ready)
            .filter_map(|i| i.path.clone())
            .collect();
        match action {
            BatchAction::Deselect => self.selection.clear(),
            // The tray offers Delete for a selection only.
            BatchAction::Delete if self.selection.is_empty() => {}
            BatchAction::Delete => {
                let count = targets.len();
                let paths: Vec<PathBuf> = targets
                    .into_iter()
                    .filter_map(|id| self.forget(id))
                    .collect();
                self.selection.clear();
                if paths.is_empty() {
                    self.flash(tr!("toast-deleted-count", count = count));
                } else {
                    let _ = self.output.send(output::Job::Trash(paths));
                }
            }
            BatchAction::Clear => {
                let everything = self.selection.is_empty();
                let mut cleared = 0;
                for id in targets {
                    // Clearing everything keeps pinned captures (FR-25); a selection is explicit.
                    if everything && self.buffer.get(id).is_some_and(|i| i.pinned) {
                        continue;
                    }
                    self.forget(id);
                    cleared += 1;
                }
                self.selection.clear();
                self.flash(tr!("toast-cleared", count = cleared));
            }
            _ if files.is_empty() => self.flash(tr!("toast-nothing-saved")),
            BatchAction::CopyAll => {
                // One image pastes as an image anywhere; several go as files.
                let job = match <[PathBuf; 1]>::try_from(files) {
                    Ok([file]) => output::Job::CopyFile(file),
                    Err(files) => output::Job::CopyFiles(files),
                };
                let _ = self.output.send(job);
            }
            BatchAction::SaveAll => self.save_all(frame, files),
            BatchAction::ExportPdf => {
                let images: Vec<PathBuf> = targets
                    .iter()
                    .filter_map(|&id| self.buffer.get(id))
                    .filter(|i| i.state == ItemState::Ready && i.kind == ItemKind::Image)
                    .filter_map(|i| i.path.clone())
                    .collect();
                // Oldest first: pages read in the order the captures were taken.
                self.export_pdf(frame, images.into_iter().rev().collect());
            }
        }
    }

    /// Ask for a folder on the dialog's own thread; the output worker copies `files` there.
    fn save_all(&mut self, frame: &eframe::Frame, files: Vec<PathBuf>) {
        let dialog = rfd::FileDialog::new()
            .set_title(tr!("dialog-save-all", count = files.len()))
            .set_directory(&self.captures_dir)
            .set_parent(frame);
        let output = self.output.clone();
        std::thread::Builder::new()
            .name("save all dialog".into())
            .spawn(move || {
                if let Some(dir) = dialog.pick_folder() {
                    let _ = output.send(output::Job::SaveAllTo { files, dir });
                }
            })
            .expect("spawn save all dialog thread");
    }

    /// Ask where to write the PDF, on the dialog's own thread; the output worker writes it.
    fn export_pdf(&mut self, frame: &eframe::Frame, files: Vec<PathBuf>) {
        if files.is_empty() {
            return self.flash(tr!("toast-nothing-to-export"));
        }
        let name = output::file_name(&self.bar.prefix, SystemTime::now());
        let dialog = rfd::FileDialog::new()
            .set_title(tr!("dialog-export-pdf", count = files.len()))
            .add_filter(tr!("dialog-pdf-filter"), &["pdf"])
            .set_directory(&self.captures_dir)
            .set_file_name(Path::new(&name).with_extension("pdf").to_string_lossy())
            .set_parent(frame);
        let output = self.output.clone();
        std::thread::Builder::new()
            .name("export pdf dialog".into())
            .spawn(move || {
                if let Some(to) = dialog.save_file() {
                    let _ = output.send(output::Job::ExportPdf { files, to });
                }
            })
            .expect("spawn export pdf dialog thread");
    }

    /// Ask where to save a copy of `from`, on the dialog's own thread; the output worker copies.
    fn save_copy(&mut self, frame: &eframe::Frame, from: PathBuf) {
        let mut dialog = rfd::FileDialog::new()
            .set_title(tr!("dialog-save-copy"))
            .add_filter(tr!("dialog-png-filter"), &["png"])
            .set_parent(frame);
        if let Some(name) = from.file_name() {
            dialog = dialog.set_file_name(name.to_string_lossy());
        }
        let output = self.output.clone();
        std::thread::Builder::new()
            .name("save dialog".into())
            .spawn(move || {
                if let Some(to) = dialog.save_file() {
                    let _ = output.send(output::Job::SaveAs { from, to });
                }
            })
            .expect("spawn save dialog thread");
    }

    /// Open the editor on a capture (US-7.1): at once when its image is in memory, or once
    /// the output worker has read its file.
    fn annotate(&mut self, item: ItemId) {
        if let Some(editor) = &mut self.editor {
            editor.focus();
            if editor.item() != item {
                self.flash(tr!("toast-editor-busy"));
            }
            return;
        }
        let Some(found) = self.buffer.get(item) else {
            return;
        };
        if found.kind != ItemKind::Image {
            return self.flash(tr!("toast-annotate-images-only"));
        }
        // The editor works on the capture as taken, with its annotations as layers.
        let ready_file = (found.state == ItemState::Ready)
            .then(|| found.original.clone().or_else(|| found.path.clone()))
            .flatten();
        let in_memory = self
            .newest_frame
            .as_ref()
            .filter(|(id, _)| *id == item && found.original.is_none())
            .map(|(_, frame)| frame.clone());
        match (in_memory, ready_file) {
            (Some(frame), _) => self.open_editor(item, frame),
            (None, Some(path)) => {
                self.editor_loading = Some(item);
                let _ = self.output.send(output::Job::Load { item, path });
            }
            (None, None) => self.flash(tr!("toast-not-ready")),
        }
    }

    fn open_editor(&mut self, item: ItemId, base: Arc<Frame>) {
        let annotations = self
            .buffer
            .get(item)
            .map(|i| i.annotations.clone())
            .unwrap_or_default();
        let title = self.editor_title(item);
        self.editor = Some(Editor::new(item, base, title, annotations));
    }

    fn editor_title(&self, item: ItemId) -> String {
        let name = self
            .buffer
            .get(item)
            .and_then(|i| i.path.as_ref()?.file_name())
            .map(|n| n.to_string_lossy().into_owned());
        match name {
            Some(name) => tr!("editor-title-file", name = name),
            None => tr!("editor-title"),
        }
    }

    /// The settings as they are now, wherever the app keeps each one.
    fn settings(&self) -> Settings {
        Settings {
            folder: self.captures_dir.clone(),
            prefix: self.bar.prefix.clone(),
            microphone: self.bar.mic_on,
            ..self.prefs.clone()
        }
    }

    /// Put `settings` in effect at once (US-8.2).
    fn apply_settings(&mut self, settings: Settings) {
        if settings.retention_days != self.prefs.retention_days {
            self.swept_at = None;
        }
        if settings.language != self.prefs.language {
            cb_ui::i18n::set_language(settings.language);
            if let Some(tray) = &self._tray {
                tray.set_labels(tray_labels().each_ref().map(String::as_str));
            }
        }
        let rebind =
            settings.shortcuts != self.prefs.shortcuts || settings.plus() != self.prefs.plus();
        self.captures_dir = settings.folder.clone();
        self.bar.prefix = output::clean_prefix(&settings.prefix);
        self.bar.mic_on = settings.microphone;
        cb_ui::set_tooltips(settings.tooltips);
        cb_ui::set_editor_palette(settings.editor_palette());
        if settings.editor_font != self.prefs.editor_font {
            let (tx, rx) = mpsc::channel();
            let (path, repaint) = (settings.editor_font.clone(), self.repaint.clone());
            let _ = std::thread::Builder::new()
                .name("editor font".into())
                .spawn(move || {
                    let font = read_font(&path);
                    let _ = tx.send((path, font));
                    repaint.request_repaint();
                });
            self.editor_font_rx = Some(rx);
        }
        self.prefs = settings;
        if rebind {
            cb_ui::set_shortcuts(&self.prefs);
            self.rebind_shortcuts();
        }
    }

    /// Register the shortcuts as set now, unless Settings is recording new keys (US-3.2).
    fn rebind_shortcuts(&mut self) {
        let Some(hotkeys) = &mut self.hotkeys else {
            return;
        };
        if self.shortcuts_suspended {
            hotkeys.suspend();
        } else {
            self.shortcut_conflicts = hotkeys.apply(&self.prefs, self.bar.demo);
        }
    }

    /// Take in the font list and the editor font once their threads are done.
    fn receive_fonts(&mut self, ctx: &egui::Context) {
        if let Some(list) = self.font_list_rx.as_ref().and_then(|rx| rx.try_recv().ok()) {
            self.font_list = Some(list);
            self.font_list_rx = None;
        }
        let Some((path, font)) = self
            .editor_font_rx
            .as_ref()
            .and_then(|rx| rx.try_recv().ok())
        else {
            return;
        };
        self.editor_font_rx = None;
        // A later pick wins: one read for an older one is dropped.
        if path != self.prefs.editor_font {
            return;
        }
        self.annotation_font = annotation_font(font.as_deref(), self.system_fonts.as_ref());
        cb_ui::install_fonts_with(ctx, self.system_fonts.clone(), font);
    }

    /// List the installed fonts for Settings, on a thread: it reads every font file.
    fn find_fonts(&mut self) {
        if self.font_list.is_some() || self.font_list_rx.is_some() {
            return;
        }
        let (tx, rx) = mpsc::channel();
        let repaint = self.repaint.clone();
        let _ = std::thread::Builder::new()
            .name("font list".into())
            .spawn(move || {
                let mut list: Vec<(String, String)> = cb_platform::font_files()
                    .into_iter()
                    .filter_map(|path| {
                        let name = cb_annotate::font_name(&std::fs::read(&path).ok()?)?;
                        Some((name, path.to_string_lossy().into_owned()))
                    })
                    .collect();
                list.sort_by_key(|(name, _)| name.to_lowercase());
                list.dedup_by(|a, b| a.0 == b.0);
                let _ = tx.send(list);
                repaint.request_repaint();
            });
        self.font_list_rx = Some(rx);
    }

    fn show_settings(&mut self, ctx: &egui::Context) {
        self.receive_fonts(ctx);
        let monitor = self.bar_display();
        let before = self.settings();
        let mut settings = before.clone();
        let Some(window) = &mut self.settings_window else {
            return;
        };
        let mut autostart = self.autostart;
        let out = window.show(
            ctx,
            monitor.as_ref(),
            &mut settings,
            &self.settings_file,
            &mut autostart,
            &self.shortcut_conflicts,
            self.font_list.as_deref(),
        );
        if out.recording_keys != self.shortcuts_suspended {
            self.shortcuts_suspended = out.recording_keys;
            if settings == before {
                self.rebind_shortcuts();
            }
        }
        if settings != before {
            self.apply_settings(settings);
        }
        if autostart != self.autostart {
            self.set_autostart(ctx, autostart);
        }
        self.folder_requested |= out.choose_folder;
        if out.closed {
            self.settings_window = None;
            if self.shortcuts_suspended {
                self.shortcuts_suspended = false;
                self.rebind_shortcuts();
            }
        }
    }

    /// Register or unregister the bar to start at login, off the UI thread (NFR-4).
    fn set_autostart(&mut self, ctx: &egui::Context, on: bool) {
        if self.autostart_done.is_some() {
            return;
        }
        self.autostart = on;
        let (tx, rx) = mpsc::channel();
        let ctx = ctx.clone();
        std::thread::Builder::new()
            .name("autostart".into())
            .spawn(move || {
                let _ = tx.send(cb_platform::autostart::set_enabled(on));
                ctx.request_repaint();
            })
            .expect("spawn autostart thread");
        self.autostart_done = Some(rx);
    }

    fn poll_autostart(&mut self) {
        let Some(rx) = &self.autostart_done else {
            return;
        };
        match rx.try_recv() {
            Err(mpsc::TryRecvError::Empty) => return,
            Ok(Ok(())) => {}
            Ok(Err(error)) => {
                tracing::warn!(%error, "start at login not changed");
                self.flash(tr!("toast-autostart-failed", error = error.to_string()));
            }
            Err(mpsc::TryRecvError::Disconnected) => {}
        }
        self.autostart_done = None;
        self.autostart = cb_platform::autostart::enabled();
    }

    /// Send the settings to be written when they changed, from any window.
    fn save_settings(&mut self) {
        let now = self.settings();
        if now != self.saved_settings {
            let _ = self.settings_writer.send(now.clone());
            self.saved_settings = now;
        }
    }

    /// Open Quick Look on a capture, or show it in the one already open.
    fn peek(&mut self, ctx: &egui::Context, item: ItemId) {
        let Some(found) = self.buffer.get(item) else {
            return;
        };
        let video = matches!(found.kind, ItemKind::Video { .. });
        let name = found
            .path
            .as_deref()
            .and_then(Path::file_name)
            .map_or_else(|| tr!("editor-title"), |n| n.to_string_lossy().into_owned());
        match &mut self.peek {
            Some(peek) => peek.show_item(item, name, video),
            None => self.peek = Some(Peek::new(item, name, video)),
        }
        self.peek_loading = None;
        self.peek_video = None;
        if video {
            // Still being saved: its first frame stands in.
            if let (ItemState::Ready, Some(path)) = (&found.state, &found.path) {
                let ctx = ctx.clone();
                self.peek_video = Some(peek_video::PeekVideo::start(path, move || {
                    ctx.request_repaint()
                }));
            }
            return;
        }
        // As shown in the tray: with its annotations drawn in.
        let in_memory = self
            .newest_frame
            .as_ref()
            .filter(|(id, _)| *id == item)
            .map(|(_, frame)| frame.clone());
        let ready_file = (found.state == ItemState::Ready)
            .then(|| found.path.clone())
            .flatten();
        match (in_memory, ready_file) {
            (Some(frame), _) => {
                if let Some(peek) = &mut self.peek {
                    peek.set_frame(item, frame);
                }
            }
            (None, Some(path)) => {
                self.peek_loading = Some(item);
                let _ = self.output.send(output::Job::Load { item, path });
            }
            // Still being saved: the thumbnail stands in.
            (None, None) => {}
        }
    }

    fn show_peek(&mut self, ctx: &egui::Context) {
        let Some(peek) = &mut self.peek else {
            return;
        };
        let images: Vec<ItemId> = self.buffer.newest_first().map(|i| i.id).collect();
        let Some(index) = images.iter().position(|&id| id == peek.item()) else {
            // Deleted or undone meanwhile.
            self.peek = None;
            self.peek_video = None;
            return;
        };
        if let Some(video) = &mut self.peek_video {
            let wake = ctx.clone();
            match video.poll(move || wake.request_repaint()) {
                Some(Some(frame)) => peek.set_frame(peek.item(), Arc::new(frame)),
                Some(None) => self.peek_video = None,
                None => {}
            }
            if let Some(video) = &self.peek_video {
                ctx.request_repaint_after(video.wait());
            }
        }
        let thumbnail = self.thumbnails.get(&peek.item());
        let item = peek.item();
        let monitor = self
            .bar_pos
            .and_then(|at| cb_core::display_at(at, &self.displays));
        match peek.show(ctx, monitor, thumbnail, (index + 1, images.len())) {
            Some(PeekAction::Close) => self.close_peek(),
            Some(PeekAction::Previous) if index > 0 => self.peek(ctx, images[index - 1]),
            Some(PeekAction::Next) if index + 1 < images.len() => self.peek(ctx, images[index + 1]),
            Some(PeekAction::Annotate) => {
                self.close_peek();
                self.annotate(item);
            }
            Some(PeekAction::Play) => {
                self.close_peek();
                if let Some(path) = self.buffer.get(item).and_then(|i| i.path.clone()) {
                    let _ = self.output.send(output::Job::Play(path));
                }
            }
            _ => {}
        }
    }

    fn close_peek(&mut self) {
        self.peek = None;
        self.peek_loading = None;
        self.peek_video = None;
    }

    fn show_editor(&mut self, ctx: &egui::Context) {
        let monitor = self.bar_display();
        let Some(editor) = &mut self.editor else {
            return;
        };
        let Some(result) = editor.show(ctx, monitor.as_ref()) else {
            return;
        };
        let editor = self.editor.take().expect("just shown");
        if let EditorResult::Save(shapes) = result {
            self.save_annotation(editor.item(), editor.base().clone(), shapes);
        }
    }

    /// Draw the annotations into the capture's own file, on the output worker: the
    /// buffer item stays the same, with the new picture (US-7.1).
    fn save_annotation(&mut self, item: ItemId, base: Arc<Frame>, shapes: Vec<cb_annotate::Shape>) {
        let Some(found) = self.buffer.get(item) else {
            return self.flash(tr!("toast-annotate-removed"));
        };
        match (&found.state, found.path.clone()) {
            (ItemState::Ready, Some(path)) => {
                let job = output::Job::Annotate {
                    item,
                    base,
                    shapes,
                    font: self.annotation_font.clone(),
                    path,
                    original: found.original.clone(),
                    originals: self.originals_dir.clone(),
                };
                let _ = self.output.send(job);
            }
            (ItemState::Failed(_), _) => {
                self.flash(tr!("toast-annotate-unsaved"));
            }
            _ => self.annotation_waiting = Some((item, base, shapes)),
        }
    }

    /// A grabbed frame goes to the worker, unless the capture was undone meanwhile.
    fn store(&mut self, item: ItemId, frame: Frame) {
        if self.buffer.get(item).is_none() {
            return;
        }
        // Stills of a demo session go into its folder (US-10.1, DR-3).
        let recording = self.recording.is_some();
        let path = match self.demo.as_mut().filter(|d| recording && d.item.is_some()) {
            Some(demo) => demo.place_still(item, frame.captured_at),
            None => self
                .captures_dir
                .join(output::file_name(&self.bar.prefix, frame.captured_at)),
        };
        let frame = Arc::new(frame);
        self.newest_frame = Some((item, frame.clone()));
        let _ = self.output.send(output::Job::Store { item, frame, path });
    }

    fn poll_output(&mut self, ctx: &egui::Context) {
        while let Ok(done) = self.output_done.try_recv() {
            match done {
                output::Done::Thumbnail { item, thumbnail } => {
                    // Undone while it was being made: nothing would ever remove it.
                    if self.buffer.get(item).is_some() {
                        let image = egui::ColorImage::from_rgba_unmultiplied(
                            [thumbnail.width as usize, thumbnail.height as usize],
                            &thumbnail.rgba,
                        );
                        let name = format!("thumbnail {}", item.0);
                        let texture = ctx.load_texture(name, image, egui::TextureOptions::LINEAR);
                        self.thumbnails.insert(item, texture);
                    }
                }
                output::Done::Preview { item, frames } => {
                    // Gone while it was decoded: nothing would ever remove it.
                    if self.buffer.get(item).is_none() {
                        continue;
                    }
                    let textures: Vec<_> = frames
                        .iter()
                        .enumerate()
                        .map(|(n, frame)| {
                            let image = egui::ColorImage::from_rgba_unmultiplied(
                                [frame.width as usize, frame.height as usize],
                                &frame.rgba,
                            );
                            let name = format!("preview {} {n}", item.0);
                            ctx.load_texture(name, image, egui::TextureOptions::LINEAR)
                        })
                        .collect();
                    if let Some(first) = textures.first() {
                        self.thumbnails.insert(item, first.clone());
                    }
                    self.previews.insert(item, textures);
                }
                output::Done::Message(message) => self.flash(message),
                output::Done::Exporting { item, percent } => match percent {
                    Some(_) => {
                        self.exports.insert(item, percent);
                    }
                    None => {
                        self.exports.remove(&item);
                    }
                },
                output::Done::Loaded { item, result } => {
                    let for_editor = self.editor_loading.take_if(|i| *i == item).is_some();
                    let for_peek = self.peek_loading.take_if(|i| *i == item).is_some();
                    if !for_editor && !for_peek {
                        continue;
                    }
                    match result {
                        Ok(frame) if self.buffer.get(item).is_some() => {
                            let frame = Arc::new(frame);
                            if let (true, Some(peek)) = (for_peek, &mut self.peek) {
                                peek.set_frame(item, frame.clone());
                            }
                            if for_editor {
                                self.open_editor(item, frame);
                            }
                        }
                        Ok(_) => {}
                        Err(error) => {
                            self.flash(tr!("toast-open-failed", error = error.to_string()))
                        }
                    }
                }
                output::Done::Annotated {
                    item,
                    shapes,
                    result,
                } => match result {
                    Ok(output::Annotated { frame, original }) => {
                        if let Some((id, newest)) = &mut self.newest_frame {
                            if *id == item {
                                *newest = frame;
                            }
                        }
                        let reverted = shapes.is_empty();
                        match self.buffer.get_mut(item) {
                            Some(found) => {
                                found.original = original;
                                found.annotations = shapes;
                            }
                            // Removed while saving: its original has no one left to come back to.
                            None => {
                                if let Some(original) = original {
                                    let _ = self.output.send(output::Job::Remove(original));
                                }
                            }
                        }
                        self.flash(if reverted {
                            tr!("toast-reverted")
                        } else {
                            tr!("toast-annotations-saved")
                        });
                    }
                    Err(error) => {
                        self.flash(tr!("toast-annotate-failed", error = error.to_string()))
                    }
                },
                output::Done::Renamed { item, path } => {
                    if let Some(found) = self.buffer.get_mut(item) {
                        found.path = Some(path);
                    }
                }
                output::Done::ClipboardFailed(error) => {
                    tracing::warn!(%error, "clipboard");
                    self.flash(tr!("toast-saved-not-copied"));
                }
                output::Done::Stored { item, result } => {
                    let Some(i) = self.buffer.get_mut(item) else {
                        // Undone while it was being written.
                        if let Ok(path) = result {
                            let _ = std::fs::remove_file(&path);
                        }
                        continue;
                    };
                    match result {
                        Ok(path) => {
                            i.state = ItemState::Ready;
                            i.path = Some(path);
                        }
                        Err(error) => {
                            i.state = ItemState::Failed(error.clone());
                            self.flash(tr!("toast-save-failed", error = error.to_string()));
                        }
                    }
                    if let Some((item, base, shapes)) =
                        self.annotation_waiting.take_if(|(id, ..)| *id == item)
                    {
                        self.save_annotation(item, base, shapes);
                    }
                }
            }
        }
    }

    /// Ask for a folder without blocking the bar: the dialog runs on its own thread. The
    /// bar is its parent, or Windows opens it behind whatever app was last in front.
    fn choose_folder(&mut self, ctx: &egui::Context, frame: &eframe::Frame) {
        if self.folder_choice.is_some() {
            return;
        }
        let (tx, rx) = mpsc::channel();
        let dialog = rfd::FileDialog::new()
            .set_title(tr!("dialog-folder"))
            .set_directory(&self.captures_dir)
            .set_parent(frame);
        let ctx = ctx.clone();
        std::thread::Builder::new()
            .name("folder dialog".into())
            .spawn(move || {
                let choice = dialog.pick_folder();
                let _ = tx.send(choice);
                ctx.request_repaint();
            })
            .expect("spawn folder dialog thread");
        self.folder_choice = Some(rx);
    }

    fn poll_folder_choice(&mut self) {
        let Some(rx) = &self.folder_choice else {
            return;
        };
        match rx.try_recv() {
            Ok(choice) => {
                tracing::debug!(?choice, "folder dialog closed");
                self.folder_choice = None;
                if let Some(dir) = choice {
                    self.captures_dir = dir;
                }
            }
            Err(mpsc::TryRecvError::Empty) => {}
            Err(mpsc::TryRecvError::Disconnected) => self.folder_choice = None,
        }
    }

    /// Minimize to the taskbar when asked, and take the taskbar button away again once
    /// the bar is back, however it came back: the button, the shortcut or the tray icon.
    fn follow_taskbar(&mut self, ctx: &egui::Context, frame: &eframe::Frame) {
        let minimized = ctx.input(|i| i.viewport().minimized).unwrap_or(false);
        let before = self.taskbar;
        self.taskbar = match self.taskbar {
            Taskbar::Asked => match cb_platform::show_in_taskbar(frame, true) {
                Ok(()) => {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
                    Taskbar::In
                }
                Err(e) => {
                    tracing::warn!(error = %e, "no taskbar button: collapsing instead");
                    self.collapsed = true;
                    Taskbar::Off
                }
            },
            Taskbar::In if !minimized => {
                if let Err(e) = cb_platform::show_in_taskbar(frame, false) {
                    tracing::warn!(error = %e, "taskbar button stays");
                }
                Taskbar::Off
            }
            state => state,
        };
        if self.taskbar != before {
            tracing::debug!(state = ?self.taskbar, minimized, "taskbar");
        }
    }

    /// Stop the recording, or start one in the current mode (FR-14). Window and Region
    /// open the overlay first, to pick what to record.
    fn toggle_recording(&mut self) {
        // Pressed again during the countdown: no recording after all.
        if self.countdown.take().is_some() {
            return;
        }
        if let Some(recording) = self.recording.take() {
            self.bar.recording = false;
            self.bar.paused = false;
            // Stopping returns at once; `poll_recordings` gets the file.
            recording.handle.stop();
            return;
        }
        match self.bar.mode {
            CaptureMode::Screen if !self.pick_screen() => {
                self.start_recording(CaptureMode::Screen, self.screen_target());
            }
            mode => {
                self.begin_capture(mode);
                self.overlay_records = self.overlay_request.is_some();
            }
        }
    }

    /// Pause or resume the recording (FR-15); nothing to do without one.
    fn toggle_pause(&mut self) {
        let Some(recording) = &mut self.recording else {
            return;
        };
        let now = Instant::now();
        if recording.clock.paused() {
            recording.clock.resume(now);
            recording.handle.resume();
        } else {
            recording.clock.pause(now);
            recording.handle.pause();
        }
        self.bar.paused = recording.clock.paused();
    }

    /// Stop the recording and drop it from the buffer (FR-19): `poll_recordings` sends
    /// the finished file to the Recycle Bin, as for a card deleted while recording.
    fn discard_recording(&mut self) {
        let Some(recording) = self.recording.take() else {
            return;
        };
        self.bar.recording = false;
        self.bar.paused = false;
        self.buffer.remove(recording.item);
        recording.handle.stop();
        self.flash(tr!("toast-record-discarded"));
    }

    /// Record `target`, after the countdown when there is one.
    fn start_recording(&mut self, mode: CaptureMode, target: CaptureTarget) {
        self.overlay_records = false;
        if self.recording.is_some() || self.countdown.is_some() {
            return;
        }
        let total = Duration::from_secs(u64::from(self.prefs.countdown_secs));
        let center = self
            .target_center(&target)
            .or_else(cb_platform::cursor_position);
        match center {
            Some(center) if !total.is_zero() => {
                self.countdown = Some(Countdown {
                    mode,
                    target,
                    center,
                    ends: Instant::now() + total,
                    total,
                    dressed: false,
                    shown: false,
                });
            }
            _ => self.record_now(mode, target),
        }
    }

    /// The centre of what `target` covers on screen, in global physical pixels.
    fn target_center(&self, target: &CaptureTarget) -> Option<(i32, i32)> {
        let rect = match target {
            CaptureTarget::Region(rect) => *rect,
            CaptureTarget::Display(id) => self.displays.iter().find(|d| d.id == *id)?.bounds,
            CaptureTarget::Window(id) => {
                let windows = self.screen.windows().ok()?;
                windows.into_iter().find(|w| w.id == *id)?.bounds
            }
            CaptureTarget::Desktop => return None,
        };
        Some((
            rect.x + (rect.width / 2) as i32,
            rect.y + (rect.height / 2) as i32,
        ))
    }

    /// Follow the cam toggle: start or stop the webcam, and show its bubble while on.
    fn show_camera(&mut self, ctx: &egui::Context) {
        let wanted = self.bar.cam_on;
        if wanted != self.camera.on {
            cb_platform::set_camera(wanted);
            if !wanted {
                cb_platform::set_camera_bubble(None);
                self.camera = Camera::default();
                return;
            }
            self.camera.on = true;
        }
        if !wanted {
            return;
        }
        let state = cb_platform::camera_state();
        if let cb_platform::CameraState::Failed(error) = state {
            self.bar.cam_on = false;
            self.flash(tr!("toast-camera-failed", error = error));
            return;
        }
        // No bigger than the bubble shows it, whatever the camera gives.
        let high = self.bubble_rect.map_or(360, |r| r.height);
        if let Some((seq, frame)) = cb_platform::camera_frame(self.camera.seen, high) {
            self.camera.bubble.set_picture(ctx, &frame);
            self.camera.seen = Some(seq);
        }
        // Where it last was, if that is still on a screen.
        let saved = self.bubble_rect.filter(|r| {
            let centre = (r.x + r.width as i32 / 2, r.y + r.height as i32 / 2);
            cb_core::display_at(centre, &self.displays).is_some()
        });
        let shape = self.prefs.bubble_shape;
        let start = *self.camera.start.get_or_insert_with(|| {
            if let Some(saved) = saved {
                return saved;
            }
            // Else the bottom-right corner of the screen under the cursor, clear of its
            // edges, at that screen's scale.
            let display = cb_platform::cursor_position()
                .and_then(|cursor| cb_core::display_at(cursor, &self.displays))
                .or(self.displays.first());
            display.map_or(
                cb_core::Rect {
                    x: 100,
                    y: 100,
                    width: 180,
                    height: 180,
                },
                |d| {
                    let size = (cb_ui::BUBBLE_SIZE * d.scale_factor).round() as u32;
                    let width = (size as f32 * shape.aspect()).round() as u32;
                    let area = d.work_area;
                    let margin = size / 6;
                    cb_core::Rect {
                        x: area.x + (area.width - width - margin) as i32,
                        y: area.y + (area.height - size - margin) as i32,
                        width,
                        height: size,
                    }
                },
            )
        });
        let rect = self
            .camera
            .bubble
            .show(ctx, start, self.camera.dressed, shape);
        if rect.is_some() {
            self.bubble_rect = rect;
        }
        // Recordings draw the camera only once there is a picture to draw.
        let running = state == cb_platform::CameraState::Running;
        cb_platform::set_camera_bubble(rect.filter(|_| running).map(|r| (r, shape)));
        if !self.camera.dressed && dress_window(&tr!("bubble-title"), true) {
            self.camera.dressed = true;
            // Dragged across monitors of different scales, a resize could bounce it
            // back and forth at the border.
            if let Some(window) = cb_platform::own_window(&tr!("bubble-title")) {
                let _ = cb_platform::keep_pixel_size(&window);
            }
        }
    }

    /// Draw the countdown, or start the recording once it is over. The countdown window
    /// is not drawn in the frame the recording starts, so it is gone from the video.
    fn show_countdown(&mut self, ctx: &egui::Context) {
        let Some(countdown) = &mut self.countdown else {
            return;
        };
        let scale = cb_core::display_at(countdown.center, &self.displays)
            .map_or_else(|| ctx.pixels_per_point(), |d| d.scale_factor);
        let left = countdown.ends.saturating_duration_since(Instant::now());
        if left.is_zero() {
            let Countdown { mode, target, .. } = self.countdown.take().expect("checked above");
            self.record_now(mode, target);
        } else {
            cb_ui::show_countdown(
                ctx,
                countdown.center,
                scale,
                left.as_secs_f32(),
                countdown.total.as_secs_f32(),
                countdown.dressed,
                &mut countdown.shown,
            );
            countdown.dressed = countdown.dressed || dress_window(&tr!("countdown-title"), true);
        }
    }

    /// The recording shows in the buffer at once, as "saving" until its file is complete.
    fn record_now(&mut self, mode: CaptureMode, target: CaptureTarget) {
        let demo = self.demo.as_ref().is_some_and(|d| d.item.is_none());
        let path = match &self.demo {
            Some(d) if demo => d.screen_path(),
            _ => {
                let name = output::file_name(&self.bar.prefix, SystemTime::now());
                self.captures_dir.join(name).with_extension("mp4")
            }
        };
        let item = self.buffer.push(ItemKind::Video { duration_ms: 0 }, mode);
        let clap_center = self
            .target_center(&target)
            .or_else(cb_platform::cursor_position);
        let (sender, repaint) = (self.recorded.0.clone(), self.repaint.clone());
        let done = Box::new(move |result| {
            let _ = sender.send((item, result));
            repaint.request_repaint();
        });
        match self.screen.start_recording(
            &target,
            self.prefs.frame_rate,
            AudioSources {
                microphone: self.bar.mic_on,
                system: self.prefs.system_audio,
            },
            &path,
            done,
        ) {
            Ok(handle) => {
                self.recording = Some(ActiveRecording {
                    handle,
                    item,
                    clock: Stopwatch::start(Instant::now()),
                });
                self.bar.recording = true;
                if let Some(d) = self.demo.as_mut().filter(|_| demo) {
                    d.item = Some(item);
                    if let Some(every) = clap_every() {
                        tracing::info!(secs = every.as_secs(), "clap test on");
                        self.flash(tr!("toast-clap-on", seconds = every.as_secs()));
                    }
                    self.clap = clap_every().map(|every| Clap {
                        every,
                        next: Instant::now() + every,
                        lit_until: Instant::now(),
                        center: clap_center.unwrap_or((0, 0)),
                        dressed: false,
                        shown: false,
                        played: mpsc::channel(),
                    });
                }
            }
            Err(error) => {
                if demo {
                    self.demo = None;
                }
                self.buffer.remove(item);
                self.flash(tr!("toast-record-failed", error = error.to_string()));
            }
        }
    }

    fn poll_recordings(&mut self) {
        while let Ok((item, result)) = self.recorded.1.try_recv() {
            let demo = self.demo.take_if(|d| d.item == Some(item));
            // A recording that failed while running has nothing left to stop.
            if self.recording.as_ref().is_some_and(|r| r.item == item) {
                self.recording = None;
                self.bar.recording = false;
                self.bar.paused = false;
            }
            let Some(found) = self.buffer.get_mut(item) else {
                // Deleted while it was recording.
                if let Ok(recording) = result {
                    let _ = self.output.send(output::Job::Trash(vec![recording.path]));
                }
                continue;
            };
            match result {
                Ok(recording) => {
                    if recording.dropped > 0 {
                        tracing::warn!(
                            dropped = recording.dropped,
                            frames = recording.frames,
                            "recording skipped frames"
                        );
                    }
                    let duration_ms = recording.duration.as_millis() as u64;
                    found.kind = ItemKind::Video { duration_ms };
                    found.path = Some(recording.path.clone());
                    found.state = ItemState::Ready;
                    if let Some(demo) = &demo {
                        let _ = self.output.send(output::Job::Write {
                            path: demo.dir.join("manifest.json"),
                            bytes: demo.manifest(&recording),
                        });
                        let _ = self.output.send(output::Job::Write {
                            path: demo.dir.join("markers.json"),
                            bytes: demo.markers_json(),
                        });
                        if !demo.claps.claps.is_empty() {
                            // Kept first, so the report can be made again if this one is
                            // cut short: `takebox clap-report <folder>`.
                            let _ = self.output.send(output::Job::Write {
                                path: demo.dir.join(demo::CLAP_LOG),
                                bytes: demo.claps.to_json(),
                            });
                            let _ = self.output.send(output::Job::ClapReport(demo.dir.clone()));
                        }
                    }
                    let _ = self.output.send(output::Job::Preview {
                        item,
                        path: recording.path,
                    });
                    match &demo {
                        Some(demo) if !demo.claps.claps.is_empty() => {
                            self.flash(tr!("toast-clap-measuring"))
                        }
                        Some(demo) => self.flash(tr!(
                            "toast-demo-saved",
                            folder = demo.dir.display().to_string()
                        )),
                        None => {
                            self.flash(tr!("toast-recorded", seconds = duration_ms.div_ceil(1000)))
                        }
                    }
                }
                Err(error) => {
                    tracing::warn!(%error, "recording");
                    found.state = ItemState::Failed(error.to_string());
                    self.flash(tr!("toast-record-failed", error = error.to_string()));
                }
            }
        }
    }

    fn poll_captures(&mut self) {
        let events: Vec<_> = self.capture.poll().collect();
        for ev in events {
            match ev {
                CaptureEvent::Still { request_id, frame } => {
                    if let Some((_, mode)) = self
                        .overlay_request
                        .take_if(|(request, _)| *request == request_id)
                    {
                        self.open_overlay(mode, frame);
                    } else if let Some(item) = self.pending.remove(&request_id) {
                        self.store(item, frame);
                        if let Some((_, region)) =
                            self.highlight_after.take_if(|(after, _)| *after == item)
                        {
                            self.highlight = Some(Highlight {
                                region,
                                since: Instant::now(),
                                dressed: false,
                                shown: false,
                            });
                        }
                    }
                }
                CaptureEvent::Failed { request_id, error } => {
                    if self
                        .overlay_request
                        .take_if(|(request, _)| *request == request_id)
                        .is_some()
                    {
                        self.flash(tr!("toast-capture-failed", error = error.to_string()));
                    } else if let Some(item) = self.pending.remove(&request_id) {
                        if let Some(i) = self.buffer.get_mut(item) {
                            i.state = ItemState::Failed(error.to_string());
                            self.flash(tr!("toast-capture-failed", error = error.to_string()));
                        }
                    }
                }
            }
        }
    }

    fn draw(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        // Frozen during a drag: changing the band would move the bar under the pointer.
        if !self.dragging {
            // The window grows for tooltips only when they are shown.
            self.band = self.bar_hovered && !self.bar.tray_open && self.prefs.tooltips;
        }
        if self
            .toast
            .as_ref()
            .is_some_and(|t| t.shown_at.elapsed() >= TOAST_TIME)
        {
            self.toast = None;
        }
        let toast_on = self.toast.is_some();
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
        if out.closes_tray() {
            self.bar.tray_open = false;
        }
        if out.toggle_mic {
            self.bar.mic_on = !self.bar.mic_on;
        }
        if out.toggle_cam {
            self.bar.cam_on = !self.bar.cam_on;
        }
        if out.collapse {
            // Windows minimizes to the taskbar; elsewhere the bar shrinks to its pill (M2.3).
            if cfg!(windows) {
                self.taskbar = Taskbar::Asked;
            } else {
                self.collapsed = true;
            }
            self.bar.tray_open = false;
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
            let focus = std::mem::take(&mut self.focus_prefix);
            if focus {
                ui.ctx().send_viewport_cmd(egui::ViewportCommand::Focus);
            }
            let next = output::file_name(&self.bar.prefix, SystemTime::now());
            let to_rename = self.to_rename().len();
            let destination = cb_ui::Destination {
                prefix: &mut self.bar.prefix,
                folder: &self.captures_dir,
                next_file: &next,
                focus,
                rename_offer: (self.rename_offer && to_rename > 0).then_some(to_rename),
            };
            let width = if beside || self.collapsed {
                TRAY_WIDTH_BESIDE_BAR
            } else {
                self.bar_size.x
            };
            let (buffer, selection) = (&self.buffer, &self.selection);
            let (thumbnails, previews, exports) = (&self.thumbnails, &self.previews, &self.exports);
            let mut asked = cb_ui::TrayOutput::default();
            self.tray_size = content_size(ui, |ui| {
                asked = cb_ui::tray(
                    ui,
                    buffer,
                    selection,
                    thumbnails,
                    previews,
                    exports,
                    destination,
                    width,
                );
            });
            self.bar.prefix = output::clean_prefix(&self.bar.prefix);
            self.item_action = self.item_action.or(asked.item);
            self.batch_action = self.batch_action.or(asked.batch);
            self.folder_requested |= asked.choose_folder;
            if asked.prefix_done && self.bar.prefix != self.named_prefix {
                self.named_prefix = self.bar.prefix.clone();
                self.rename_offer = true;
            }
            match asked.rename {
                Some(true) => {
                    let files = self.to_rename();
                    let prefix = self.bar.prefix.clone();
                    let _ = self.output.send(output::Job::Rename { files, prefix });
                    self.rename_offer = false;
                }
                Some(false) => self.rename_offer = false,
                None => {}
            }
        }
        if let Some(toast) = &self.toast {
            let capture = toast.capture;
            let asked = cb_ui::toast(ui, &toast.message, capture.is_some());
            match (asked, capture) {
                (Some(ToastAction::Undo), Some(item)) => self.undo(item),
                (Some(ToastAction::Annotate), Some(item)) => {
                    self.toast = None;
                    self.annotate(item);
                }
                _ => {}
            }
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
                let mut extra = 0.0;
                if self.bar.tray_open {
                    extra += self.tray_size.y + 6.0;
                }
                if extra == 0.0 {
                    extra = cb_ui::TOOLTIP_ROOM;
                }
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

    /// The monitor the bar is on, where its windows open.
    fn bar_display(&self) -> Option<cb_core::DisplayInfo> {
        let at = self.bar_pos?;
        cb_core::display_at(at, &self.displays).cloned()
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
        let actual = ctx.input(|i| i.viewport().inner_rect.map(|r| r.size()));
        if needs_resize(size, self.window_size, actual, self.dragging) {
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
        eframe::set_value(storage, MODE_STORAGE_KEY, &self.bar.mode);
        eframe::set_value(storage, REGION_STORAGE_KEY, &self.last_region);
        eframe::set_value(storage, BUBBLE_STORAGE_KEY, &self.bubble_rect);
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

    fn raw_input_hook(&mut self, ctx: &egui::Context, raw_input: &mut egui::RawInput) {
        if std::mem::take(&mut self.release_pointer) {
            let pos = ctx.input(|i| i.pointer.latest_pos()).unwrap_or_default();
            raw_input.events.insert(
                0,
                egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: egui::Modifiers::NONE,
                },
            );
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        let ctx = &ui.ctx().clone();
        while let Ok(cmd) = self.commands.try_recv() {
            self.handle(cmd);
        }
        // A demo session whose recording never started: its overlay or countdown was
        // cancelled.
        let waiting = self.recording.is_some()
            || self.countdown.is_some()
            || self.overlay.is_some()
            || self.overlay_request.is_some();
        if !waiting && self.demo.as_ref().is_some_and(|d| d.item.is_none()) {
            self.demo = None;
        }
        self.poll_captures();
        self.poll_recordings();
        self.poll_stills();
        self.poll_output(ctx);
        self.poll_folder_choice();
        self.poll_autostart();
        self.follow_taskbar(ctx, frame);
        self.sweep();
        self.track_position(ctx);

        if let Some(recording) = &self.recording {
            self.bar.elapsed_secs = recording.clock.elapsed(Instant::now()).as_secs();
        }
        self.bar.mic_level = self.recording.as_ref().and_then(|r| r.handle.mic_level());

        if std::mem::take(&mut self.focus_bar) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        }
        if self.click_through != self.hidden {
            self.click_through = self.hidden;
            ctx.send_viewport_cmd(egui::ViewportCommand::MousePassthrough(self.hidden));
        }
        let launched_at = *self.launched_at.get_or_insert_with(Instant::now);
        let flashing = launched_at.elapsed() < ATTENTION_TIME;
        self.bar.attention = attention_level(launched_at.elapsed());
        if flashing {
            ctx.request_repaint();
        }
        // Typing the prefix counts as being there, wherever the cursor went.
        let cursor_in = self.dragging
            || ctx.input(|i| i.pointer.hover_pos().is_some())
            || ctx.memory(|m| m.focused().is_some())
            || self.peek.is_some();
        let cursor_near = flashing || self.bar_hovered || cursor_in;
        if tray_idle(
            self.bar.tray_open,
            cursor_in,
            &mut self.tray_seen,
            Instant::now(),
        ) {
            self.bar.tray_open = false;
        }
        self.bar.has_last_region = self.last_region.is_some();
        self.bar.plus = self.prefs.plus();
        let demo = self.demo.as_ref().is_some_and(|d| d.item.is_some());
        if demo != self.bar.demo {
            // The marker shortcuts are only registered while a session records.
            self.bar.demo = demo;
            self.rebind_shortcuts();
        }
        self.marker = self
            .marker
            .filter(|(_, at)| demo && at.elapsed() < MARKER_SHOWN);
        self.bar.marker = self.marker.map(|(kind, _)| kind);
        if let Some((_, at)) = self.marker {
            ctx.request_repaint_after(MARKER_SHOWN.saturating_sub(at.elapsed()));
        }
        let (shown, fade) =
            animated_opacity(ctx, self.hidden, cursor_near || !self.prefs.fade_when_away);
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
        self.show_camera(ctx);
        self.show_countdown(ctx);
        self.show_overlay(ctx);
        self.show_peek(ctx);
        self.show_editor(ctx);
        self.show_settings(ctx);
        self.show_demo_setup(ctx);
        self.show_clap(ctx);
        self.show_highlight(ctx);
        if std::mem::take(&mut self.folder_requested) {
            self.choose_folder(ctx, frame);
        }
        if let Some((item, action)) = self.item_action.take() {
            self.item_action(ctx, frame, item, action);
        }
        if let Some(action) = self.batch_action.take() {
            self.batch_action(frame, action);
        }
        self.save_index();
        self.save_settings();

        // Keep the timer and toast fresh without spinning the CPU (NFR-7). A drag polls
        // faster, to show the snap preview and catch the drop.
        // The mic meter moves at 20 fps.
        let wake = if self.dragging {
            16
        } else if self.bar.cam_on {
            // The bubble follows the camera at 30 fps.
            33
        } else if self.bar.mic_level.is_some() {
            50
        } else {
            250
        };
        ctx.request_repaint_after(Duration::from_millis(wake));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// US-5.2: the timer counts recorded time only, so pauses do not add to it.
    #[test]
    fn the_recording_timer_stands_still_while_paused() {
        let t0 = Instant::now();
        let s = Duration::from_secs;
        let mut clock = Stopwatch::start(t0);
        assert_eq!(clock.elapsed(t0 + s(10)), s(10));
        clock.pause(t0 + s(10));
        assert!(clock.paused());
        assert_eq!(clock.elapsed(t0 + s(25)), s(10));
        clock.resume(t0 + s(25));
        clock.resume(t0 + s(26)); // a second resume changes nothing
        assert_eq!(clock.elapsed(t0 + s(30)), s(15));
    }

    #[test]
    fn the_tray_folds_after_the_cursor_has_been_away_a_while() {
        let start = Instant::now();
        let mut seen = start;
        let tick = Duration::from_millis(100);
        let mut now = start + TRAY_IDLE - tick;
        assert!(!tray_idle(true, false, &mut seen, now));
        now += tick;
        assert!(tray_idle(true, false, &mut seen, now));
        now += tick;
        assert!(
            !tray_idle(true, true, &mut seen, now),
            "the cursor came back"
        );
        let back = now;
        now = back + TRAY_IDLE - tick;
        assert!(
            !tray_idle(true, false, &mut seen, now),
            "the wait started again"
        );
        now = back + TRAY_IDLE;
        assert!(tray_idle(true, false, &mut seen, now));
        now += TRAY_IDLE * 10;
        assert!(
            !tray_idle(false, false, &mut seen, now),
            "closed: the wait resets"
        );
        now += tick;
        assert!(
            !tray_idle(true, false, &mut seen, now),
            "so a reopened tray waits again"
        );
    }

    #[derive(Default)]
    struct MemoryStorage(HashMap<String, String>);

    impl eframe::Storage for MemoryStorage {
        fn get_string(&self, key: &str) -> Option<String> {
            self.0.get(key).cloned()
        }
        fn set_string(&mut self, key: &str, value: String) {
            self.0.insert(key.to_owned(), value);
        }
        fn remove_string(&mut self, key: &str) {
            self.0.remove(key);
        }
        fn flush(&mut self) {}
    }

    #[test]
    fn attention_pulses_smoothly_and_then_stops() {
        let at = |millis| attention_level(Duration::from_millis(millis));
        assert!(at(0) < 0.01, "starts from nothing");
        assert!(at(500) > 0.99, "first pulse peaks");
        assert!(at(1000) < 0.01, "and falls before the next");
        assert!(at(2500) > 0.99, "third pulse peaks");
        assert!(at(2999) < 0.01, "ends smoothly");
        assert_eq!(at(3000), 0.0);
        assert_eq!(at(60_000), 0.0);
    }

    #[test]
    fn mode_survives_a_restart() {
        let mut storage = MemoryStorage::default();
        assert_eq!(saved_mode(Some(&storage)), CaptureMode::Region);
        assert_eq!(saved_mode(None), CaptureMode::Region);
        eframe::set_value(&mut storage, MODE_STORAGE_KEY, &CaptureMode::Screen);
        assert_eq!(saved_mode(Some(&storage)), CaptureMode::Screen);
    }

    fn hotkey_event(id: u32, state: HotKeyState) -> GlobalHotKeyEvent {
        GlobalHotKeyEvent { id, state }
    }

    #[test]
    fn a_shortcut_fires_on_press_only() {
        let map = HashMap::from([(7, Command::Capture)]);
        let fire = |id, state| shortcut_command(&map, &hotkey_event(id, state));
        assert_eq!(fire(7, HotKeyState::Pressed), Some(Command::Capture));
        assert_eq!(fire(7, HotKeyState::Released), None);
        assert_eq!(fire(8, HotKeyState::Pressed), None);
    }

    #[test]
    fn the_repeat_highlight_holds_then_fades_out() {
        let since = Instant::now();
        let highlight = Highlight {
            region: Rect {
                x: 0,
                y: 0,
                width: 10,
                height: 10,
            },
            since,
            dressed: true,
            shown: true,
        };
        let at = |ms| highlight.level(since + Duration::from_millis(ms));
        assert_eq!(at(0), Some(1.0));
        assert_eq!(at(300), Some(1.0));
        let fading = at(525).unwrap();
        assert!((fading - 0.5).abs() < 0.01, "{fading}");
        assert_eq!(at(700), None);
    }

    #[test]
    fn a_size_the_os_put_back_is_asked_for_again() {
        let wanted = egui::vec2(1000.0, 230.0);
        let docked = egui::vec2(700.0, 500.0);
        assert!(
            needs_resize(wanted, docked, Some(docked), false),
            "new size"
        );
        assert!(
            needs_resize(wanted, wanted, Some(docked), false),
            "the end of the drag undid it"
        );
        assert!(
            !needs_resize(wanted, wanted, Some(docked), true),
            "mid-drag"
        );
        let rounded = wanted - egui::vec2(0.4, 0.8);
        assert!(!needs_resize(wanted, wanted, Some(rounded), false));
        assert!(!needs_resize(wanted, wanted, None, false));
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
