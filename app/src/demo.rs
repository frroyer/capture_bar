//! Demo sessions (Demo Mode PRD, US-10.1): one recording and the stills taken during it,
//! kept together in a `.cbdemo` folder laid out as the PRD's section 5. The rest of the
//! package (audio tracks, transcript, script, timeline) comes with E11–E13. The event
//! log, `events.jsonl` (US-10.4), is written by `cb_platform::record_events`.
//!
//! One clock (US-10.2, DR-2): the recording's own, from [`cb_core::StreamHandle::media_time`].
//! Time zero is the first video frame, pauses are left out as the video leaves them out,
//! and the sound in the file is placed on the same clock by its capture time. Everything
//! here carries `t_ms` on it, and the wall-clock time where it has one.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use cb_buffer::ItemId;
use cb_core::{Language, MarkerKind, Recording};
use chrono::{DateTime, Local, SecondsFormat};
use serde::{Deserialize, Serialize};

use crate::output::clean_prefix;

/// The version of the package layout written in `manifest.json` (DNFR-8).
pub const SCHEMA_VERSION: &str = "1.0";

#[derive(Debug)]
pub struct DemoSession {
    pub title: String,
    pub language: Language,
    /// The `.cbdemo` folder. Made by whatever writes into it first.
    pub dir: PathBuf,
    /// The session's recording in the buffer; none until it starts.
    pub item: Option<ItemId>,
    /// When each still asked for was taken, in `t_ms`, until its file is placed.
    pub still_times: HashMap<ItemId, u64>,
    /// The clap test's log (US-10.2).
    pub claps: ClapLog,
    /// Whether `events.jsonl` is being written (US-10.4); not on every platform yet.
    pub events: bool,
    stills: Vec<Still>,
    markers: Vec<Marker>,
}

/// One marker of `markers.json` (US-10.3, DR-4).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct Marker {
    kind: MarkerKind,
    t_ms: u64,
    /// A retake starts at the previous marker, or at the start.
    #[serde(skip_serializing_if = "Option::is_none")]
    from_ms: Option<u64>,
    at: String,
}

#[derive(Debug)]
struct Still {
    path: PathBuf,
    t_ms: Option<u64>,
    at: SystemTime,
}

impl DemoSession {
    /// A session about to start, in a new folder of `captures_dir`.
    pub fn new(title: &str, language: Language, captures_dir: &Path, now: SystemTime) -> Self {
        Self {
            title: title.trim().to_owned(),
            language,
            dir: captures_dir.join(folder_name(title, now)),
            item: None,
            still_times: HashMap::new(),
            claps: ClapLog::default(),
            events: false,
            stills: Vec::new(),
            markers: Vec::new(),
        }
    }

    /// Put a `kind` marker down at `t_ms`, wall-clock `at`.
    pub fn mark(&mut self, kind: MarkerKind, t_ms: u64, at: SystemTime) {
        let from_ms = (kind == MarkerKind::Retake)
            .then(|| self.markers.last().map_or(0, |m| m.t_ms.min(t_ms)));
        self.markers.push(Marker {
            kind,
            t_ms,
            from_ms,
            at: iso(at),
        });
    }

    /// `markers.json`: the markers in the order they were put down.
    pub fn markers_json(&self) -> Vec<u8> {
        serde_json::to_vec_pretty(&serde_json::json!({
            "schema_version": SCHEMA_VERSION,
            "markers": self.markers,
        }))
        .expect("markers always serialize")
    }

    pub fn screen_path(&self) -> PathBuf {
        self.dir.join("media").join("screen.mp4")
    }

    /// Where the still `item`, taken at wall-clock `at`, goes: `media/stills/0001.png`,
    /// `0002.png`… It keeps the `t_ms` noted for it in `still_times`.
    pub fn place_still(&mut self, item: ItemId, at: SystemTime) -> PathBuf {
        let path = self
            .dir
            .join("media")
            .join("stills")
            .join(format!("{:04}.png", self.stills.len() + 1));
        self.stills.push(Still {
            path: path.clone(),
            t_ms: self.still_times.remove(&item),
            at,
        });
        path
    }

