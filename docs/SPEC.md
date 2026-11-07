# Capture Bar — Product Specification & Backlog

Copied from the Claude Doc "Capture Bar — Product Specification & Backlog" (rev 9), Nov 6, 2026.
That doc's Mockup link and the architecture diagram did not carry over. Tick tasks here as they are done.

## 1. Overview

Capture Bar is a single, self-contained executable for macOS, Windows and Linux: one permanent floating bar, driven by global shortcuts, that captures screenshots and recordings into a non-blocking buffer. The user never installs a runtime, a framework or a codec pack.

Mockup: Capture Bar design canvas. Every Epic and User Story below cites the artboard it implements:

| Ref | Artboard | Shows |
| --- | --- | --- |
| M1 | Live bar + buffer (interactive) | Full bar, mode switch, Capture / Record, buffer tray, toast |
| M2.1 | Bar states · Idle | Default bar layout |
| M2.2 | Bar states · Recording | Compact recording bar, pause, capture while recording |
| M2.3 | Bar states · Collapsed pill | Minimised bar |
| M2.4 | Bar states · Region select overlay | Selection, dimensions, key hints |
| M2.5 | Bar states · Docked to a screen edge | Vertical dock, multi-monitor |
| M2.6 | Bar states · Instant confirmation | Post-capture toast with Annotate / Undo |
| M3 | Shortcuts & behaviour | Global shortcut map, buffer rules |

### Goals

- Zero-install: download one file, run it.
- Any capture in one keystroke, and the next capture possible immediately (no save dialog, no blocking encode).
- One bar the user can move anywhere, on any monitor, and that stays out of the way.
- Same behaviour and shortcuts on the three desktop OSes.

### Non-goals (v1)

- Video editing beyond trim; cloud hosting or sharing links; mobile platforms; live streaming.

## 2. Technology choice

Decision: Rust, one codebase compiled to one native binary per OS. The app uses no WebView, runtime, VM or garbage collector, and calls each OS's own capture and encoder APIs directly.

Why Rust wins for this tool:

- Efficiency: native speed with no GC pauses, which matters while copying 60 frames per second. Zero-copy GPU frame paths are possible.
- Portability: first-class targets for macOS (x86_64 + arm64), Windows (x86_64 + arm64) and Linux. Mature bindings exist to every native API we need (objc2, windows-rs, x11rb, pipewire).
- Autonomy: dependencies are compiled in statically, so the output is a single file. System libraries are loaded at runtime only where the OS guarantees them.
- Safety: memory safety in a long-running background process that handles large buffers.

### Stack

| Concern | Choice | Notes |
| --- | --- | --- |
| Windowing | winit | Borderless, transparent, always-on-top, multi-monitor, per-monitor DPI |
| Bar UI rendering | egui on wgpu (fallback softbuffer) | Immediate-mode, small, no WebView |
| Global shortcuts | global-hotkey + XDG GlobalShortcuts portal (Wayland) | Carbon / RegisterHotKey / X11 grab |
| Capture · macOS | ScreenCaptureKit via objc2 | macOS 12.3+; excludes the bar from recordings |
| Capture · Windows | Windows.Graphics.Capture, DXGI Desktop Duplication fallback | windows-rs |
| Capture · Linux | X11: XShm / XComposite; Wayland: xdg-desktop-portal ScreenCast + PipeWire | PipeWire loaded with dlopen |
| Video encode | Hardware first: VideoToolbox / Media Foundation / VA-API; software fallback bundled | Container MP4 (H.264); GIF export |
| Audio | cpal (mic) + ScreenCaptureKit audio / WASAPI loopback / PipeWire monitor (system audio) | |
| Images | png, image crates | Encoded off the UI thread |
| Clipboard, tray | arboard, tray-icon | |
| Config & buffer index | TOML + a small embedded index (redb) | Nothing for the user to install |

### Alternatives considered

