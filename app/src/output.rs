//! Where finished frames go: the clipboard and a PNG file, on a worker thread, so the
//! UI thread never waits for them (FR-12, FR-20).

use std::fs::File;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::Arc;
use std::time::SystemTime;

use cb_buffer::ItemId;
use cb_core::{create_unique, Frame};
use cb_ui::tr;
use chrono::{DateTime, Local};

/// Twice the tray's card size in points, to stay sharp on HiDPI screens.
const THUMBNAIL_PIXELS: (u32, u32) = (
    cb_ui::THUMBNAIL_SIZE.x as u32 * 2,
    cb_ui::THUMBNAIL_SIZE.y as u32 * 2,
);

/// Frames in a recording's hover preview: 3 s at the tray's 4 per second.
const PREVIEW_FRAMES: u32 = 12;

/// A GIF's frame rate and largest size: 10 s of it fit in 15 MB (US-6.7).
const GIF_FPS: u32 = 15;
const GIF_PIXELS: (u32, u32) = (1280, 720);

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
    /// Decode a recording's thumbnail and hover preview frames.
    Preview {
        item: ItemId,
        path: PathBuf,
    },
    /// Open a recording in the system's video player.
    Play(PathBuf),
    /// Make the thumbnail of a capture restored from the last run (FR-25).
    Thumbnail {
        item: ItemId,
        path: PathBuf,
    },
    /// Read a capture's image, for the annotation editor (US-7.1).
    Load {
        item: ItemId,
        path: PathBuf,
    },
    /// Draw annotations into `base`, the capture as taken, and write the result over the
    /// capture's file, then make its thumbnail and copy it, as for a new capture (US-7.1).
    /// The file as taken is kept first, in `originals`, unless `original` already holds it.
    /// No shapes puts the original back in place of the annotated file.
    Annotate {
        item: ItemId,
        base: Arc<Frame>,
        shapes: Vec<cb_annotate::Shape>,
        font: Arc<[u8]>,
        path: PathBuf,
        original: Option<PathBuf>,
        originals: PathBuf,
    },
    /// Delete files nobody will see again, such as the original of a capture that left the
    /// buffer. Says nothing.
    Remove(PathBuf),
    /// Write `bytes` to `path`, making its folder, such as a demo session's manifest
    /// (US-10.1). Says nothing.
    Write {
        path: PathBuf,
        bytes: Vec<u8>,
    },
    /// Find a demo session's claps in its `video` and write the drift report to `to`
    /// (US-10.2). Says how it went.
    ClapReport(PathBuf),
    /// Find the face in a demo session's webcam track and write `webcam.json` (US-10.6).
    WebcamLog(crate::demo::WebcamLog),
    /// Make a GIF of the recording at `from`, next to it (US-6.7). Exports queue and run
    /// one at a time, in order, reporting progress (US-6.2).
    ExportGif {
        item: ItemId,
        from: PathBuf,
    },
}

pub enum Done {
    /// How far a recording's GIF export is, in percent; `None` once it is over.
    Exporting {
        item: ItemId,
        percent: Option<u8>,
    },
    /// A recording's preview, thumbnail-sized, the first frame its thumbnail.
    Preview {
        item: ItemId,
        frames: Vec<Frame>,
    },
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
    Loaded {
        item: ItemId,
        result: Result<Frame, String>,
    },
    /// The capture's file now holds `shapes` over its original.
    Annotated {
        item: ItemId,
        shapes: Vec<cb_annotate::Shape>,
        result: Result<Annotated, String>,
    },
}

pub struct Annotated {
    /// What the capture's file holds now.
    pub frame: Arc<Frame>,
    /// Where the capture as taken is kept, while it has annotations.
    pub original: Option<PathBuf>,
}