    /// `manifest.json` for the session's finished `recording`.
    pub fn manifest(&self, recording: &Recording) -> Vec<u8> {
        let stills: Vec<_> = self
            .stills
            .iter()
            .map(|s| {
                serde_json::json!({
                    "path": self.relative(&s.path),
                    "t_ms": s.t_ms,
                    "at": iso(s.at),
                })
            })
            .collect();
        let manifest = serde_json::json!({
            "schema_version": SCHEMA_VERSION,
            "title": self.title,
            "language": self.language,
            "started_at": iso(recording.started_at),
            "duration_ms": recording.duration.as_millis() as u64,
            "clock": "t_ms: milliseconds from the first video frame, pauses left out",
            "tracks": [{
                "id": "screen",
                "kind": "video",
                "path": self.relative(&recording.path),
                "offset_ms": 0,
                "size": [recording.width, recording.height],
            }],
            "stills": stills,
            "events": self.events.then(|| serde_json::json!({
                "path": cb_core::events::EVENTS_FILE,
                "coordinates": "desktop pixels",
            })),
            "generator": { "app": "Takebox", "version": env!("CARGO_PKG_VERSION") },
        });
        serde_json::to_vec_pretty(&manifest).expect("a JSON value always serializes")
    }

    /// `path` from the package folder, with `/` as the package format has it.
    fn relative(&self, path: &Path) -> String {
        path.strip_prefix(&self.dir)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/")
    }
}

fn iso(at: SystemTime) -> String {
    DateTime::<Local>::from(at).to_rfc3339_opts(SecondsFormat::Millis, false)
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

/// The clap test (US-10.2, DNFR-1). Each clap flashes a black square white at the centre
/// of what is recorded, logged at its `t_ms`, and plays a beep through the sound card,
/// which says when the beep really played: logged on the same clock. In the file, the
/// flash and the beep are found again. The flash lands a steady screen latency after its
/// log, and the beep where it was played if the sound is stamped right. A track keeps
/// time if each clap's offset stays within [`ClapReport::LIMIT_MS`] of the track's
/// median offset, its steady latency. The video alone can only place a flash on a
/// screen refresh and then on a frame (33 ms at 30 fps), so its offsets spread by
/// about that much without any drift.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ClapLog {
    /// When each clap flashed, in `t_ms`.
    pub claps: Vec<u64>,
    /// When each beep started playing, by the sound card's clock, in `t_ms`.
    pub beeps: Vec<u64>,
}

/// The clap log's file in a package.
pub const CLAP_LOG: &str = "claps.json";

impl ClapLog {
    pub fn to_json(&self) -> Vec<u8> {
        serde_json::to_vec(self).expect("a clap log always serializes")
    }
}

/// One clap, in `t_ms`: as logged, the flash found in the video, the beep as the sound
/// card played it, and the beep found in the sound.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Clap {
    pub t_ms: u64,
    pub flash_ms: Option<u64>,
    pub played_ms: Option<u64>,
    pub beep_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ClapReport {
    pub claps: Vec<Clap>,
}

/// A flash is the centre square turning brighter than this (0–255).
const FLASH_LUMA: f32 = 128.0;
/// Where to look for a flash around its clap, and for a beep around when it played, in
/// ms: wide on the sound side, to see a track that slipped.
const FLASH_SEARCH: (i64, i64) = (-200, 800);
const BEEP_SEARCH: (i64, i64) = (-500, 3000);

impl ClapReport {
    /// The most a clap's offset may stray from its track's median offset (DNFR-1).
    pub const LIMIT_MS: i64 = 50;
    /// The side of the centre square probed for the flash, in video pixels.
    pub const PATCH: usize = 40;
    /// The length of one audio level window.
    pub const WINDOW: Duration = Duration::from_millis(5);

    /// Find each clap of `log` in the frames' centre `luma` and the sound's `levels`,
    /// one per [`Self::WINDOW`].
    pub fn new(log: &ClapLog, luma: &[(Duration, f32)], levels: &[f32]) -> Self {
        let window_ms = Self::WINDOW.as_millis() as u64;
        let near = |(from, to): (i64, i64), t: u64, at: u64| {
            let d = at as i64 - t as i64;
            d >= from && d <= to
        };
        let claps = log
            .claps
            .iter()
            .map(|&t_ms| {
                let flash_ms = luma
                    .iter()
                    .map(|(at, y)| (at.as_millis() as u64, *y))
                    .find(|&(at, y)| near(FLASH_SEARCH, t_ms, at) && y > FLASH_LUMA)
                    .map(|(at, _)| at);
                // Its beep starts playing within a second of the clap.
                let played_ms = log
                    .beeps
                    .iter()
                    .copied()
                    .find(|&b| b >= t_ms && b < t_ms + 1000);
                let beep_ms = played_ms.and_then(|played| {
                    // Louder than the room was just before: four times its median, at least.
                    let quiet: Vec<f32> = (played.saturating_sub(1500) / window_ms
                        ..played.saturating_sub(500) / window_ms)
                        .filter_map(|i| levels.get(i as usize).copied())
                        .collect();
                    let floor = (median(quiet) * 4.0).max(0.05);
                    (0..levels.len() as u64).map(|i| i * window_ms).find(|&at| {
                        near(BEEP_SEARCH, played, at) && levels[(at / window_ms) as usize] > floor
                    })
                });
                Clap {
                    t_ms,
                    flash_ms,
                    played_ms,
                    beep_ms,
                }
            })
            .collect();
        Self { claps }
    }