| Option | Verdict | Main reason |
| --- | --- | --- |
| Rust (native) | Chosen | Single binary, native APIs, no runtime |
| C++ + Qt | Rejected | Static Qt needs a commercial licence or LGPL relinking. Heavier binary, slower to build safely. |
| Go | Rejected | Every capture API needs cgo, which complicates cross-builds. GC pauses during capture. |
| Tauri / Electron | Rejected | Needs a WebView (webkit2gtk must be installed on Linux) or ships Chromium (~150 MB). |
| C# / .NET NativeAOT | Rejected | Weak native UI on macOS and Linux |
| Zig | Rejected | Ecosystem and bindings too young for three OS capture stacks |

### Deliverables per OS

- Windows: one signed CaptureBar.exe (portable, no installer needed). An optional MSIX/winget package comes later.
- macOS: one signed and notarized universal Capture Bar.app, delivered as a .dmg. macOS only grants Screen Recording permission to a bundled, signed app, so a bare binary is not viable there.
- Linux: one AppImage (x86_64, aarch64), plus a raw binary for advanced users.

## 3. Functional specification

IDs (FR-x) are referenced by the User Stories in section 6.

### 3.1 The bar (M1, M2.1, M2.3, M2.5)

- FR-1 A single floating bar is always on top of other windows, including full-screen apps where the OS allows it. It has no taskbar or Dock entry; a tray/menu-bar icon is kept as a fallback.
- FR-2 The bar is dragged by its grip to any position on any monitor. Its position is remembered per display layout and restored on relaunch.
- FR-3 Dropped within 24 px of a screen edge, the bar snaps and docks there. It turns vertical on the left and right edges (M2.5). Double-clicking the grip docks it to the nearest edge.
- FR-4 The bar has three sizes: full (M2.1), collapsed pill (M2.3) and hidden. ⌥⇧Space / Ctrl+Shift+Space toggles visibility.
- FR-5 The bar fades to 40% opacity when the cursor is away and returns to full opacity on hover. It never takes keyboard focus away from the active app unless clicked.
- FR-6 The bar is excluded from every screenshot and recording.

### 3.2 Capture modes (M1, M2.4)

- FR-7 Three modes: Region, Window, Full screen. The selected mode persists between sessions.
- FR-8 Region overlay (M2.4): crosshair, live dimensions in px, resize handles, click-to-snap to a window. ⇧ locks the aspect ratio, Enter captures, R records the selection, Space reuses the last region, Esc cancels.
- FR-9 Window mode highlights the window under the cursor; one click captures it.
- FR-10 Full screen captures the monitor under the cursor; a setting allows all monitors stitched together.

### 3.3 Screenshots

- FR-11 Capture (⌥⇧S) acts in the current mode. ⌥⇧1/2/3 switch mode and capture in one keystroke.
- FR-12 The image goes to the buffer and to the clipboard. A confirmation toast (M2.6) shows for 2 s with Annotate and Undo.
- FR-13 Captures are full native resolution, HiDPI-aware, saved as lossless PNG (JPEG/WebP optional).

### 3.4 Recording (M1, M2.2)

- FR-14 Record (⌥⇧R) starts and stops recording in the current mode. ⌥⇧P pauses and resumes.
- FR-15 During a recording the bar shows a compact form (M2.2): elapsed time, stop, pause, Capture and the buffer counter.
- FR-16 Screenshots remain available during a recording and go to the buffer.
- FR-17 Mic and webcam overlay toggles are on the bar (M1). System audio capture is a setting.
- FR-18 Output is MP4 (H.264, 30/60 fps, configurable), with a GIF export from the buffer.
- FR-19 Esc during a recording discards it after confirmation.

### 3.5 Buffer (M1, M3)

- FR-20 Every capture is appended to the buffer within 100 ms of the shortcut. The next capture is accepted immediately, with no dialogs or filename prompts.
- FR-21 Recordings encode in a background queue; the thumbnail shows encoding progress (M1).
- FR-22 The buffer tray (⌥⇧B, or the Buffer button) shows thumbnails newest first with type, duration and time.
- FR-23 Per item: copy, save, annotate and delete. Items can be dragged out to other apps as files.
- FR-24 Multi-select allows batch copy, save all to folder, export as PDF (images) and delete. Clear empties the buffer.
- FR-25 The buffer persists on disk (default ~/Captures/.buffer) and survives a crash or restart. Retention is 7 days by default and configurable; pinned items are kept.
- FR-26 The Buffer button shows a live counter.

