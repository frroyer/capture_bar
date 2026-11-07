//! The capture buffer (Epic E6). Appending must never block the caller
//! (FR-20, NFR-4). v0: in-memory list. Next: write-ahead index + files on disk
//! (US-6.1, US-6.6).

use cb_core::CaptureMode;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::PathBuf;
use std::time::SystemTime;

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
}

#[derive(Debug, Default)]
pub struct Buffer {
    items: Vec<BufferItem>,
    next_id: u64,
}

impl Buffer {
    pub fn new() -> Self {
        Self::default()
    }

    /// Append a new item and return its id. Newest items are last internally;
    /// use [`Buffer::newest_first`] for display (FR-22).
    pub fn push(&mut self, kind: ItemKind, mode: CaptureMode) -> ItemId {
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
        Some(self.items.remove(idx))
    }

    /// Clear everything except pinned items (FR-24, FR-25).
    pub fn clear_unpinned(&mut self) {
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