/// Run an [`Job::Annotate`]. On failure the capture's file and its original are as they were.
fn annotate(
    base: &Arc<Frame>,
    shapes: &[cb_annotate::Shape],
    font: &[u8],
    path: &Path,
    original: Option<&Path>,
    originals: &Path,
) -> Result<Annotated, String> {
    if shapes.is_empty() {
        if let Some(original) = original {
            std::fs::rename(original, path).map_err(|e| e.to_string())?;
        }
        return Ok(Annotated {
            frame: base.clone(),
            original: None,
        });
    }
    let frame = cb_annotate::flatten(base, shapes, font).map_err(|e| e.to_string())?;
    let (original, kept_now) = match original {
        Some(original) => (original.to_path_buf(), false),
        None => {
            std::fs::create_dir_all(originals).map_err(|e| e.to_string())?;
            let kept = save_into(path, originals).map_err(|e| e.to_string())?;
            (kept, true)
        }
    };
    if let Err(e) = overwrite_png(&frame, path) {
        if kept_now {
            let _ = std::fs::remove_file(&original);
        }
        return Err(e);
    }
    Ok(Annotated {
        frame: Arc::new(frame),
        original: Some(original),
    })
}

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

/// Replace the file at `path` with `frame`, all at once: a failure leaves the old file whole.
fn overwrite_png(frame: &Frame, path: &Path) -> Result<(), String> {
    let mut temporary = path.as_os_str().to_owned();
    temporary.push(".tmp");
    let temporary = PathBuf::from(temporary);
    let written = File::create(&temporary)
        .map_err(|e| e.to_string())
        .and_then(|file| {
            cb_encode::write_png(frame, std::io::BufWriter::new(file)).map_err(|e| e.to_string())
        })
        .and_then(|()| std::fs::rename(&temporary, path).map_err(|e| e.to_string()));
    if written.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    written
}

fn read_image(path: &Path) -> Result<Frame, String> {
    let file = File::open(path).map_err(|e| e.to_string())?;
    cb_encode::read_png(std::io::BufReader::new(file)).map_err(|e| e.to_string())
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
    let frame = read_image(path)?;
    cb_encode::thumbnail(&frame, THUMBNAIL_PIXELS.0, THUMBNAIL_PIXELS.1).map_err(|e| e.to_string())
}

