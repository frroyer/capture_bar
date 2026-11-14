//! Global shortcuts (US-3.1) as the settings bind them, re-registered whenever they
//! change, with the ones that could not be had (US-3.2).

use std::collections::{BTreeMap, HashMap};
use std::str::FromStr;
use std::sync::{Arc, Mutex};

use cb_core::{Command, Settings, Shortcut, ShortcutConflict};
use global_hotkey::hotkey::HotKey;
use global_hotkey::GlobalHotKeyManager;

/// Which command each registered hotkey id fires; shared with the event handler.
pub type Fired = Arc<Mutex<HashMap<u32, Command>>>;

/// The shortcuts registered with the OS now.
pub struct Hotkeys {
    manager: GlobalHotKeyManager,
    registered: Vec<HotKey>,
    fired: Fired,
}

impl Hotkeys {
    pub fn new() -> global_hotkey::Result<Self> {
        Ok(Self {
            manager: GlobalHotKeyManager::new()?,
            registered: Vec::new(),
            fired: Fired::default(),
        })
    }

    pub fn fired(&self) -> Fired {
        self.fired.clone()
    }

    /// Register the bindings of `settings` in place of the current ones, the markers only
    /// while a demo `session` records. Returns the shortcuts not in effect, and why.
    pub fn apply(
        &mut self,
        settings: &Settings,
        session: bool,
    ) -> BTreeMap<Shortcut, ShortcutConflict> {
        self.suspend();
        let (wanted, mut conflicts) = plan(settings);
        let mut fired = HashMap::new();
        for (action, hotkey) in wanted {
            if action.session_only() && !session {
                continue;
            }
            match self.manager.register(hotkey) {
                Ok(()) => {
                    self.registered.push(hotkey);
                    fired.insert(hotkey.id(), action.command());
                }
                Err(e) => {
                    tracing::warn!(?action, error = %e, "shortcut unavailable");
                    conflicts.insert(action, ShortcutConflict::Taken);
                }
            }
        }
        *self.fired.lock().unwrap_or_else(|e| e.into_inner()) = fired;
        conflicts
    }

    /// Let every key through, as while a new binding is being recorded.
    pub fn suspend(&mut self) {
        if let Err(e) = self.manager.unregister_all(&self.registered) {
            tracing::warn!(error = %e, "shortcuts not unregistered");
        }
        self.registered.clear();
        self.fired.lock().unwrap_or_else(|e| e.into_inner()).clear();
    }
}

