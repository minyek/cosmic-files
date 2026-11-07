use std::fs::Metadata;
use std::time::{Duration, SystemTime};

use crate::trash::{TrashItem, TrashItemSize};

#[derive(Clone, Debug)]
pub enum ItemMetadata {
    Path {
        metadata: Metadata,
        children_opt: Option<usize>,
    },
    Trash {
        size: TrashItemSize,
        entry: TrashItem,
    },
    SimpleDir {
        entries: u64,
    },
    SimpleFile {
        size: u64,
    },
    #[cfg(feature = "gvfs")]
    GvfsPath {
        mtime: u64,
        size_opt: Option<u64>,
        children_opt: Option<usize>,
        is_dir: bool,
    },
}

impl ItemMetadata {
    pub fn is_dir(&self) -> bool {
        match self {
            Self::Path { metadata, .. } => metadata.is_dir(),
            Self::Trash { size, .. } => match size {
                TrashItemSize::Entries(_) => true,
                TrashItemSize::Bytes(_) => false,
            },
            Self::SimpleDir { .. } => true,
            Self::SimpleFile { .. } => false,
            #[cfg(feature = "gvfs")]
            Self::GvfsPath { is_dir, .. } => *is_dir,
        }
    }

    pub fn modified(&self) -> Option<SystemTime> {
        match self {
            Self::Path { metadata, .. } => metadata.modified().ok(),
            #[cfg(feature = "gvfs")]
            Self::GvfsPath { mtime, .. } => {
                SystemTime::UNIX_EPOCH.checked_add(Duration::from_secs(*mtime))
            }
            _ => None,
        }
    }

    pub fn file_size(&self) -> Option<u64> {
        match self {
            Self::Path { metadata, .. } => (!metadata.is_dir()).then_some(metadata.len()),
            Self::Trash { size, .. } => match size {
                TrashItemSize::Bytes(size) => Some(*size),
                TrashItemSize::Entries(_) => None,
            },
            #[cfg(feature = "gvfs")]
            Self::GvfsPath { size_opt, .. } => *size_opt,
            _ => None,
        }
    }

    /// A directory whose child count the scan deliberately left unread.
    pub(crate) fn children_count_pending(&self) -> bool {
        match self {
            Self::Path {
                metadata,
                children_opt,
            } => metadata.is_dir() && children_opt.is_none(),
            #[cfg(feature = "gvfs")]
            Self::GvfsPath {
                children_opt,
                is_dir,
                ..
            } => *is_dir && children_opt.is_none(),
            _ => false,
        }
    }

    pub(crate) fn set_children_count(&mut self, children: usize) {
        match self {
            Self::Path { children_opt, .. } => *children_opt = Some(children),
            #[cfg(feature = "gvfs")]
            Self::GvfsPath { children_opt, .. } => *children_opt = Some(children),
            _ => {}
        }
    }

    pub fn children_count(&self) -> Option<&usize> {
        match &self {
            ItemMetadata::Path { children_opt, .. } => children_opt.as_ref(),
            #[cfg(feature = "gvfs")]
            ItemMetadata::GvfsPath { children_opt, .. } => children_opt.as_ref(),
            _ => None,
        }
    }
}
