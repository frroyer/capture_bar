# Capture Bar — instructions for Claude Code

Capture Bar is a single, self-contained executable (macOS, Windows, Linux):
one permanent floating bar, global shortcuts, and a non-blocking capture buffer.

- **Spec & backlog (source of truth):** `docs/SPEC.md` — Epics E1–E9, User Stories US-x.y,
  requirements FR-x / NFR-x. Export it from the Claude Doc "Capture Bar — Product
  Specification & Backlog" (Export → Markdown) if the file is missing.
- **Mockup:** Claude Design canvas "Capture Bar" — artboards M1 (live bar + buffer),
  M2.1–M2.6 (bar states), M3 (shortcuts). Stories cite these refs.

## Working rules

1. Work **one User Story at a time**. Name branches `us-<x>-<y>-short-name`, reference the
   story ID in commits and PRs, and tick the story's tasks in `docs/SPEC.md` when done.
2. Meet the story's acceptance criteria (AC) and add tests for them where they can run headless.
3. Before every commit: `cargo fmt --all`, `cargo clippy --workspace --all-targets -- -D warnings`,
   `cargo test --workspace`.
4. Keep the app **autonomous** (NFR-1): no runtime the user must install. New dependencies must be
   MIT / Apache-2.0 / BSD / zlib / ISC (`cargo deny check` enforces it, NFR-15). System libraries
   on Linux are loaded with `dlopen`, never required at link time where avoidable.
5. Never block the UI thread: capture, encode and disk I/O run on worker threads and talk to the
   UI through channels (FR-20, NFR-4).

## Architecture (spec §5)

```
app/              entry point: window, shortcuts, command dispatch
crates/core       Command, CaptureMode, Frame, traits ScreenSource / FrameSink / StreamHandle
crates/ui         egui drawing of the bar (M1/M2.x) and tray; returns commands only
crates/capture    CaptureService: queues grabs on a worker thread
crates/encode     PNG, GIF (frames → animation) and PDF (images → pages); MP4 is written by the platform encoder
crates/buffer     the capture buffer; write-ahead index + files next (US-6.1, 6.6)
crates/annotate   annotation shapes, undo history, flattening into the image (US-7.1)
crates/platform   OS backends behind the core traits: macos.rs / windows.rs / linux.rs
```

Only `crates/platform` may contain OS-specific code. Note: the spec lists `platform-{macos,windows,linux}`
crates; the scaffold starts with one crate and `cfg` modules — split it if a backend grows large.

The UI uses `eframe` with the `glow` (OpenGL) renderer to keep the binary small; switch to `wgpu`
only if a spike shows a need.

## Current state (scaffold)

- Builds and runs: floating, borderless, transparent, always-on-top bar; mode switch;
  Capture / Record buttons; buffer counter and tray; collapse pill; drag by the grip.
