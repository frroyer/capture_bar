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
    /// Copy the image in a PNG file to the clipboard (FR-23).
    CopyFile(PathBuf),
    /// Save a copy of a capture where the user chose. Overwrites: the dialog asked.
    SaveAs {
        from: PathBuf,
        to: PathBuf,
    },
    /// Move deleted captures' files to the Recycle Bin or Trash.
    Trash(Vec<PathBuf>),
    /// One PDF of the images in `files`, a page each, written to `to` (US-6.7). Overwrites:
    /// the dialog asked.
    ExportPdf {
        files: Vec<PathBuf>,
        to: PathBuf,
    },
    /// Put several files on the clipboard, to paste into a folder, a mail or a chat (FR-24).
    CopyFiles(Vec<PathBuf>),
    /// Copy files into `dir`, never overwriting what is there (FR-24).
    SaveAllTo {
        files: Vec<PathBuf>,
        dir: PathBuf,
    },
    /// Rename captures' files to a new prefix, keeping their timestamps (FR-30).
    Rename {
        files: Vec<(ItemId, PathBuf)>,
        prefix: String,
    },
    /// Make the thumbnail of a capture restored from the last run (FR-25).
    Thumbnail {
        item: ItemId,
        path: PathBuf,
    },
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
    /// How a one-off job went, for a toast.
    Message(String),
    /// A capture's file has a new name.
    Renamed {
        item: ItemId,
        path: PathBuf,
    },
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

/// Whether `s` is a timestamp as [`file_name`] writes it: `YYYY-MM-DD_HH-MM-SS`.
fn is_stamp(s: &str) -> bool {
    s.len() == 19
        && s.bytes().enumerate().all(|(i, b)| match i {
            4 | 7 | 13 | 16 => b == b'-',
            10 => b == b'_',
            _ => b.is_ascii_digit(),
        })
}

/// The timestamp at the end of `s`, if it ends with one.
fn stamp_of(s: &str) -> Option<&str> {
    s.get(s.len().checked_sub(19)?..).filter(|t| is_stamp(t))
}