    /// Measure the session in the package `dir` from `media/screen.mp4` and its clap log,
    /// and write `clap-report.json` there. Slow: it decodes the whole video.
    pub fn measure(dir: &Path) -> Result<Self, String> {
        let log = std::fs::read(dir.join(CLAP_LOG)).map_err(|e| format!("{CLAP_LOG}: {e}"))?;
        let log: ClapLog = serde_json::from_slice(&log).map_err(|e| format!("{CLAP_LOG}: {e}"))?;
        let video = dir.join("media").join("screen.mp4");
        let (luma, levels) = cb_platform::clap_probes(&video, Self::PATCH, Self::WINDOW)
            .map_err(|e| e.to_string())?;
        let report = Self::new(&log, &luma, &levels);
        std::fs::write(dir.join("clap-report.json"), report.to_json())
            .map_err(|e| e.to_string())?;
        Ok(report)
    }

    /// Each clap's offset on the video (flash after its log) and on the sound (beep in
    /// the file after it played), where both ends were found.
    pub fn offsets(&self) -> (Vec<i64>, Vec<i64>) {
        let diff = |a: Option<u64>, b: Option<u64>| Some(a? as i64 - b? as i64);
        let video = self
            .claps
            .iter()
            .filter_map(|c| diff(c.flash_ms, Some(c.t_ms)))
            .collect();
        let sound = self
            .claps
            .iter()
            .filter_map(|c| diff(c.beep_ms, c.played_ms))
            .collect();
        (video, sound)
    }

    /// How far a clap's offset strays at most from its track's median offset: video,
    /// sound. `None` when no clap was found on the track.
    pub fn drifts(&self) -> (Option<i64>, Option<i64>) {
        let drift = |mut o: Vec<i64>| {
            o.sort_unstable();
            let median = *o.get(o.len() / 2)?;
            o.iter().map(|v| (v - median).abs()).max()
        };
        let (video, sound) = self.offsets();
        (drift(video), drift(sound))
    }

    /// Every clap found on both tracks, and both drifts within the limit.
    pub fn passed(&self) -> bool {
        let all_found = self
            .claps
            .iter()
            .all(|c| c.flash_ms.is_some() && c.played_ms.is_some() && c.beep_ms.is_some());
        let (video, sound) = self.drifts();
        all_found
            && !self.claps.is_empty()
            && video.is_some_and(|v| v <= Self::LIMIT_MS)
            && sound.is_some_and(|s| s <= Self::LIMIT_MS)
    }

    pub fn to_json(&self) -> Vec<u8> {
        let (video, sound) = self.drifts();
        serde_json::to_vec_pretty(&serde_json::json!({
            "passed": self.passed(),
            "limit_ms": Self::LIMIT_MS,
            "video_drift_ms": video,
            "sound_drift_ms": sound,
            "claps": self.claps,
        }))
        .expect("a JSON value always serializes")
    }
}

