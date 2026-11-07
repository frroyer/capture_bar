//! Where finished frames go: the clipboard and a PNG file, on a worker thread, so the
//! UI thread never waits for them (FR-12, FR-20).

use std::fs::{File, OpenOptions};
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::Arc;
use std::time::SystemTime;

use cb_buffer::ItemId;
use cb_core::Frame;
use chrono::{DateTime, Local};

/// Twice the tray's card size in points, to stay sharp on HiDPI screens.
const THUMBNAIL_PIXELS: (u32, u32) = (
    cb_ui::THUMBNAIL_SIZE.x as u32 * 2,
    cb_ui::THUMBNAIL_SIZE.y as u32 * 2,
);

pub enum Job {
    /// Copy to the clipboard, then write the file.
    Store {
        item: ItemId,
        frame: Arc<Frame>,
        path: PathBuf,
    },
    Copy(Arc<Frame>),
}

pub enum Done {
    /// Sent before the slower clipboard and file work, so the tray shows it early.
    Thumbnail {
        item: ItemId,
        thumbnail: Frame,
    },
    Stored {
        item: ItemId,
        /// Where the file went.
        result: Result<PathBuf, String>,
    },
    ClipboardFailed(String),
}

/// The prefix new captures get until the user picks one (FR-30).
pub const DEFAULT_PREFIX: &str = "capture";

/// `prefix` without what a file name cannot hold on any of our systems.
pub fn clean_prefix(prefix: &str) -> String {
    prefix
        .chars()
        .filter(|c| !c.is_control() && !r#"<>:"/\|?*"#.contains(*c))
        .collect()
}

/// `<prefix>_<local date and time>.png`, or the timestamp alone without a prefix (FR-30).
pub fn file_name(prefix: &str, at: SystemTime) -> String {
    let stamp = DateTime::<Local>::from(at).format("%Y-%m-%d_%H-%M-%S");
    match clean_prefix(prefix).trim() {
        "" => format!("{stamp}.png"),
        prefix => format!("{prefix}_{stamp}.png"),
    }
}

/// Create `path`, or `<stem>-2.png`, `-3`… when it exists: a file is never overwritten.
fn create_unique(path: &Path) -> std::io::Result<(File, PathBuf)> {
    let stem = path.file_stem().unwrap_or_default().to_string_lossy();
    let extension = path.extension().unwrap_or_default().to_string_lossy();
    for n in 1u32.. {
        let candidate = match n {
            1 => path.to_path_buf(),
            n => path.with_file_name(format!("{stem}-{n}.{extension}")),
        };
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&candidate)
        {
            Err(e) if e.kind() == ErrorKind::AlreadyExists => continue,
            opened => return opened.map(|file| (file, candidate)),
        }
    }
    unreachable!("every name from -2 to -{} exists", u32::MAX)
}

/// Returns where the file went, which differs from `path` when that was taken.
fn write_png(frame: &Frame, path: &Path) -> Result<PathBuf, String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let (file, path) = create_unique(path).map_err(|e| e.to_string())?;
    cb_encode::write_png(frame, std::io::BufWriter::new(file))
        .map(|()| path.clone())
        .map_err(|e| {
            // Our own half-written file: no use to anyone.
            let _ = std::fs::remove_file(&path);
            e.to_string()
        })
}

fn copy(clipboard: &mut Option<arboard::Clipboard>, frame: &Frame) -> Result<(), String> {
    if clipboard.is_none() {
        *clipboard = Some(arboard::Clipboard::new().map_err(|e| e.to_string())?);
    }
    let image = arboard::ImageData {
        width: frame.width as usize,
        height: frame.height as usize,
        bytes: (&frame.rgba[..]).into(),
    };
    clipboard
        .as_mut()
        .expect("just set")
        .set_image(image)
        .map_err(|e| e.to_string())
}

