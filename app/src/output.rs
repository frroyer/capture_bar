//! Where finished frames go: the clipboard and a PNG file, on a worker thread, so the
//! UI thread never waits for them (FR-12, FR-20).

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::Arc;
use std::time::UNIX_EPOCH;

use cb_buffer::ItemId;
use cb_core::Frame;

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
        path: PathBuf,
        result: Result<(), String>,
    },
    ClipboardFailed(String),
}

/// A name no other capture has: item ids start over at each launch.
pub fn file_name(frame: &Frame, item: ItemId) -> String {
    let millis = frame
        .captured_at
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    format!("capture-{millis}-{}.png", item.0)
}

fn write_png(frame: &Frame, path: &Path) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let file = std::fs::File::create(path).map_err(|e| e.to_string())?;
    cb_encode::write_png(frame, std::io::BufWriter::new(file)).map_err(|e| e.to_string())
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
                    let _ = done_tx.send(Done::Stored { item, path, result });
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
    fn file_names_differ_across_sessions_with_the_same_item_id() {
        let start = SystemTime::now();
        let first = file_name(&frame_at(start), ItemId(1));
        let later = file_name(&frame_at(start + Duration::from_secs(60)), ItemId(1));
        assert_ne!(first, later);
        assert!(first.starts_with("capture-") && first.ends_with("-1.png"));
    }

    #[test]
    fn store_writes_a_png_even_without_a_clipboard() {
        let dir = std::env::temp_dir().join(format!("cb-output-test-{}", std::process::id()));
        let path = dir.join("nested").join("a.png");
        let (jobs, done) = spawn(|| {});
        jobs.send(Job::Store {
            item: ItemId(3),
            frame: Arc::new(frame_at(SystemTime::now())),
            path: path.clone(),
        })
        .unwrap();
        let mut thumbnail_seen = None;
        let stored = done
            .iter()
            .find_map(|d| match d {
                Done::Stored { item, result, .. } => Some((item, result)),
                Done::Thumbnail { item, thumbnail } => {
                    thumbnail_seen = Some((item, thumbnail.width));
                    None
                }
                Done::ClipboardFailed(_) => None,
            })
            .unwrap();
        assert_eq!(stored, (ItemId(3), Ok(())));
        assert_eq!(thumbnail_seen, Some((ItemId(3), 2)), "before the file");
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(&bytes[1..4], b"PNG");
        std::fs::remove_dir_all(dir).unwrap();
    }
}
