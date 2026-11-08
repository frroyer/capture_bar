//! The capture buffer (Epic E6). Appending must never block the caller
//! (FR-20, NFR-4). The list lives in memory; [`index`] keeps a copy on disk so it
//! survives a crash or a restart (FR-25, US-6.6).

pub mod index;

use cb_core::CaptureMode;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ItemId(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ItemKind {
    Image,
    Video { duration_ms: u64 },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ItemState {
    /// Frames received, file still being written (FR-21).
    Encoding {
        progress_pct: u8,
    },
    Ready,
    Failed(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BufferItem {
    pub id: ItemId,
    pub kind: ItemKind,
    pub mode: CaptureMode,
    pub created_at: SystemTime,
    pub state: ItemState,
    pub path: Option<PathBuf>,
    pub pinned: bool,
    /// The capture as taken, kept out of sight once it is annotated, so the annotations
    /// can be changed or taken back later (US-7.1). `path` holds the annotated picture.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub original: Option<PathBuf>,
    /// The annotations drawn into `path` over `original`, bottom first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub annotations: Vec<cb_annotate::Shape>,
}

#[derive(Debug, Default)]
pub struct Buffer {
    items: Vec<BufferItem>,
    next_id: u64,
    /// Goes up with every change, to tell when the index on disk is out of date.
    generation: u64,
}

impl Buffer {
    pub fn new() -> Self {
        Self::default()
    }

    /// The buffer from a previous run, as its index left it. Items whose file is gone
    /// (moved or deleted outside the app) are left out.
    pub fn restore(items: Vec<BufferItem>) -> Self {
        let mut items: Vec<_> = items
            .into_iter()
            .filter(|i| {
                i.state == ItemState::Ready && i.path.as_deref().is_some_and(|p| p.is_file())
            })
            .collect();
        for item in &mut items {
            // Without the original, the annotations are only in the picture: nothing to edit.
            if item.original.as_deref().is_some_and(|p| !p.is_file()) {
                item.original = None;
                item.annotations.clear();
            }
        }
        items.sort_by_key(|i| i.id);
        let next_id = items.last().map_or(0, |i| i.id.0);
        Self {
            items,
            next_id,
            generation: 0,
        }
    }

    /// What the index on disk keeps: the items whose file was written.
    pub fn snapshot(&self) -> Vec<BufferItem> {
        self.items
            .iter()
            .filter(|i| i.state == ItemState::Ready && i.path.is_some())
            .cloned()
            .collect()
    }

    /// Changes whenever the buffer may have changed.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Unpinned items older than `keep` at `now`, to take out of the buffer (FR-25).
    pub fn expired(&self, now: SystemTime, keep: Duration) -> Vec<ItemId> {
        self.items
            .iter()
            .filter(|i| !i.pinned)
            .filter(|i| now.duration_since(i.created_at).is_ok_and(|age| age > keep))
            .map(|i| i.id)
            .collect()
    }

    /// Append a new item and return its id. Newest items are last internally;
    /// use [`Buffer::newest_first`] for display (FR-22).
    pub fn push(&mut self, kind: ItemKind, mode: CaptureMode) -> ItemId {
        self.generation += 1;
        self.next_id += 1;
        let id = ItemId(self.next_id);
        let state = match kind {
            ItemKind::Image => ItemState::Ready,
            ItemKind::Video { .. } => ItemState::Encoding { progress_pct: 0 },
        };
        self.items.push(BufferItem {
            id,
            kind,
            mode,
            created_at: SystemTime::now(),
            state,
            path: None,
            pinned: false,
            original: None,
            annotations: Vec::new(),
        });
        id
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn get(&self, id: ItemId) -> Option<&BufferItem> {
        self.items.iter().find(|i| i.id == id)
    }

    pub fn get_mut(&mut self, id: ItemId) -> Option<&mut BufferItem> {
        self.generation += 1;
        self.items.iter_mut().find(|i| i.id == id)
    }

    pub fn newest_first(&self) -> impl Iterator<Item = &BufferItem> {
        self.items.iter().rev()
    }

    pub fn newest(&self) -> Option<&BufferItem> {
        self.items.last()
    }

    pub fn remove(&mut self, id: ItemId) -> Option<BufferItem> {
        let idx = self.items.iter().position(|i| i.id == id)?;
        self.generation += 1;
        Some(self.items.remove(idx))
    }

    /// Clear everything except pinned items (FR-24, FR-25).
    pub fn clear_unpinned(&mut self) {
        self.generation += 1;
        self.items.retain(|i| i.pinned);
    }

    /// What a batch action covers, newest first: the selected items, or all of them when
    /// none is selected (FR-24).
    pub fn targets(&self, selection: &Selection) -> Vec<ItemId> {
        self.newest_first()
            .map(|i| i.id)
            .filter(|id| selection.is_empty() || selection.contains(*id))
            .collect()
    }
}

/// The captures picked in the tray for a batch action (FR-24).
#[derive(Debug, Default, Clone)]
pub struct Selection {
    ids: BTreeSet<ItemId>,
    /// Where a shift-click range starts: the last item clicked.
    anchor: Option<ItemId>,
}

impl Selection {
    pub fn contains(&self, id: ItemId) -> bool {
        self.ids.contains(&id)
    }

    pub fn len(&self) -> usize {
        self.ids.len()
    }

    pub fn is_empty(&self) -> bool {
        self.ids.is_empty()
    }

    pub fn clear(&mut self) {
        self.ids.clear();
        self.anchor = None;
    }

    pub fn remove(&mut self, id: ItemId) {
        self.ids.remove(&id);
        if self.anchor == Some(id) {
            self.anchor = None;
        }
    }

    /// A plain click: this item alone, or nothing when it already was alone.
    pub fn click(&mut self, id: ItemId) {
        let alone = self.ids.len() == 1 && self.contains(id);
        self.clear();
        if !alone {
            self.ids.insert(id);
            self.anchor = Some(id);
        }
    }

    /// Ctrl-click (Cmd on macOS): add or remove this item.
    pub fn toggle(&mut self, id: ItemId) {
        if !self.ids.remove(&id) {
            self.ids.insert(id);
        }
        self.anchor = Some(id);
    }

    /// Shift-click: add every item from the last one clicked to `id`, in `order`, the
    /// order the tray shows them in.
    pub fn extend_to(&mut self, id: ItemId, order: &[ItemId]) {
        let anchor = self.anchor.unwrap_or(id);
        let at = |wanted| order.iter().position(|&i| i == wanted);
        match (at(anchor), at(id)) {
            (Some(a), Some(b)) => self.ids.extend(&order[a.min(b)..=a.max(b)]),
            _ => {
                self.ids.insert(id);
            }
        }
        self.anchor = Some(id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_is_ordered_and_newest_first() {
        let mut b = Buffer::new();
        let a = b.push(ItemKind::Image, CaptureMode::Region);
        let c = b.push(ItemKind::Video { duration_ms: 9_000 }, CaptureMode::Screen);
        assert_eq!(b.len(), 2);
        let ids: Vec<_> = b.newest_first().map(|i| i.id).collect();
        assert_eq!(ids, vec![c, a]);
        assert!(matches!(
            b.get(c).unwrap().state,
            ItemState::Encoding { .. }
        ));
    }

    #[test]
    fn clear_keeps_pinned() {
        let mut b = Buffer::new();
        let a = b.push(ItemKind::Image, CaptureMode::Region);
        b.push(ItemKind::Image, CaptureMode::Window);
        b.get_mut(a).unwrap().pinned = true;
        b.clear_unpinned();
        assert_eq!(b.len(), 1);
        assert_eq!(b.newest().unwrap().id, a);
    }

    #[test]
    fn batch_targets_are_the_selection_or_everything() {
        let mut b = Buffer::new();
        let ids: Vec<_> = (0..4)
            .map(|_| b.push(ItemKind::Image, CaptureMode::Region))
            .collect();
        let order: Vec<_> = b.newest_first().map(|i| i.id).collect();
        let mut s = Selection::default();
        assert_eq!(b.targets(&s), order, "nothing selected: all, newest first");

        s.click(ids[0]);
        assert_eq!(b.targets(&s), vec![ids[0]]);
        s.click(ids[0]);
        assert!(
            s.is_empty(),
            "a click on the only selected item deselects it"
        );

        s.click(ids[3]);
        s.toggle(ids[0]);
        assert_eq!(b.targets(&s), vec![ids[3], ids[0]]);
        s.toggle(ids[0]);
        assert_eq!(b.targets(&s), vec![ids[3]]);

        // Shift from the last item clicked (the oldest) to the third shown: a range in tray order.
        s.extend_to(ids[1], &order);
        assert_eq!(b.targets(&s), vec![ids[3], ids[1], ids[0]]);

        s.click(ids[1]);
        assert_eq!(s.len(), 1, "a plain click starts over");
        b.remove(ids[1]);
        s.remove(ids[1]);
        assert_eq!(
            b.targets(&s).len(),
            3,
            "an emptied selection covers everything left"
        );
    }

    #[test]
    fn a_restored_buffer_keeps_items_whose_file_is_there() {
        let dir = std::env::temp_dir().join(format!("cb-restore-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut b = Buffer::new();
        let ids: Vec<_> = (0..3)
            .map(|n| {
                let id = b.push(ItemKind::Image, CaptureMode::Region);
                let path = dir.join(format!("{n}.png"));
                if n != 1 {
                    std::fs::write(&path, "png").unwrap();
                }
                b.get_mut(id).unwrap().path = Some(path);
                id
            })
            .collect();
        let saving = b.push(ItemKind::Image, CaptureMode::Region);
        b.get_mut(saving).unwrap().state = ItemState::Encoding { progress_pct: 0 };
        assert_eq!(b.snapshot().len(), 3, "not the one still being written");

        let mut back = Buffer::restore(b.snapshot());
        let kept: Vec<_> = back.newest_first().map(|i| i.id).collect();
        assert_eq!(kept, vec![ids[2], ids[0]], "1.png was deleted meanwhile");
        let next = back.push(ItemKind::Image, CaptureMode::Region);
        assert!(next > ids[2], "ids go on from the newest");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn annotations_come_back_only_with_their_original() {
        let dir = std::env::temp_dir().join(format!("cb-buffer-original-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let shape = cb_annotate::Shape::Text {
            at: cb_annotate::Point::new(1.0, 2.0),
            text: "x".into(),
            color: cb_annotate::Rgba([1, 2, 3, 255]),
            size: 12.0,
        };
        let mut b = Buffer::new();
        let ids: Vec<_> = ["kept", "lost"]
            .iter()
            .map(|name| {
                let id = b.push(ItemKind::Image, CaptureMode::Region);
                let path = dir.join(format!("{name}.png"));
                std::fs::write(&path, "png").unwrap();
                let item = b.get_mut(id).unwrap();
                item.path = Some(path);
                item.original = Some(dir.join(format!("{name}-original.png")));
                item.annotations = vec![shape.clone()];
                id
            })
            .collect();
        std::fs::write(dir.join("kept-original.png"), "png").unwrap();

        let index = dir.join(index::INDEX_FILE);
        index::write(&index, &b.snapshot()).unwrap();
        let back = Buffer::restore(index::read(&index).unwrap());
        let kept = back.get(ids[0]).unwrap();
        assert_eq!(kept.annotations, vec![shape]);
        assert!(kept.original.is_some());
        let lost = back.get(ids[1]).unwrap();
        assert!(lost.original.is_none() && lost.annotations.is_empty());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn an_index_from_before_annotations_still_reads() {
        let dir = std::env::temp_dir().join(format!("cb-buffer-v1-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let index = dir.join(index::INDEX_FILE);
        let mut b = Buffer::new();
        let id = b.push(ItemKind::Image, CaptureMode::Region);
        b.get_mut(id).unwrap().path = Some(dir.join("a.png"));
        index::write(&index, &b.snapshot()).unwrap();
        let text = std::fs::read_to_string(&index).unwrap();
        assert!(
            !text.contains("annotations"),
            "unannotated items keep the old form"
        );
        assert_eq!(index::read(&index).unwrap().len(), 1);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn old_unpinned_items_expire() {
        let mut b = Buffer::new();
        let old = b.push(ItemKind::Image, CaptureMode::Region);
        let pinned = b.push(ItemKind::Image, CaptureMode::Region);
        let fresh = b.push(ItemKind::Image, CaptureMode::Region);
        let week = Duration::from_secs(7 * 24 * 60 * 60);
        let week_ago = SystemTime::now() - week - Duration::from_secs(60);
        for id in [old, pinned] {
            b.get_mut(id).unwrap().created_at = week_ago;
        }
        b.get_mut(pinned).unwrap().pinned = true;
        assert_eq!(b.expired(SystemTime::now(), week), vec![old]);
        assert!(b.get(fresh).is_some());
    }

    #[test]
    fn every_change_moves_the_generation() {
        let mut b = Buffer::new();
        let mut seen = b.generation();
        let mut changed = |b: &Buffer| {
            let moved = b.generation() != seen;
            seen = b.generation();
            moved
        };
        let id = b.push(ItemKind::Image, CaptureMode::Region);
        assert!(changed(&b));
        b.get_mut(id).unwrap().pinned = true;
        assert!(changed(&b));
        let _ = b.get(id);
        assert!(!changed(&b), "reading changes nothing");
        b.remove(id);
        assert!(changed(&b));
    }

    #[test]
    fn ten_pushes_keep_order() {
        let mut b = Buffer::new();
        let ids: Vec<_> = (0..10)
            .map(|_| b.push(ItemKind::Image, CaptureMode::Region))
            .collect();
        let mut seen: Vec<_> = b.newest_first().map(|i| i.id).collect();
        seen.reverse();
        assert_eq!(ids, seen);
    }
}