/// `wake` runs after each finished job, to get the UI to read the result.
pub fn spawn(wake: impl Fn() + Send + 'static) -> (mpsc::Sender<Job>, mpsc::Receiver<Done>) {
    let (job_tx, jobs) = mpsc::channel::<Job>();
    let (done_tx, done) = mpsc::channel();
    std::thread::Builder::new()
        .name("output".into())
        .spawn(move || {
            // Kept for the worker's life: on X11 the image goes when its owner does.
            let mut clipboard = None;
            for job in jobs {
                let frame = match &job {
                    Job::Store { frame, .. } | Job::Copy(frame) => frame,
                };
                if let Job::Store { item, frame, .. } = &job {
                    match cb_encode::thumbnail(frame, THUMBNAIL_PIXELS.0, THUMBNAIL_PIXELS.1) {
                        Ok(thumbnail) => {
                            let item = *item;
                            let _ = done_tx.send(Done::Thumbnail { item, thumbnail });
                            wake();
                        }
                        Err(e) => tracing::warn!(error = %e, "no thumbnail"),
                    }
                }
                if let Err(e) = copy(&mut clipboard, frame) {
                    let _ = done_tx.send(Done::ClipboardFailed(e));
                }
                if let Job::Store { item, frame, path } = job {
                    let result = write_png(&frame, &path);
                    let _ = done_tx.send(Done::Stored { item, result });
                }
                wake();
            }
        })
        .expect("spawn output thread");
    (job_tx, done)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, SystemTime};

    fn frame_at(captured_at: SystemTime) -> Frame {
        Frame {
            width: 2,
            height: 1,
            rgba: vec![255; 8],
            captured_at,
        }
    }

    #[test]
    fn file_name_is_the_prefix_and_the_local_time() {
        let at = SystemTime::now();
        let stamp = DateTime::<Local>::from(at)
            .format("%Y-%m-%d_%H-%M-%S")
            .to_string();
        assert_eq!(file_name("acme", at), format!("acme_{stamp}.png"));
        assert_eq!(file_name("", at), format!("{stamp}.png"));
        assert_eq!(file_name("  ", at), format!("{stamp}.png"));
        assert_eq!(file_name(" a b ", at), format!("a b_{stamp}.png"));
        let later = file_name("acme", at + Duration::from_secs(1));
        assert_ne!(later, file_name("acme", at));
    }

    #[test]
    fn prefixes_cannot_leave_the_folder_or_break_the_name() {
        assert_eq!(clean_prefix(r#"../a\b:c*d?"e<f>g|h/"#), "..abcdefgh");
        assert_eq!(clean_prefix("tab\there\n"), "tabhere");
        assert_eq!(clean_prefix("Équipe 3 — été"), "Équipe 3 — été");
    }

    /// Runs one Store job to the end and returns where the file went.
    fn store(path: &Path, item: u64) -> Result<PathBuf, String> {
        let (jobs, done) = spawn(|| {});
        jobs.send(Job::Store {
            item: ItemId(item),
            frame: Arc::new(frame_at(SystemTime::now())),
            path: path.to_path_buf(),
        })
        .unwrap();
        let mut thumbnail_seen = None;
        let stored = done
            .iter()
            .find_map(|d| match d {
                Done::Stored { item, result } => Some((item, result)),
                Done::Thumbnail { item, thumbnail } => {
                    thumbnail_seen = Some((item, thumbnail.width));
                    None
                }
                Done::ClipboardFailed(_) => None,
            })
            .unwrap();
        assert_eq!(stored.0, ItemId(item));
        assert_eq!(thumbnail_seen, Some((ItemId(item), 2)), "before the file");
        stored.1
    }

    #[test]
    fn store_writes_a_png_and_never_overwrites() {
        let dir = std::env::temp_dir().join(format!("cb-output-test-{}", std::process::id()));
        let path = dir.join("nested").join("a.png");
        assert_eq!(store(&path, 3), Ok(path.clone()));
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(&bytes[1..4], b"PNG");

        std::fs::write(dir.join("nested").join("a-2.png"), "keep me").unwrap();
        assert_eq!(store(&path, 4), Ok(dir.join("nested").join("a-3.png")));
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        assert_eq!(
            std::fs::read_to_string(dir.join("nested").join("a-2.png")).unwrap(),
            "keep me"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }
}
