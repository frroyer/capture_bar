# Takebox

One permanent, movable bar for screenshots and screen recordings — triggered by a
global shortcut, with a buffer so you can keep capturing without saving each shot first.

Single self-contained executable for **Windows, macOS and Linux**. Written in Rust.

> Status: scaffold. The bar, shortcuts and buffer UI run. Screen and region stills work on Windows;
> the macOS and Linux backends are stubs (next: their US-1.2 spikes). See `docs/SPEC.md` and `CLAUDE.md`.

## Build

Requirements for developers only (end users need nothing):

- Rust (stable) from <https://rustup.rs>
- Windows: Visual Studio Build Tools with "Desktop development with C++"
- macOS: `xcode-select --install`
- Linux (Debian/Ubuntu): `sudo apt install build-essential pkg-config libx11-dev libxkbcommon-dev libwayland-dev libgl1-mesa-dev libxdo-dev`

```
cargo run -p takebox
cargo build -p takebox --release   # -> target/release/takebox(.exe)
```

## Default shortcuts

| macOS | Windows / Linux | Action |
| --- | --- | --- |
| ⌥⇧ Space | Ctrl+Shift+Space | Collapse / expand the bar |
| ⌥⇧ S | Ctrl+Shift+S | Capture with the current mode |
| ⌥⇧ R | Ctrl+Shift+R | Start / stop recording |
| ⌥⇧ P | Ctrl+Shift+P | Pause / resume recording |
| ⌥⇧ 1 / 2 / 3 | Ctrl+Shift+1 / 2 / 3 | Region / Window / Screen + capture |
| ⌥⇧ A | Ctrl+Shift+A | Repeat last region |
| ⌥⇧ B | Ctrl+Shift+B | Open / close the buffer |
| ⌥⇧ C | Ctrl+Shift+C | Copy newest capture |

## Layout

See `CLAUDE.md` → Architecture.

## Licence

MIT OR Apache-2.0 (to be confirmed — see open questions in the spec).