/// A recording's hover preview, thumbnail-sized; empty if it cannot be decoded.
fn preview(path: &Path) -> Vec<Frame> {
    let mut frames = Vec::new();
    let decoded = cb_platform::video_frames(path, PREVIEW_FRAMES, &mut |frame| {
        let (w, h) = THUMBNAIL_PIXELS;
        match cb_encode::thumbnail(&frame, w, h) {
            Ok(small) => frames.push(small),
            Err(e) => tracing::warn!(error = %e, "preview frame"),
        }
    });
    if let Err(e) = decoded {
        tracing::warn!(error = %e, ?path, "no preview");
    }
    frames
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

/// Write the recording at `from` as a GIF beside it, never overwriting, telling `progress`
/// each new percent. Decodes on a second thread while this one encodes. A failure leaves
/// no half-written GIF behind. Returns where it went.
fn export_gif(from: &Path, progress: &mut dyn FnMut(u8)) -> Result<PathBuf, String> {
    let (file, to) = create_unique(&from.with_extension("gif")).map_err(|e| e.to_string())?;
    let (frames_tx, frames) = mpsc::sync_channel::<(Frame, f32)>(2);
    let (decoded, written) = std::thread::scope(|scope| {
        let decoder = scope.spawn(move || {
            cb_platform::video_clip(from, GIF_FPS, &mut |frame, done| {
                let frame = cb_encode::thumbnail(&frame, GIF_PIXELS.0, GIF_PIXELS.1);
                // Stop when the encoder has given up.
                frame.is_ok_and(|frame| frames_tx.send((frame, done)).is_ok())
            })
        });
        let mut shown = None;
        let frames = frames.iter().map(|(frame, done)| {
            let percent = (done * 100.0) as u8;
            if shown != Some(percent) {
                shown = Some(percent);
                progress(percent);
            }
            frame
        });
        let written = cb_encode::write_gif(frames, GIF_FPS, std::io::BufWriter::new(file));
        (decoder.join().expect("GIF decoder panicked"), written)
    });
    match decoded
        .map_err(|e| e.to_string())
        .and(written.map_err(|e| e.to_string()))
    {
        Ok(_) => Ok(to),
        Err(e) => {
            let _ = std::fs::remove_file(&to);
            Err(e)
        }
    }
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
/// The last part of `path`, to name it in a toast.
fn file_name_of(path: &Path) -> String {
    path.file_name().map_or_else(
        || path.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    )
}

fn one_off(clipboard: &mut Option<arboard::Clipboard>, job: Job) -> String {
    let file_name = file_name_of;
    match job {
        Job::CopyFile(path) => {
            let copied = File::open(&path)
                .map_err(|e| e.to_string())
                .and_then(|file| {
                    cb_encode::read_png(std::io::BufReader::new(file)).map_err(|e| e.to_string())
                })
                .and_then(|frame| copy(clipboard, &frame));
            match copied {
                Ok(()) => tr!("toast-copied"),
                Err(e) => tr!("toast-copy-failed", error = e.to_string()),
            }
        }
        Job::SaveAs { from, to } => match std::fs::copy(&from, &to) {
            Ok(_) => tr!("toast-saved-copy", file = file_name(&to)),
            Err(e) => tr!("toast-save-copy-failed", error = e.to_string()),
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
                (None, [one]) => tr!("toast-trashed-one", file = file_name(one)),
                (None, _) => tr!("toast-trashed", count = paths.len()),
                (Some(e), _) => tr!("toast-trash-failed", error = e.to_string()),
            }
        }
        Job::ExportPdf { files, to } => match export_pdf(&files, &to) {
            Ok(pages) => tr!("toast-exported", count = pages, file = file_name(&to)),
            Err(e) => tr!("toast-export-failed", error = e.to_string()),
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
                Ok(()) => tr!("toast-copied-files", count = files.len()),
                Err(e) => tr!("toast-copy-failed", error = e.to_string()),
            }
        }
        Job::SaveAllTo { files, dir } => {
            let saved = files
                .iter()
                .filter(|from| save_into(from, &dir).is_ok())
                .count();
            match files.len() - saved {
                0 => tr!("toast-saved-all", count = saved, folder = file_name(&dir)),
                failed => tr!(
                    "toast-saved-some",
                    count = saved,
                    folder = file_name(&dir),
                    failed = failed,
                ),
            }
        }
        Job::Store { .. }
        | Job::Copy(_)
        | Job::Thumbnail { .. }
        | Job::Preview { .. }
        | Job::Play(_)
        | Job::Rename { .. }
        | Job::Load { .. }
        | Job::Annotate { .. }
        | Job::Remove(_)
        | Job::Write { .. }
        | Job::ClapReport(_)
        | Job::WebcamLog(_)
        | Job::ExportGif { .. } => {
            unreachable!("not a one-off job")
        }
    }
}

/// Measure the clap test of the demo package `dir` (US-10.2), and say how it went.
pub fn clap_message(dir: &Path) -> String {
    match crate::demo::ClapReport::measure(dir) {
        Ok(report) => {
            let (video, sound) = report.drifts();
            let ms = |v: Option<i64>| v.map_or_else(|| "?".to_owned(), |v| v.to_string());
            let (video, sound) = (ms(video), ms(sound));
            if report.passed() {
                tr!("toast-clap-passed", video = video, sound = sound)
            } else {
                tr!("toast-clap-failed", video = video, sound = sound)
            }
        }
        Err(e) => tr!("toast-clap-error", error = e),
    }
}