### 3.6 Annotation (v1.1)

- FR-27 A lightweight editor offers arrow, rectangle, text, highlight, blur/pixelate and crop, and saves back to the same buffer item.

### 3.7 Shortcuts & settings (M3)

- FR-28 The default shortcut map (M3) uses ⌥⇧ on macOS and Ctrl+Shift on Windows and Linux. Every shortcut is remappable, with conflict detection.
- FR-29 Settings: output folder, formats, frame rate, audio sources, retention, launch at login, theme (dark/light).

## 4. Non-functional requirements

The figures below are v1 targets, to be confirmed by benchmark in Epic E1.

| ID | Area | Requirement |
| --- | --- | --- |
| NFR-1 | Autonomy | One file per OS. No runtime, framework, codec pack or admin rights needed to run. |
| NFR-2 | Binary size | ≤ 25 MB per platform build |
| NFR-3 | Startup | Bar visible ≤ 300 ms after launch |
| NFR-4 | Capture latency | Shortcut → item in buffer ≤ 100 ms (screenshot) |
| NFR-5 | Throughput | ≥ 10 screenshots per second sustained with no dropped request |
| NFR-6 | Recording | 1080p60 with ≤ 1% dropped frames on hardware encode; CPU ≤ 15% on a 4-core 2020 laptop |
| NFR-7 | Idle footprint | RAM ≤ 60 MB, CPU ≈ 0% when idle |
| NFR-8 | Platforms | macOS 12.3+ (Intel, Apple silicon); Windows 10 1903+ and 11 (x64, ARM64); Linux X11 and Wayland (GNOME 42+, KDE 5.27+, wlroots), glibc 2.31+ |
| NFR-9 | HiDPI / multi-monitor | Correct at mixed scale factors (100–300%) and for monitors added or removed at runtime |
| NFR-10 | Privacy | Fully offline, no telemetry by default, no network calls except an opt-in update check |
| NFR-11 | Security | Signed binaries (Authenticode, Apple Developer ID + notarization, AppImage GPG); reproducible CI builds |
| NFR-12 | Reliability | No buffer loss on crash (write-ahead index); auto-recovery of a recording interrupted mid-way |
| NFR-13 | Accessibility | Every bar action reachable by keyboard; screen-reader labels; contrast ≥ 4.5:1 |
| NFR-14 | Localisation | UI strings externalised; EN and FR at launch |
| NFR-15 | Licensing | Dependencies MIT, Apache-2.0, BSD or zlib only (no GPL/AGPL linked into the binary) |

## 5. Architecture

The UI sends commands, never OS calls. Every capture flows capture → encode → buffer, so the shortcut returns as soon as the frame is queued (FR-20). Only the bottom layer differs per OS.

| Crate | Responsibility | Key stories |
| --- | --- | --- |
| ui | Bar, overlay, tray, toast, settings (egui + winit) | US-2.x, 4.2, 6.3, 8.2 |
| core | Command bus, state, config, i18n | US-1.1, 3.1 |
| capture | Frame sources, window enumeration, stills tap | US-4.x, 5.5 |
| encode | Job queue, PNG/MP4/GIF/PDF writers, A/V sync | US-5.1, 5.3, 6.2, 6.7 |
| buffer | Write-ahead index, files, retention, recovery | US-6.1, 6.6 |
| platform-* | OS backends implementing the core traits | US-1.2, 9.x |

Threads: a UI thread, a capture thread per active source, an encode worker pool, and a buffer I/O thread, connected by bounded lock-free channels (crossbeam).

## 6. Backlog — Epics, User Stories, Tasks

The backlog has 9 Epics and 34 User Stories. Each story names its mockup artboard (M-ref, section 1) and the requirements it covers (FR/NFR). Story IDs are stable so they can be imported into Jira, Linear or GitHub Issues as they are.

