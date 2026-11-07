use cosmic::widget;
use regex::Regex;
use std::path::Path;
use std::sync::{LazyLock, Mutex};

use crate::config::IconSizes;
use crate::item::{Item, SearchItem};

#[cfg(feature = "gvfs")]
#[path = "gvfs.rs"]
mod imp;

#[cfg(not(feature = "gvfs"))]
#[path = "fallback.rs"]
mod imp;

pub use imp::*;

// from trash-rs, license MIT
/// Size of a [`TrashItem`] in bytes or entries
#[derive(Debug, Clone, Copy, Ord, PartialOrd, Eq, PartialEq, Hash)]
pub enum TrashItemSize {
    /// Number of bytes in a file
    Bytes(u64),
    /// Number of entries in a directory, non-recursive
    Entries(usize),
}

struct TrashState {
    request: crate::navigation::PendingRequest,
    is_empty: bool,
}

impl Default for TrashState {
    fn default() -> Self {
        Self {
            request: crate::navigation::PendingRequest::default(),
            is_empty: true,
        }
    }
}

impl TrashState {
    fn begin(&mut self) -> widget::Id {
        self.request.begin()
    }

    fn publish(&mut self, request: &widget::Id, is_empty: bool) -> bool {
        if !self.request.finish(request) {
            return false;
        }
        self.is_empty = is_empty;
        true
    }
}

static TRASH_STATE: LazyLock<Mutex<TrashState>> =
    LazyLock::new(|| Mutex::new(TrashState::default()));

pub trait TrashExt {
    /// Queries the trash backend; call from a background thread.
    fn is_empty() -> bool {
        Self::entries() == 0
    }

    fn is_empty_cached() -> bool {
        TRASH_STATE
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .is_empty
    }

    fn begin_refresh() -> widget::Id {
        TRASH_STATE
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .begin()
    }

    fn finish_refresh(request: &widget::Id, is_empty: bool) -> bool {
        TRASH_STATE
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .publish(request, is_empty)
    }

    fn entries() -> usize;

    fn list() -> Result<Vec<TrashItem>, String>;

    fn purge(_item: TrashItem) -> Result<(), String>;

    fn restore(_item: TrashItem) -> Result<(), String>;

    fn trash<P: AsRef<Path>>(_path: P) -> Result<(), String>;

    fn scan(_sizes: IconSizes) -> Vec<Item>;

    fn scan_search<F: Fn(SearchItem) -> bool + Sync>(_callback: F, _regex: &Regex);

    fn icon(icon_size: u16) -> widget::icon::Handle {
        widget::icon::from_name(if Self::is_empty_cached() {
            "user-trash"
        } else {
            "user-trash-full"
        })
        .size(icon_size)
        .handle()
    }

    fn icon_symbolic(icon_size: u16) -> widget::icon::Handle {
        widget::icon::from_name(if Self::is_empty_cached() {
            "user-trash-symbolic"
        } else {
            "user-trash-full-symbolic"
        })
        .size(icon_size)
        .handle()
    }
}

fn is_trash_root(root: &Path) -> bool {
    let Some(name) = root.file_name().and_then(|x| x.to_str()) else {
        return false;
    };
    (name == "Trash" || name.starts_with(".Trash")) && root.join("info").is_dir()
}

fn trash_files_dir(path: &Path) -> Option<&Path> {
    path.ancestors()
        .find(|a| a.ends_with("files") && a.parent().is_some_and(is_trash_root))
}

/// Check whether a path is inside any trash `files/` directory.
pub fn is_trash_path(path: &Path) -> bool {
    trash_files_dir(path).is_some()
}

pub struct Trash;

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};

    use regex::Regex;

    use super::{IconSizes, Item, Path, SearchItem, TrashExt, TrashItem};

    static WALKED: AtomicBool = AtomicBool::new(false);

    struct FullTrash;

    impl TrashExt for FullTrash {
        fn is_empty() -> bool {
            WALKED.store(true, Ordering::Relaxed);
            false
        }

        fn entries() -> usize {
            WALKED.store(true, Ordering::Relaxed);
            1
        }

        fn list() -> Result<Vec<TrashItem>, String> {
            unimplemented!()
        }

        fn purge(_item: TrashItem) -> Result<(), String> {
            unimplemented!()
        }

        fn restore(_item: TrashItem) -> Result<(), String> {
            unimplemented!()
        }

        fn trash<P: AsRef<Path>>(_path: P) -> Result<(), String> {
            unimplemented!()
        }

        fn scan(_sizes: IconSizes) -> Vec<Item> {
            unimplemented!()
        }

        fn scan_search<F: Fn(SearchItem) -> bool + Sync>(_callback: F, _regex: &Regex) {
            unimplemented!()
        }
    }

    #[test]
    fn the_icon_reads_the_cache_the_refresh_publishes() {
        let mut cache = super::TrashState::default();
        assert!(cache.is_empty);
        let request = cache.begin();
        assert!(cache.publish(&request, FullTrash::is_empty()));
        assert!(!cache.is_empty);

        WALKED.store(false, Ordering::Relaxed);
        let _ = FullTrash::icon(16);
        let _ = FullTrash::icon_symbolic(16);
        assert!(
            !WALKED.load(Ordering::Relaxed),
            "the sidebar icons are built on the GUI thread and must read the cache, not query \
             the trash backend"
        );
    }

    #[test]
    fn review_an_old_trash_result_cannot_overwrite_a_newer_result() {
        let mut cache = super::TrashState::default();
        let old = cache.begin();
        let new = cache.begin();
        assert!(cache.publish(&new, false));
        assert!(!cache.publish(&old, true));
        assert!(!cache.is_empty);
    }

    #[test]
    fn a_superseded_trash_result_cannot_publish_while_the_latest_is_pending() {
        let mut cache = super::TrashState::default();
        let old = cache.begin();
        let current = cache.begin();
        assert!(!cache.publish(&old, false));
        assert!(cache.is_empty);
        assert!(cache.publish(&current, false));
        assert!(!cache.publish(&current, true));
        assert!(!cache.is_empty);
    }
}