/// `wake` runs after each finished job, to get the UI to read the result.
pub fn spawn(wake: impl Fn() + Send + Sync + 'static) -> (mpsc::Sender<Job>, mpsc::Receiver<Done>) {
    let (job_tx, jobs) = mpsc::channel::<Job>();
    let (done_tx, done) = mpsc::channel();
    let wake = Arc::new(wake);
    // Decoding a recording takes seconds: on its own thread, saves never wait for it.
    let (preview_tx, previews) = mpsc::channel::<(ItemId, PathBuf)>();
    let (preview_done, preview_wake) = (done_tx.clone(), wake.clone());
    std::thread::Builder::new()
        .name("preview".into())
        .spawn(move || {
            for (item, path) in previews {
                let frames = preview(&path);
                if !frames.is_empty() {
                    let _ = preview_done.send(Done::Preview { item, frames });
                    preview_wake();
                }
            }
        })
        .expect("spawn preview thread");
    // A GIF takes as long as its clip or more: on its own thread too, one at a time so
    // they finish in the order asked (US-6.2).
    let (export_tx, exports) = mpsc::channel::<(ItemId, PathBuf)>();
    let (export_done, export_wake) = (done_tx.clone(), wake.clone());
    std::thread::Builder::new()
        .name("export".into())
        .spawn(move || {
            for (item, from) in exports {
                let send = |percent| {
                    let _ = export_done.send(Done::Exporting { item, percent });
                    export_wake();
                };
                let result = export_gif(&from, &mut |percent| send(Some(percent)));
                let message = match result {
                    Ok(to) => tr!("toast-gif-exported", file = file_name_of(&to)),
                    Err(e) => {
                        tracing::warn!(error = %e, ?from, "GIF export");
                        tr!("toast-gif-failed", error = e)
                    }
                };
                let _ = export_done.send(Done::Message(message));
                send(None);
            }
        })
        .expect("spawn export thread");
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
                            0 => tr!("toast-renamed"),
                            n => tr!("toast-renamed-some", count = n),
                        };
                        let _ = done_tx.send(Done::Message(message));
                        wake();
                        continue;
                    }
                    Job::Preview { item, path } => {
                        let _ = preview_tx.send((*item, path.clone()));
                        continue;
                    }
                    Job::ExportGif { item, from } => {
                        let _ = export_tx.send((*item, from.clone()));
                        continue;
                    }
                    Job::Play(path) => {
                        if let Err(e) = cb_platform::open_file(path) {
                            tracing::warn!(error = %e, ?path, "play");
                            let message = tr!("toast-play-failed", error = e.to_string());
                            let _ = done_tx.send(Done::Message(message));
                            wake();
                        }
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
                    Job::Load { item, path } => {
                        let result = read_image(path);
                        let _ = done_tx.send(Done::Loaded {
                            item: *item,
                            result,
                        });
                        wake();
                        continue;
                    }
                    Job::Remove(path) => {
                        if let Err(e) = std::fs::remove_file(path) {
                            tracing::warn!(error = %e, ?path, "not removed");
                        }
                        continue;
                    }
                    Job::Write { path, bytes } => {
                        let written = path
                            .parent()
                            .map_or(Ok(()), std::fs::create_dir_all)
                            .and_then(|()| std::fs::write(path, bytes));
                        if let Err(e) = written {
                            tracing::warn!(error = %e, ?path, "not written");
                        }
                        continue;
                    }
                    Job::Annotate {
                        item,
                        base,
                        shapes,
                        font,
                        path,
                        original,
                        originals,
                    } => {
                        let result =
                            annotate(base, shapes, font, path, original.as_deref(), originals);
                        if let Ok(Annotated { frame, .. }) = &result {
                            if let Ok(thumbnail) =
                                cb_encode::thumbnail(frame, THUMBNAIL_PIXELS.0, THUMBNAIL_PIXELS.1)
                            {
                                let _ = done_tx.send(Done::Thumbnail {
                                    item: *item,
                                    thumbnail,
                                });
                            }
                            if let Err(e) = copy(&mut clipboard, frame) {
                                let _ = done_tx.send(Done::ClipboardFailed(e));
                            }
                        }
                        let _ = done_tx.send(Done::Annotated {
                            item: *item,
                            shapes: shapes.clone(),
                            result,
                        });
                        wake();
                        continue;
                    }
                    Job::ClapReport(dir) => {
                        // Minutes for a long session: on its own thread, not in the queue.
                        let (dir, done, wake) = (dir.clone(), done_tx.clone(), wake.clone());
                        let _ = std::thread::Builder::new()
                            .name("clap report".into())
                            .spawn(move || {
                                let _ = done.send(Done::Message(clap_message(&dir)));
                                wake();
                            });
                        continue;
                    }
                    Job::WebcamLog(log) => {
                        // Decodes the whole track: on its own thread, not in the queue.
                        let log = log.clone();
                        let _ = std::thread::Builder::new().name("webcam log".into()).spawn(
                            move || {
                                if let Err(error) = log.write() {
                                    tracing::warn!(%error, "webcam.json");
                                }
                            },
                        );
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
                Done::ClipboardFailed(_)
                | Done::Message(_)
                | Done::Preview { .. }
                | Done::Renamed { .. }
                | Done::Loaded { .. }
                | Done::Annotated { .. }
                | Done::Exporting { .. } => None,
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

    /// Runs an annotation job and returns what it answered with the capture's file then.
    fn annotate_job(
        base: &Arc<Frame>,
        shapes: Vec<cb_annotate::Shape>,
        path: &Path,
        original: Option<PathBuf>,
        originals: &Path,
    ) -> (Option<PathBuf>, Vec<u8>) {
        let (jobs, done) = spawn(|| {});
        jobs.send(Job::Annotate {
            item: ItemId(7),
            base: base.clone(),
            shapes,
            font: cb_annotate::fallback_font().into(),
            path: path.to_path_buf(),
            original,
            originals: originals.to_path_buf(),
        })
        .unwrap();
        let annotated = done
            .iter()
            .find_map(|d| match d {
                Done::Annotated { item, result, .. } => {
                    assert_eq!(item, ItemId(7));
                    Some(result.unwrap())
                }
                _ => None,
            })
            .unwrap();
        jobs.send(Job::Load {
            item: ItemId(7),
            path: path.to_path_buf(),
        })
        .unwrap();
        let on_disk = done
            .iter()
            .find_map(|d| match d {
                Done::Loaded { result, .. } => Some(result.unwrap().rgba),
                _ => None,
            })
            .unwrap();
        assert_eq!(
            annotated.frame.rgba, on_disk,
            "what it says is what the file holds"
        );
        (annotated.original, on_disk)
    }

    #[test]
    fn annotating_keeps_the_original_out_of_sight_until_it_is_put_back() {
        let dir = std::env::temp_dir().join(format!("cb-annotate-test-{}", std::process::id()));
        let originals = dir.join(".buffer").join("originals");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("a.png");
        let mut base = frame_at(SystemTime::now());
        base.rgba = [0, 0, 0, 255].repeat(2);
        let base = Arc::new(base);
        overwrite_png(&base, &path).unwrap();
        let pixel = |x: f32, color: [u8; 4]| cb_annotate::Shape::Highlight {
            min: cb_annotate::Point::new(x, 0.0),
            max: cb_annotate::Point::new(x + 1.0, 1.0),
            color: cb_annotate::Rgba(color),
        };
        let files = |dir: &Path| std::fs::read_dir(dir).unwrap().count();

        let (original, rgba) = annotate_job(
            &base,
            vec![pixel(0.0, [255, 0, 0, 255])],
            &path,
            None,
            &originals,
        );
        assert_eq!(rgba, [255, 0, 0, 255, 0, 0, 0, 255]);
        let original = original.expect("kept");
        assert_eq!(original.parent(), Some(originals.as_path()));
        assert_eq!(read_image(&original).unwrap().rgba, base.rgba, "as taken");
        assert_eq!(files(&dir), 2, "the capture and .buffer: no temporary left");

        // Edited again: the shapes go over the original, which is kept, not copied again.
        let shapes = vec![pixel(1.0, [0, 0, 255, 255])];
        let again = annotate_job(&base, shapes, &path, Some(original.clone()), &originals);
        assert_eq!(
            again,
            (Some(original.clone()), vec![0, 0, 0, 255, 0, 0, 255, 255])
        );
        assert_eq!(files(&originals), 1);

        // Every annotation undone: the original takes its place again.
        let back = annotate_job(&base, Vec::new(), &path, Some(original), &originals);
        assert_eq!(back, (None, base.rgba.clone()));
        assert_eq!(files(&originals), 0);
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
    fn a_failed_gif_export_leaves_no_file() {
        let dir = std::env::temp_dir().join(format!("cb-gif-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let clip = dir.join("not-a-video.mp4");
        std::fs::write(&clip, b"nothing to decode").unwrap();
        let mut seen = Vec::new();
        assert!(export_gif(&clip, &mut |percent| seen.push(percent)).is_err());
        assert!(!dir.join("not-a-video.gif").exists());
        assert!(seen.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
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