| Epic | Title | Mockup | Covers | Release |
| --- | --- | --- | --- | --- |
| E1 | Cross-platform core & tech spikes | — | NFR-1, 2, 8, 15 | MVP |
| E2 | Floating bar | M1, M2.1, M2.3, M2.5 | FR-1–6 | MVP |
| E3 | Global shortcuts | M3 | FR-11, 14, 28 | MVP |
| E4 | Screenshot capture | M1, M2.4, M2.6 | FR-7–13 | MVP |
| E5 | Screen recording | M1, M2.2 | FR-14–19 | MVP |
| E6 | Capture buffer | M1, M3 | FR-20–26 | MVP |
| E7 | Annotation | M2.6 | FR-27 | v1.1 |
| E8 | Settings, permissions & onboarding | M1 (settings button) | FR-29, NFR-10, 13, 14 | MVP |
| E9 | Packaging, signing & distribution | — | NFR-1, 11 | MVP |

### E1 · Cross-platform core & tech spikes

**US-1.1** As a developer, I want a Rust workspace with per-OS backends behind common traits, so that features are written once.

- AC: cargo build produces a runnable binary on macOS, Windows and Linux in CI. Platform code lives only in platform/* crates.

- [ ] Create workspace: core, ui, capture, encode, buffer, platform-{macos,windows,linux}
- [ ] Define traits ScreenSource, FrameSink, Encoder, HotkeyService, OverlayWindow
- [ ] CI matrix (GitHub Actions): macOS arm64/x64, Windows x64/ARM64, Ubuntu x64/arm64
- [ ] cargo-deny licence gate (NFR-15)

**US-1.2** As a tech lead, I want spikes proving capture and encoding on each OS, so that risks are retired before feature work.

- AC: each spike records 10 s of 1080p60 into MP4 and saves 1 PNG; results are logged against NFR-4/6.

- [ ] Spike ScreenCaptureKit + VideoToolbox (macOS)
- [x] Spike Windows.Graphics.Capture + Media Foundation — results: `docs/spikes/us-1.2-windows.md`
- [ ] Spike PipeWire portal (Wayland) and XShm (X11) + VA-API / software fallback — spike encodes through an ffmpeg pipe (never shipped); in-process encoder decided in US-5.1
- [ ] Spike Wayland bar placement (layer-shell vs. portal limits) and write the decision record — proposed, untested: `docs/spikes/us-1.2-wayland-bar.md`; needs its test matrix run on Linux

### E2 · Floating bar

**US-2.1** As a user, I want a permanent bar that floats over every app, so that capture is always one click away. (M1, M2.1 · FR-1, FR-6)

- AC: the bar stays on top when switching apps and over full-screen windows where the OS allows. It never appears in a capture.

- [ ] Borderless transparent always-on-top window (winit) per OS — Windows done; macOS and Linux not run
- [ ] Exclude the bar from capture (SCK exclusion / SetWindowDisplayAffinity / compositor hint) — Windows done (10 1903–1909 untested); macOS waits for its backend; Linux has no exclusion, see `docs/spikes/us-1.2-wayland-bar.md`
- [x] Render the bar layout of M2.1 in egui: grip, mode switch, Capture, Record, Buffer, mic, cam, settings, collapse
- [x] Hover, pressed and focus states; tooltips with shortcuts — shortcuts appear in tooltips only, not on button labels, to keep the bar narrow

**US-2.2** As a user, I want to drag the bar anywhere, on any monitor, and find it there next time. (M1, M2.5 · FR-2)

- AC: dragging across monitors with different DPI keeps the bar crisp. Position is restored per display layout after relaunch.

- [x] Drag via grip with OS move loop
- [x] Persist position keyed by monitor-set fingerprint — Windows verified; macOS and Linux lack display listing, so they keep one position
- [x] Handle monitor hot-plug: pull the bar back on-screen — Windows verified; needs display listing on macOS and Linux

**US-2.3** As a user, I want the bar to dock vertically on a screen edge. (M2.5 · FR-3)

- AC: a drop within 24 px of an edge snaps it. Left/right edges switch to the vertical layout. A double-click on the grip docks to the nearest edge.

- [x] Snap logic + magnetic preview — Windows verified; the preview is the bar's border turning blue; macOS and Linux need display listing and drop detection
- [x] Vertical layout variant — icons only, built from the M2.5 description without the artboard

**US-2.4** As a user, I want to collapse or hide the bar, so that it never gets in the way. (M2.3 · FR-4, FR-5)

- AC: the collapse button shows the pill of M2.3; ⌥⇧Space hides and shows. The bar fades to 40% when the cursor is away.

- [x] Pill layout — built from the M2.3 description without the artboard; shows the count, and the time while recording
- [x] Show/hide and fade animation (≤ 150 ms) — Windows verified; hidden keeps the window open, transparent and click-through
- [x] Tray/menu-bar icon fallback — Windows verified; macOS untested; Linux has none yet, because the tray-icon crate links GTK (NFR-1)

### E3 · Global shortcuts

**US-3.1** As a user, I want every action on a global shortcut that works from any app. (M3 · FR-11, FR-14, FR-28)

- AC: all 9 shortcuts of M3 fire while another app is focused, on all three OSes and on Wayland via the GlobalShortcuts portal.

- [x] Integrate global-hotkey (macOS / Windows / X11) — Windows verified with another app focused; macOS and X11 not run yet
- [ ] Wayland: XDG GlobalShortcuts portal backend, with a fallback message if the portal is missing — not started, needs a Linux machine; the message exists for when no shortcut can be registered
- [x] Action dispatcher (shortcut → command bus)

**US-3.2** As a user, I want to remap shortcuts and be warned about conflicts. (M3 · FR-28)

- AC: a new binding is applied without restart; a binding already taken by the OS or another app is flagged.

- [ ] Shortcut recorder UI in settings
- [ ] Registration-failure detection → conflict warning

### E4 · Screenshot capture

**US-4.1** As a user, I want to choose Region, Window or Screen once and have it remembered. (M1 · FR-7)

- AC: the selected segment is highlighted as in M1 and persists across launches.

- [x] Mode state in config; segmented control

**US-4.2** As a user, I want a precise region selector with keyboard hints. (M2.4 · FR-8)

- AC: the overlay shows dimensions and handles; Enter, R, Space, Esc and ⇧ behave as in M2.4, across all monitors.

- [ ] Full-screen overlay per monitor with frozen-frame background
- [ ] Handles, aspect lock, window snap
- [ ] Key hints chip row

**US-4.3** As a user, I want to capture a window or a whole screen in one click. (M1 · FR-9, FR-10)

- AC: the hovered window is highlighted; the capture excludes shadows unless enabled.

- [x] Window enumeration + hit-testing per OS — Windows verified; macOS and Linux not started
- [x] Monitor-under-cursor and all-monitors stitch — Windows, one monitor verified; the overlay uses the stitch, and its setting comes with US-8.2

**US-4.4** As a user, I want each screenshot to land in the buffer and clipboard instantly, with a confirmation I can undo. (M2.6 · FR-12, FR-13, NFR-4)

- AC: shortcut → toast ≤ 100 ms; the image is on the clipboard; Undo removes it from the buffer within 2 s.

- [x] Frame grab → lock-free queue → PNG worker — a plain channel to one worker thread; bounded memory is US-6.1
- [x] Clipboard write (arboard) — Windows verified
- [x] Toast component with Undo (M2.6), built without the artboard — Annotate comes with the editor (E7)

### E5 · Screen recording

**US-5.1** As a user, I want to start and stop a recording with one shortcut in my current mode. (M1 · FR-14, FR-18, NFR-6)

- AC: ⌥⇧R starts within 300 ms; stopping produces a valid MP4 in the buffer; 1080p60 meets NFR-6.

- [ ] Capture pipeline: GPU frames → hardware encoder (VideoToolbox / Media Foundation / VA-API)
- [ ] Software H.264 fallback + licence review
- [ ] MP4 muxer, fragmented writes so a crash leaves a playable file (NFR-12)

**US-5.2** As a user, I want a compact recording bar with timer, pause and stop. (M2.2 · FR-15, FR-19)

- AC: the bar switches to the M2.2 layout with a red border; the timer is accurate to 1 s; Esc asks before discarding.

- [ ] Recording layout + timer
- [ ] Pause/resume (⌥⇧P) with timestamp re-basing
- [ ] Discard confirmation

**US-5.3** As a user, I want to record my mic and optionally system audio. (M1 · FR-17)

- AC: the mic toggle reflects state (M1); audio stays in sync within 40 ms over 10 min.

- [ ] Mic capture (cpal)
- [ ] System audio: SCK audio / WASAPI loopback / PipeWire monitor
- [ ] AAC encode + A/V sync

**US-5.4** As a user, I want a webcam bubble in my recording. (M1 · FR-17)

- AC: the cam toggle shows a draggable round overlay that is composited into the video.

- [ ] Camera capture per OS (AVFoundation / Media Foundation / V4L2-PipeWire)
- [ ] GPU compositing of the bubble

**US-5.5** As a user, I want to take screenshots while recording. (M2.2 · FR-16)

- AC: Capture during a recording adds a PNG to the buffer without a frame drop in the video.

- [ ] Tap the frame stream for stills instead of a second capture session

### E6 · Capture buffer

**US-6.1** As a user, I want each capture appended instantly, so that I can fire the next one without waiting. (M1 · FR-20, NFR-4, NFR-5)

- AC: 10 screenshots in 1 s produce 10 items in order; the UI thread never blocks.

- [ ] Bounded in-memory ring + spill-to-disk
- [ ] Write-ahead index (redb) for every item
- [ ] Load test: 10 captures/s for 60 s

**US-6.2** As a user, I want recordings to finish encoding in the background, with visible progress. (M1 · FR-21)

- AC: the thumbnail shows a progress bar (M1); several clips queue and finish in order.

- [ ] Encode job queue with priorities
- [ ] Progress events → UI

**US-6.3** As a user, I want a tray of thumbnails newest first, with a live counter on the bar. (M1 · FR-22, FR-26)

- AC: ⌥⇧B and the Buffer button toggle the tray; the counter matches the item count; the empty state shows the hint text from M1.

- [ ] Tray panel anchored to the bar (above or below, depending on space)
- [ ] Async thumbnail generation + cache

**US-6.4** As a user, I want to copy, save, annotate, delete or drag out any capture. (M1 · FR-23)

- AC: each action works per item; dragging into Slack, a mail client or Finder/Explorer delivers the file.

- [ ] Per-item action row
- [ ] OS drag-and-drop source (NSDraggingSource / OLE IDropSource / XDND + Wayland DnD)

**US-6.5** As a user, I want batch actions on several captures. (M1 · FR-24)

- AC: shift/cmd-click multi-select; Copy all, Save all to folder and Clear act on the selection (or on everything when nothing is selected).

- [ ] Selection model + batch commands

**US-6.6** As a user, I want the buffer to survive restarts and clean itself up. (M3 · FR-25, NFR-12)

- AC: after a kill -9 or reboot, every item is back; items older than the retention period are purged unless pinned.

- [ ] Recovery on launch from the index
- [ ] Retention sweeper + pin flag

**US-6.7** As a user, I want to export clips as GIF and images as a single PDF. (M3 · FR-18, FR-24)

- AC: a GIF of 10 s at 720p is ≤ 15 MB; the PDF keeps native resolution.

- [ ] GIF encoder (palette quantisation, gif crate)
- [ ] PDF writer (printpdf or similar, permissive licence)

### E7 · Annotation (v1.1)

**US-7.1** As a user, I want to annotate a capture with arrows, boxes, text and highlights. (M2.6 · FR-27)

- AC: Annotate from the toast or the tray opens the editor; saving updates the same buffer item; undo/redo is unlimited.

- [ ] Canvas editor window (egui)
- [ ] Vector layer model, flattened on export

**US-7.2** As a user, I want to blur sensitive info and crop. (FR-27)

- AC: the blur is irreversible in the exported PNG.

- [ ] Pixelate/blur tool; crop tool

### E8 · Settings, permissions & onboarding

**US-8.1** As a first-time user, I want to be guided through OS permissions in under a minute. (NFR-10)

- AC: macOS Screen Recording, Microphone, Camera and Accessibility prompts are explained before they appear; Wayland portal consent is explained; a denied permission shows how to fix it.

- [ ] Permission checker per OS
- [ ] 3-step onboarding screen

**US-8.2** As a user, I want one settings panel for output, formats, audio and retention. (M1 settings button · FR-29)

- AC: changes apply without restart and are stored in a TOML file next to the binary (portable) or in the user config dir.

- [ ] Settings window
- [ ] Config schema + migration

**US-8.3** As a user, I want the bar to start at login. (FR-29)

- AC: a toggle registers or unregisters (LaunchAgent / Run key / XDG autostart).

- [ ] Autostart per OS

**US-8.4** As a keyboard or screen-reader user, I want every control reachable and labelled. (NFR-13, NFR-14)

- AC: Tab order covers the whole bar; VoiceOver, Narrator and Orca read the labels; FR and EN strings ship.

- [ ] AccessKit integration (egui)
- [ ] i18n with Fluent

### E9 · Packaging, signing & distribution

**US-9.1** As a Windows user, I want one signed .exe I can run without installing. (NFR-1, NFR-11)

- AC: SmartScreen shows the verified publisher; it runs from a USB stick with no admin rights.

- [ ] Static CRT build; Authenticode signing in CI

**US-9.2** As a macOS user, I want a notarized universal app in a DMG. (NFR-1, NFR-11)

- AC: it opens without a Gatekeeper warning on Intel and Apple silicon.

- [ ] Universal binary (lipo); hardened runtime; notarization in CI

**US-9.3** As a Linux user, I want a single AppImage that runs on X11 and Wayland. (NFR-1, NFR-8)

- AC: it runs on Ubuntu 22.04+, Fedora 39+ and Arch with no extra packages.

- [ ] AppImage build; dlopen of PipeWire, VA-API, Wayland and X11
- [ ] Distro test matrix

**US-9.4** As a user, I want optional in-app updates. (NFR-10)

- AC: off by default; when enabled, it verifies signatures before replacing the binary.

- [ ] Signed update manifest + self-replace

## 7. Risks, open questions, release plan

### Risks

| Risk | Impact | Mitigation |
| --- | --- | --- |
| Wayland forbids apps from placing their own windows and from global key grabs | Bar position and shortcuts may not work on GNOME Wayland | Use wlr-layer-shell where available (KDE, wlroots) and the GlobalShortcuts portal. On GNOME, fall back to a tray icon plus portal shortcuts. Spike in US-1.2. |
| H.264 software encoding carries patent/licence exposure | Legal cost for the fallback path | Hardware encoders first (licensed by the OS/GPU vendor). Fallback options: Cisco's OpenH264 binary downloaded on demand, or AV1/VP9 in WebM. Legal review in US-5.1. |
| macOS permissions require a signed .app | A bare binary cannot record on macOS | Ship a notarized .app in a DMG (US-9.2) |
| Code-signing certificates and Apple Developer account | Blocks distribution | Budget and order them in sprint 1 |
| Multi-GPU / mixed-DPI edge cases | Blurry bar, black frames | Test matrix in CI plus manual device lab |

### Open questions

- [ ] Final product name ("Capture Bar" is a placeholder).
- [ ] Software video fallback: OpenH264 on demand vs. AV1/VP9 WebM?
- [ ] Default retention: 7 days, or a size cap (e.g. 5 GB)?
- [ ] Open source or proprietary licence for the app itself?
- [ ] Is a portable mode (config next to the exe) the default on Windows?

### Release plan

1. Spikes (E1): retire capture, encode and Wayland risks; freeze the architecture.
2. MVP alpha: E2, E3, E4, E6.1–6.4. Screenshots plus the buffer on all three OSes.
3. MVP beta: E5, E6.5–6.7, E8, E9. Recording, exports, onboarding, signed builds.
4. v1.0: hardening against the NFRs, accessibility audit, FR/EN.
5. v1.1: E7 annotation, in-app update (US-9.4).
