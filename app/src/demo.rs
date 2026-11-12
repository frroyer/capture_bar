//! Demo sessions (Demo Mode PRD, US-10.1): one recording and the stills taken during it,
//! kept together in a `.cbdemo` folder laid out as the PRD's section 5. The rest of the
//! package (audio tracks, transcript, script, timeline) comes with E11–E13.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use cb_buffer::ItemId;
use cb_core::Language;
use chrono::{DateTime, Local, SecondsFormat};

use crate::output::clean_prefix;

/// The version of the package layout written in `manifest.json` (DNFR-8).
pub const SCHEMA_VERSION: &str = "1.0";

#[derive(Debug)]
pub struct DemoSession {
    pub title: String,
    pub language: Language,
    /// The `.cbdemo` folder. Made by whatever writes into it first.
    pub dir: PathBuf,
    /// t0: when the recording started (DR-2).
    pub started_at: SystemTime,
    /// The session's recording in the buffer; none until it starts.
    pub item: Option<ItemId>,
    stills: u32,
}

impl DemoSession {
    /// A session about to start, in a new folder of `captures_dir`.
    pub fn new(title: &str, language: Language, captures_dir: &Path, now: SystemTime) -> Self {
        Self {
            title: title.trim().to_owned(),
            language,
            dir: captures_dir.join(folder_name(title, now)),
            started_at: now,
            item: None,
            stills: 0,
        }
    }

    pub fn screen_path(&self) -> PathBuf {
        self.dir.join("media").join("screen.mp4")
    }

    /// Where the next still goes: `media/stills/0001.png`, `0002.png`…
    pub fn next_still(&mut self) -> PathBuf {
        self.stills += 1;
        self.dir
            .join("media")
            .join("stills")
            .join(format!("{:04}.png", self.stills))
    }

    /// `manifest.json` for the finished recording at `screen`.
    pub fn manifest(&self, screen: &Path, duration_ms: u64) -> Vec<u8> {
        let screen = screen
            .strip_prefix(&self.dir)
            .unwrap_or(screen)
            .to_string_lossy()
            .replace('\\', "/");
        let manifest = serde_json::json!({
            "schema_version": SCHEMA_VERSION,
            "title": self.title,
            "language": self.language,
            "started_at": DateTime::<Local>::from(self.started_at)
                .to_rfc3339_opts(SecondsFormat::Millis, false),
            "duration_ms": duration_ms,
            "tracks": [{ "id": "screen", "kind": "video", "path": screen, "offset_ms": 0 }],
            "generator": { "app": "Takebox", "version": env!("CARGO_PKG_VERSION") },
        });
        serde_json::to_vec_pretty(&manifest).expect("a JSON value always serializes")
    }
}

/// `<title>_<local date and time>.cbdemo`, or `demo_…` without a title. The time keeps
/// two sessions of the same title apart.
pub fn folder_name(title: &str, at: SystemTime) -> String {
    let stamp = DateTime::<Local>::from(at).format("%Y-%m-%d_%H-%M-%S");
    match clean_prefix(title).trim() {
        "" => format!("demo_{stamp}.cbdemo"),
        title => format!("{title}_{stamp}.cbdemo"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_folder_is_the_clean_title_and_the_time() {
        let at = SystemTime::now();
        let name = folder_name(" Onboarding: v2/3 ", at);
        assert!(name.starts_with("Onboarding v23_"), "{name}");
        assert!(name.ends_with(".cbdemo"));
        assert!(folder_name("  ", at).starts_with("demo_"));
    }

    #[test]
    fn media_go_where_the_package_puts_them_and_the_manifest_points_at_them() {
        let mut session =
            DemoSession::new("Tour", Language::Fr, Path::new("caps"), SystemTime::now());
        let screen = session.screen_path();
        assert!(screen.starts_with(&session.dir));
        assert!(screen.ends_with("media/screen.mp4"));
        assert!(session.next_still().ends_with("media/stills/0001.png"));
        assert!(session.next_still().ends_with("media/stills/0002.png"));

        let json: serde_json::Value =
            serde_json::from_slice(&session.manifest(&screen, 4200)).unwrap();
        assert_eq!(json["schema_version"], SCHEMA_VERSION);
        assert_eq!(json["title"], "Tour");
        assert_eq!(json["language"], "fr");
        assert_eq!(json["duration_ms"], 4200);
        assert_eq!(json["tracks"][0]["path"], "media/screen.mp4");
    }
}
