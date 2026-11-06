# US-1.2 decision record: bar placement on Wayland

**Status: proposed, not tested.** This comes from protocol docs and third-party reports, researched
2026-11-06. No code ran on any compositor, because no Linux machine was available. The spike task
stays open until the test matrix below has been run.

## Decision

1. **Run the bar through XWayland first, on every Wayland desktop.** Force winit's X11 backend and
   keep the X11 always-on-top and positioning code. The app then has one window path on X11 and
   Wayland.
2. **Add a layer-shell surface only if testing shows XWayland fails** on KWin or wlroots-family
   compositors. It is a second window stack, so it needs evidence first.
3. **Use the GlobalShortcuts portal where it exists, X11 grabs on older GNOME, and a message plus
   the tray elsewhere.**
4. **Hide the bar before capturing a monitor on Linux.** Nothing else meets FR-6 there. FR-6 needs a
   product decision for recordings (see Open decisions).

This replaces the mitigation in the spec's risk table ("layer-shell on KDE and wlroots, tray on
GNOME"). Update that row once the matrix confirms it.

## Why

### Placement

| Option | GNOME (Mutter) | KWin | wlroots family, COSMIC | Cost |
| --- | --- | --- | --- | --- |
| Native `xdg_toplevel` | No always-on-top, no positioning | Same | Same | None, but fails FR-1 and FR-2 |
| `wlr-layer-shell` | Not implemented | Yes | Yes | Second window stack |
| XWayland | On top reported, position untested | Untested | Untested, tiling may ignore it | Depends on XWayland; may blur when scaled |

- **Native Wayland cannot do it.** `gdk_window_set_keep_above()` does nothing on Wayland, while
  Firefox's picture-in-picture window stayed on top under XWayland (Mozilla bug 1536747).
- **Layer-shell does not reach GNOME.** Wayland Explorer lists KWin, COSMIC, Hyprland, Sway, Labwc,
  Wayfire, niri and others as supporting it. Mutter, Weston and Cage do not.
- **Layer-shell does not fit our window stack.** eframe sits on winit 0.30.13 here. winit's
  layer-shell request (issue 2582) is still open, with the work in PR 4044. Using it today means
  replacing winit with smithay-client-toolkit on that path.
- **Layer-shell makes FR-2 harder.** Placement uses an anchor and margins. A drag across monitors
  probably needs the surface recreated on the new output; confirm this if option 2 is ever built.
- **XWayland is cheap to try.** `NativeOptions::event_loop_builder` plus winit's `with_x11()` is
  enough; both exist in the locked versions.

### Shortcuts

- **KDE:** the portal works, and the user confirms the bindings in a dialog.
- **GNOME 48+:** the portal is officially supported (GNOME 48 developer notes).
- **GNOME below 48:** no portal. Ubuntu 22.04 and 24.04 LTS are in this range, and NFR-8 promises
  GNOME 42+, so US-3.1's AC ("on Wayland via the portal") cannot hold there. X11 grabs may work on
  these versions, but this is inferred, not reported.
- **GNOME 49+:** Mutter is reported to no longer honour XWayland global grabs, so hotkeys fire only
  when the app has focus. The portal is the only path. This comes from one project's test notes.
- **wlroots:** `xdg-desktop-portal-wlr` has no backend, and on niri `BindShortcuts` is reported to
  fail. Hyprland has its own backend, but reportedly ignores preferred keys.

Portal requirements, from the same single source:

- From portal 1.20, unsandboxed apps must call `org.freedesktop.host.portal.Registry.Register`.
- From portal 1.21, an empty app id is rejected.
- GNOME needs a reverse-DNS app id backed by an installed `.desktop` file, so the AppImage must
  install one (US-9.3).
- A denied consent persists, and later calls fail silently. Onboarding must explain this (US-8.x).

### Capture exclusion (FR-6)

- **The portal offers no exclusion.** `ScreenCast.SelectSources` has no parameter to filter windows
  out of a monitor stream.
- **XWayland does not help.** The bar is still on the composited output.
- **Plain X11 has the same problem.** A root-window XShm grab includes the bar.
- **A protocol may be coming.** `ext_surface_capture_control_v1` turned up in a search, but its
  status could not be confirmed and compositors may ignore it. niri has a per-window block-out rule,
  which is user configuration. Neither is something to build on.

So US-2.1's "compositor hint" task has no implementation on Linux today:

- **Stills:** hide the bar, wait for a composited frame, grab, show. This eats into NFR-4's 100 ms.
- **Recording a monitor:** the bar stays hidden for the whole recording, and stopping goes through a
  shortcut or the tray. Where neither exists, such as wlroots, stopping needs another answer.
- **Window capture:** unaffected.

## Open decisions

1. **FR-6 wording.** It says "every screenshot and recording". On Linux that holds only if the bar
   is hidden during capture. Proposed: "excluded, or hidden while capturing where the OS has no
   exclusion".
2. **NFR-8 and US-3.1.** Either keep GNOME 42+ and accept X11-grab shortcuts below 48, or raise the
   floor, which drops Ubuntu 24.04 LTS. Proposed: keep it, if the matrix confirms the grabs work.

## Test matrix to close the spike

Run each on real hardware, at 100% and fractional scaling:

| Check | GNOME 46 | GNOME 48 | GNOME 49+ | KDE 6 | Sway | Hyprland |
| --- | --- | --- | --- | --- | --- | --- |
| XWayland bar stays on top across app switches | | | | | | |
| Over full-screen windows (FR-1) | | | | | | |
| Drag, and position restored after relaunch (FR-2) | | | | | | |
| Bar takes no focus (FR-5) | | | | | | |
| Bar is crisp when scaled | | | | | | |
| Shortcut fires with another app focused | | | | | | |
| Hide, grab, show time against NFR-4 | | | | | | |

**Also check:** a report that a similar overlay forced through XWayland on GNOME broke resize and
hover (Hover issue 26; only its title was read). The bar's hover fade (FR-5) depends on this.

## Sources

Not confirmed: the `xdg_toplevel` always-on-top proposal (wayland-protocols MR 369), whose page was
blocked, and Mutter's position on layer-shell (Mutter issue 973).

- [wlr-layer-shell, compositor support](https://wayland.app/protocols/wlr-layer-shell-unstable-v1)
- [ScreenCast portal](https://flatpak.github.io/xdg-desktop-portal/docs/doc-org.freedesktop.portal.ScreenCast.html)
- [What's New for Developers in GNOME 48](https://release.gnome.org/48/developers/index.html)
- [Mozilla bug 1536747, picture-in-picture on Wayland](https://bugzilla.mozilla.org/show_bug.cgi?id=1536747)
- [winit issue 2582, layer-shell](https://github.com/rust-windowing/winit/issues/2582)
- [claude-desktop-debian, Wayland shortcut notes](https://github.com/aaddrick/claude-desktop-debian/blob/main/docs/learnings/wayland-global-shortcuts-portal.md) (single source, its author's own tests)
- [Wayland global hotkeys write-up, 2024](https://dec05eba.com/2024/03/29/wayland-global-hotkeys-shortcut-is-mostly-useless/) (KDE and Hyprland behaviour)
- [Hover issue 26](https://github.com/4regab/Hover/issues/26)
