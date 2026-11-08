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
crates/encode     PNG, GIF (frames → animation) and PDF (images → pages); MP4 and the encode queue next
crates/buffer     the capture buffer; write-ahead index + files next (US-6.1, 6.6)
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
- The collapse button shows the M2.3 pill; Ctrl+Shift+Space hides and shows. Hidden means transparent and click-through, not an OS-hidden window, because eframe stops running hidden windows. The bar fades to 40% when the cursor is away. A tray icon (not on Linux yet) toggles the bar and quits.
- Bar UI follows M2.1 (vector icons in `crates/ui/src/icons.rs`; shortcuts in tooltips only), except the mode selector: icons only, with a sliding highlight (user's choice, not in M2.1). In Region mode, once a region was captured, a repeat button beside Capture takes it again. The window
  follows its content and grows while hovered to give tooltips room. The buffer tray (folds itself after 5 s with the cursor elsewhere) shows a thumbnail card per capture (US-6.3), each with Copy, Save a copy, Annotate (stub until US-7.1) and Delete (to the Recycle Bin); on Windows the picture drags out as a file (US-6.4). Click, Ctrl/Cmd and Shift select cards; Copy all, Save all and Clear act on the selection or on everything (US-6.5).
- Bar position and dock are saved per monitor layout through eframe storage and pulled back on-screen when
  monitors change. The bar docks to screen edges, vertical on left and right. `App::bar_pos` is the source of
  truth; the window follows it, because the tray, toast and tooltip band open on whichever side has room.
- Screenshots work on Windows (Windows.Graphics.Capture): Screen takes the monitor under the cursor; Window and Region open one overlay window across all monitors, over a frozen image; Region opens with the last region selected, with Capture / Record buttons under it (double-click inside also captures). Each capture goes to the buffer, the clipboard and a PNG on a worker thread, with an Undo toast. Files are `<prefix>_<local timestamp>.png`, never overwriting; the prefix and folder (default `~/Captures`) are set from a bar chip or Ctrl+Shift+L (US-6.8). The buffer survives restarts: its index is `~/Captures/.buffer/index.ron`, and unpinned captures leave it after 7 days while their files stay (US-6.6).
- Windows: the bar is excluded from captures and has no OS frame. `start_stream` is a stub. The US-1.2 spike records MP4 via `windows_spike`; results
  in `docs/spikes/us-1.2-windows.md`.
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
