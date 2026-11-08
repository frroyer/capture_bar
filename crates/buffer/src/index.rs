//! The buffer's index on disk (FR-25, NFR-12). Every change rewrites it whole: a temporary
//! file is written and synced, then renamed over the old one, so a crash or a kill at any
//! moment leaves either the previous index or the new one, never half of one.

use crate::BufferItem;
use serde::{Deserialize, Serialize};
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc;

/// The index file name inside the buffer folder.
pub const INDEX_FILE: &str = "index.ron";

#[derive(Serialize, Deserialize)]
struct Index {
    /// Raised when the format changes in a way older builds cannot read.
    version: u32,
    items: Vec<BufferItem>,
}

const VERSION: u32 = 1;

/// Write `items` to `path` atomically.
pub fn write(path: &Path, items: &[BufferItem]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let index = Index {
        version: VERSION,
        items: items.to_vec(),
    };
    let text = ron::ser::to_string_pretty(&index, ron::ser::PrettyConfig::default())
        .map_err(|e| std::io::Error::new(ErrorKind::InvalidData, e))?;
    let temporary = path.with_extension("tmp");
    let mut file = std::fs::File::create(&temporary)?;
    file.write_all(text.as_bytes())?;
    file.sync_all()?;
    drop(file);
    std::fs::rename(&temporary, path)
}

/// The items in the index at `path`; none when there is no index yet.
pub fn read(path: &Path) -> std::io::Result<Vec<BufferItem>> {
    let text = match std::fs::read_to_string(path) {
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
        read => read?,
    };
    let index: Index =
        ron::from_str(&text).map_err(|e| std::io::Error::new(ErrorKind::InvalidData, e))?;
    if index.version > VERSION {
        return Err(std::io::Error::new(
            ErrorKind::InvalidData,
            format!("index version {} is newer than this build", index.version),
        ));
    }
    Ok(index.items)
}

/// A thread that writes each snapshot sent to it to `path`. When several are waiting, only
/// the newest is written. `failed` hears about each write that did not make it.
pub fn spawn_writer(
    path: PathBuf,
    failed: impl Fn(std::io::Error) + Send + 'static,
) -> mpsc::Sender<Vec<BufferItem>> {
    let (tx, rx) = mpsc::channel::<Vec<BufferItem>>();
    std::thread::Builder::new()
        .name("buffer index".into())
        .spawn(move || {
            while let Ok(mut items) = rx.recv() {
                while let Ok(newer) = rx.try_recv() {
                    items = newer;
                }
                if let Err(e) = write(&path, &items) {
                    failed(e);
                }
            }
        })
        .expect("spawn buffer index thread");
    tx
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Buffer, ItemKind};
    use cb_core::CaptureMode;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("cb-index-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn what_the_writer_wrote_is_read_back_after_a_crash() {
        let dir = temp_dir("crash");
        let path = dir.join(".buffer").join(INDEX_FILE);
        assert!(read(&path).unwrap().is_empty(), "no index yet: nothing");

        let mut buffer = Buffer::new();
        let id = buffer.push(ItemKind::Image, CaptureMode::Window);
        let item = buffer.get_mut(id).unwrap();
        item.path = Some(dir.join("a.png"));
        item.pinned = true;
        let writer = spawn_writer(path.clone(), |e| panic!("{e}"));
        writer.send(buffer.snapshot()).unwrap();
        // The thread is never told to stop, as with a kill: the index is still whole.
        drop(writer);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let items = loop {
            let items = read(&path).unwrap();
            if !items.is_empty() || std::time::Instant::now() > deadline {
                break items;
            }
            std::thread::yield_now();
        };
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].id, id);
        assert!(items[0].pinned);
        assert!(!path.with_extension("tmp").exists());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_damaged_index_is_an_error_not_an_empty_buffer() {
        let dir = temp_dir("damaged");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(INDEX_FILE);
        std::fs::write(&path, "(version: 1, items: [(id: ").unwrap();
        assert!(read(&path).is_err());
        std::fs::write(&path, "(version: 99, items: [])").unwrap();
        assert!(read(&path).is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