/// The name `file` would have with `prefix`, keeping its timestamp: `old_<stamp>-2.png`
/// becomes `new_<stamp>.png` (a clash is resolved when renaming). `None` when the name
/// holds no timestamp of ours, or would not change.
pub fn renamed(file: &str, prefix: &str) -> Option<String> {
    let (stem, extension) = file.rsplit_once('.')?;
    // A `-N` added to avoid overwriting belongs to the old name only.
    let stamp = stem
        .rsplit_once('-')
        .filter(|(_, n)| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
        .and_then(|(core, _)| stamp_of(core))
        .or_else(|| stamp_of(stem))?;
    let name = match clean_prefix(prefix).trim() {
        "" => format!("{stamp}.{extension}"),
        prefix => format!("{prefix}_{stamp}.{extension}"),
    };
    (name != file).then_some(name)
}

/// Move `from` to `name` in the same folder, or `-2`, `-3`… when that is taken: a file is
/// never overwritten. Returns where it went.
fn rename_unique(from: &Path, name: &str) -> std::io::Result<PathBuf> {
    let target = from.with_file_name(name);
    let stem = target
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    let extension = target
        .extension()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    for n in 1u32.. {
        let candidate = match n {
            1 => target.clone(),
            n => target.with_file_name(format!("{stem}-{n}.{extension}")),
        };
        if !candidate.exists() {
            std::fs::rename(from, &candidate)?;
            return Ok(candidate);
        }
    }
    unreachable!("every name from -2 to -{} exists", u32::MAX)
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

fn read_thumbnail(path: &Path) -> Result<Frame, String> {
    let file = File::open(path).map_err(|e| e.to_string())?;
    let frame = cb_encode::read_png(std::io::BufReader::new(file)).map_err(|e| e.to_string())?;
    cb_encode::thumbnail(&frame, THUMBNAIL_PIXELS.0, THUMBNAIL_PIXELS.1).map_err(|e| e.to_string())
}

/// Write the PNGs in `files` as one PDF at `to`, reading each only for its page. A
/// failure leaves no half-written PDF behind.
fn export_pdf(files: &[PathBuf], to: &Path) -> Result<usize, String> {
    let pages = files.iter().map(|path| {
        let file = File::open(path)?;
        cb_encode::read_png(std::io::BufReader::new(file))
    });
    let out = File::create(to).map_err(|e| e.to_string())?;
    cb_encode::write_pdf(pages, std::io::BufWriter::new(out)).map_err(|e| {
        let _ = std::fs::remove_file(to);
        e.to_string()
    })
}

/// Copy `from` into `dir` under its own name, or `-2`, `-3`… when that is taken.
fn save_into(from: &Path, dir: &Path) -> std::io::Result<PathBuf> {
    let name = from.file_name().ok_or(ErrorKind::InvalidInput)?;
    let mut source = File::open(from)?;
    let (mut file, path) = create_unique(&dir.join(name))?;
    match std::io::copy(&mut source, &mut file) {
        Ok(_) => Ok(path),
        Err(e) => {
            drop(file);
            let _ = std::fs::remove_file(&path);
            Err(e)
        }
    }
}

/// Runs a job that works on existing files and says how it went.
fn one_off(clipboard: &mut Option<arboard::Clipboard>, job: Job) -> String {
    let file_name = |path: &Path| {
        path.file_name().map_or_else(
            || path.display().to_string(),
            |n| n.to_string_lossy().into_owned(),
        )
    };
    match job {
        Job::CopyFile(path) => {
            let copied = File::open(&path)
                .map_err(|e| e.to_string())
                .and_then(|file| {
                    cb_encode::read_png(std::io::BufReader::new(file)).map_err(|e| e.to_string())
                })
                .and_then(|frame| copy(clipboard, &frame));
            match copied {
                Ok(()) => "Copied".into(),
                Err(e) => format!("Could not copy: {e}"),
            }
        }
        Job::SaveAs { from, to } => match std::fs::copy(&from, &to) {
            Ok(_) => format!("Saved a copy as {}", file_name(&to)),
            Err(e) => format!("Could not save a copy: {e}"),
        },
        Job::Trash(paths) => {
            let mut failed = None;
            for path in &paths {
                if let Err(e) = cb_platform::move_to_trash(path) {
                    tracing::warn!(error = %e, ?path, "deleted capture stays on disk");
                    failed = Some(e);
                }
            }
            match (failed, paths.as_slice()) {
                (None, [one]) => format!("Deleted · {} is in the Recycle Bin", file_name(one)),
                (None, _) => format!("Deleted {} · the files are in the Recycle Bin", paths.len()),
                (Some(e), _) => format!("Removed from the buffer · some files stay: {e}"),
            }
        }
        Job::ExportPdf { files, to } => match export_pdf(&files, &to) {
            Ok(pages) => format!("Exported {pages} pages to {}", file_name(&to)),
            Err(e) => format!("Could not export the PDF: {e}"),
        },
        Job::CopyFiles(files) => {
            let copied = match clipboard {
                Some(clipboard) => clipboard.set().file_list(&files),
                None => arboard::Clipboard::new().and_then(|mut new| {
                    let result = new.set().file_list(&files);
                    *clipboard = Some(new);
                    result
                }),
            };
            match copied {
                Ok(()) => format!("Copied {} files", files.len()),
                Err(e) => format!("Could not copy: {e}"),
            }
        }
        Job::SaveAllTo { files, dir } => {
            let saved = files
                .iter()
                .filter(|from| save_into(from, &dir).is_ok())
                .count();
            match files.len() - saved {
                0 => format!("Saved {saved} copies in {}", file_name(&dir)),
                failed => format!(
                    "Saved {saved} copies in {}; {failed} failed",
                    file_name(&dir)
                ),
            }
        }
        Job::Store { .. } | Job::Copy(_) | Job::Thumbnail { .. } | Job::Rename { .. } => {
            unreachable!("not a one-off job")
        }
    }
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
                    Job::Rename { files, prefix } => {
                        let mut failed = 0;
                        for (item, path) in files {
                            let name = path
                                .file_name()
                                .and_then(|n| renamed(&n.to_string_lossy(), prefix));
                            let Some(name) = name else { continue };
                            match rename_unique(path, &name) {
                                Ok(path) => {
                                    let _ = done_tx.send(Done::Renamed { item: *item, path });
                                }
                                Err(e) => {
                                    tracing::warn!(error = %e, ?path, "not renamed");
                                    failed += 1;
                                }
                            }
                        }
                        let message = match failed {
                            0 => "Renamed".to_string(),
                            n => format!("Renamed, but {n} files could not be"),
                        };
                        let _ = done_tx.send(Done::Message(message));
                        wake();
                        continue;
                    }
                    Job::Thumbnail { item, path } => {
                        match read_thumbnail(path) {
                            Ok(thumbnail) => {
                                let item = *item;
                                let _ = done_tx.send(Done::Thumbnail { item, thumbnail });
                                wake();
                            }
                            Err(e) => tracing::warn!(error = %e, ?path, "no thumbnail"),
                        }
                        continue;
                    }
                    Job::CopyFile(_)
                    | Job::SaveAs { .. }
                    | Job::Trash(_)
                    | Job::ExportPdf { .. }
                    | Job::CopyFiles(_)
                    | Job::SaveAllTo { .. } => {
                        let _ = done_tx.send(Done::Message(one_off(&mut clipboard, job)));
                        wake();
                        continue;
                    }
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
    fn renaming_keeps_the_timestamp_and_drops_the_old_clash_number() {
        let stamp = "2026-11-08_10-12-03";
        assert_eq!(
            renamed(&format!("old_{stamp}.png"), "new"),
            Some(format!("new_{stamp}.png"))
        );
        assert_eq!(
            renamed(&format!("old_{stamp}-2.png"), "new"),
            Some(format!("new_{stamp}.png"))
        );
        assert_eq!(
            renamed(&format!("{stamp}.png"), "new"),
            Some(format!("new_{stamp}.png"))
        );
        assert_eq!(
            renamed(&format!("old_{stamp}.png"), " "),
            Some(format!("{stamp}.png"))
        );
        assert_eq!(
            renamed(&format!("a-b_{stamp}.png"), "été"),
            Some(format!("été_{stamp}.png"))
        );
        assert_eq!(
            renamed(&format!("new_{stamp}.png"), "new"),
            None,
            "already named so"
        );
        assert_eq!(renamed("holiday.png", "new"), None, "not one of ours");
        assert_eq!(renamed("é.png", "new"), None);
    }

    #[test]
    fn a_rename_never_overwrites() {
        let dir = std::env::temp_dir().join(format!("cb-rename-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let stamp = "2026-11-08_10-12-03";
        let (a, taken) = (
            dir.join(format!("old_{stamp}.png")),
            dir.join(format!("new_{stamp}.png")),
        );
        std::fs::write(&a, "a").unwrap();
        std::fs::write(&taken, "keep me").unwrap();

        let (jobs, done) = spawn(|| {});
        jobs.send(Job::Rename {
            files: vec![(ItemId(1), a.clone())],
            prefix: "new".into(),
        })
        .unwrap();
        let moved = done
            .iter()
            .find_map(|d| match d {
                Done::Renamed { item, path } => Some((item, path)),
                _ => None,
            })
            .unwrap();
        assert_eq!(moved, (ItemId(1), dir.join(format!("new_{stamp}-2.png"))));
        assert!(!a.exists());
        assert_eq!(std::fs::read_to_string(&taken).unwrap(), "keep me");
        assert_eq!(std::fs::read_to_string(&moved.1).unwrap(), "a");
        std::fs::remove_dir_all(dir).unwrap();
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
                Done::ClipboardFailed(_) | Done::Message(_) | Done::Renamed { .. } => None,
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

    /// Runs one job and returns the toast it ends with.
    fn message_of(job: Job) -> String {
        let (jobs, done) = spawn(|| {});
        jobs.send(job).unwrap();
        match done.recv().unwrap() {
            Done::Message(message) => message,
            _ => panic!("a one-off job answers with a message"),
        }
    }

    #[test]
    fn save_as_copies_the_file_and_says_so() {
        let dir = std::env::temp_dir().join(format!("cb-save-as-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (from, to) = (dir.join("a.png"), dir.join("b.png"));
        std::fs::write(&from, "pixels").unwrap();
        let message = message_of(Job::SaveAs {
            from: from.clone(),
            to: to.clone(),
        });
        assert_eq!(message, "Saved a copy as b.png");
        assert_eq!(std::fs::read_to_string(&to).unwrap(), "pixels");
        assert!(from.exists());

        let missing = message_of(Job::SaveAs {
            from: dir.join("gone.png"),
            to,
        });
        assert!(missing.starts_with("Could not save a copy"), "{missing}");
        let unreadable = message_of(Job::CopyFile(from));
        assert!(unreadable.starts_with("Could not copy"), "{unreadable}");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn export_pdf_makes_a_page_per_image_or_nothing_at_all() {
        let dir = std::env::temp_dir().join(format!("cb-pdf-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let png = |name: &str| {
            let path = dir.join(name);
            let file = File::create(&path).unwrap();
            cb_encode::write_png(&frame_at(SystemTime::now()), file).unwrap();
            path
        };
        let (a, b, to) = (png("a.png"), png("b.png"), dir.join("out.pdf"));
        let message = message_of(Job::ExportPdf {
            files: vec![a.clone(), b],
            to: to.clone(),
        });
        assert_eq!(message, "Exported 2 pages to out.pdf");
        assert!(std::fs::read(&to).unwrap().starts_with(b"%PDF"));

        let broken = dir.join("broken.pdf");
        let message = message_of(Job::ExportPdf {
            files: vec![a, dir.join("gone.png")],
            to: broken.clone(),
        });
        assert!(message.starts_with("Could not export"), "{message}");
        assert!(!broken.exists(), "no half-written PDF");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn save_all_copies_into_the_folder_without_overwriting() {
        let dir = std::env::temp_dir().join(format!("cb-save-all-test-{}", std::process::id()));
        let into = dir.join("picked");
        std::fs::create_dir_all(&into).unwrap();
        let (a, b) = (dir.join("a.png"), dir.join("b.png"));
        std::fs::write(&a, "a").unwrap();
        std::fs::write(&b, "b").unwrap();
        std::fs::write(into.join("a.png"), "older").unwrap();

        let message = message_of(Job::SaveAllTo {
            files: vec![a, b, dir.join("gone.png")],
            dir: into.clone(),
        });
        assert_eq!(message, "Saved 2 copies in picked; 1 failed");
        let read = |name: &str| std::fs::read_to_string(into.join(name)).unwrap();
        assert_eq!(
            (read("a.png"), read("a-2.png"), read("b.png")),
            ("older".into(), "a".into(), "b".into())
        );
        std::fs::remove_dir_all(dir).unwrap();
    }
}
