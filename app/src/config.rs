//! The settings file (US-8.2, FR-29): TOML, next to the binary when a `capture-bar.toml`
//! is already there (portable), otherwise in the user's config folder.

use std::path::{Path, PathBuf};
use std::sync::mpsc;

use cb_core::Settings;

/// The file name, both next to the binary and in the config folder.
pub const FILE_NAME: &str = "capture-bar.toml";

/// Where settings are read and written. A file next to the binary wins, so a copy on a
/// USB stick keeps its own settings.
pub fn path() -> PathBuf {
    let portable = std::env::current_exe()
        .ok()
        .and_then(|exe| Some(exe.parent()?.join(FILE_NAME)));
    if let Some(portable) = portable.filter(|p| p.is_file()) {
        return portable;
    }
    directories::ProjectDirs::from("", "", "Capture Bar")
        .map(|dirs| dirs.config_dir().join(FILE_NAME))
        .unwrap_or_else(|| PathBuf::from(FILE_NAME))
}

/// What `load` found.
#[derive(Debug, PartialEq)]
pub enum Loaded {
    /// The file was read.
    Read(Settings),
    /// There is no file yet: these are the defaults, with whatever `migrate` added.
    New(Settings),
    /// The file could not be read or parsed: the defaults, and why.
    Damaged(Settings, String),
}

impl Loaded {
    pub fn settings(&self) -> &Settings {
        match self {
            Loaded::Read(s) | Loaded::New(s) | Loaded::Damaged(s, _) => s,
        }
    }
}

/// Read the settings at `path`. Before a file existed, the folder and prefix were kept in
/// eframe's storage: `migrate` brings those over to a new file.
pub fn load(path: &Path, migrate: impl FnOnce(&mut Settings)) -> Loaded {
    match std::fs::read_to_string(path) {
        Ok(text) => match toml::from_str::<Settings>(&text) {
            Ok(settings) => Loaded::Read(settings.sanitized()),
            Err(e) => Loaded::Damaged(Settings::default(), e.to_string()),
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let mut settings = Settings::default();
            migrate(&mut settings);
            Loaded::New(settings.sanitized())
        }
        Err(e) => Loaded::Damaged(Settings::default(), e.to_string()),
    }
}

/// Write `settings` to `path`, through a temporary file so a crash never leaves half a file.
pub fn write(path: &Path, settings: &Settings) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let text = toml::to_string_pretty(settings).map_err(std::io::Error::other)?;
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, path)
}

/// A thread that writes each settings sent to it, keeping disk I/O off the UI (NFR-4).
/// Only the newest of several waiting is written.
pub fn spawn_writer(
    path: PathBuf,
    on_error: impl Fn(std::io::Error) + Send + 'static,
) -> mpsc::Sender<Settings> {
    let (tx, rx) = mpsc::channel::<Settings>();
    std::thread::Builder::new()
        .name("settings writer".into())
        .spawn(move || {
            while let Ok(mut settings) = rx.recv() {
                while let Ok(newer) = rx.try_recv() {
                    settings = newer;
                }
                if let Err(e) = write(&path, &settings) {
                    on_error(e);
                }
            }
        })
        .expect("spawn settings writer");
    tx
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("cb-config-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn settings_written_are_read_back() {
        let path = temp_dir("roundtrip").join(FILE_NAME);
        let mut settings = Settings {
            prefix: "acme".into(),
            retention_days: 30,
            frame_rate: 60,
            microphone: false,
            ..Settings::default()
        };
        settings.set_keys(cb_core::Shortcut::Capture, "Ctrl+Alt+F5".into());
        settings.set_keys(cb_core::Shortcut::Copy, String::new());
        let record = cb_core::Shortcut::Record;
        settings.set_keys(record, record.default_keys());
        assert_eq!(settings.shortcuts.len(), 2, "defaults are not kept");
        write(&path, &settings).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("capture = \"Ctrl+Alt+F5\""), "{text}");
        assert_eq!(
            load(&path, |_| panic!("no migration")),
            Loaded::Read(settings)
        );
    }

    #[test]
    fn a_missing_file_takes_the_old_values_and_defaults() {
        let path = temp_dir("missing").join(FILE_NAME);
        let loaded = load(&path, |s| s.prefix = "old".into());
        let expected = Settings {
            prefix: "old".into(),
            ..Settings::default()
        };
        assert_eq!(loaded, Loaded::New(expected));
    }

    #[test]
    fn keys_an_older_file_lacks_take_their_default_and_bad_values_are_fixed() {
        let path = temp_dir("older").join(FILE_NAME);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            "prefix = \"x\"\nframe_rate = 25\nretention_days = 0\ncountdown_secs = 99\n",
        )
        .unwrap();
        let Loaded::Read(s) = load(&path, |_| {}) else {
            panic!("not read");
        };
        assert_eq!(s.prefix, "x");
        assert_eq!(s.frame_rate, 30);
        assert_eq!(s.retention_days, 1);
        assert_eq!(s.version, Settings::VERSION);
        assert!(s.system_audio);
        assert_eq!(s.countdown_secs, 10);
        assert_eq!(Settings::default().countdown_secs, 3);
    }

    #[test]
    fn a_damaged_file_gives_the_defaults() {
        let path = temp_dir("damaged").join(FILE_NAME);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "prefix = ").unwrap();
        assert!(matches!(load(&path, |_| {}), Loaded::Damaged(s, _) if s == Settings::default()));
    }
}
