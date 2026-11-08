//! The tray / menu-bar icon (FR-1): the way back to a hidden bar when its shortcut
//! is taken or unavailable.

use std::sync::Arc;

use cb_core::{CaptureError, CaptureResult};
use tray_icon::menu::{Menu, MenuEvent, MenuItem};
use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

use crate::TrayAction;

pub(crate) const ICON_SIZE: u32 = 32;

/// Dropping this removes the icon.
pub struct Tray {
    _icon: TrayIcon,
    toggle: MenuItem,
    quit: MenuItem,
}

impl Tray {
    /// Rename the menu's two items: show or hide the bar, and quit.
    pub fn set_labels(&self, [toggle, quit]: [&str; 2]) {
        self.toggle.set_text(toggle);
        self.quit.set_text(quit);
    }
}

fn platform_error(e: impl std::fmt::Display) -> CaptureError {
    CaptureError::Platform(e.to_string())
}

/// Must run on the thread of the event loop. `on_action` runs on that thread too.
pub fn create(
    [toggle, quit]: [&str; 2],
    on_action: impl Fn(TrayAction) + Send + Sync + 'static,
) -> CaptureResult<Tray> {
    let toggle = MenuItem::new(toggle, true, None);
    let quit = MenuItem::new(quit, true, None);
    let menu = Menu::new();
    menu.append(&toggle).map_err(platform_error)?;
    menu.append(&quit).map_err(platform_error)?;

    let on_action = Arc::new(on_action);
    let on_click = on_action.clone();
    TrayIconEvent::set_event_handler(Some(move |event| {
        if let TrayIconEvent::Click {
            button: MouseButton::Left,
            button_state: MouseButtonState::Up,
            ..
        } = event
        {
            on_click(TrayAction::ToggleBar);
        }
    }));
    let toggle_id = toggle.id().clone();
    let quit_id = quit.id().clone();
    MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
        if event.id == toggle_id {
            on_action(TrayAction::ToggleBar);
        } else if event.id == quit_id {
            on_action(TrayAction::Quit);
        }
    }));

    let icon = Icon::from_rgba(icon_pixels(), ICON_SIZE, ICON_SIZE).map_err(platform_error)?;
    let icon = TrayIconBuilder::new()
        .with_icon(icon)
        .with_tooltip("Capture Bar")
        .with_menu(Box::new(menu))
        .with_menu_on_left_click(false)
        .build()
        .map_err(platform_error)?;
    Ok(Tray {
        _icon: icon,
        toggle,
        quit,
    })
}

/// A white shutter ring on a blue disc: readable on light and dark taskbars.
/// Drawn in code, so the binary needs no image decoder for it.
pub(crate) fn icon_pixels() -> Vec<u8> {
    let center = (ICON_SIZE as f32 - 1.0) / 2.0;
    let mut pixels = Vec::with_capacity((ICON_SIZE * ICON_SIZE * 4) as usize);
    for y in 0..ICON_SIZE {
        for x in 0..ICON_SIZE {
            let distance = (x as f32 - center).hypot(y as f32 - center);
            let edge = (center + 0.5 - distance).clamp(0.0, 1.0);
            let ring = distance > 5.0 && distance < 9.5;
            let [r, g, b] = if ring {
                [0xFF, 0xFF, 0xFF]
            } else {
                [0x3B, 0x82, 0xF6]
            };
            pixels.extend_from_slice(&[r, g, b, (edge * 255.0) as u8]);
        }
    }
    pixels
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icon_is_a_valid_opaque_disc() {
        let pixels = icon_pixels();
        assert!(Icon::from_rgba(pixels.clone(), ICON_SIZE, ICON_SIZE).is_ok());
        let alpha = |x: u32, y: u32| pixels[((y * ICON_SIZE + x) * 4 + 3) as usize];
        assert_eq!(alpha(0, 0), 0, "corners are transparent");
        assert_eq!(alpha(ICON_SIZE / 2, ICON_SIZE / 2), 255, "centre is opaque");
    }
}