- Global shortcuts (Ctrl+Shift / ⌥⇧ map from M3) fire from any app on Windows; macOS and X11 are not run yet, and the Wayland portal backend is not started. If none can be registered, a toast says so.
- The collapse button minimizes the bar to the taskbar on Windows (user's choice over the M2.3 pill, which macOS and Linux keep): the bar gets a taskbar button only while minimized (`cb_platform::show_in_taskbar`). eframe runs no frames while minimized, so shortcuts and the tray icon restore the window from their own thread first (`cb_platform::Restorer`). Ctrl+Shift+Space hides and shows. Hidden means transparent and click-through, not an OS-hidden window, because eframe stops running hidden windows. The bar fades to 40% when the cursor is away. A tray icon (not on Linux yet) toggles the bar and quits.
- Bar UI follows M2.1 (vector icons in `crates/ui/src/icons.rs`; shortcuts in tooltips only), except the mode selector: icons only, with a sliding highlight (user's choice, not in M2.1). In Region mode, once a region was captured, a repeat button beside Capture takes it again. The window
  follows its content and grows while hovered to give tooltips room. The buffer tray (folds itself after 2 s with the cursor elsewhere, or when another bar button is pressed) shows a thumbnail card per capture (US-6.3), each with Copy, Save a copy, Annotate and Delete (to the Recycle Bin); on Windows the picture drags out as a file (US-6.4). Click, Ctrl/Cmd and Shift select cards; Copy all, Save all and Clear act on the selection or on everything (US-6.5).
- Annotate, from the capture toast or a tray card, opens the editor window (US-7.1, `crates/ui/src/editor.rs`): arrows, boxes, freehand pen lines, text, highlights, pixelates and a crop (US-7.2) stay layers with unlimited undo until Save, which draws them into the same file on the output worker. The capture as taken is kept in `.buffer/originals` and the shapes in the buffer item, so a later Annotate reopens them as layers and Original rolls back; the original is deleted when the item leaves the buffer. Text uses the bar's semibold font, so the editor and the file match.
- Settings (US-8.2): the bar's Settings button opens a window for folder, prefix, frame rate, audio sources and buffer retention; changes apply at once and go to `capture-bar.toml`, next to the binary if one is already there (portable), else in the user config dir (`app/src/config.rs`, schema `cb_core::Settings`). The old folder/prefix in eframe storage move over on first run. Frame rate and system sound apply to the next recording; theme is not there yet. Its "Start Capture Bar when I log in" box registers the bar with the OS (US-8.3, `cb_platform::autostart`): the HKCU Run key on Windows, a LaunchAgent on macOS, an XDG autostart entry on Linux (the AppImage path when there is one); the OS entry is the setting, not the TOML file.
- Accessibility and languages (US-8.4): eframe's `accesskit` feature exposes the bar to Narrator / VoiceOver / Orca; every control is labelled, reachable with Tab and drawn with a focus ring (`focus_ring` in `crates/ui/src/lib.rs`), and showing the bar with Ctrl+Shift+Space gives it the keyboard. All user-facing text goes through `tr!("id", var = …)` (`crates/ui/src/i18n.rs`), with English and French in `crates/ui/locales/{en,fr}.ftl`; tests fail if a message is missing from either file or a `tr!` names an unknown id. The language follows the system unless set in Settings, and switches without restart.
- Bar position and dock are saved per monitor layout through eframe storage and pulled back on-screen when
  monitors change. The bar docks to screen edges, vertical on left and right. `App::bar_pos` is the source of
  truth; the window follows it, because the tray, toast and tooltip band open on whichever side has room.
- Screenshots work on Windows (Windows.Graphics.Capture): Screen takes the monitor under the cursor; Window and Region open one overlay window across all monitors, over a frozen image; Region opens with the last region selected, with Capture / Record buttons under it (double-click inside also captures). Each capture goes to the buffer, the clipboard and a PNG on a worker thread, with an Undo toast. Files are `<prefix>_<local timestamp>.png`, never overwriting; the prefix and folder (default `~/Captures`) are set on the tray's title line, which Ctrl+Shift+L opens with the keyboard in the prefix; after a prefix change the tray offers to rename the buffer's captures, keeping their timestamps (US-6.8). The buffer survives restarts: its index is `~/Captures/.buffer/index.ron`, and unpinned captures leave it after 7 days while their files stay (US-6.6).
- Windows: the bar is excluded from captures and has no OS frame.
- Recording (US-5.1) works on Windows: Record / Ctrl+Shift+R records the monitor under the cursor in Screen mode;
  Window and Region open the overlay to pick what to record. A countdown (Settings, default 3 s, 0 = off) shows
  centred on the target first, in its own click-through viewport (`crates/ui/src/countdown.rs`); Record again
  cancels it. egui gives no handle for child viewports, so `cb_platform::own_window` finds it by title to strip
  the Windows 11 frame and exclude it from captures. `ScreenSource::start_recording` returns at once and
  runs on its own thread: capture textures are cropped on the GPU and go straight to the hardware H.264 encoder
  (`windows_record.rs`, `windows_mp4.rs`), into a fragmented MP4 `<prefix>_<timestamp>.mp4` in the captures
  folder. The buffer shows it as saving until the file is complete. A warm-up thread at launch loads the GPU
  device and encoder so the first recording starts in ~240 ms, not ~650. A recording's card shows its first
  frame and plays 12 frames spread over it while hovered (`cb_platform::video_frames`, decoded on the output
  worker); its Play button opens the system player, and Copy puts the file on the clipboard. While recording, the
  bar shows the M2.2 layout (US-5.2): Stop with the timer, Pause / Resume (Ctrl+Shift+P; the timer and the
  video skip the pause) and Discard, which Esc also triggers when the bar has the keyboard; both ask once
  before throwing the file away. Sound (US-5.3): the bar's mic toggle and Settings' system sound pick the sources when a recording starts; WASAPI
  takes the default mic and the default output in loopback at 48 kHz on their own thread (`windows_audio.rs`), and `audio_mix.rs` places each
  packet on the video's timeline by its capture time (re-synced past 20 ms of clock drift), into an AAC track of the same MP4. The recording
  never waits for the devices: the track is silent until they run. Webcam (US-5.4): the cam toggle starts the first camera (`windows_camera.rs`, a
  global behind `cb_platform::set_camera`) and shows a round, mirrored, draggable bubble window (`crates/ui/src/bubble.rs`), kept out of
  captures; the app reports its rect with `set_camera_bubble`, and the recording loop draws the camera there on each frame with a D3D11
  shader (`windows_bubble.rs`). A separate camera file (a setting) is not done; capture-while-recording is not done (US-5.5). The US-1.2 CPU-path spike stays in
  `windows_spike`; results in `docs/spikes/us-1.2-windows.md`.
- macOS and Linux backends are still **stubs**: captures appear in the buffer, then show
  "not implemented on this platform yet". Next step is the rest of **US-1.2** (macOS, Linux spikes).

## Useful commands

```
cargo run -p capture-bar                 # run the bar (debug)
cargo build -p capture-bar --release     # optimised single binary in target/release
RUST_LOG=debug cargo run -p capture-bar  # verbose logs (PowerShell: $env:RUST_LOG="debug")
CB_SHOW_IN_CAPTURE=1 cargo run -p capture-bar  # debug builds only: let screenshots include the bar
cargo test -p cb-platform --test exclude_from_capture -- --ignored  # Windows, needs a desktop
```