fn median(mut values: Vec<f32>) -> f32 {
    if values.is_empty() {
        return 0.0;
    }
    values.sort_by(f32::total_cmp);
    values[values.len() / 2]
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
        session.still_times.insert(ItemId(7), 1500);
        let at = SystemTime::now();
        assert!(session
            .place_still(ItemId(7), at)
            .ends_with("media/stills/0001.png"));
        assert!(session
            .place_still(ItemId(8), at)
            .ends_with("media/stills/0002.png"));
        assert!(session.still_times.is_empty());

        let recording = Recording {
            path: screen,
            width: 1920,
            height: 1080,
            duration: Duration::from_millis(4200),
            frames: 126,
            dropped: 0,
            start_latency: Duration::ZERO,
            started_at: at,
        };
        let json: serde_json::Value =
            serde_json::from_slice(&session.manifest(&recording)).unwrap();
        assert_eq!(json["schema_version"], SCHEMA_VERSION);
        assert_eq!(json["title"], "Tour");
        assert_eq!(json["language"], "fr");
        assert_eq!(json["duration_ms"], 4200);
        assert_eq!(json["started_at"], iso(at));
        assert_eq!(json["tracks"][0]["path"], "media/screen.mp4");
        assert_eq!(json["stills"][0]["path"], "media/stills/0001.png");
        assert_eq!(json["stills"][0]["t_ms"], 1500);
        assert!(json["stills"][1]["t_ms"].is_null());
        assert!(json["events"].is_null());
        session.events = true;
        let json: serde_json::Value =
            serde_json::from_slice(&session.manifest(&recording)).unwrap();
        assert_eq!(json["events"]["path"], "events.jsonl");
    }

    #[test]
    fn markers_keep_their_order_and_a_retake_runs_from_the_previous_marker() {
        let mut session =
            DemoSession::new("Tour", Language::En, Path::new("caps"), SystemTime::now());
        let at = SystemTime::now();
        session.mark(MarkerKind::Retake, 900, at);
        session.mark(MarkerKind::Scene, 3000, at);
        session.mark(MarkerKind::Highlight, 4500, at);
        session.mark(MarkerKind::Retake, 7000, at);
        let json: serde_json::Value = serde_json::from_slice(&session.markers_json()).unwrap();
        assert_eq!(json["schema_version"], SCHEMA_VERSION);
        let markers = json["markers"].as_array().unwrap();
        let kinds: Vec<_> = markers
            .iter()
            .map(|m| m["kind"].as_str().unwrap())
            .collect();
        assert_eq!(kinds, ["retake", "scene", "highlight", "retake"]);
        assert_eq!(markers[0]["from_ms"], 0);
        assert_eq!(markers[3]["t_ms"], 7000);
        assert_eq!(markers[3]["from_ms"], 4500);
        assert!(markers[1].get("from_ms").is_none());
        assert_eq!(markers[2]["at"], iso(at));
        // No markers: the file is still there, empty.
        let empty = DemoSession::new("", Language::En, Path::new("caps"), at);
        let json: serde_json::Value = serde_json::from_slice(&empty.markers_json()).unwrap();
        assert_eq!(json["markers"], serde_json::json!([]));
    }

    /// A 30 fps video with a flash `video_lag` ms after each clap, a beep played 110 ms
    /// after it and found `sound_lag` ms after it played, over a quiet room.
    fn session(claps: &[u64], video_lag: &[u64], sound_lag: &[u64]) -> ClapReport {
        let end = claps.last().unwrap() + 4000;
        let log = ClapLog {
            claps: claps.to_vec(),
            beeps: claps.iter().map(|c| c + 110).collect(),
        };
        let luma: Vec<_> = (0..end * 30 / 1000)
            .map(|f| {
                let at = f * 1000 / 30;
                let lit = claps
                    .iter()
                    .zip(video_lag)
                    .any(|(&c, &lag)| at >= c + lag && at < c + lag + 100);
                (Duration::from_millis(at), if lit { 235.0 } else { 16.0 })
            })
            .collect();
        let levels: Vec<f32> = (0..end / 5)
            .map(|w| {
                let at = w * 5;
                let loud = log
                    .beeps
                    .iter()
                    .zip(sound_lag)
                    .any(|(&played, &lag)| at >= played + lag && at < played + lag + 120);
                if loud {
                    0.35
                } else {
                    0.01
                }
            })
            .collect();
        ClapReport::new(&log, &luma, &levels)
    }

    #[test]
    fn steady_tracks_pass() {
        let report = session(&[2000, 12_000, 22_000], &[40, 40, 40], &[5, 5, 5]);
        assert!(report.passed(), "{report:?}");
        let (video, sound) = report.drifts();
        assert!(
            video.unwrap() <= 34 && sound.unwrap() <= 5,
            "{video:?} {sound:?}"
        );
        // Flashes landing on different 30 fps frames: jitter, not drift.
        let jitter = session(&[2000, 12_000, 22_000], &[8, 27, 60], &[5, 5, 5]);
        assert!(jitter.passed(), "{jitter:?}");
    }

    #[test]
    fn a_track_that_slips_fails() {
        let claps = [2000, 12_000, 22_000];
        let video = session(&claps, &[40, 40, 120], &[5, 5, 5]);
        assert!(!video.passed());
        assert!(video.drifts().0.unwrap() >= 60);
        // The sound 800 ms late after a while, as seen with a screen saver: still found.
        let sound = session(&claps, &[40, 40, 40], &[5, 805, 805]);
        assert!(!sound.passed());
        assert_eq!(sound.offsets().1, [5, 805, 805]);
    }

    #[test]
    fn a_clap_missing_from_a_track_fails() {
        let mut report = session(&[2000, 12_000], &[40, 40], &[5, 5]);
        report.claps[1].beep_ms = None;
        assert!(!report.passed());
        let json: serde_json::Value = serde_json::from_slice(&report.to_json()).unwrap();
        assert_eq!(json["passed"], false);
        assert!(json["claps"][1]["beep_ms"].is_null());
        assert!(!ClapReport::new(&ClapLog::default(), &[], &[]).passed());
        // A log from before beeps were timed: nothing to measure the sound against.
        let old: ClapLog = serde_json::from_str(r#"{"claps":[5,15]}"#).unwrap();
        assert_eq!((old.claps.len(), old.beeps.len()), (2, 0));
    }
}