/// The hotkey of each bound action, and the ones that cannot be registered: keys the
/// crate cannot read, or already given to an action earlier in [`Shortcut::ALL`].
/// Takebox+ actions are left out of the free version, so their keys stay free.
pub fn plan(
    settings: &Settings,
) -> (
    Vec<(Shortcut, HotKey)>,
    BTreeMap<Shortcut, ShortcutConflict>,
) {
    let mut wanted: Vec<(Shortcut, HotKey)> = Vec::new();
    let mut conflicts = BTreeMap::new();
    for action in Shortcut::ALL {
        let keys = settings.keys(action);
        if keys.is_empty() || (action.plus_only() && !settings.plus()) {
            continue;
        }
        let Ok(hotkey) = HotKey::from_str(&keys) else {
            conflicts.insert(action, ShortcutConflict::Invalid);
            continue;
        };
        match wanted.iter().find(|(_, h)| h.id() == hotkey.id()) {
            Some((first, _)) => {
                conflicts.insert(action, ShortcutConflict::SameAs(*first));
            }
            None => wanted.push((action, hotkey)),
        }
    }
    (wanted, conflicts)
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui;
    use global_hotkey::hotkey::{Code, Modifiers};

    #[test]
    fn the_defaults_are_the_m3_map_without_conflicts() {
        let (wanted, conflicts) = plan(&Settings::default());
        assert!(conflicts.is_empty());
        // Demo and its markers are Takebox+ only.
        assert_eq!(wanted.len(), Shortcut::ALL.len() - 4);
        assert!(wanted.iter().all(|(a, _)| !a.plus_only()));
        let plus = Settings {
            plus_key: "key".into(),
            ..Settings::default()
        };
        assert_eq!(plan(&plus).0.len(), Shortcut::ALL.len());
        let mods = if cfg!(target_os = "macos") {
            Modifiers::ALT | Modifiers::SHIFT
        } else {
            Modifiers::CONTROL | Modifiers::SHIFT
        };
        let capture = wanted
            .iter()
            .find(|(a, _)| *a == Shortcut::Capture)
            .unwrap();
        assert_eq!(capture.1, HotKey::new(Some(mods), Code::KeyS));
        let screen = wanted.iter().find(|(a, _)| *a == Shortcut::Screen).unwrap();
        assert_eq!(screen.1, HotKey::new(Some(mods), Code::Digit3));
    }

    #[test]
    fn duplicates_bad_keys_and_removed_shortcuts_are_reported() {
        let mut settings = Settings::default();
        settings.set_keys(Shortcut::Copy, Shortcut::Capture.default_keys());
        settings.set_keys(Shortcut::Buffer, "Ctrl+Nope".into());
        settings.set_keys(Shortcut::Prefix, String::new());
        let (wanted, conflicts) = plan(&settings);
        assert_eq!(
            conflicts.get(&Shortcut::Copy),
            Some(&ShortcutConflict::SameAs(Shortcut::Capture))
        );
        assert_eq!(
            conflicts.get(&Shortcut::Buffer),
            Some(&ShortcutConflict::Invalid)
        );
        assert_eq!(conflicts.len(), 2);
        assert!(wanted.iter().all(|(a, _)| *a != Shortcut::Prefix));
        assert_eq!(wanted.len(), Shortcut::ALL.len() - 7);
    }

    #[test]
    fn every_key_the_recorder_gives_can_be_registered() {
        let mods = egui::Modifiers::CTRL | egui::Modifiers::SHIFT;
        let mut accepted = 0;
        for key in egui::Key::ALL.iter().copied() {
            if let cb_ui::Recorded::Keys(keys) = cb_ui::record_key(key, mods) {
                assert!(HotKey::from_str(&keys).is_ok(), "{key:?} gave {keys}");
                accepted += 1;
            }
        }
        // Letters, digits and F1 to F24 at least.
        assert!(accepted >= 26 + 10 + 24, "{accepted}");
    }

    #[test]
    fn the_recorder_needs_a_modifier_except_for_function_keys() {
        use cb_ui::{record_key, Recorded};
        use egui::{Key, Modifiers as M};
        assert_eq!(record_key(Key::S, M::NONE), Recorded::Ignored);
        assert_eq!(record_key(Key::S, M::SHIFT), Recorded::Ignored);
        assert_eq!(
            record_key(Key::S, M::CTRL | M::ALT),
            Recorded::Keys("Ctrl+Alt+S".into())
        );
        assert_eq!(record_key(Key::F9, M::NONE), Recorded::Keys("F9".into()));
        assert_eq!(record_key(Key::Escape, M::NONE), Recorded::Cancel);
        assert_eq!(record_key(Key::Backspace, M::NONE), Recorded::Clear);
        assert_eq!(
            record_key(Key::Delete, M::CTRL),
            Recorded::Keys("Ctrl+Delete".into())
        );
        assert_eq!(record_key(Key::Tab, M::CTRL), Recorded::Ignored);
        // Recorded keys read back as the same hotkey.
        let Recorded::Keys(keys) = record_key(Key::Num2, M::ALT | M::SHIFT) else {
            panic!()
        };
        assert_eq!(
            HotKey::from_str(&keys).unwrap(),
            HotKey::new(Some(Modifiers::ALT | Modifiers::SHIFT), Code::Digit2)
        );
    }

    /// The AC: keys another app holds are flagged. Windows only, where the OS refuses a
    /// hotkey a second window registers.
    #[cfg(windows)]
    #[test]
    fn keys_another_app_holds_are_flagged_and_applied_without_restart() {
        let keys = "Ctrl+Alt+Shift+F13";
        let other = GlobalHotKeyManager::new().unwrap();
        other.register(HotKey::from_str(keys).unwrap()).unwrap();

        let mut hotkeys = Hotkeys::new().unwrap();
        let mut settings = Settings::default();
        settings.set_keys(Shortcut::Capture, keys.into());
        let conflicts = hotkeys.apply(&settings, false);
        assert_eq!(
            conflicts.get(&Shortcut::Capture),
            Some(&ShortcutConflict::Taken)
        );

        // A new binding takes effect at once.
        settings.set_keys(Shortcut::Capture, "Ctrl+Alt+Shift+F14".into());
        let conflicts = hotkeys.apply(&settings, false);
        assert!(!conflicts.contains_key(&Shortcut::Capture), "{conflicts:?}");
        let f14 = HotKey::from_str("Ctrl+Alt+Shift+F14").unwrap();
        assert_eq!(
            hotkeys.fired().lock().unwrap().get(&f14.id()),
            Some(&Command::Capture)
        );
    }

    /// US-10.3: the marker keys are only held while a demo session records.
    #[cfg(windows)]
    #[test]
    fn marker_keys_are_only_registered_during_a_session() {
        let keys = "Ctrl+Alt+Shift+F15";
        let mut hotkeys = Hotkeys::new().unwrap();
        let mut settings = Settings {
            plus_key: "key".into(),
            ..Settings::default()
        };
        settings.set_keys(Shortcut::Scene, keys.into());
        let id = HotKey::from_str(keys).unwrap().id();
        let fired = |h: &Hotkeys| h.fired().lock().unwrap().get(&id).cloned();
        let conflicts = hotkeys.apply(&settings, false);
        assert!(!conflicts.contains_key(&Shortcut::Scene), "{conflicts:?}");
        assert_eq!(fired(&hotkeys), None);
        // Free for another app meanwhile.
        let other = GlobalHotKeyManager::new().unwrap();
        other.register(HotKey::from_str(keys).unwrap()).unwrap();
        other.unregister(HotKey::from_str(keys).unwrap()).unwrap();
        hotkeys.apply(&settings, true);
        assert_eq!(
            fired(&hotkeys),
            Some(Command::Mark(cb_core::MarkerKind::Scene))
        );
    }
}
