use cosmic::iced::Rectangle;
use cosmic::widget;
use std::cell::Cell;
use std::sync::atomic;

use crate::item::ChecksumState;

#[derive(Clone, Debug)]
pub struct ItemState {
    pub button_id: widget::Id,
    /// Identifies this item's background work (sizes, counts, thumbnails, checksums), so a result
    /// computed for an entry a rescan has since replaced is not applied to the replacement.
    pub(crate) work_id: u64,
    pub checksums: ChecksumState,
    pub cut: bool,
    pub highlighted: bool,
    pub overlaps_drag_rect: bool,
    pub pos_opt: Cell<Option<(usize, usize)>>,
    pub rect_opt: Cell<Option<Rectangle>>,
    pub selected: bool,
}

impl Default for ItemState {
    fn default() -> Self {
        Self {
            button_id: widget::Id::unique(),
            work_id: next_work_id(),
            checksums: ChecksumState::default(),
            cut: false,
            highlighted: false,
            overlaps_drag_rect: false,
            pos_opt: Cell::new(None),
            rect_opt: Cell::new(None),
            selected: false,
        }
    }
}

pub(crate) fn next_work_id() -> u64 {
    static NEXT: atomic::AtomicU64 = atomic::AtomicU64::new(1);
    NEXT.fetch_add(1, atomic::Ordering::Relaxed)
}
