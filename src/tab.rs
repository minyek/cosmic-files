#[cfg(feature = "desktop")]
use cosmic::desktop::fde::{DesktopEntry, get_languages_from_env};
use cosmic::iced::advanced::graphics;
use cosmic::iced::advanced::text::{self, Paragraph};
use cosmic::iced::alignment::Vertical;
use cosmic::iced::clipboard::dnd::DndAction;
use cosmic::iced::core::mouse::ScrollDelta;
use cosmic::iced::core::widget::tree;
use cosmic::iced::futures::{self, SinkExt};
use cosmic::iced::keyboard::Modifiers;
use cosmic::iced::widget::scrollable::{self, AbsoluteOffset, Viewport};
use cosmic::iced::widget::{rule, stack};
use cosmic::iced::{
    Alignment, Border, Color, ContentFit, Length, Point, Rectangle, Size, Subscription, Vector,
    padding, stream, window,
};
use cosmic::widget::menu::action::MenuAction;
use cosmic::widget::menu::key_bind::KeyBind;
use cosmic::widget::{self, DndDestination, DndSource, Id, RcElementWrapper, Widget, space};
use cosmic::{Apply, Element, cosmic_theme, font, theme};
use icu::datetime::input::DateTime;
use icu::datetime::options::TimePrecision;
use icu::datetime::{DateTimeFormatter, DateTimeFormatterPreferences, fieldsets};
use icu::locale::preferences::extensions::unicode::keywords::HourCycle;
use image::{DynamicImage, ImageReader};
use jiff_icu::ConvertFrom;
use mime_guess::{Mime, mime};
use rayon::prelude::*;
use rustc_hash::{FxHashMap, FxHashSet};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::borrow::Cow;
use std::cell::{Cell, RefCell};
use std::cmp::{Ordering, Reverse};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::error::Error;
use std::fmt::{self, Display};
use std::fs::{self, File, Metadata};
use std::hash::Hash;
use std::io::{BufRead, BufReader, Read};
#[cfg(unix)]
use std::os::unix::fs::MetadataExt;
use std::path::{self, Path, PathBuf};
use std::sync::{Arc, LazyLock, RwLock, atomic};
use std::time::{Duration, Instant, SystemTime};
use tempfile::NamedTempFile;
use tokio::sync::mpsc;
use trash::{TrashItem, TrashItemMetadata, TrashItemSize};
use walkdir::WalkDir;

use crate::app::{Action, PreviewKind};
use crate::clipboard::{ClipboardCopy, ClipboardKind, ClipboardPaste};
use crate::config::{
    ContextActionPreset, DesktopConfig, ICON_SCALE_MAX, ICON_SIZE_GRID, IconSizes, TabConfig,
    ThumbCfg,
};
use crate::dialog::DialogKind;
use crate::large_image::{
    LargeImageManager, decode_large_image, exceeds_memory_limit, should_use_dedicated_worker,
    should_use_tiling,
};
use crate::localize::{LANGUAGE_SORTER, LOCALE};
use crate::mime_icon::{mime_for_path, mime_icon};
use crate::mounter::MOUNTERS;
use crate::operation::{Controller, OperationError};
use crate::thumbnail_cacher::{CachedThumbnail, ThumbnailCacher, ThumbnailSize};
use crate::thumbnailer::thumbnailer;
use crate::trash::{Trash, TrashExt};
use crate::{FxOrderMap, fl, menu, mime_app, mouse_area};

pub const DOUBLE_CLICK_DURATION: Duration = Duration::from_millis(500);
pub const HOVER_DURATION: Duration = Duration::from_millis(1600);
pub const TYPE_SELECT_TIMEOUT: Duration = Duration::from_millis(1000);
//TODO: best limit for search items
const MAX_SEARCH_LATENCY: Duration = Duration::from_millis(20);
const MAX_SEARCH_RESULTS: usize = 200;
//TODO: configurable thumbnail size?
const THUMBNAIL_SIZE: u32 = (ICON_SIZE_GRID as u32) * (ICON_SCALE_MAX as u32);
/// Maximum bytes of text to pass to the editor for preview; caps shaping work to avoid blocking.
/// Files larger than this get a truncated preview (first N bytes only). Shaping holds the
/// process-wide font system and costs more than linear in the input, so this is sized for what
/// the preview can show.
const TEXT_PREVIEW_MAX_BYTES: usize = 16 * 1024; // 16 KiB
/// Maximum file size (bytes) to attempt text preview; files larger than this are skipped entirely.
const TEXT_PREVIEW_MAX_FILE_BYTES: u64 = 8 * 1000 * 1000; // 8 MB

const SVG_PREVIEW_MAX_FILE_BYTES: u64 = 8 * 1000 * 1000; // 8 MB

// Thumbnail generation semaphore - limits parallel thumbnail workers
// Uses 4 workers for balanced throughput and memory usage
pub static THUMB_SEMAPHORE: LazyLock<tokio::sync::Semaphore> =
    LazyLock::new(|| tokio::sync::Semaphore::const_new(num_cpus::get().min(4)));

/// Identifies an item's button across widget-tree reconciliation.
///
/// Widget reconciliation reuses unnamed IDs by position. Named IDs preserve item identity
/// as virtualized rows move.
pub(crate) fn next_button_id() -> widget::Id {
    static NEXT: atomic::AtomicU64 = atomic::AtomicU64::new(0);
    widget::Id::new(format!(
        "item-button-{}",
        NEXT.fetch_add(1, atomic::Ordering::Relaxed)
    ))
}

pub(crate) fn next_work_id() -> u64 {
    static NEXT: atomic::AtomicU64 = atomic::AtomicU64::new(1);
    NEXT.fetch_add(1, atomic::Ordering::Relaxed)
}

#[derive(Clone, Debug)]
pub struct EntryRefresh {
    pub path: PathBuf,
    pub work_id: Option<u64>,
    pub item_opt: Option<Item>,
}

pub(crate) static SORT_OPTION_FALLBACK: LazyLock<FxHashMap<String, (HeadingOptions, bool)>> =
    LazyLock::new(|| {
        FxHashMap::from_iter(dirs::download_dir().into_iter().map(|dir| {
            (
                Location::Path(dir).normalize().to_string(),
                (HeadingOptions::Modified, false),
            )
        }))
    });

static MODE_NAMES: LazyLock<Vec<String>> = LazyLock::new(|| {
    vec![
        // Mode 0
        fl!("none"),
        // Mode 1
        fl!("execute-only"),
        // Mode 2
        fl!("write-only"),
        // Mode 3
        fl!("write-execute"),
        // Mode 4
        fl!("read-only"),
        // Mode 5
        fl!("read-execute"),
        // Mode 6
        fl!("read-write"),
        // Mode 7
        fl!("read-write-execute"),
    ]
});

static SPECIAL_DIRS: LazyLock<FxHashMap<PathBuf, &'static str>> = LazyLock::new(|| {
    let mut special_dirs = FxHashMap::default();
    if let Some(dir) = dirs::document_dir() {
        special_dirs.insert(dir, "folder-documents");
    }
    if let Some(dir) = dirs::download_dir() {
        special_dirs.insert(dir, "folder-download");
    }
    if let Some(dir) = dirs::audio_dir() {
        special_dirs.insert(dir, "folder-music");
    }
    if let Some(dir) = dirs::picture_dir() {
        special_dirs.insert(dir, "folder-pictures");
    }
    if let Some(dir) = dirs::public_dir() {
        special_dirs.insert(dir, "folder-publicshare");
    }
    if let Some(dir) = dirs::template_dir() {
        special_dirs.insert(dir, "folder-templates");
    }
    if let Some(dir) = dirs::video_dir() {
        special_dirs.insert(dir, "folder-videos");
    }
    if let Some(dir) = dirs::desktop_dir() {
        special_dirs.insert(dir, "user-desktop");
    }
    if let Some(dir) = dirs::home_dir() {
        special_dirs.insert(dir, "user-home");
    }
    special_dirs
});

fn button_appearance(
    theme: &theme::Theme,
    selected: bool,
    highlighted: bool,
    cut: bool,
    focused: bool,
    accent: bool,
    condensed_radius: bool,
    desktop: bool,
) -> widget::button::Style {
    let cosmic = theme.cosmic();
    let mut appearance = widget::button::Style::new();
    if selected {
        if accent {
            appearance.background = Some(Color::from(cosmic.accent_color()).into());
            appearance.icon_color = Some(Color::from(cosmic.on_accent_color()));
            if cut {
                appearance.text_color = Some(Color::from(cosmic.accent.on_disabled));
            } else {
                appearance.text_color = Some(Color::from(cosmic.on_accent_color()));
            }
        } else {
            appearance.background = Some(Color::from(cosmic.bg_component_color()).into());
        }
    } else if highlighted {
        if accent {
            appearance.background = Some(Color::from(cosmic.bg_component_color()).into());
            appearance.icon_color = Some(Color::from(cosmic.on_bg_component_color()));
            appearance.text_color = Some(Color::from(cosmic.on_bg_component_color()));
            if cut {
                appearance.text_color = Some(Color::from(
                    cosmic.background(theme.transparent).component.on_disabled,
                ));
            } else {
                appearance.text_color = Some(Color::from(cosmic.on_bg_component_color()));
            }
        } else {
            appearance.background = Some(Color::from(cosmic.bg_component_color()).into());
        }
    } else if desktop {
        appearance.background = Some(Color::from(cosmic.bg_color()).into());
        appearance.icon_color = Some(Color::from(cosmic.on_bg_color()));
        if cut {
            appearance.text_color = Some(Color::from(
                cosmic.background(theme.transparent).component.disabled,
            ));
        } else {
            appearance.text_color = Some(Color::from(cosmic.on_bg_color()));
        }
    } else if cut {
        appearance.text_color = Some(Color::from(
            cosmic.background(theme.transparent).component.on_disabled,
        ));
    }
    if focused && accent {
        appearance.outline_width = 1.0;
        appearance.outline_color = Color::from(cosmic.accent_color());
        appearance.border_width = 2.0;
        appearance.border_color = Color::TRANSPARENT;
    }
    if condensed_radius {
        appearance.border_radius = cosmic.radius_xs().into();
    } else {
        appearance.border_radius = cosmic.radius_s().into();
    }
    appearance
}

fn button_style(
    selected: bool,
    highlighted: bool,
    cut: bool,
    accent: bool,
    condensed_radius: bool,
    desktop: bool,
) -> theme::Button {
    //TODO: move to libcosmic?
    theme::Button::Custom {
        active: Box::new(move |focused, theme| {
            button_appearance(
                theme,
                selected,
                highlighted,
                cut,
                focused,
                accent,
                condensed_radius,
                desktop,
            )
        }),
        disabled: Box::new(move |theme| {
            button_appearance(
                theme,
                selected,
                highlighted,
                cut,
                false,
                accent,
                condensed_radius,
                desktop,
            )
        }),
        hovered: Box::new(move |focused, theme| {
            button_appearance(
                theme,
                selected,
                highlighted,
                cut,
                focused,
                accent,
                condensed_radius,
                desktop,
            )
        }),
        pressed: Box::new(move |focused, theme| {
            button_appearance(
                theme,
                selected,
                highlighted,
                cut,
                focused,
                accent,
                condensed_radius,
                desktop,
            )
        }),
    }
}

pub fn folder_icon(path: &PathBuf, icon_size: u16) -> widget::icon::Handle {
    widget::icon::from_name(SPECIAL_DIRS.get(path).map_or("folder", |x| *x))
        .prefer_svg(true)
        .size(icon_size)
        .handle()
}

pub fn folder_icon_symbolic(path: &PathBuf, icon_size: u16) -> widget::icon::Handle {
    widget::icon::from_name(format!(
        "{}-symbolic",
        SPECIAL_DIRS.get(path).map_or("folder", |x| *x)
    ))
    .size(icon_size)
    .handle()
}

//TODO: replace with Path::has_trailing_sep when stable
fn has_trailing_sep(path: &Path) -> bool {
    path.as_os_str()
        .as_encoded_bytes()
        .last()
        .copied()
        .is_some_and(|b| path::is_separator(b as char))
}

fn lexically_normalize(path: &Path) -> PathBuf {
    // Parent components must survive until filesystem resolution: the previous component
    // can be a symlink whose target has a different parent.
    path.components().collect()
}

fn tab_complete(path: &Path) -> Result<Vec<(String, PathBuf)>, Box<dyn Error>> {
    let parent = if has_trailing_sep(path) && path.is_dir() {
        // Show completions inside existing child directory instead of parent
        path
    } else {
        path.parent()
            .ok_or_else(|| format!("path has no parent {}", path.display()))?
    };

    let child_os = path.strip_prefix(parent)?;
    let child = child_os
        .to_str()
        .ok_or_else(|| format!("invalid UTF-8 {}", child_os.display()))?;

    let pattern = format!("^{}", regex::escape(child));
    let regex = regex::RegexBuilder::new(&pattern)
        .case_insensitive(true)
        .build()?;

    let mut completions = Vec::new();
    for entry_res in fs::read_dir(parent)? {
        let entry = entry_res?;
        let file_name_os = entry.file_name();
        let Some(file_name) = file_name_os.to_str() else {
            continue;
        };
        // Don't list hidden files before entering a pattern
        if pattern == "^" && file_name.starts_with('.') {
            continue;
        }
        if regex.is_match(file_name) {
            completions.push((file_name.to_string(), entry.path()));
        }
    }

    completions.sort_by(|a, b| LANGUAGE_SORTER.compare(&a.0, &b.0));
    //TODO: make the list scrollable?
    completions.truncate(8);
    Ok(completions)
}

//TODO: translate, add more levels?
fn format_size(size: u64) -> String {
    const KB: u64 = 1000;
    const MB: u64 = 1000 * KB;
    const GB: u64 = 1000 * MB;
    const TB: u64 = 1000 * GB;

    if size >= TB {
        format!("{:.1} TB", size as f64 / TB as f64)
    } else if size >= GB {
        format!("{:.1} GB", size as f64 / GB as f64)
    } else if size >= MB {
        format!("{:.1} MB", size as f64 / MB as f64)
    } else if size >= KB {
        format!("{:.1} KB", size as f64 / KB as f64)
    } else {
        format!("{size} B")
    }
}

const MODE_SHIFT_USER: u32 = 6;
const MODE_SHIFT_GROUP: u32 = 3;
const MODE_SHIFT_OTHER: u32 = 0;

const fn get_mode_part(mode: u32, shift: u32) -> u32 {
    (mode >> shift) & 0o7
}

fn set_mode_part(mode: u32, shift: u32, bits: u32) -> u32 {
    assert!(bits <= 0o7);
    (mode & !(0o7 << shift)) | (bits << shift)
}

fn date_time_formatter(military_time: bool) -> DateTimeFormatter<fieldsets::YMDT> {
    let mut prefs = DateTimeFormatterPreferences::from(LOCALE.clone());
    prefs.hour_cycle = Some(if military_time {
        HourCycle::H23
    } else {
        HourCycle::H12
    });

    let mut fs = fieldsets::YMDT::medium();
    fs = fs.with_time_precision(TimePrecision::Minute);

    DateTimeFormatter::try_new(prefs, fs).expect("failed to create DateTimeFormatter")
}

fn time_formatter(military_time: bool) -> DateTimeFormatter<fieldsets::T> {
    let mut prefs = DateTimeFormatterPreferences::from(LOCALE.clone());
    prefs.hour_cycle = Some(if military_time {
        HourCycle::H23
    } else {
        HourCycle::H12
    });

    let mut fs = fieldsets::T::medium();
    fs = fs.with_time_precision(TimePrecision::Minute);

    DateTimeFormatter::try_new(prefs, fs).expect("failed to create DateTimeFormatter")
}

struct FormatTime<'a> {
    pub time: SystemTime,
    pub date_time_formatter: &'a DateTimeFormatter<fieldsets::YMDT>,
    pub time_formatter: &'a DateTimeFormatter<fieldsets::T>,
}

impl<'a> FormatTime<'a> {
    fn from_secs(
        secs: i64,
        date_time_formatter: &'a DateTimeFormatter<fieldsets::YMDT>,
        time_formatter: &'a DateTimeFormatter<fieldsets::T>,
    ) -> Option<Self> {
        // This looks convoluted because we need to ensure the units match up
        let secs: u64 = secs.try_into().ok()?;
        let now = SystemTime::now();
        let filetime_diff = now
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|from_epoch| from_epoch.as_secs())
            .ok()
            .and_then(|now_secs| now_secs.checked_sub(secs))
            .map(Duration::from_secs)?;
        now.checked_sub(filetime_diff).map(|time| Self {
            time,
            date_time_formatter,
            time_formatter,
        })
    }
}

impl Display for FormatTime<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let Ok(zoned) = jiff::Zoned::try_from(self.time) else {
            return Ok(());
        };
        let now = jiff::Zoned::now();
        let icu_datetime = DateTime::convert_from(zoned.datetime());
        if zoned.date() == now.date() {
            f.write_str(fl!("today").as_str())?;
            f.write_str(", ")?;
            self.time_formatter.format(&icu_datetime).fmt(f)
        } else {
            self.date_time_formatter.format(&icu_datetime).fmt(f)
        }
    }
}

const fn format_time<'a>(
    time: SystemTime,
    date_time_formatter: &'a DateTimeFormatter<fieldsets::YMDT>,
    time_formatter: &'a DateTimeFormatter<fieldsets::T>,
) -> FormatTime<'a> {
    FormatTime {
        time,
        date_time_formatter,
        time_formatter,
    }
}

#[cfg(not(target_os = "windows"))]
fn hidden_attribute(_metadata: &Metadata) -> bool {
    false
}

#[cfg(target_os = "windows")]
fn hidden_attribute(metadata: &Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    // https://learn.microsoft.com/en-us/windows/win32/fileio/file-attribute-constants
    const FILE_ATTRIBUTE_HIDDEN: u32 = 2;
    metadata.file_attributes() & FILE_ATTRIBUTE_HIDDEN == FILE_ATTRIBUTE_HIDDEN
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FsKind {
    Local,
    Remote,
    Gvfs,
}

#[cfg(target_os = "linux")]
pub fn fs_kind(metadata: &Metadata) -> FsKind {
    //TODO: method to reload remote filesystems dynamically
    //TODO: fix for https://github.com/eminence/procfs/issues/262
    static DEVICES: LazyLock<FxHashMap<u64, FsKind>> = LazyLock::new(|| {
        let mut devices = FxHashMap::default();
        match procfs::process::Process::myself() {
            Ok(process) => match process.mountinfo() {
                Ok(mount_infos) => {
                    devices = FxHashMap::from_iter(mount_infos.iter().filter_map(|mount_info| {
                        let mut parts = mount_info.majmin.split(':');
                        let major_str = parts.next()?;
                        let minor_str = parts.next()?;
                        let major = major_str.parse::<libc::c_uint>().ok()?;
                        let minor = minor_str.parse::<libc::c_uint>().ok()?;
                        let dev = libc::makedev(major, minor);
                        // Network and distributed filesystem types
                        // Based on common remote filesystem types found in /proc/mounts
                        let kind = match mount_info.fs_type.as_str() {
                            // SMB/CIFS variants
                            "cifs" | "smb" | "smb2" | "smbfs" => FsKind::Remote,

                            // NFS variants
                            "nfs" | "nfs4" => FsKind::Remote,

                            // FUSE-based remote filesystems
                            "fuse.rclone" | "fuse.sshfs" | "fuse.davfs2" | "fuse.ceph"
                            | "fuse.glusterfs" | "fuse.s3fs" | "fuse.goofys" | "fuse.gcsfuse"
                            | "fuse.afp" | "fuse.afpfs" => FsKind::Remote,

                            // Other network protocols
                            "afs" | "coda" | "ncpfs" | "davfs" | "davfs2" | "shfs" => {
                                FsKind::Remote
                            }

                            // Cluster/distributed filesystems
                            "ceph" | "glusterfs" | "lustre" | "gfs" | "gfs2" | "ocfs2" => {
                                FsKind::Remote
                            }

                            // GVFS (GNOME Virtual File System)
                            "fuse.gvfsd-fuse" => FsKind::Gvfs,

                            // Everything else is local
                            _ => FsKind::Local,
                        };
                        Some((dev, kind))
                    }));
                }
                Err(err) => {
                    log::warn!("failed to get mount info: {err}");
                }
            },
            Err(err) => {
                log::warn!("failed to get process info: {err}");
            }
        }
        devices
    });
    DEVICES.get(&metadata.dev()).map_or(FsKind::Local, |x| *x)
}

#[cfg(not(target_os = "linux"))]
pub fn fs_kind(_metadata: &Metadata) -> FsKind {
    //TODO: support BSD, macOS, Windows?
    FsKind::Local
}

#[cfg(not(feature = "desktop"))]
fn get_desktop_file_display_name(path: &Path) -> Option<String> {
    None
}

#[cfg(feature = "desktop")]
fn get_desktop_file_display_name(path: &Path) -> Option<String> {
    let locales = get_languages_from_env();
    let entry = match DesktopEntry::from_path(path, Some(&locales)) {
        Ok(ok) => ok,
        Err(err) => {
            log::warn!("failed to parse {}: {}", path.display(), err);
            return None;
        }
    };

    entry.name(&locales).map(|s| s.into_owned())
}

#[cfg(not(feature = "desktop"))]
fn get_desktop_file_icon(path: &Path) -> Option<String> {
    None
}

#[cfg(feature = "desktop")]
fn get_desktop_file_icon(path: &Path) -> Option<String> {
    let entry = match DesktopEntry::from_path::<&str>(path, None) {
        Ok(ok) => ok,
        Err(err) => {
            log::warn!("failed to parse {}: {}", path.display(), err);
            return None;
        }
    };

    entry.icon().map(str::to_string)
}

/// Creates an icon handle from a desktop file's Icon field value.
/// Supports both icon names (looked up in theme) and absolute paths (used directly).
fn desktop_icon_handle(icon: &str, size: u16) -> widget::icon::Handle {
    let icon_path = Path::new(icon);
    if icon_path.is_absolute() && icon_path.exists() {
        widget::icon::from_path(icon_path.to_path_buf())
    } else {
        widget::icon::from_name(icon)
            .prefer_svg(true)
            .size(size)
            .handle()
    }
}

#[cfg(feature = "desktop")]
pub fn parse_desktop_file(path: &Path) -> (Option<String>, Option<String>) {
    let locales = get_languages_from_env();
    let entry = match DesktopEntry::from_path(path, Some(&locales)) {
        Ok(ok) => ok,
        Err(err) => {
            log::warn!("failed to parse {}: {}", path.display(), err);
            return (None, None);
        }
    };
    (
        entry.name(&locales).map(|s| s.into_owned()),
        entry.icon().map(str::to_string),
    )
}

fn display_name_for_file(path: &Path, name: &str, get_from_gvfs: bool, is_desktop: bool) -> String {
    if is_desktop {
        return get_desktop_file_display_name(path).map_or_else(
            || Item::display_name(name),
            |desktop_name| Item::display_name(desktop_name.as_str()),
        );
    } else if get_from_gvfs {
        #[cfg(feature = "gvfs")]
        {
            let file = gio::File::for_path(path);
            if let Ok(info) = gio::prelude::FileExt::query_info(
                &file,
                "standard::display-name",
                gio::FileQueryInfoFlags::NONE,
                gio::Cancellable::NONE,
            ) {
                return Item::display_name(info.display_name().as_str());
            }
        }
    }
    Item::display_name(name)
}

// Whether a dir lives on a remote filesystem, according to GIO.
#[cfg(feature = "gvfs")]
fn gvfs_dir_is_remote(dir: &Path) -> bool {
    static REMOTE_CACHE: LazyLock<RwLock<FxHashMap<PathBuf, bool>>> =
        LazyLock::new(|| RwLock::new(FxHashMap::default()));

    if let Some(remote) = REMOTE_CACHE.read().unwrap().get(dir) {
        return *remote;
    }

    let remote = match gio::prelude::FileExt::query_filesystem_info(
        &gio::File::for_path(dir),
        gio::FILE_ATTRIBUTE_FILESYSTEM_REMOTE,
        gio::Cancellable::NONE,
    ) {
        Ok(info) => info.boolean(gio::FILE_ATTRIBUTE_FILESYSTEM_REMOTE),
        Err(err) => {
            log::warn!(
                "failed to get GIO filesystem info for {}: {}",
                dir.display(),
                err
            );

            //Assume remote so that the "expensive tasks" are rather skipped then actually executed.
            true
        }
    };

    REMOTE_CACHE
        .write()
        .unwrap()
        .insert(dir.to_path_buf(), remote);
    remote
}

#[cfg(feature = "gvfs")]
pub fn item_from_gvfs_info(path: PathBuf, file_info: gio::FileInfo, sizes: IconSizes) -> Item {
    let file_name = file_info
        .attribute_as_string(gio::FILE_ATTRIBUTE_STANDARD_NAME)
        .unwrap_or_default();
    let mtime = file_info.attribute_uint64(gio::FILE_ATTRIBUTE_TIME_MODIFIED);
    let mut is_desktop = false;
    let remote = path.parent().is_none_or(gvfs_dir_is_remote);
    let is_dir = matches!(file_info.file_type(), gio::FileType::Directory);

    let size_opt = (!is_dir).then_some(file_info.size() as u64);

    let (mime, icon_handle_grid, icon_handle_list, icon_handle_list_condensed) = if is_dir {
        (
            //TODO: make this a static
            "inode/directory".parse().unwrap(),
            folder_icon(&path, sizes.grid()),
            folder_icon(&path, sizes.list()),
            folder_icon(&path, sizes.list_condensed()),
        )
    } else {
        // ALWAYS assume we're remote for mime guessing here, since gvfs reading can be expensive
        // @todo - expose this as a config option?
        let mime = mime_for_path(&path, None, true);

        //TODO: clean this up, implement for trash
        let icon_name_opt = if mime == "application/x-desktop" {
            is_desktop = true;
            get_desktop_file_icon(&path)
        } else {
            None
        };
        if let Some(icon_name) = icon_name_opt {
            (
                mime,
                desktop_icon_handle(&icon_name, sizes.grid()),
                desktop_icon_handle(&icon_name, sizes.list()),
                desktop_icon_handle(&icon_name, sizes.list_condensed()),
            )
        } else {
            (
                mime.clone(),
                mime_icon(mime.clone(), sizes.grid()),
                mime_icon(mime.clone(), sizes.list()),
                mime_icon(mime, sizes.list_condensed()),
            )
        }
    };

    let mut children_opt = None;
    let mut dir_size = DirSize::NotDirectory;
    if is_dir && !remote {
        dir_size = DirSize::Calculating(Controller::default());
        match fs::read_dir(&path) {
            Ok(entries) => {
                children_opt = Some(entries.count());
            }
            Err(err) => {
                log::warn!("failed to read directory {}: {}", path.display(), err);
            }
        }
    }

    let display_name = display_name_for_file(&path, &file_info.display_name(), false, is_desktop);
    let hidden = file_name.starts_with('.');

    Item {
        name: file_name.into(),
        display_name,
        is_mount_point: false,
        is_symlink_dir: is_dir && file_info.is_symlink(),
        metadata: ItemMetadata::GvfsPath {
            mtime,
            size_opt,
            children_opt,
            is_dir,
        },
        hidden,
        image_dimensions: (!remote && mime.type_() == mime::IMAGE)
            .then(|| image::image_dimensions(&path).ok())
            .flatten(),
        location_opt: Some(Location::Path(path)),
        mime,
        icon_handle_grid,
        icon_handle_list,
        icon_handle_list_condensed,
        thumbnail_opt: if remote {
            Some(ItemThumbnail::NotImage)
        } else {
            None
        },
        button_id: next_button_id(),
        work_id: next_work_id(),
        pos_opt: Cell::new(None),
        rect_opt: Cell::new(None),
        selected: false,
        highlighted: false,
        overlaps_drag_rect: false,
        dir_size,
        cut: false,
        checksums: ChecksumState::default(),
    }
}

pub fn item_from_search_item(search_item: SearchItem, sizes: IconSizes) -> Item {
    match search_item {
        SearchItem::Path(path, name, metadata) => item_from_entry(path, name, metadata, sizes),
        SearchItem::Trash(entry, metadata) => item_from_trash_entry(entry, metadata, sizes),
    }
}

pub fn item_from_entry(
    path: PathBuf,
    name: String,
    metadata: fs::Metadata,
    sizes: IconSizes,
) -> Item {
    let is_symlink = metadata.is_symlink();
    let metadata = if is_symlink {
        fs::metadata(&path).unwrap_or(metadata)
    } else {
        metadata
    };
    let mut is_desktop = false;
    let mut is_gvfs = false;

    let hidden = name.starts_with('.') || hidden_attribute(&metadata);

    let remote = match fs_kind(&metadata) {
        FsKind::Local => false,
        FsKind::Remote => true,
        #[cfg(feature = "gvfs")]
        FsKind::Gvfs => {
            is_gvfs = true;
            path.parent().is_none_or(gvfs_dir_is_remote)
        }
        #[cfg(not(feature = "gvfs"))]
        FsKind::Gvfs => {
            log::info!(
                "gvfs feature not enabled, info may be inaccurate for {}",
                path.display()
            );
            true
        }
    };

    let (mime, icon_handle_grid, icon_handle_list, icon_handle_list_condensed) =
        if metadata.is_dir() {
            (
                //TODO: make this a static
                "inode/directory".parse().unwrap(),
                folder_icon(&path, sizes.grid()),
                folder_icon(&path, sizes.list()),
                folder_icon(&path, sizes.list_condensed()),
            )
        } else {
            let mime = mime_for_path(&path, Some(&metadata), remote);
            //TODO: clean this up, implement for trash
            let icon_name_opt = if mime == "application/x-desktop" {
                is_desktop = true;
                get_desktop_file_icon(&path)
            } else {
                None
            };
            if let Some(icon_name) = icon_name_opt {
                (
                    mime,
                    desktop_icon_handle(&icon_name, sizes.grid()),
                    desktop_icon_handle(&icon_name, sizes.list()),
                    desktop_icon_handle(&icon_name, sizes.list_condensed()),
                )
            } else {
                (
                    mime.clone(),
                    mime_icon(mime.clone(), sizes.grid()),
                    mime_icon(mime.clone(), sizes.list()),
                    mime_icon(mime, sizes.list_condensed()),
                )
            }
        };

    let dir_size = if metadata.is_dir() && !remote {
        DirSize::Calculating(Controller::default())
    } else {
        DirSize::NotDirectory
    };

    let display_name = display_name_for_file(&path, &name, is_gvfs, is_desktop);

    Item {
        name,
        display_name,
        is_mount_point: false,
        is_symlink_dir: metadata.is_dir() && (is_symlink || path.is_symlink()),
        metadata: ItemMetadata::Path {
            metadata,
            // Counting children costs one directory read per entry, almost all of them off
            // screen; the count is filled in for the rows that are displayed.
            children_opt: None,
        },
        hidden,
        location_opt: Some(Location::Path(path)),
        image_dimensions: None,
        mime,
        icon_handle_grid,
        icon_handle_list,
        icon_handle_list_condensed,
        thumbnail_opt: remote.then_some(ItemThumbnail::NotImage),
        button_id: next_button_id(),
        work_id: next_work_id(),
        pos_opt: Cell::new(None),
        rect_opt: Cell::new(None),
        selected: false,
        highlighted: false,
        overlaps_drag_rect: false,
        dir_size,
        cut: false,
        checksums: ChecksumState::default(),
    }
}

pub fn item_from_trash_entry(
    entry: TrashItem,
    metadata: TrashItemMetadata,
    sizes: IconSizes,
) -> Item {
    let original_path = entry.original_path();
    let name = entry.name.to_string_lossy().into_owned();
    let display_name = Item::display_name(&name);

    let trash_path_opt = crate::trash::trash_item_path(&entry);
    let location = trash_path_opt.clone().map(Location::Path);

    let (mime, icon_handle_grid, icon_handle_list, icon_handle_list_condensed) = match metadata.size
    {
        trash::TrashItemSize::Entries(_) => (
            //TODO: make this a static
            "inode/directory".parse().unwrap(),
            folder_icon(&original_path, sizes.grid()),
            folder_icon(&original_path, sizes.list()),
            folder_icon(&original_path, sizes.list_condensed()),
        ),
        trash::TrashItemSize::Bytes(_) => {
            // This passes remote = true so it does not read from the original path
            let mime = mime_for_path(&original_path, None, true);
            (
                mime.clone(),
                mime_icon(mime.clone(), sizes.grid()),
                mime_icon(mime.clone(), sizes.list()),
                mime_icon(mime, sizes.list_condensed()),
            )
        }
    };

    Item {
        name,
        display_name,
        is_mount_point: false,
        is_symlink_dir: matches!(metadata.size, TrashItemSize::Entries(_))
            && location
                .as_ref()
                .and_then(Location::path_opt)
                .is_some_and(|path| path.is_symlink()),
        metadata: ItemMetadata::Trash { metadata, entry },
        hidden: false,
        location_opt: location,
        // The original path no longer holds the file, so the dimensions come from the trashed copy.
        image_dimensions: trash_path_opt
            .as_ref()
            .filter(|_| mime.type_() == mime::IMAGE)
            .and_then(|path| image::image_dimensions(path).ok()),
        mime,
        icon_handle_grid,
        icon_handle_list,
        icon_handle_list_condensed,
        thumbnail_opt: Some(ItemThumbnail::NotImage),
        button_id: next_button_id(),
        work_id: next_work_id(),
        pos_opt: Cell::new(None),
        rect_opt: Cell::new(None),
        selected: false,
        highlighted: false,
        overlaps_drag_rect: false,
        dir_size: DirSize::NotDirectory,
        cut: false,
        checksums: ChecksumState::default(),
    }
}

fn item_from_trash_child(
    path: PathBuf,
    name: String,
    metadata: fs::Metadata,
    sizes: IconSizes,
) -> Option<Item> {
    let Some(original_path) = crate::trash::original_path_for_trash_child(&path) else {
        log::warn!(
            "failed to resolve original path for trash item {}, skipping entry",
            path.display()
        );
        return None;
    };
    let Some(original_parent) = original_path.parent() else {
        log::warn!(
            "trash item {} has no original parent, skipping entry",
            path.display()
        );
        return None;
    };
    let entry = trash::TrashItem {
        id: path.as_os_str().to_os_string(),
        name: std::ffi::OsString::from(&name),
        original_parent: original_parent.to_path_buf(),
        time_deleted: 0,
    };
    let size = if metadata.is_dir() {
        trash::TrashItemSize::Entries(0)
    } else {
        trash::TrashItemSize::Bytes(metadata.len())
    };
    Some(item_from_trash_entry(
        entry,
        trash::TrashItemMetadata { size },
        sizes,
    ))
}

fn get_filename_from_path(path: &Path) -> Result<String, String> {
    Ok(match path.file_name() {
        Some(name_os) => name_os
            .to_str()
            .ok_or_else(|| {
                format!(
                    "failed to parse file name for {}: {name_os:?} is not valid UTF-8",
                    path.display()
                )
            })?
            .to_string(),
        None => fl!("filesystem"),
    })
}

pub fn item_from_path<P: Into<PathBuf>>(path: P, sizes: IconSizes) -> Result<Item, String> {
    let path = path.into();
    let name = get_filename_from_path(&path)?;
    let metadata = fs::metadata(&path)
        .map_err(|err| format!("failed to read metadata for {}: {}", path.display(), err))?;
    Ok(item_from_entry(path, name, metadata, sizes))
}

pub fn scan_path(tab_path: &PathBuf, sizes: IconSizes) -> Vec<Item> {
    let mut items = Vec::new();
    let mut hidden_files = Box::from([]);
    let mut remote_scannable = false;

    #[cfg(feature = "gvfs")]
    {
        if let Ok(path_meta) = fs::metadata(tab_path)
            && fs_kind(&path_meta) == FsKind::Gvfs
        {
            let file = gio::File::for_path(tab_path);

            // gio crate expects a comma delimited string
            let attr_string = [
                gio::FILE_ATTRIBUTE_STANDARD_DISPLAY_NAME.as_str(),
                gio::FILE_ATTRIBUTE_TIME_MODIFIED.as_str(),
                gio::FILE_ATTRIBUTE_STANDARD_SIZE.as_str(),
                gio::FILE_ATTRIBUTE_STANDARD_TYPE.as_str(),
                gio::FILE_ATTRIBUTE_STANDARD_NAME.as_str(),
                gio::FILE_ATTRIBUTE_STANDARD_IS_SYMLINK.as_str(),
            ]
            .join(",");

            match gio::prelude::FileExt::enumerate_children(
                &file,
                attr_string.as_str(),
                gio::FileQueryInfoFlags::NONE,
                gio::Cancellable::NONE,
            ) {
                Ok(res) => {
                    remote_scannable = true;
                    items = res
                        .filter_map(|file| {
                            let file = file.ok()?;
                            Some(item_from_gvfs_info(tab_path.join(file.name()), file, sizes))
                        })
                        .collect();
                }
                Err(err) => {
                    log::warn!(
                        "could not enumerate {} via gio: {}",
                        tab_path.display(),
                        err
                    );
                }
            }
        }
    }

    if !remote_scannable {
        match fs::read_dir(tab_path) {
            Ok(entries) => {
                let trash = crate::trash::is_trash_path(tab_path);
                let dir_entries: Vec<fs::DirEntry> = entries
                    .filter_map(|entry_res| {
                        entry_res
                            .inspect_err(|err| {
                                log::warn!(
                                    "failed to read entry in {}: {}",
                                    tab_path.display(),
                                    err
                                )
                            })
                            .ok()
                    })
                    .collect();

                if let Some(hidden_file) = dir_entries
                    .iter()
                    .find(|entry| entry.file_name().to_str() == Some(".hidden"))
                    .map(fs::DirEntry::path)
                    .filter(|path| path.is_file())
                {
                    hidden_files = parse_hidden_file(&hidden_file);
                }

                // Each entry's metadata read and MIME sniff is an independent syscall; run
                // them concurrently so a large or slow-mount directory doesn't serialize
                // every entry's I/O behind the last one.
                items = dir_entries
                    .into_par_iter()
                    .filter_map(|entry| {
                        let path = entry.path();

                        let name = entry
                            .file_name()
                            .into_string()
                            .inspect_err(|name_os| {
                                log::warn!(
                                    "failed to parse entry at {}: {:?} is not valid UTF-8",
                                    path.display(),
                                    name_os
                                )
                            })
                            .ok()?;

                        let metadata = fs::metadata(&path)
                            .inspect_err(|err| {
                                log::warn!(
                                    "failed to read metadata for entry at {}: {}",
                                    path.display(),
                                    err
                                )
                            })
                            .ok()?;

                        if trash {
                            item_from_trash_child(path, name, metadata, sizes)
                        } else {
                            Some(item_from_entry(path, name, metadata, sizes))
                        }
                    })
                    .collect();
            }
            Err(err) => {
                log::warn!("failed to read directory {}: {}", tab_path.display(), err);
            }
        }
    }
    items.sort_unstable_by(|a, b| match (a.metadata.is_dir(), b.metadata.is_dir()) {
        (true, false) => Ordering::Less,
        (false, true) => Ordering::Greater,
        _ => LANGUAGE_SORTER.compare(&a.display_name, &b.display_name),
    });
    for item in &mut items {
        if hidden_files.contains(&item.name) {
            item.hidden = true;
        }
    }
    items
}

pub fn scan_search<F: Fn(SearchItem) -> bool + Sync>(
    search_location: &SearchLocation,
    term: &str,
    show_hidden: bool,
    callback: F,
) {
    if term.is_empty() {
        return;
    }

    let pattern = regex::escape(term);
    let regex = match regex::RegexBuilder::new(&pattern)
        .case_insensitive(true)
        .build()
    {
        Ok(ok) => ok,
        Err(err) => {
            log::warn!("failed to parse regex {pattern:?}: {err}");
            return;
        }
    };

    match search_location {
        SearchLocation::Path(tab_path) => {
            ignore::WalkBuilder::new(tab_path)
                .standard_filters(false)
                .hidden(!show_hidden)
                //TODO: only use this on supported targets
                .same_file_system(true)
                .build_parallel()
                .run(|| {
                    Box::new(|entry_res| {
                        let Ok(entry) = entry_res else {
                            // Skip invalid entries
                            return ignore::WalkState::Skip;
                        };

                        let Some(file_name) = entry.file_name().to_str() else {
                            // Skip anything with an invalid name
                            return ignore::WalkState::Skip;
                        };

                        if regex.is_match(file_name) {
                            let path = entry.path();

                            let metadata = match entry.metadata() {
                                Ok(ok) => ok,
                                Err(err) => {
                                    log::warn!(
                                        "failed to read metadata for entry at {}: {}",
                                        path.display(),
                                        err
                                    );
                                    return ignore::WalkState::Continue;
                                }
                            };

                            if !callback(SearchItem::Path(
                                path.to_path_buf(),
                                file_name.to_string(),
                                metadata,
                            )) {
                                return ignore::WalkState::Quit;
                            }
                        }

                        ignore::WalkState::Continue
                    })
                });
        }
        SearchLocation::Recents => {
            let recent_files = match recently_used_xbel::parse_file() {
                Ok(recent_files) => recent_files,
                Err(err) => {
                    log::warn!("Error reading recent files: {err:?}");
                    return;
                }
            };

            for bookmark in recent_files.bookmarks {
                let path = uri_to_path(bookmark.href);
                if let Some(path) = path
                    && path.exists()
                {
                    let file_name = path.file_name();
                    if let Some(file_name) = file_name {
                        let file_name = file_name.to_string_lossy();
                        if regex.is_match(&file_name) {
                            match path.metadata() {
                                Ok(metadata) => {
                                    if !callback(SearchItem::Path(
                                        path.to_path_buf(),
                                        file_name.to_string(),
                                        metadata,
                                    )) {
                                        break;
                                    }
                                }
                                Err(err) => {
                                    log::warn!(
                                        "failed to read metadata for entry at {}: {}",
                                        path.display(),
                                        err
                                    );
                                }
                            };
                        }
                    }
                }
            }
        }
        SearchLocation::Trash => {
            Trash::scan_search(callback, &regex);
        }
    }
}

fn uri_to_path(uri: String) -> Option<PathBuf> {
    uri.parse::<url::Url>().ok().and_then(|url| {
        //TODO support for external drive or cloud?
        if url.scheme() == "file" {
            url.to_file_path().ok()
        } else {
            None
        }
    })
}

pub fn has_recents() -> bool {
    match recently_used_xbel::parse_file() {
        Ok(recent_files) => !recent_files.bookmarks.is_empty(),
        Err(_) => false,
    }
}

pub fn scan_recents(sizes: IconSizes) -> Vec<Item> {
    let recent_files = match recently_used_xbel::parse_file() {
        Ok(recent_files) => recent_files,
        Err(err) => {
            log::warn!("Error reading recent files: {err:?}");
            return Vec::new();
        }
    };
    let mut recents: Vec<_> = recent_files
        .bookmarks
        .into_iter()
        .filter_map(|bookmark| {
            let path = uri_to_path(bookmark.href)?;
            let last_edit = bookmark.modified.parse::<jiff::Timestamp>().ok()?;
            let last_visit = bookmark.visited.parse::<jiff::Timestamp>().ok()?;

            if path.exists() {
                let file_name = path.file_name()?;
                let name = file_name.to_string_lossy().to_string();

                let metadata = match path.metadata() {
                    Ok(ok) => ok,
                    Err(err) => {
                        log::warn!(
                            "failed to read metadata for entry at {}: {}",
                            path.display(),
                            err
                        );
                        return None;
                    }
                };

                let item = item_from_entry(path, name, metadata, sizes);
                Some((item, last_edit.min(last_visit)))
            } else {
                log::warn!("recent file path does not exist: {}", path.display());
                None
            }
        })
        .collect();

    recents.sort_by_key(|recent| Reverse(recent.1));

    recents.into_iter().take(50).map(|(item, _)| item).collect()
}

pub fn scan_network(uri: &str, sizes: IconSizes) -> Vec<Item> {
    for mounter in MOUNTERS.values() {
        match mounter.network_scan(uri, sizes) {
            Some(Ok(items)) => return items,
            Some(Err(err)) => {
                log::warn!("failed to scan {uri:?}: {err}");
            }
            None => {}
        }
    }
    Vec::new()
}

//TODO: organize desktop items based on display
pub fn scan_desktop(
    tab_path: &PathBuf,
    _display: &str,
    desktop_config: DesktopConfig,
    mut sizes: IconSizes,
) -> Vec<Item> {
    sizes.grid = desktop_config.icon_size;

    let mut items = Vec::new();

    if desktop_config.show_content {
        items.extend(scan_path(tab_path, sizes));
    }

    if desktop_config.show_mounted_drives {
        for mounter in MOUNTERS.values() {
            let Some(mounter_items) = mounter.items(sizes) else {
                continue;
            };
            items.extend(mounter_items.into_iter().filter_map(|mounter_item| {
                let path = mounter_item.path()?;
                // Get most item data from path
                let mut item = match item_from_path(&path, sizes) {
                    Ok(item) => item,
                    Err(err) => {
                        log::warn!(
                            "failed to get item from mounter item {}: {}",
                            path.display(),
                            err
                        );
                        return None;
                    }
                };

                //Override some data with mounter information
                item.name = mounter_item.name();
                item.display_name = Item::display_name(&item.name);

                //TODO: use icon size for mounter item icon
                if let Some(icon) = mounter_item.icon(false) {
                    item.icon_handle_grid.clone_from(&icon);
                    item.icon_handle_list.clone_from(&icon);
                    item.icon_handle_list_condensed = icon;
                }

                Some(item)
            }));
        }
    }

    if desktop_config.show_trash {
        let name = fl!("trash");
        let display_name = Item::display_name(&name);

        let metadata = ItemMetadata::SimpleDir {
            entries: Trash::entries() as u64,
        };

        let (mime, icon_handle_grid, icon_handle_list, icon_handle_list_condensed) = {
            (
                "inode/directory".parse().unwrap(),
                Trash::icon(sizes.grid()),
                Trash::icon(sizes.list()),
                Trash::icon(sizes.list_condensed()),
            )
        };

        items.push(Item {
            name,
            display_name,
            is_mount_point: false,
            is_symlink_dir: false,
            metadata,
            hidden: false,
            location_opt: Some(Location::Trash),
            image_dimensions: None,
            mime,
            icon_handle_grid,
            icon_handle_list,
            icon_handle_list_condensed,
            thumbnail_opt: Some(ItemThumbnail::NotImage),
            button_id: next_button_id(),
            work_id: next_work_id(),
            pos_opt: Cell::new(None),
            rect_opt: Cell::new(None),
            selected: false,
            highlighted: false,
            overlaps_drag_rect: false,
            dir_size: DirSize::NotDirectory,
            cut: false,
            checksums: ChecksumState::default(),
        });
    }

    items
}

#[derive(Clone, Debug)]
pub struct EditLocation {
    pub location: Location,
    pub completions: Option<Vec<(String, PathBuf)>>,
    pub selected: Option<usize>,
}

impl EditLocation {
    pub fn resolve(&self) -> Option<Location> {
        if let Location::Network(uri, ..) = &self.location {
            MOUNTERS
                .values()
                .find_map(|mounter| mounter.dir_info(uri))
                .map(|(uri, display_name, path_opt)| Location::Network(uri, display_name, path_opt))
        } else {
            let Some(selected) = self.selected else {
                return Some(self.location.clone());
            };
            let completions = self.completions.as_ref()?;
            let completion = completions.get(selected)?;
            Some(self.location.with_path(completion.1.clone()).normalize())
        }
    }

    pub fn select(&mut self, forwards: bool) {
        if let Some(completions) = &self.completions {
            if completions.is_empty() {
                self.selected = None;
            } else {
                let mut selected = if forwards {
                    self.selected.and_then(|x| x.checked_add(1)).unwrap_or(0)
                } else {
                    self.selected
                        .and_then(|x| x.checked_sub(1))
                        .unwrap_or(completions.len() - 1)
                };
                if selected >= completions.len() {
                    selected = 0;
                }
                self.selected = Some(selected);

                // Automatically resolve if there is only one completion
                if completions.len() == 1
                    && let Some(resolved) = self.resolve()
                {
                    self.location = resolved;
                    self.selected = None;
                }
            }
        } else {
            self.selected = None;
        }
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum SearchLocation {
    Path(PathBuf),
    Recents,
    Trash,
}

impl std::fmt::Display for SearchLocation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Path(path) => write!(f, "{}", path.display()),
            Self::Recents => write!(f, "recents"),
            Self::Trash => write!(f, "trash"),
        }
    }
}

#[derive(Clone, Debug)]
pub enum SearchItem {
    Path(PathBuf, String, fs::Metadata),
    Trash(TrashItem, TrashItemMetadata),
}

impl From<Location> for EditLocation {
    fn from(location: Location) -> Self {
        Self {
            location,
            completions: None,
            selected: None,
        }
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum Location {
    Desktop(PathBuf, String, DesktopConfig),
    Network(String, String, Option<PathBuf>),
    Path(PathBuf),
    Recents,
    Search(SearchLocation, String, bool, Instant),
    Trash,
}

impl std::fmt::Display for Location {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Desktop(path, display, ..) => {
                write!(f, "{} on display {display}", path.display())
            }
            Self::Network(uri, ..) => write!(f, "{uri}"),
            Self::Path(path) => write!(f, "{}", path.display()),
            Self::Recents => write!(f, "recents"),
            Self::Search(location, term, ..) => {
                write!(f, "search {} for {}", location, term)
            }
            Self::Trash => write!(f, "trash"),
        }
    }
}

impl Location {
    pub fn normalize(&self) -> Self {
        if let Location::Network(uri, ..) = self {
            if !uri.ends_with('/') {
                let mut uri = uri.clone();
                uri.push('/');
                self.with_uri(uri)
            } else {
                self.clone()
            }
        } else if let Some(path) = self.path_opt() {
            // Normalization is lexical: canonicalizing would stat every path component and
            // symlink, blocking the GUI thread on slow mounts. The trailing separator is
            // unconditional because a location addresses a directory, and its string form is the
            // persisted per-folder sort-setting key.
            let mut path = lexically_normalize(&Self::expand_tilde(path.clone()));
            path.push("");
            self.with_path(path)
        } else {
            self.clone()
        }
    }

    pub fn ancestors(&self) -> Vec<(Self, String)> {
        self.path_opt().map_or_else(Default::default, |path| {
            path.ancestors()
                .scan(false, |found_home, ancestor| {
                    (!*found_home).then(|| {
                        let (name, is_home) = folder_name(ancestor);
                        *found_home = is_home;
                        (self.with_path(ancestor.to_path_buf()), name)
                    })
                })
                .collect()
        })
    }

    pub const fn path_opt(&self) -> Option<&PathBuf> {
        match self {
            Self::Desktop(path, ..) => Some(path),
            Self::Path(path) => Some(path),
            Self::Search(SearchLocation::Path(path), ..) => Some(path),
            Self::Network(_, _, path) => path.as_ref(),
            _ => None,
        }
    }

    pub(crate) fn into_path_opt(self) -> Option<PathBuf> {
        match self {
            Self::Desktop(path, ..) => Some(path),
            Self::Path(path) => Some(path),
            Self::Search(SearchLocation::Path(path), ..) => Some(path),
            Self::Network(_, _, path) => path,
            _ => None,
        }
    }

    pub fn with_path(&self, path: PathBuf) -> Self {
        let path = Self::expand_tilde(path);
        match self {
            Self::Desktop(_, display, desktop_config) => {
                Self::Desktop(path, display.clone(), *desktop_config)
            }
            Self::Path(..) => Self::Path(path),
            Self::Search(SearchLocation::Path(_), term, show_hidden, time) => Self::Search(
                SearchLocation::Path(path),
                term.clone(),
                *show_hidden,
                *time,
            ),

            other => other.clone(),
        }
    }

    pub fn with_uri(&self, uri: String) -> Self {
        if let Self::Network(_, name, path) = self {
            Self::Network(uri, name.clone(), path.clone())
        } else {
            self.clone()
        }
    }

    pub fn scan(&self, sizes: IconSizes) -> (Option<Box<Item>>, Vec<Item>) {
        let items = match self {
            Self::Desktop(path, display, desktop_config) => {
                scan_desktop(path, display, *desktop_config, sizes)
            }
            Self::Path(path) => scan_path(path, sizes),
            Self::Search(..) => {
                // Search is done incrementally
                Vec::new()
            }
            Self::Trash => Trash::scan(sizes),
            Self::Recents => scan_recents(sizes),
            Self::Network(uri, _, _) => scan_network(uri, sizes),
        };
        let parent_item_opt = match self.path_opt() {
            Some(path) => match item_from_path(path, sizes) {
                Ok(item) => Some(Box::new(item)),
                Err(err) => {
                    log::warn!("failed to get item for {}: {}", path.display(), err);
                    None
                }
            },
            //TODO: support other locations?
            None => None,
        };
        (parent_item_opt, items)
    }

    pub fn title(&self) -> String {
        match self {
            Self::Desktop(path, _, _) => {
                let (name, _) = folder_name(path);
                name
            }
            Self::Path(path) => {
                let (name, _) = folder_name(path);
                name
            }
            Self::Search(location, term, ..) => {
                let name = match location {
                    SearchLocation::Path(path) => folder_name(path).0,
                    SearchLocation::Trash => fl!("trash"),
                    SearchLocation::Recents => fl!("recents"),
                };

                //TODO: translate
                format!("Search \"{term}\": {name}")
            }
            Self::Trash => {
                fl!("trash")
            }
            Self::Recents => {
                fl!("recents")
            }
            Self::Network(display_name, ..) => display_name.clone(),
        }
    }

    /// Expand a path that starts with "~" with the
    /// user's home directory
    pub fn expand_tilde(path: PathBuf) -> PathBuf {
        let mut components = path.components();
        match components.next() {
            Some(path::Component::Normal(os_str)) if os_str == "~" => {
                if let Some(home) = dirs::home_dir() {
                    home.join(components.as_path())
                } else {
                    path
                }
            }
            _ => path,
        }
    }

    pub fn is_trash(&self) -> bool {
        matches!(
            self,
            Location::Trash | Location::Search(SearchLocation::Trash, ..)
        )
    }

    pub fn is_recents(&self) -> bool {
        matches!(
            self,
            Location::Recents | Location::Search(SearchLocation::Recents, ..)
        )
    }

    /// Returns true if this location supports paste operations (not Trash)
    pub fn supports_paste(&self) -> bool {
        matches!(
            self,
            Self::Desktop(..)
                | Self::Path(..)
                | Self::Search(..)
                | Self::Recents
                | Self::Network(_, _, Some(_))
        )
    }
}

pub struct TaskWrapper(pub cosmic::Task<Message>);

impl From<cosmic::Task<Message>> for TaskWrapper {
    fn from(task: cosmic::Task<Message>) -> Self {
        Self(task)
    }
}

impl fmt::Debug for TaskWrapper {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TaskWrapper").finish()
    }
}

#[derive(Debug)]
pub enum Command {
    Action(Action),
    Surface(cosmic::surface::Action<Message>),
    AddNetworkDrive,
    AddToSidebar(PathBuf),
    AutoScroll(Option<f32>),
    ChangeLocation(String, Location, Option<Vec<PathBuf>>),
    Delete(Vec<PathBuf>),
    DropFiles(PathBuf, ClipboardPaste),
    ClearRecents,
    EditLocationSubmit(EditLocation, bool),
    EmptyTrash,
    #[cfg(feature = "desktop")]
    ExecEntryAction(String),
    Iced(TaskWrapper),
    OpenFile(Vec<PathBuf>),
    OpenInNewTab(PathBuf),
    OpenInNewWindow(PathBuf),
    OpenTrash,
    ParseDesktopEntry(PathBuf, u64),
    Preview(PreviewKind),
    PreviewPath(PathBuf),
    RunContextAction(usize),
    SetOpenWith(Mime, String),
    SetPermissions(PathBuf, u32),
    SetMultiplePermissions(Vec<(PathBuf, u32)>),
    SetSort(String, HeadingOptions, bool),
    WindowDrag,
    WindowToggleMaximize,
}

#[derive(Clone, Debug)]
pub enum Message {
    AddNetworkDrive,
    AutoScroll(Option<f32>),
    Click(Option<usize>),
    DoubleClick(Option<usize>),
    ClickRelease(Option<usize>),
    Config(TabConfig),
    ContextAction(Action),
    RightClickBackground,
    ContextMenuOpen(bool),
    Surface(cosmic::surface::Action<Message>),
    DesktopEntryParsed(PathBuf, u64, Option<menu::DesktopEntryActions>),
    LocationContextMenuIndex(Option<usize>),
    LocationMenuAction(LocationMenuAction),
    Drag(Option<Rectangle>),
    DragEnd,
    EditLocation(Option<EditLocation>),
    EditLocationComplete(usize),
    EditLocationEnable,
    EditLocationSubmit,
    EditLocationTab,
    OpenInNewTab(PathBuf),
    ClearRecents,
    EmptyTrash,
    #[cfg(feature = "desktop")]
    ExecEntryAction(u64, usize),
    Gallery(bool),
    GalleryPrevious,
    GalleryNext,
    GalleryToggle,
    GoNext,
    GoPrevious,
    ItemDown,
    ItemLeft,
    ItemPageDown,
    ItemPageUp,
    ItemRight,
    ItemUp,
    Location(Location),
    LocationSelect(Location, Vec<PathBuf>),
    LocationUp,
    Open(Option<PathBuf>),
    Reload,
    RightClick(Option<Point>, Option<usize>),
    MiddleClick(usize),
    Resize(Rectangle),
    Scroll(Viewport),
    ScrollTab(f32),
    ScrollToFocused,
    SearchContext(Location, SearchContextWrapper),
    SearchReady(bool),
    SelectAll,
    SelectFirst,
    SelectLast,
    SetOpenWith(Mime, String),
    RunContextAction(usize),
    SetPermissions(PathBuf, u32),
    ShiftPermissions(Option<(PathBuf, u32)>, u32, u32),
    SetSort(HeadingOptions, bool),
    TabComplete(PathBuf, Vec<(String, PathBuf)>),
    Thumbnail(PathBuf, ItemThumbnail, u64),
    ToggleSort(HeadingOptions),
    Drop(Option<(Location, ClipboardPaste)>),
    DndHover(Location),
    DndEnter(Location),
    DndLeave(Location),
    WindowDrag,
    WindowToggleMaximize,
    ZoomIn,
    ZoomOut,
    HighlightDeactivate(usize),
    HighlightActivate(usize),
    DirectorySize(PathBuf, DirSize, u64),
    DirectoryChildren(PathBuf, Option<usize>, u64),
    Checksums(PathBuf, ChecksumState, u64),
    CalculateChecksums(PathBuf),
    CopyChecksum(String),
    ImageDecodeFailed(PathBuf, u64),
    ImageDecoded(PathBuf, u32, u32, Vec<u8>, Option<(u32, u32)>, u64), // path, width, height, pixels, display_size, generation
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum LocationMenuAction {
    OpenInNewTab(usize),
    OpenInNewWindow(usize),
    Preview(usize),
    AddToSidebar(usize),
}

impl MenuAction for LocationMenuAction {
    type Message = Message;

    fn message(&self) -> Self::Message {
        Message::LocationMenuAction(*self)
    }
}

#[derive(Clone, Debug)]
pub enum DirSize {
    Calculating(Controller),
    Directory(u64),
    NotDirectory,
    Error(String),
}

/// Checksums computed for a file. Only SHA256 is currently exposed; see
/// [`calculate_checksums`] for how to add more.
#[derive(Clone, Debug, Default)]
pub struct FileChecksums {
    pub sha256: String,
}

/// State of checksum computation for a file.
#[derive(Clone, Debug, Default)]
pub enum ChecksumState {
    #[default]
    NotCalculated,
    Calculating,
    Calculated(FileChecksums),
    Error(String),
}

#[derive(Clone, Debug)]
pub enum ItemMetadata {
    Path {
        metadata: Metadata,
        children_opt: Option<usize>,
    },
    Trash {
        metadata: trash::TrashItemMetadata,
        entry: trash::TrashItem,
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
            Self::Trash { metadata, .. } => match metadata.size {
                trash::TrashItemSize::Entries(_) => true,
                trash::TrashItemSize::Bytes(_) => false,
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
            Self::Trash { metadata, .. } => match metadata.size {
                TrashItemSize::Bytes(size) => Some(size),
                TrashItemSize::Entries(_) => None,
            },
            #[cfg(feature = "gvfs")]
            Self::GvfsPath { size_opt, .. } => *size_opt,
            _ => None,
        }
    }

    /// A directory whose child count the scan deliberately left unread.
    fn children_count_pending(&self) -> bool {
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

    fn set_children_count(&mut self, children: usize) {
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

#[derive(Debug)]
pub enum ItemThumbnail {
    NotImage,
    Image(widget::image::Handle, Option<(u32, u32)>),
    Svg(widget::svg::Handle),
    Text(widget::text_editor::Content),
}

impl Clone for ItemThumbnail {
    fn clone(&self) -> Self {
        match self {
            Self::NotImage => Self::NotImage,
            Self::Image(handle, size_opt) => Self::Image(handle.clone(), *size_opt),
            Self::Svg(handle) => Self::Svg(handle.clone()),
            // Content cannot be cloned simply
            Self::Text(content) => {
                Self::Text(widget::text_editor::Content::with_text(&content.text()))
            }
        }
    }
}

impl ItemThumbnail {
    pub fn new(
        path: &Path,
        metadata: ItemMetadata,
        mime: mime::Mime,
        mut thumbnail_size: u32,
        max_mem: u64,
        jobs: usize,
        max_size_mb: u64,
    ) -> Self {
        let thumbnail_cacher =
            ThumbnailCacher::new(path, ThumbnailSize::from_pixel_size(thumbnail_size));
        match thumbnail_cacher.as_ref() {
            Ok(cache) => match cache.get_cached_thumbnail() {
                CachedThumbnail::Valid((thumbnail_path, _)) => {
                    let original_dims = image::image_dimensions(path).ok();

                    return Self::Image(
                        widget::image::Handle::from_path(thumbnail_path),
                        original_dims,
                    );
                }
                CachedThumbnail::Failed => {
                    if mime.type_() != mime::IMAGE {
                        return Self::NotImage;
                    }
                }
                CachedThumbnail::RequiresUpdate(size) => {
                    thumbnail_size = size.pixel_size();
                }
            },
            Err(err) => {
                log::warn!(
                    "failed to create ThumbnailCache for {}: {}",
                    path.display(),
                    err
                );
            }
        }

        let size = metadata.file_size().unwrap_or_default();
        let check_size = |thumbnailer: &str, max_size| {
            if size <= max_size {
                true
            } else {
                log::warn!(
                    "skipping internal {} thumbnailer for {}: file size {} is larger than {}",
                    thumbnailer,
                    path.display(),
                    format_size(size),
                    format_size(max_size)
                );
                false
            }
        };

        let mut tried_supported_file = false;
        // First try built-in image thumbnailer
        if mime.type_() == mime::IMAGE && check_size("image", max_size_mb * 1000 * 1000) {
            // Check if image dimensions would exceed available memory budget
            // The GPU tiling system can handle large images, but we still need to decode them first
            let dimensions_ok = match image::image_dimensions(path) {
                Ok((width, height)) => {
                    if exceeds_memory_limit(width, height, max_mem) {
                        log::warn!(
                            "skipping thumbnail generation for {}: {}x{} image would exceed {}MB memory budget",
                            path.display(),
                            width,
                            height,
                            max_mem
                        );
                        false
                    } else {
                        if should_use_tiling(width, height) {
                            log::info!(
                                "Large image {}x{} detected, will use GPU tiling for display",
                                width,
                                height
                            );
                        }
                        true
                    }
                }
                Err(err) => {
                    log::debug!(
                        "failed to read dimensions for {}: {}, will try decoding",
                        path.display(),
                        err
                    );
                    true // If we can't read dimensions, try anyway
                }
            };

            if !dimensions_ok {
                // Skip this image entirely since it is too large to safely decode
                return Self::NotImage;
            }

            tried_supported_file = true;
            let dyn_img = match image::ImageReader::open(path)
                .and_then(image::ImageReader::with_guessed_format)
            {
                Ok(mut reader) => {
                    let mut limits = image::Limits::default();
                    let max_ram = max_mem * 1000 * 1000 / jobs as u64;
                    limits.max_alloc = Some(max_ram);
                    reader.limits(limits);
                    match reader.decode() {
                        Ok(reader) => Some(reader),
                        Err(err) => {
                            log::warn!("failed to decode {}: {}", path.display(), err);
                            None
                        }
                    }
                }
                Err(err) => {
                    log::warn!("failed to read {}: {}", path.display(), err);
                    None
                }
            };

            if let Some(dyn_img) = dyn_img {
                let (img_width, img_height) = (dyn_img.width(), dyn_img.height());

                if let Ok(cacher) = thumbnail_cacher.as_ref() {
                    match cacher.update_with_image(dyn_img) {
                        Ok(thumb_path) => {
                            return Self::Image(
                                widget::image::Handle::from_path(thumb_path),
                                Some((img_width, img_height)),
                            );
                        }
                        Err(err) => {
                            log::warn!("cacher failed to decode {}: {}", path.display(), err);
                        }
                    }
                } else {
                    // Fallback for when thumbnail cacher isn't available.
                    let thumbnail = dyn_img
                        .thumbnail(thumbnail_size, thumbnail_size)
                        .into_rgba8();
                    return Self::Image(
                        widget::image::Handle::from_rgba(
                            thumbnail.width(),
                            thumbnail.height(),
                            thumbnail.into_raw(),
                        ),
                        Some((img_width, img_height)),
                    );
                }
            }
        }

        // Try external thumbnailers.
        let thumbnail_dir = thumbnail_cacher
            .as_ref()
            .ok()
            .map(ThumbnailCacher::thumbnail_dir);
        if let Some((item_thumbnail, temp_file)) =
            Self::generate_thumbnail_external(path, &mime, thumbnail_size, thumbnail_dir)
        {
            if let Ok(cache) = thumbnail_cacher
                && let Err(err) = cache.update_with_temp_file(temp_file)
            {
                log::warn!("failed to update cache for {}: {}", path.display(), err);
            }
            return item_thumbnail;
        }

        tried_supported_file = tried_supported_file || !thumbnailer(&mime).is_empty();

        // Try internal thumbnailers that don't get cached.
        //TODO: adjust limits for internal thumbnailers as desired
        if mime.type_() == mime::IMAGE
            && mime.subtype() == mime::SVG
            && check_size("svg", SVG_PREVIEW_MAX_FILE_BYTES)
        {
            tried_supported_file = true;
            // Try built-in svg thumbnailer
            match fs::read(path) {
                Ok(data) => {
                    //TODO: validate SVG data
                    return Self::Svg(widget::svg::Handle::from_memory(data));
                }
                Err(err) => {
                    log::warn!("failed to read {}: {}", path.display(), err);
                }
            }
        } else if mime.type_() == mime::TEXT && check_size("text", TEXT_PREVIEW_MAX_FILE_BYTES) {
            tried_supported_file = true;
            if size > 0 {
                // Reuse size from metadata above; cap allocation and read
                let read_cap = (size.min(TEXT_PREVIEW_MAX_BYTES as u64)) as usize;
                let mut buf = vec![0u8; read_cap];
                match File::open(path).and_then(|f| {
                    let n = Read::read(&mut f.take(read_cap as u64), &mut buf)?;
                    buf.truncate(n);
                    Ok(())
                }) {
                    Ok(()) => {
                        let text = match std::str::from_utf8(&buf) {
                            Ok(s) => s.to_string(),
                            Err(e) => {
                                // Use only the valid UTF-8 prefix (slice is guaranteed valid by valid_up_to())
                                std::str::from_utf8(&buf[..e.valid_up_to()])
                                    .unwrap_or("")
                                    .to_string()
                            }
                        };
                        if !text.is_empty() {
                            return Self::Text(widget::text_editor::Content::with_text(&text));
                        }
                    }
                    Err(err) => {
                        log::warn!("failed to read {}: {}", path.display(), err);
                    }
                }
            }
            // size == 0: empty file or unknown size; skip read and allocation
        }

        // If we weren't able to create a thumbnail, but we should have
        // been able to, create a fail marker so that it isn't tried the
        // next time.
        if let Ok(cacher) = thumbnail_cacher
            && tried_supported_file
            && let Err(err) = cacher.create_fail_marker()
        {
            log::warn!(
                "failed to create thumbnail fail marker for {}: {}",
                path.display(),
                err
            );
        }

        Self::NotImage
    }

    fn generate_thumbnail_external(
        path: &Path,
        mime: &mime::Mime,
        thumbnail_size: u32,
        thumbnail_dir: Option<&Path>,
    ) -> Option<(Self, NamedTempFile)> {
        // Try external thumbnailers
        for thumbnailer in thumbnailer(mime) {
            let is_evince = thumbnailer.exec.starts_with("evince-thumbnailer ");
            let prefix = if is_evince {
                //TODO: apparmor config for evince-thumbnailer does not allow /tmp/cosmic-files*
                "gnome-desktop-"
            } else {
                "cosmic-files-"
            };

            // It's preferable to create the tempfile in the same directory as the final cached
            // thumbnail to ensure that no copies across filesytems need to be made. However,
            // the apparmor config for evince-thumbnailer does not allow this, so we need to
            // fallback to the system tempdir.
            let dir = if is_evince { None } else { thumbnail_dir };
            let file = match dir {
                Some(d) => tempfile::Builder::new().prefix(prefix).tempfile_in(d),
                None => tempfile::Builder::new().prefix(prefix).tempfile(),
            };
            let file = match file {
                Ok(ok) => ok,
                Err(err) => {
                    log::warn!(
                        "failed to create temporary file for thumbnail of {}: {}",
                        path.display(),
                        err
                    );
                    continue;
                }
            };

            let Some(mut command) = thumbnailer.command(path, file.path(), thumbnail_size) else {
                continue;
            };
            match command.status() {
                Ok(status) => {
                    if status.success() {
                        match image::ImageReader::open(file.path())
                            .and_then(ImageReader::with_guessed_format)
                        {
                            Ok(reader) => match reader.decode().map(DynamicImage::into_rgba8) {
                                Ok(image) => {
                                    return Some((
                                        Self::Image(
                                            widget::image::Handle::from_rgba(
                                                image.width(),
                                                image.height(),
                                                image.into_raw(),
                                            ),
                                            None,
                                        ),
                                        file,
                                    ));
                                }
                                Err(err) => {
                                    log::warn!("failed to decode {}: {}", path.display(), err);
                                }
                            },
                            Err(err) => {
                                log::warn!("failed to read {}: {}", path.display(), err);
                            }
                        }
                    } else {
                        log::warn!(
                            "failed to run {:?} for {}: {}",
                            thumbnailer,
                            path.display(),
                            status
                        );
                    }
                }
                Err(err) => {
                    log::warn!(
                        "failed to run {thumbnailer:?} for {}: {}",
                        path.display(),
                        err
                    );
                }
            }
        }

        None
    }
}

/// Identifies an entry's content, so a rescan can tell an unchanged entry from a rewritten one.
type ItemFingerprint = (Option<SystemTime>, Option<u64>);

/// State an [`Item`] accumulated after the directory scan, moved onto its replacement when a
/// rescan produces the same entry unchanged.
struct ItemCarryover {
    children_opt: Option<usize>,
    image_dimensions: Option<(u32, u32)>,
    icon_handle_grid: widget::icon::Handle,
    icon_handle_list: widget::icon::Handle,
    icon_handle_list_condensed: widget::icon::Handle,
    thumbnail_opt: Option<ItemThumbnail>,
    checksums: ChecksumState,
}

impl From<Item> for ItemCarryover {
    fn from(item: Item) -> Self {
        Self {
            children_opt: item.metadata.children_count().copied(),
            image_dimensions: item.image_dimensions,
            icon_handle_grid: item.icon_handle_grid,
            icon_handle_list: item.icon_handle_list,
            icon_handle_list_condensed: item.icon_handle_list_condensed,
            thumbnail_opt: item.thumbnail_opt,
            checksums: item.checksums,
        }
    }
}

impl ItemCarryover {
    /// Only finished results are moved over. An in-progress computation belongs to the item it
    /// was started for, and is restarted for the replacement.
    fn apply(self, item: &mut Item) {
        if let Some(children) = self.children_opt {
            item.metadata.set_children_count(children);
        }
        if self.thumbnail_opt.is_some() {
            item.image_dimensions = self.image_dimensions;
            item.icon_handle_grid = self.icon_handle_grid;
            item.icon_handle_list = self.icon_handle_list;
            item.icon_handle_list_condensed = self.icon_handle_list_condensed;
            item.thumbnail_opt = self.thumbnail_opt;
        }
        if matches!(self.checksums, ChecksumState::Calculated(_)) {
            item.checksums = self.checksums;
        }
    }
}

#[derive(Clone, Debug)]
pub struct Item {
    pub name: String,
    pub is_mount_point: bool,
    pub is_symlink_dir: bool,
    pub display_name: String,
    pub metadata: ItemMetadata,
    pub hidden: bool,
    pub location_opt: Option<Location>,
    pub mime: Mime,
    pub image_dimensions: Option<(u32, u32)>,
    pub icon_handle_grid: widget::icon::Handle,
    pub icon_handle_list: widget::icon::Handle,
    pub icon_handle_list_condensed: widget::icon::Handle,
    pub thumbnail_opt: Option<ItemThumbnail>,
    pub button_id: widget::Id,
    pub(crate) work_id: u64,
    pub pos_opt: Cell<Option<(usize, usize)>>,
    pub rect_opt: Cell<Option<Rectangle>>,
    pub selected: bool,
    pub highlighted: bool,
    pub cut: bool,
    pub overlaps_drag_rect: bool,
    pub dir_size: DirSize,
    pub checksums: ChecksumState,
}

impl Item {
    fn fingerprint(&self) -> ItemFingerprint {
        (self.metadata.modified(), self.metadata.file_size())
    }

    fn icon(&self, handle: &widget::icon::Handle, size: u16) -> Element<'static, Message> {
        let icon = widget::icon::icon(handle.clone())
            .content_fit(ContentFit::Contain)
            .size(size);
        if !self.is_symlink_dir {
            return icon.into();
        }

        static SYMLINK_BADGE: LazyLock<widget::icon::Handle> = LazyLock::new(|| {
            widget::icon::from_svg_bytes(
                include_bytes!("../res/icons/emblem-symbolic-link.svg").as_slice(),
            )
        });
        let badge = widget::icon::icon(SYMLINK_BADGE.clone()).size((size / 3).max(8).min(size));
        stack![
            icon,
            widget::container(badge)
                .align_left(Length::Fixed(f32::from(size)))
                .align_bottom(Length::Fixed(f32::from(size)))
        ]
        .into()
    }

    fn display_name(name: &str) -> String {
        // In order to wrap at periods and underscores, add a zero width space after each one
        name.replace('.', ".\u{200B}").replace('_', "_\u{200B}")
    }

    /// Text widget for a filename in grid/icon view: word-or-glyph wrapping, middle-ellipsized to 3 lines.
    fn grid_display_name<'a>(
        name: impl Into<Cow<'a, str>> + 'a,
    ) -> widget::Text<'a, cosmic::Theme, cosmic::Renderer> {
        widget::text::body(name)
            .wrapping(text::Wrapping::WordOrGlyph)
            .align_x(text::Alignment::Center)
            .ellipsize(text::Ellipsize::Middle(text::EllipsizeHeightLimit::Lines(
                3,
            )))
    }

    /// Text widget for a filename in list view: word-or-glyph wrapping, middle-ellipsized to 1 line.
    fn list_display_name<'a>(
        name: impl Into<Cow<'a, str>> + 'a,
    ) -> widget::Text<'a, cosmic::Theme, cosmic::Renderer> {
        widget::text::body(name)
            .wrapping(text::Wrapping::WordOrGlyph)
            .ellipsize(text::Ellipsize::Middle(text::EllipsizeHeightLimit::Lines(
                1,
            )))
    }

    pub fn path_opt(&self) -> Option<&PathBuf> {
        self.location_opt.as_ref()?.path_opt()
    }

    pub fn can_gallery(&self) -> bool {
        self.mime.type_() == mime::IMAGE || self.mime.type_() == mime::TEXT
    }

    /// Full metadata available without filesystem I/O, so it is safe to read on the GUI thread.
    /// Only a local path caches it; every other kind of item returns `None` and is rendered from
    /// the cached [`ItemMetadata`] fields.
    pub fn cached_metadata(&self) -> Option<Metadata> {
        match &self.metadata {
            ItemMetadata::Path { metadata, .. } => Some(metadata.clone()),
            _ => None,
        }
    }

    /// Human-readable size text for a directory item, from the cached dir-size calculation
    /// (empty when there is nothing to show).
    fn dir_size_text(&self) -> String {
        match &self.dir_size {
            DirSize::Calculating(_) => fl!("calculating"),
            DirSize::Directory(size) => format_size(*size),
            DirSize::NotDirectory => String::new(),
            DirSize::Error(err) => err.clone(),
        }
    }

    fn preview(&self) -> Element<'_, Message> {
        let spacing = cosmic::theme::spacing();
        // This loads the image only if thumbnailing worked
        let icon = widget::icon::icon(self.icon_handle_grid.clone())
            .content_fit(ContentFit::Contain)
            .size(IconSizes::default().grid())
            .into();
        match self
            .thumbnail_opt
            .as_ref()
            .unwrap_or(&ItemThumbnail::NotImage)
        {
            ItemThumbnail::NotImage => icon,
            ItemThumbnail::Image(handle, _original_dims) => {
                // Preview pane: ALWAYS show thumbnail for instant, responsive UI
                // Full resolution loading happens in gallery mode
                widget::image(handle.clone()).into()
            }
            ItemThumbnail::Svg(handle) => widget::svg(handle.clone()).into(),
            ItemThumbnail::Text(content) => widget::text_editor::text_editor(content)
                .style(text_editor_class)
                .width(THUMBNAIL_SIZE as f32)
                .height(Length::Fixed(THUMBNAIL_SIZE as f32))
                .padding(spacing.space_xxs)
                .into(),
        }
    }

    pub fn preview_actions(&self) -> Element<'_, Message> {
        let mut row = widget::row::with_capacity(3)
            .align_y(Alignment::Center)
            .spacing(theme::spacing().space_xxs)
            .push(
                widget::button::icon(widget::icon::from_name("go-previous-symbolic"))
                    .on_press(Message::ItemLeft),
            )
            .push(
                widget::button::icon(widget::icon::from_name("go-next-symbolic"))
                    .on_press(Message::ItemRight),
            );
        if self.can_gallery()
            && let Some(_path) = self.path_opt()
        {
            row = row.push(
                widget::button::icon(widget::icon::from_name("view-fullscreen-symbolic"))
                    .on_press(Message::Gallery(true)),
            );
        }
        row.into()
    }

    pub fn preview_view<'a>(
        &'a self,
        mime_app_cache_opt: Option<&'a mime_app::MimeAppCache>,
        military_time: bool,
    ) -> Element<'a, Message> {
        let cosmic_theme::Spacing {
            space_xxxs,
            space_m,
            ..
        } = theme::spacing();

        let mut column = widget::column::with_capacity(4).spacing(space_m);

        column = column.push(
            widget::container(self.preview())
                .center_x(Length::Fill)
                .max_height(THUMBNAIL_SIZE as f32),
        );

        let mut details = widget::column::with_capacity(8).spacing(space_xxxs);
        details = details.push(widget::selectable_text::heading(self.name.clone()));
        details = details.push(widget::text::body(fl!(
            "type",
            mime = self.mime.to_string()
        )));
        let mut settings = Vec::new();
        if let Some(mime_app_cache) = mime_app_cache_opt {
            let mime_apps = mime_app_cache.get_apps_for_mime(&self.mime, false);
            if !mime_apps.is_empty() {
                let (names, icons) = mime_apps
                    .iter()
                    .map(|(app, _)| (Cow::Owned(app.name.clone()), app.icon()))
                    .collect::<(Vec<_>, Vec<_>)>();
                settings.push(
                    widget::settings::item::builder(fl!("open-with")).control(
                        Element::from(
                            widget::dropdown(
                                names,
                                mime_apps.iter().position(|(x, _)| x.is_default(&self.mime)),
                                move |index| index,
                            )
                            .icons(Cow::Owned(icons)),
                        )
                        .map(move |index| {
                            let mime_app = &mime_apps[index].0;
                            Message::SetOpenWith(self.mime.clone(), mime_app.id.clone())
                        }),
                    ),
                );
            }
        }

        if self.metadata.is_dir() {
            if let Some(children) = self.metadata.children_count() {
                details = details.push(widget::text::body(fl!("items", items = children)));
            }
            let size = self.dir_size_text();
            if !size.is_empty() {
                details = details.push(widget::text::body(fl!("item-size", size = size)));
            }
        } else if let Some(size) = self.metadata.file_size() {
            details = details.push(widget::text::body(fl!(
                "item-size",
                size = format_size(size)
            )));
        }

        let date_time_formatter = date_time_formatter(military_time);
        let time_formatter = time_formatter(military_time);

        // Creation and access times, and the permission controls below, need full metadata, which
        // only a local path caches: any other item would have to be stat-ed here, on the GUI
        // thread, every frame the preview is shown.
        let cached_metadata = self.cached_metadata();

        if let Some(time) = cached_metadata.as_ref().and_then(|m| m.created().ok()) {
            details = details.push(widget::selectable_text::body(fl!(
                "item-created",
                created = format_time(time, &date_time_formatter, &time_formatter).to_string()
            )));
        }

        if let Some(time) = self.metadata.modified() {
            details = details.push(widget::selectable_text::body(fl!(
                "item-modified",
                modified = format_time(time, &date_time_formatter, &time_formatter).to_string()
            )));
        }

        if let Some(time) = cached_metadata.as_ref().and_then(|m| m.accessed().ok()) {
            details = details.push(widget::selectable_text::body(fl!(
                "item-accessed",
                accessed = format_time(time, &date_time_formatter, &time_formatter).to_string()
            )));
        }

        #[cfg(unix)]
        if let Some(metadata) = cached_metadata.as_ref()
            && let Some(path) = self.path_opt()
        {
            let mode = metadata.mode();

            let user_name = uzers::get_user_by_uid(metadata.uid())
                .and_then(|user| user.name().to_str().map(ToOwned::to_owned))
                .unwrap_or_default();
            let user_path = path.clone();
            settings.push(
                widget::settings::item::builder(user_name)
                    .description(fl!("owner"))
                    .control(widget::dropdown(
                        Cow::Borrowed(MODE_NAMES.as_slice()),
                        Some(get_mode_part(mode, MODE_SHIFT_USER).try_into().unwrap()),
                        move |selected| {
                            Message::SetPermissions(
                                user_path.clone(),
                                set_mode_part(mode, MODE_SHIFT_USER, selected.try_into().unwrap()),
                            )
                        },
                    )),
            );

            let group_name = uzers::get_group_by_gid(metadata.gid())
                .and_then(|group| group.name().to_str().map(ToOwned::to_owned))
                .unwrap_or_default();
            let group_path = path.clone();
            settings.push(
                widget::settings::item::builder(group_name)
                    .description(fl!("group"))
                    .control(widget::dropdown(
                        Cow::Borrowed(MODE_NAMES.as_slice()),
                        Some(get_mode_part(mode, MODE_SHIFT_GROUP).try_into().unwrap()),
                        move |selected| {
                            Message::SetPermissions(
                                group_path.clone(),
                                set_mode_part(mode, MODE_SHIFT_GROUP, selected.try_into().unwrap()),
                            )
                        },
                    )),
            );

            let other_path = path.clone();
            settings.push(
                widget::settings::item::builder(fl!("other")).control(widget::dropdown(
                    Cow::Borrowed(MODE_NAMES.as_slice()),
                    Some(get_mode_part(mode, MODE_SHIFT_OTHER).try_into().unwrap()),
                    move |selected| {
                        Message::SetPermissions(
                            other_path.clone(),
                            set_mode_part(mode, MODE_SHIFT_OTHER, selected.try_into().unwrap()),
                        )
                    },
                )),
            );
        }

        if let Some((width, height)) = self.image_dimensions {
            details = details.push(widget::text::body(format!("{width}x{height}")));
        }
        column = column.push(details);

        if !self.metadata.is_dir()
            && let Some(path) = self.path_opt()
        {
            let control: Element<'_, Message> = match &self.checksums {
                ChecksumState::NotCalculated => widget::button::standard(fl!("calculate"))
                    .on_press(Message::CalculateChecksums(path.clone()))
                    .into(),
                ChecksumState::Calculating => widget::row::with_capacity(2)
                    .align_y(Alignment::Center)
                    .spacing(space_xxxs)
                    .push(widget::indeterminate_circular().size(16.0))
                    .push(widget::text::body(fl!("calculating")))
                    .into(),
                ChecksumState::Calculated(checksums) => {
                    let value = checksums.sha256.clone();
                    // Middle-ellipsize the digest to fit, full value on hover.
                    let value_text = widget::tooltip(
                        widget::text::body(value.clone())
                            .font(cosmic::font::mono())
                            .width(Length::Fill)
                            .wrapping(text::Wrapping::None)
                            .ellipsize(text::Ellipsize::Middle(text::EllipsizeHeightLimit::Lines(
                                1,
                            ))),
                        widget::text::body(value.clone()),
                        widget::tooltip::Position::Bottom,
                    );
                    let copy_button = widget::button::icon(
                        widget::icon::from_name("edit-copy-symbolic").size(16),
                    )
                    .on_press(Message::CopyChecksum(value.clone()))
                    .tooltip(fl!("copy"));
                    widget::row::with_capacity(2)
                        .align_y(Alignment::Center)
                        .spacing(space_xxxs)
                        .push(value_text)
                        .push(copy_button)
                        .into()
                }
                ChecksumState::Error(err) => {
                    widget::text::body(format!("{}: {}", fl!("error"), err)).into()
                }
            };
            settings.push(
                widget::settings::item::builder(fl!("checksum", kind = "SHA256")).control(control),
            );
        }

        if let Some(path) = self.path_opt()
            && self.selected
        {
            // Decide navigate-vs-open from cached metadata so Message::Open's handler never
            // stats the path on the GUI thread.
            let message = if self.metadata.is_dir() {
                Message::Location(Location::Path(path.clone()))
            } else {
                Message::Open(Some(path.clone()))
            };
            column = column.push(widget::button::standard(fl!("open")).on_press(message));
        }

        if !settings.is_empty() {
            let mut section = widget::settings::section();
            section = section.extend(settings);
            column = column.push(section);
        }

        column.into()
    }

    pub fn replace_view(&self, heading: String, military_time: bool) -> Element<'_, Message> {
        let cosmic_theme::Spacing { space_xxxs, .. } = theme::spacing();

        let mut row = widget::row::with_capacity(2).spacing(space_xxxs);
        row = row.push(self.preview());

        let mut column = widget::column::with_capacity(3).spacing(space_xxxs);
        column = column.push(widget::text::heading(heading));

        //TODO: translate!
        //TODO: correct display of folder size?
        if let ItemMetadata::Path {
            metadata,
            children_opt,
        } = &self.metadata
        {
            if metadata.is_dir() {
                if let Some(children) = children_opt {
                    column = column.push(widget::text::body(format!("Items: {children}")));
                }
            } else {
                column = column.push(widget::text::body(format!(
                    "Size: {}",
                    format_size(metadata.len())
                )));
            }
            if let Ok(time) = metadata.modified() {
                let date_time_formatter = date_time_formatter(military_time);
                let time_formatter = time_formatter(military_time);

                column = column.push(widget::text::body(format!(
                    "Last modified: {}",
                    format_time(time, &date_time_formatter, &time_formatter)
                )));
            }
        } else {
            //TODO: other metadata
        }

        row = row.push(column);
        row.into()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub enum View {
    Grid,
    List,
}
#[derive(Clone, Copy, Debug, Hash, PartialEq, PartialOrd, Ord, Eq, Deserialize, Serialize)]
pub enum HeadingOptions {
    Name = 0,
    Modified,
    Size,
    TrashedOn,
}

impl fmt::Display for HeadingOptions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Name => write!(f, "{}", fl!("name")),
            Self::Modified => write!(f, "{}", fl!("modified")),
            Self::Size => write!(f, "{}", fl!("size")),
            Self::TrashedOn => write!(f, "{}", fl!("trashed-on")),
        }
    }
}

impl HeadingOptions {
    pub fn names() -> Vec<String> {
        vec![
            Self::Name.to_string(),
            Self::Modified.to_string(),
            Self::Size.to_string(),
            Self::TrashedOn.to_string(),
        ]
    }
}

#[derive(Clone, Debug)]
pub enum Mode {
    App,
    Desktop,
    Dialog(DialogKind),
}

impl Mode {
    /// Whether multiple files can be selected in this mode
    pub fn multiple(&self) -> bool {
        match self {
            Self::App | Self::Desktop => true,
            Self::Dialog(dialog) => dialog.multiple(),
        }
    }
}

struct SearchContext {
    // Items are built off-thread in the search worker (item_from_search_item does read_dir /
    // gvfs / image-header I/O); the GUI-thread drain only inserts the finished Item.
    results_rx: mpsc::Receiver<Item>,
    ready: Arc<atomic::AtomicBool>,
    last_modified_opt: Arc<RwLock<Option<SystemTime>>>,
}

pub struct SearchContextWrapper(Option<SearchContext>);

impl Clone for SearchContextWrapper {
    fn clone(&self) -> Self {
        Self(None)
    }
}

impl fmt::Debug for SearchContextWrapper {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SearchContextWrapper").finish()
    }
}

/// Heading to sort by, ascending flag, and whether folders are grouped first.
pub(crate) type SortOptions = (HeadingOptions, bool, bool);

// TODO when creating items, pass <Arc<SelectedItems>> to each item
// as a drag data, so that when dnd is initiated, they are all included
pub struct Tab {
    //TODO: make more items private
    pub location: Location,
    pub location_ancestors: Vec<(Location, String)>,
    pub location_title: String,
    /// Breadcrumb whose context menu is open, drawn as active while it shows
    pub location_context_menu_index: Option<usize>,
    pub context_menu_desktop_entry: Option<(PathBuf, u64, menu::DesktopEntryActions)>,
    context_menu_snapshot: menu::MenuSnapshot<Message>,
    pub mode: Mode,
    pub scroll_opt: Option<AbsoluteOffset>,
    pub size_opt: Cell<Option<Size>>,
    pub content_height_opt: Cell<Option<f32>>,
    pub viewport_opt: Option<Rectangle>,
    pub item_view_size_opt: Cell<Option<Size>>,
    pub edit_location: Option<EditLocation>,
    pub edit_location_id: widget::Id,
    pub history_i: usize,
    pub history: Vec<Location>,
    pub config: TabConfig,
    pub thumb_config: ThumbCfg,
    pub sort_name: HeadingOptions,
    pub sort_direction: bool,
    pub gallery: bool,
    pub(crate) parent_item_opt: Option<Box<Item>>,
    pub(crate) items_opt: Option<Vec<Item>>,
    sort_cache: RefCell<Option<(SortOptions, Vec<usize>)>>,
    /// Directories whose child count could not be read, so it is not asked for again.
    children_count_failed: FxHashSet<PathBuf>,
    pub(crate) scans: crate::scan::ScanState,
    pub dnd_hovered: Option<(Location, Instant)>,
    pub(crate) scrollable_id: widget::Id,
    select_focus: Option<usize>,
    select_range: Option<(usize, usize)>,
    clicked: Option<usize>,
    selected_clicked: bool,
    last_right_click: Option<usize>,
    search_context: Option<SearchContext>,
    date_time_formatter: DateTimeFormatter<fieldsets::YMDT>,
    time_formatter: DateTimeFormatter<fieldsets::T>,
    window_id: Option<window::Id>,
    large_image_manager: LargeImageManager,
}

async fn calculate_dir_size(path: &Path, controller: Controller) -> Result<u64, OperationError> {
    let mut total = 0;
    for entry_res in WalkDir::new(path) {
        controller
            .check()
            .await
            .map_err(|s| OperationError::from_state(s, &controller))?;

        //TODO: report more errors?
        if let Ok(entry) = entry_res
            && let Ok(metadata) = entry.metadata()
            && metadata.is_file()
        {
            total += metadata.len();
        }

        // Yield in case this process takes a while.
        tokio::task::yield_now().await;
    }
    Ok(total)
}

/// Calculate file checksums in a single pass over the file. To add another
/// digest, hash it alongside `sha256_hasher` in the loop below and add a field
/// to [`FileChecksums`]; the file is only read once.
async fn calculate_checksums(path: &Path) -> Result<FileChecksums, String> {
    let path = path.to_path_buf();
    tokio::task::spawn_blocking(move || {
        let mut file = File::open(&path).map_err(|e| e.to_string())?;
        let mut sha256_hasher = Sha256::new();

        let mut buffer = [0u8; 8192];
        loop {
            let bytes_read = file.read(&mut buffer).map_err(|e| e.to_string())?;
            if bytes_read == 0 {
                break;
            }
            sha256_hasher.update(&buffer[..bytes_read]);
        }

        Ok(FileChecksums {
            sha256: format!("{:x}", sha256_hasher.finalize()),
        })
    })
    .await
    .map_err(|e| e.to_string())?
}

fn folder_name<P: AsRef<Path>>(path: P) -> (String, bool) {
    let path = path.as_ref();
    let mut found_home = false;
    let name = match path.file_name() {
        Some(name) => {
            if path == crate::home_dir() {
                found_home = true;
                fl!("home")
            } else {
                // The name comes from the path component alone: every tab title and breadcrumb
                // calls this on the GUI thread, where a stat blocks on slow mounts.
                match get_filename_from_path(path) {
                    Ok(name) => display_name_for_file(path, &name, false, false),
                    Err(_) => name.to_string_lossy().into_owned(),
                }
            }
        }
        None => {
            fl!("filesystem")
        }
    };
    (name, found_home)
}

// parse .hidden file and return files path
pub fn parse_hidden_file(path: &PathBuf) -> Box<[String]> {
    let Ok(file) = File::open(path) else {
        return Default::default();
    };

    BufReader::new(file)
        .lines()
        .map_while(Result::ok)
        .filter_map(|line| {
            let line = line.trim();
            (!line.is_empty()).then_some(line.to_owned())
        })
        .collect()
}

impl Tab {
    pub fn new(
        location: Location,
        config: TabConfig,
        thumb_config: ThumbCfg,
        sorting_options: Option<&FxOrderMap<String, (HeadingOptions, bool)>>,
        scrollable_id: widget::Id,
        window_id: Option<window::Id>,
    ) -> Self {
        let location_str = location.to_string();
        let (sort_name, sort_direction) = sorting_options
            .and_then(|opts| opts.get(&location_str))
            .or_else(|| SORT_OPTION_FALLBACK.get(&location_str))
            .copied()
            .unwrap_or((HeadingOptions::Name, true));
        let location = location.normalize();
        let location_ancestors = location.ancestors();
        let location_title = location.title();
        let history = vec![location.clone()];
        Self {
            location,
            location_ancestors,
            location_title,
            context_menu_desktop_entry: None,
            context_menu_snapshot: menu::MenuSnapshot::new(),
            location_context_menu_index: None,
            mode: Mode::App,
            scroll_opt: None,
            size_opt: Cell::new(None),
            content_height_opt: Cell::new(None),
            viewport_opt: None,
            item_view_size_opt: Cell::new(None),
            edit_location: None,
            edit_location_id: widget::Id::unique(),
            history_i: 0,
            history,
            config,
            thumb_config,
            sort_name,
            sort_direction,
            gallery: false,
            parent_item_opt: None,
            items_opt: None,
            sort_cache: RefCell::new(None),
            children_count_failed: FxHashSet::default(),
            scans: crate::scan::ScanState::default(),
            scrollable_id,
            select_focus: None,
            select_range: None,
            clicked: None,
            dnd_hovered: None,
            selected_clicked: false,
            last_right_click: None,
            search_context: None,
            date_time_formatter: date_time_formatter(config.military_time),
            time_formatter: time_formatter(config.military_time),
            window_id,
            large_image_manager: LargeImageManager::new(),
        }
    }

    pub fn title(&self) -> String {
        //TODO: is it possible to return a &str?
        self.location_title.clone()
    }

    pub const fn items_opt(&self) -> Option<&Vec<Item>> {
        self.items_opt.as_ref()
    }

    pub fn set_items(&mut self, mut items: Vec<Item>) {
        let highlighted = self
            .items_opt
            .as_ref()
            .and_then(|items| items.iter().enumerate().find(|i| i.1.highlighted))
            .map(|(i, _)| i);
        let selected = self.selected_locations();
        let mut carried = Self::take_rescan_carryover(self.items_opt.take());
        for item in &mut items {
            item.selected = false;
            if let Some(location) = &item.location_opt {
                if selected.contains(location) {
                    item.selected = true;
                }
                if let Some(carryover) = carried.remove(&(location.clone(), item.fingerprint())) {
                    carryover.apply(item);
                } else if let Some(path) = item.path_opt() {
                    self.large_image_manager.invalidate(path);
                }
            }
        }
        self.sort_cache.get_mut().take();
        self.children_count_failed.clear();
        self.items_opt = Some(items);
        if let Some(i) = highlighted
            .zip(self.items_opt.as_mut())
            .and_then(|(h, items)| items.get_mut(h))
        {
            i.highlighted = true;
        }
    }

    pub(crate) fn refresh_requests(&mut self, paths: Vec<PathBuf>) -> Vec<(PathBuf, Option<u64>)> {
        let mut requested: FxHashMap<_, _> = paths.into_iter().map(|path| (path, None)).collect();
        if let Some(items) = self.items_opt.as_mut() {
            for item in items {
                if let Some(work_id) = item.path_opt().and_then(|path| requested.get_mut(path)) {
                    item.work_id = next_work_id();
                    *work_id = Some(item.work_id);
                }
            }
        }
        requested.into_iter().collect()
    }

    pub fn refresh_entries(&mut self, refreshed: Vec<EntryRefresh>) -> bool {
        let Some(items) = self.items_opt.as_mut() else {
            return true;
        };
        let mut wanted: FxHashMap<_, _> = refreshed
            .into_iter()
            .map(|refresh| (refresh.path.clone(), refresh))
            .collect();
        let mut complete = true;
        let mut refreshed_any = false;
        let mut type_changed = false;
        let mut name_changed = false;
        for item in items {
            let Some(refresh) = item.path_opt().and_then(|path| wanted.remove(path)) else {
                continue;
            };
            if refresh.work_id != Some(item.work_id) {
                continue;
            }
            let Some(mut replacement) = refresh.item_opt else {
                complete = false;
                continue;
            };
            type_changed |= item.metadata.is_dir() != replacement.metadata.is_dir();
            name_changed |= item.display_name != replacement.display_name;
            replacement.is_mount_point = item.is_mount_point;
            replacement.selected = item.selected;
            replacement.highlighted = item.highlighted;
            replacement.cut = item.cut;
            replacement.overlaps_drag_rect = item.overlaps_drag_rect;
            replacement.button_id = item.button_id.clone();
            replacement.pos_opt.set(item.pos_opt.get());
            replacement.rect_opt.set(item.rect_opt.get());
            replacement.hidden |= item.hidden;
            *item = replacement;
            self.large_image_manager.invalidate(&refresh.path);
            self.children_count_failed.remove(&refresh.path);
            refreshed_any = true;
        }
        if type_changed {
            self.sort_cache.get_mut().take();
        } else if refreshed_any {
            self.resort_if_ordered_by(&[HeadingOptions::Size, HeadingOptions::Modified]);
        }
        if name_changed {
            self.resort_if_ordered_by(&[HeadingOptions::Name]);
        }
        complete && wanted.is_empty()
    }

    /// Reusable results keyed by location and content fingerprint.
    fn take_rescan_carryover(
        items_opt: Option<Vec<Item>>,
    ) -> FxHashMap<(Location, ItemFingerprint), ItemCarryover> {
        let Some(items) = items_opt else {
            return FxHashMap::default();
        };
        items
            .into_iter()
            .filter_map(|item| {
                let location = item.location_opt.clone()?;
                let key = (location, item.fingerprint());
                Some((key, ItemCarryover::from(item)))
            })
            .collect()
    }

    pub fn cut_selected(&mut self) {
        if let Some(ref mut items) = self.items_opt {
            for item in items.iter_mut() {
                item.cut = item.selected;
            }
        }
    }

    pub fn refresh_cut(&mut self, locations: &[PathBuf]) {
        if let Some(ref mut items) = self.items_opt {
            for item in items.iter_mut() {
                item.cut = false;
                if let Some(location_path) = item.location_opt.as_ref().and_then(Location::path_opt)
                    && locations.contains(location_path)
                {
                    item.cut = true;
                }
            }
        }
    }

    pub fn selected_locations(&self) -> Vec<Location> {
        if let Some(ref items) = self.items_opt {
            items
                .iter()
                .filter_map(|item| {
                    if item.selected {
                        item.location_opt.clone()
                    } else {
                        None
                    }
                })
                .collect()
        } else {
            Vec::new()
        }
    }

    /// Whether this tab's listing is exactly the contents of `dir`, so it can answer whether a
    /// name in `dir` is taken. A search tab is listing matches from anywhere below its root, so
    /// a name it holds may belong to a subdirectory.
    pub fn lists_directory(&self, dir: &Path) -> bool {
        !matches!(self.location, Location::Search(..))
            && self.location.path_opt().map(PathBuf::as_path) == Some(dir)
    }

    /// Returns the directory flag from the cached listing, without filesystem I/O.
    pub fn find_entry_is_dir(&self, name: &str) -> Option<bool> {
        self.items_opt.as_ref()?.iter().find_map(|item| {
            let item_name = item.path_opt()?.file_name()?.to_str()?;
            (item_name == name).then(|| item.metadata.is_dir())
        })
    }

    pub fn selected_paths_with_dir(&self) -> Vec<(PathBuf, bool)> {
        let Some(ref items) = self.items_opt else {
            return Vec::new();
        };
        items
            .iter()
            .filter(|item| item.selected)
            .filter_map(|item| Some((item.path_opt()?.to_path_buf(), item.metadata.is_dir())))
            .collect()
    }

    pub fn select_all(&mut self) {
        if let Some(ref mut items) = self.items_opt {
            for item in items.iter_mut() {
                if !self.config.show_hidden && item.hidden {
                    item.selected = false;
                    continue;
                }
                item.selected = true;
            }
        }
    }

    pub fn select_none(&mut self) -> bool {
        self.select_focus = None;
        let mut had_selection = false;
        if let Some(ref mut items) = self.items_opt {
            for item in items.iter_mut() {
                if item.selected {
                    item.selected = false;
                    had_selection = true;
                }
            }
        }
        had_selection
    }

    pub fn select_name(&mut self, name: &str) {
        self.select_focus = None;
        if let Some(ref mut items) = self.items_opt {
            for (i, item) in items.iter_mut().enumerate() {
                item.selected = item.name == name;
                if item.selected {
                    self.select_focus = Some(i);
                }
            }
        }
    }

    /// Selects the first item whose name starts with the given prefix (case-insensitive).
    /// Returns true if an item was selected.
    pub fn select_by_prefix(&mut self, prefix: &str) -> bool {
        let prefix_lower = prefix.to_lowercase();
        let focus = self.select_focus.take();

        if let Some(ref mut items) = self.items_opt {
            // First, deselect all items
            for item in items.iter_mut() {
                item.selected = false;
            }

            // Determine the start index of the search. When the index is before the currently focused item, it will be
            // considered first, otherwise last. Consider the focused item last when only a single character has been
            // typed, so we eagerly switch focus on the first character and stay on the same item as long as the prefix
            // matches.
            let single_char = prefix_lower.chars().count() == 1;
            let start = if single_char {
                Self::index_after_focus(focus, self.sort_direction)
            } else {
                Self::index_before_focus(focus, self.sort_direction)
            };
            self.select_focus = Self::select_first_prefix_from_index(
                &prefix_lower,
                items,
                start,
                self.sort_direction,
            );

            if self.select_focus.is_some() || single_char {
                return self.select_focus.is_some();
            }

            let mut chars = prefix_lower.chars();
            let Some(first) = chars.next() else {
                log::error!("search term is empty");
                return self.select_focus.is_some();
            };

            // Check if all entered characters are the same
            if !chars.all(|c| c == first) {
                return self.select_focus.is_some();
            }

            // Search for a single character when all entered characters are the same.
            // This allows cycling through items starting with the same character by repeatedly pressing a key.
            let start = Self::index_after_focus(focus, self.sort_direction);
            self.select_focus = Self::select_first_prefix_from_index(
                &first.to_string(),
                items,
                start,
                self.sort_direction,
            );

            return self.select_focus.is_some();
        }
        false
    }

    fn index_before_focus(current_focus: Option<usize>, forward: bool) -> usize {
        current_focus.map_or(0, |i| if forward { i } else { i + 1 })
    }

    fn index_after_focus(current_focus: Option<usize>, forward: bool) -> usize {
        current_focus.map_or(0, |i| if forward { i + 1 } else { i })
    }

    fn select_first_prefix_from_index(
        prefix_lower: &str,
        items: &mut [Item],
        start: usize,
        forward: bool,
    ) -> Option<usize> {
        // Order the search item so they begin at `start`.
        let Some((until, after)) = items.split_at_mut_checked(start) else {
            log::error!(
                "invalid start index {start} for items of length {}",
                items.len()
            );
            return None;
        };
        let search_items = after
            .iter_mut()
            .enumerate()
            .map(|(i, item)| (i + start, item))
            .chain(until.iter_mut().enumerate());

        if forward {
            Self::select_first_prefix_match(prefix_lower, search_items)
        } else {
            Self::select_first_prefix_match(prefix_lower, search_items.rev())
        }
    }

    /// Selects the first item in the given iterator whose name starts with the given prefix.
    ///
    /// The `prefix` must be lowercase.
    fn select_first_prefix_match<'a>(
        prefix: &str,
        items: impl Iterator<Item = (usize, &'a mut Item)>,
    ) -> Option<usize> {
        for (i, item) in items {
            if item.name.to_lowercase().starts_with(prefix) {
                item.selected = true;
                return Some(i);
            }
        }
        None
    }

    pub fn select_paths(&mut self, paths: Vec<PathBuf>) {
        self.select_focus = None;
        if let Some(ref mut items) = self.items_opt {
            for (i, item) in items.iter_mut().enumerate() {
                item.selected = false;
                if let Some(path) = item.path_opt()
                    && paths.contains(path)
                {
                    item.selected = true;
                    self.select_focus = Some(i);
                }
            }
        }
    }

    fn select_position(&mut self, row: usize, col: usize, mod_shift: bool) -> bool {
        let mut start = (row, col);
        let mut end = (row, col);
        if mod_shift {
            if self.select_focus.is_none() || self.select_range.is_none() {
                // Set select range to initial state if necessary
                self.select_range = self.select_focus.map(|i| (i, i));
            }

            if let Some(pos) = self.select_range_start_pos_opt() {
                if pos.0 < row || (pos.0 == row && pos.1 < col) {
                    start = pos;
                } else {
                    end = pos;
                }
            }
        }

        let mut found = false;
        if let Some(ref mut items) = self.items_opt {
            for (i, item) in items.iter_mut().enumerate() {
                item.selected = false;
                let pos = match item.pos_opt.get() {
                    Some(some) => some,
                    None => continue,
                };
                if pos.0 < start.0 || (pos.0 == start.0 && pos.1 < start.1) {
                    // Before start
                    continue;
                }
                if pos.0 > end.0 || (pos.0 == end.0 && pos.1 > end.1) {
                    // After end
                    continue;
                }
                if pos == (row, col) {
                    // Update focus if this is what we wanted to select
                    self.select_focus = Some(i);
                    self.select_range = if mod_shift {
                        self.select_range.map(|r| (r.0, i))
                    } else {
                        Some((i, i))
                    };
                    found = true;
                }
                item.selected = true;
            }
        }
        found
    }

    pub fn select_rect(&mut self, rect: Rectangle, mod_ctrl: bool, mod_shift: bool) {
        if let Some(ref mut items) = self.items_opt {
            for item in items.iter_mut() {
                let was_overlapped = item.overlaps_drag_rect;
                item.overlaps_drag_rect = item.rect_opt.get().is_some_and(|r| r.intersects(&rect));

                item.selected = if mod_ctrl || mod_shift {
                    if was_overlapped == item.overlaps_drag_rect {
                        item.selected
                    } else {
                        !item.selected
                    }
                } else {
                    item.overlaps_drag_rect
                };
            }
        }
    }

    pub fn select_focus_id(&self) -> Option<widget::Id> {
        let items = self.items_opt.as_ref()?;
        let item = items.get(self.select_focus?)?;
        Some(item.button_id.clone())
    }

    fn select_focus_pos_opt(&self) -> Option<(usize, usize)> {
        let items = self.items_opt.as_ref()?;
        let item = items.get(self.select_focus?)?;
        item.pos_opt.get()
    }

    fn dehighlight_all(&mut self) {
        if let Some(items) = self.items_opt.as_mut() {
            for item in items.iter_mut() {
                item.highlighted = false;
            }
        }
    }

    pub(crate) fn select_focus_scroll(&mut self) -> Option<AbsoluteOffset> {
        let items = self.items_opt.as_ref()?;
        let item = items.get(self.select_focus?)?;
        let rect = item.rect_opt.get()?;

        //TODO: move to function
        let visible_rect = {
            let point = match self.scroll_opt {
                Some(offset) => Point::new(0.0, offset.y),
                None => Point::new(0.0, 0.0),
            };
            let size = self
                .item_view_size_opt
                .get()
                .unwrap_or_else(|| Size::new(0.0, 0.0));
            Rectangle::new(point, size)
        };

        if rect.y < visible_rect.y {
            // Scroll up to rect
            self.scroll_opt = Some(AbsoluteOffset { x: 0.0, y: rect.y });
            self.scroll_opt
        } else if (rect.y + rect.height) > (visible_rect.y + visible_rect.height) {
            // Scroll down to rect
            self.scroll_opt = Some(AbsoluteOffset {
                x: 0.0,
                y: rect.y + rect.height - visible_rect.height,
            });
            self.scroll_opt
        } else {
            // Do not scroll
            None
        }
    }

    fn rows_per_page(&self) -> usize {
        let viewport_height = self.item_view_size_opt.get().map_or(0.0, |s| s.height);
        let row_height = self
            .select_focus
            .and_then(|i| self.items_opt.as_ref()?.get(i))
            .and_then(|item| item.rect_opt.get())
            .map_or(0.0, |r| r.height);
        if row_height > 0.0 && viewport_height > 0.0 {
            (viewport_height / row_height).floor() as usize
        } else {
            1
        }
    }

    fn select_range_start_pos_opt(&self) -> Option<(usize, usize)> {
        let items = self.items_opt.as_ref()?;
        let item = items.get(self.select_range.map(|r| r.0)?)?;
        item.pos_opt.get()
    }

    fn select_first_pos_opt(&self) -> Option<(usize, usize)> {
        let items = self.items_opt.as_ref()?;
        let mut first = None;
        for item in items {
            if !item.selected {
                continue;
            }

            let (row, col) = match item.pos_opt.get() {
                Some(some) => some,
                None => continue,
            };

            first = Some(match first {
                Some((first_row, first_col)) => match row.cmp(&first_row) {
                    Ordering::Less => (row, col),
                    Ordering::Equal => (row, col.min(first_row)),
                    Ordering::Greater => (first_row, first_col),
                },
                None => (row, col),
            });
        }
        first
    }

    fn select_last_pos_opt(&self) -> Option<(usize, usize)> {
        let items = self.items_opt.as_ref()?;
        let mut last = None;
        for item in items {
            if !item.selected {
                continue;
            }

            let (row, col) = match item.pos_opt.get() {
                Some(some) => some,
                None => continue,
            };

            last = Some(match last {
                Some((last_row, last_col)) => match row.cmp(&last_row) {
                    Ordering::Greater => (row, col),
                    Ordering::Equal => (row, col.max(last_row)),
                    Ordering::Less => (last_row, last_col),
                },
                None => (row, col),
            });
        }
        last
    }

    fn trigger_async_decode(&mut self) -> Vec<Command> {
        // Only trigger decode in gallery mode for the currently selected image
        if !self.gallery {
            return Vec::new();
        }

        let Some(index) = self.select_focus else {
            return Vec::new();
        };

        let Some(items) = &self.items_opt else {
            return Vec::new();
        };

        let Some(item) = items.get(index) else {
            return Vec::new();
        };

        let Some(ItemThumbnail::Image(_, Some(original_dims))) = &item.thumbnail_opt else {
            return Vec::new();
        };
        if !should_use_tiling(original_dims.0, original_dims.1) {
            return Vec::new();
        }

        let Some(path) = item.path_opt() else {
            return Vec::new();
        };

        // Clone path to avoid borrow checker issues
        let path = path.to_path_buf();

        // Get display size for adaptive resolution
        let display_dimensions = self
            .size_opt
            .get()
            .map(|size| (size.width as u32, size.height as u32));

        let original_dimensions = *original_dims;

        // Try to decode the image using LargeImageManager with adaptive resolution
        let (should_decode, target_dimensions, generation) =
            self.large_image_manager
                .try_decode(&path, display_dimensions, original_dimensions);
        if should_decode {
            vec![Command::Iced(
                cosmic::iced::Task::perform(
                    decode_large_image(path.clone(), target_dimensions),
                    move |result| {
                        result
                            .map(|(path, width, height, pixels)| {
                                Message::ImageDecoded(
                                    path,
                                    width,
                                    height,
                                    pixels,
                                    display_dimensions,
                                    generation,
                                )
                            })
                            .unwrap_or_else(|| Message::ImageDecodeFailed(path.clone(), generation))
                    },
                )
                .into(),
            )]
        } else {
            Vec::new()
        }
    }

    pub fn change_location(&mut self, location: &Location, history_i_opt: Option<usize>) {
        self.location = location.normalize();
        self.location_ancestors = self.location.ancestors();
        self.location_title = self.location.title();
        self.edit_location = None;
        self.items_opt = None;
        //TODO: remember scroll by location?
        self.scroll_opt = None;
        self.select_focus = None;
        self.search_context = None;
        if let Some(history_i) = history_i_opt {
            // Navigating in history
            self.history_i = history_i;
        } else {
            // Truncate history to remove next entries
            self.history.truncate(self.history_i + 1);

            // Compact consecutive matching paths
            {
                let mut remove = false;
                if let Some(last_location) = self.history.last() {
                    if let Location::Network(last_uri, ..) = last_location
                        && let Location::Network(uri, ..) = location
                    {
                        remove = last_uri == uri;
                    } else if let Some(last_path) = last_location.path_opt()
                        && let Some(path) = location.path_opt()
                    {
                        remove = last_path == path;
                    }
                }
                if remove {
                    self.history.pop();
                }
            }

            // Push to the front of history
            self.history_i = self.history.len();
            self.history.push(location.clone());
        }
    }

    pub fn update(&mut self, message: Message, modifiers: Modifiers) -> Vec<Command> {
        let mut commands = Vec::new();
        let mut cd = None;
        // Paths to select once `cd` finishes navigating, set only by Message::LocationSelect (a
        // typed/completed file redirected to its parent off-thread). When None, the cd block
        // selects the directory navigated away from.
        let mut cd_selection: Option<Vec<PathBuf>> = None;
        let mut history_i_opt = None;
        let mod_ctrl = modifiers.contains(Modifiers::CTRL) && self.mode.multiple();
        let mod_shift = modifiers.contains(Modifiers::SHIFT) && self.mode.multiple();
        match message {
            Message::AddNetworkDrive => {
                commands.push(Command::AddNetworkDrive);
            }
            Message::AutoScroll(auto_scroll) => {
                commands.push(Command::AutoScroll(auto_scroll));
            }
            Message::ClickRelease(click_i_opt) => {
                // Single click to open.
                if !mod_ctrl && self.config.single_click {
                    let mut paths_to_open = Vec::new();
                    if let Some(ref mut items) = self.items_opt {
                        for (i, item) in items.iter_mut().enumerate() {
                            if Some(i) == click_i_opt {
                                if let Some(location) = &item.location_opt {
                                    if item.metadata.is_dir() {
                                        cd = Some(location.clone());
                                    } else if let Some(path) = location.path_opt() {
                                        paths_to_open.push(path.clone());
                                    } else {
                                        log::warn!("no path for item {item:?}");
                                    }
                                } else {
                                    log::warn!("no location for item {item:?}");
                                }
                            }
                        }
                    }
                    if !paths_to_open.is_empty() {
                        commands.push(Command::OpenFile(paths_to_open));
                    }
                }

                if click_i_opt != self.clicked.take() {
                    if let Some(ref mut items) = self.items_opt {
                        for (i, item) in items.iter_mut().enumerate() {
                            if mod_ctrl {
                                if Some(i) == click_i_opt && item.selected {
                                    item.selected = false;
                                    self.select_range = None;
                                }
                            } else if Some(i) != click_i_opt {
                                item.selected = false;
                            }
                        }
                    }
                }
            }
            Message::DragEnd => {
                self.clicked = None;
            }
            Message::DoubleClick(click_i_opt) => {
                if let Some(clicked_item) = self
                    .items_opt
                    .as_ref()
                    .and_then(|items| click_i_opt.and_then(|click_i| items.get(click_i)))
                {
                    if let Some(location) = &clicked_item.location_opt {
                        if clicked_item.metadata.is_dir() {
                            cd = Some(location.clone());
                        } else if let Some(path) = location.path_opt() {
                            commands.push(Command::OpenFile(vec![path.clone()]));
                        } else {
                            log::warn!("no path for item {clicked_item:?}");
                        }
                    } else {
                        log::warn!("no location for item {clicked_item:?}");
                    }
                } else {
                    log::warn!("no item for click index {click_i_opt:?}");
                }
            }
            Message::Click(click_i_opt) => {
                self.selected_clicked = false;
                self.edit_location = None;
                if click_i_opt.is_none() {
                    self.clicked = click_i_opt;
                }

                if mod_shift {
                    if let Some(click_i) = click_i_opt {
                        self.select_range = self
                            .select_range
                            .map_or(Some((click_i, click_i)), |r| Some((r.0, click_i)));
                        if let Some(range) = self.select_range {
                            let range_min = range.0.min(range.1);
                            let range_max = range.0.max(range.1);
                            // A sorted tab's items can't be linearly selected
                            // Let's say we have:
                            // index | file
                            // 0     | file0
                            // 1     | file1
                            // 2     | file2
                            // This is both the default sort and internal ordering
                            // When sorted it may be displayed as:
                            // 1     | file1
                            // 0     | file0
                            // 2     | file2
                            // However, the internal ordering is still the same thus
                            // linearly selecting items doesn't work. Shift selecting
                            // file0 and file2 would select indices 0 to 2 when it should
                            // select indices 0 AND 2 from items_opt
                            let indices: Vec<_> = self
                                .column_sort()
                                .map(|sorted| sorted.into_iter().map(|(i, _)| i).collect())
                                .unwrap_or_else(|| {
                                    let len = self
                                        .items_opt
                                        .as_deref()
                                        .map(<[Item]>::len)
                                        .unwrap_or_default();
                                    (0..len).collect()
                                });

                            // Find the true indices for the min and max element w.r.t.
                            // a sorted tab.
                            let min = indices
                                .iter()
                                .copied()
                                .position(|offset| offset == range_min)
                                .unwrap_or_default();
                            // We can't skip `min_real` elements here because the index of
                            // `max` may actually be before `min` in a sorted tab
                            let max = indices
                                .iter()
                                .copied()
                                .position(|offset| offset == range_max)
                                .unwrap_or(indices.len());
                            let min_real = min.min(max);
                            let max_real = max.max(min);

                            if let Some(ref mut items) = self.items_opt {
                                for index in indices
                                    .into_iter()
                                    .skip(min_real)
                                    .take(max_real - min_real + 1)
                                {
                                    if let Some(item) = items.get_mut(index) {
                                        if item.hidden {
                                            if self.config.show_hidden {
                                                item.selected = true;
                                            }
                                        } else {
                                            item.selected = true;
                                        }
                                    }
                                }
                            }
                        }
                        self.clicked = click_i_opt;
                        self.select_focus = click_i_opt;
                        self.selected_clicked = true;
                    }
                } else {
                    let dont_unset = mod_ctrl
                        || self.column_sort().is_some_and(|l| {
                            l.iter()
                                .any(|&(e_i, e)| Some(e_i) == click_i_opt && e.selected)
                        });
                    if let Some(ref mut items) = self.items_opt {
                        for (i, item) in items.iter_mut().enumerate() {
                            if Some(i) == click_i_opt {
                                // Filter out selection if it does not match dialog kind
                                if let Mode::Dialog(dialog) = &self.mode {
                                    let item_is_dir = item.metadata.is_dir();
                                    if item_is_dir != dialog.is_dir() {
                                        // Allow selecting folder if dialog is for files to make it
                                        // possible to double click
                                        //TODO: clear any other selection when selecting a folder
                                        if !item_is_dir {
                                            continue;
                                        }
                                    }
                                }
                                if !item.selected {
                                    self.clicked = click_i_opt;
                                    item.selected = true;
                                }
                                self.select_range = Some((i, i));
                                self.select_focus = click_i_opt;
                                self.selected_clicked = true;
                            } else if !dont_unset && item.selected {
                                self.clicked = click_i_opt;
                                item.selected = false;
                            }
                        }
                    }
                }
            }
            Message::Config(config) => {
                // View is preserved for existing tabs
                let view = self.config.view;
                let military_time_changed = self.config.military_time != config.military_time;
                let show_hidden_changed = self.config.show_hidden != config.show_hidden;
                self.config = config;
                self.config.view = view;
                if military_time_changed {
                    self.date_time_formatter = date_time_formatter(self.config.military_time);
                    self.time_formatter = time_formatter(self.config.military_time);
                }
                if show_hidden_changed && let Location::Search(path, term, ..) = &self.location {
                    cd = Some(Location::Search(
                        path.clone(),
                        term.clone(),
                        self.config.show_hidden,
                        Instant::now(),
                    ));
                }
                // Unhighlight all items when config changes
                if let Some(ref mut items) = self.items_opt {
                    for item in items.iter_mut() {
                        item.highlighted = false;
                    }
                }
            }
            Message::ContextAction(action) => {
                commands.push(Command::Action(action));
            }
            Message::RunContextAction(action) => {
                commands.push(Command::RunContextAction(action));
            }
            Message::ContextMenuOpen(open) => self.context_menu_snapshot.set_open(open),
            Message::RightClickBackground => {
                self.edit_location = None;

                //TODO: hack for clearing selecting when right clicking empty space
                if self.last_right_click.take().is_none()
                    && let Some(ref mut items) = self.items_opt
                {
                    for item in items.iter_mut() {
                        item.selected = false;
                    }
                }

                if menu::desktop_entry_actions(self).is_none()
                    && let Some(items) = self.items_opt.as_ref()
                    && let Some(path) = crate::menu::lone_desktop_entry(items)
                    && let Some(item) = items.iter().find(|item| item.selected)
                {
                    commands.push(Command::ParseDesktopEntry(path.to_path_buf(), item.work_id));
                }
            }
            Message::DesktopEntryParsed(path, work_id, actions) => {
                if self.items_opt.as_ref().is_some_and(|items| {
                    crate::menu::lone_desktop_entry(items) == Some(path.as_path())
                        && items.iter().any(|item| {
                            item.selected
                                && item.work_id == work_id
                                && item.path_opt() == Some(&path)
                        })
                }) {
                    self.context_menu_desktop_entry =
                        actions.map(|actions| (path, work_id, actions));
                }
            }
            Message::Surface(action) => {
                commands.push(Command::Surface(action));
            }
            Message::LocationContextMenuIndex(index) => {
                self.location_context_menu_index = index;
            }
            Message::LocationMenuAction(action) => {
                let path_for_index = |ancestor_index| {
                    self.location
                        .path_opt()
                        .and_then(|path| path.ancestors().nth(ancestor_index))
                        .map(Path::to_path_buf)
                };
                match action {
                    LocationMenuAction::OpenInNewTab(ancestor_index) => {
                        if let Some(path) = path_for_index(ancestor_index) {
                            commands.push(Command::OpenInNewTab(path));
                        }
                    }
                    LocationMenuAction::OpenInNewWindow(ancestor_index) => {
                        if let Some(path) = path_for_index(ancestor_index) {
                            commands.push(Command::OpenInNewWindow(path));
                        }
                    }
                    LocationMenuAction::Preview(ancestor_index) => {
                        if let Some(path) = path_for_index(ancestor_index) {
                            // Building the ancestor's Item stats it; Command::PreviewPath does
                            // that off-thread so a slow mount cannot freeze the GUI.
                            commands.push(Command::PreviewPath(path));
                        }
                    }
                    LocationMenuAction::AddToSidebar(ancestor_index) => {
                        if let Some(path) = path_for_index(ancestor_index) {
                            commands.push(Command::AddToSidebar(path));
                        } else {
                            log::warn!(
                                "no ancestor {ancestor_index} for location {:?}",
                                self.location
                            );
                        }
                    }
                }
            }
            Message::Drag(rect_opt) => {
                if let Some(rect) = rect_opt {
                    if self.mode.multiple() {
                        self.select_rect(rect, mod_ctrl, mod_shift);
                    }
                    if self.select_focus.take().is_some() {
                        // Unfocus currently focused button
                        commands.push(Command::Iced(
                            widget::button::focus(widget::Id::unique()).into(),
                        ));
                    }
                }
            }
            Message::EditLocation(edit_location) => {
                self.edit_location = edit_location;
                if self.edit_location.is_some() {
                    commands.push(Command::Iced(
                        widget::text_input::focus(self.edit_location_id.clone()).into(),
                    ));
                }
            }
            Message::EditLocationComplete(selected) => {
                if let Some(mut edit_location) = self.edit_location.take()
                    && !matches!(edit_location.location, Location::Network(..))
                {
                    edit_location.selected = Some(selected);
                    // The file-vs-directory decision (a completed file opens its parent with it
                    // selected) stats the path, which blocks on slow mounts; resolve it off-thread
                    // via Command::EditLocationSubmit.
                    commands.push(Command::EditLocationSubmit(edit_location, false));
                }
            }
            Message::EditLocationEnable => {
                commands.push(Command::Iced(
                    widget::text_input::focus(self.edit_location_id.clone()).into(),
                ));
                self.edit_location = Some(self.location.clone().into());
            }
            Message::EditLocationSubmit => {
                if let Some(mut edit_location) = self.edit_location.take() {
                    let typed_opt = match &edit_location.location {
                        Location::Path(path) => path.to_str().map(str::to_string),
                        Location::Network(uri, ..) => Some(uri.clone()),
                        _ => None,
                    };
                    let mut typed_uri = false;
                    if let Some(typed) = typed_opt {
                        match typed.trim().parse::<url::Url>() {
                            Ok(url) if url.scheme() != "file" && url.has_host() => {
                                let uri = url.as_str().to_string();
                                edit_location =
                                    Location::Network(uri.clone(), uri, None).normalize().into();
                                typed_uri = true;
                            }
                            Err(_) if matches!(edit_location.location, Location::Network(..)) => {
                                edit_location =
                                    Location::Path(PathBuf::from(typed)).normalize().into();
                            }
                            _ => {}
                        }
                    }

                    // Picking a completion, resolving it and the file-vs-directory check all
                    // stat the path; defer them off-thread via Command::EditLocationSubmit so
                    // pressing Enter on a path on a slow mount does not block the GUI thread.
                    commands.push(Command::EditLocationSubmit(edit_location, typed_uri));
                }
            }
            Message::EditLocationTab => {
                if let Some(edit_location) = &mut self.edit_location {
                    edit_location.select(!modifiers.contains(Modifiers::SHIFT));
                }
            }
            Message::OpenInNewTab(path) => {
                commands.push(Command::OpenInNewTab(path));
            }
            Message::ClearRecents => {
                commands.push(Command::ClearRecents);
            }
            Message::EmptyTrash => {
                commands.push(Command::EmptyTrash);
            }
            #[cfg(feature = "desktop")]
            Message::ExecEntryAction(snapshot_id, action) => {
                if let Some(snapshot) = menu::desktop_entry_actions(self)
                    && snapshot.id == snapshot_id
                    && let Some(action) = snapshot.actions.get(action)
                {
                    commands.push(Command::ExecEntryAction(action.exec.clone()));
                }
            }
            Message::Gallery(gallery) => {
                self.gallery = gallery;

                if gallery {
                    commands.extend(self.trigger_async_decode());
                }
            }
            Message::GalleryPrevious | Message::GalleryNext => {
                let mut pos_opt = None;
                if let Some(mut indices) = self.column_sort() {
                    if matches!(message, Message::GalleryPrevious) {
                        indices.reverse();
                    }
                    let mut found = false;
                    for (index, item) in indices {
                        if self.select_focus.is_none() {
                            found = true;
                        }
                        if self.select_focus == Some(index) {
                            found = true;
                            continue;
                        }
                        if found && item.can_gallery() {
                            pos_opt = item.pos_opt.get();
                            if pos_opt.is_some() {
                                break;
                            }
                        }
                    }
                }
                if let Some((row, col)) = pos_opt {
                    // Should mod_shift be available?
                    self.select_position(row, col, mod_shift);

                    commands.extend(self.trigger_async_decode());
                }
                if let Some(offset) = self.select_focus_scroll() {
                    commands.push(Command::Iced(
                        scrollable::scroll_to(
                            self.scrollable_id.clone(),
                            AbsoluteOffset {
                                x: Some(offset.x),
                                y: Some(offset.y),
                            },
                        )
                        .into(),
                    ));
                }
                if let Some(id) = self.select_focus_id() {
                    commands.push(Command::Iced(widget::button::focus(id).into()));
                }
            }
            Message::GalleryToggle => {
                if let Some(indices) = self.column_sort() {
                    for (_, item) in &indices {
                        if item.selected && item.can_gallery() {
                            self.gallery = !self.gallery;

                            if self.gallery {
                                commands.extend(self.trigger_async_decode());
                            }
                            break;
                        }
                    }
                }
            }
            Message::GoNext => {
                if let Some(history_i) = self.history_i.checked_add(1)
                    && let Some(location) = self.history.get(history_i)
                {
                    cd = Some(location.clone());
                    history_i_opt = Some(history_i);
                }
            }
            Message::GoPrevious => {
                if let Some(history_i) = self.history_i.checked_sub(1)
                    && let Some(location) = self.history.get(history_i)
                {
                    cd = Some(location.clone());
                    history_i_opt = Some(history_i);
                }
            }
            Message::ItemDown => {
                self.dehighlight_all();
                if let Some(edit_location) = &mut self.edit_location {
                    edit_location.select(true);
                } else if self.gallery {
                    commands.append(&mut self.update(Message::GalleryNext, modifiers));
                } else {
                    if let Some((row, col)) =
                        self.select_focus_pos_opt().or(self.select_last_pos_opt())
                    {
                        if self.select_focus.is_none() {
                            // Select last item in current selection to focus it.
                            self.select_position(row, col, mod_shift);
                        }

                        //TODO: Shift modifier should select items in between
                        // Try to select item in next row
                        if !self.select_position(row + 1, col, mod_shift) {
                            // Ensure current item is still selected if there are no other items
                            self.select_position(row, col, mod_shift);
                        }
                    } else {
                        // Select first item
                        //TODO: select first in scroll
                        self.select_position(0, 0, mod_shift);
                    }
                    if let Some(offset) = self.select_focus_scroll() {
                        commands.push(Command::Iced(
                            scrollable::scroll_to(
                                self.scrollable_id.clone(),
                                AbsoluteOffset {
                                    x: Some(offset.x),
                                    y: Some(offset.y),
                                },
                            )
                            .into(),
                        ));
                    }
                    if let Some(id) = self.select_focus_id() {
                        commands.push(Command::Iced(widget::button::focus(id).into()));
                    }
                }
            }
            Message::ItemPageDown => {
                self.dehighlight_all();
                if let Some((row, col)) = self.select_focus_pos_opt().or(self.select_last_pos_opt())
                {
                    if self.select_focus.is_none() {
                        self.select_position(row, col, mod_shift);
                    }

                    let rows_per_page = self.rows_per_page().max(1);
                    let target_row = row.saturating_add(rows_per_page);

                    if !self.select_position(target_row, col, mod_shift) {
                        // Fall back to the last item at or before target_row
                        let best = self.items_opt.as_ref().and_then(|items| {
                            items
                                .iter()
                                .filter_map(|item| item.pos_opt.get())
                                .filter(|(r, _)| *r <= target_row)
                                .max()
                        });
                        if let Some((best_row, best_col)) = best {
                            self.select_position(best_row, best_col, mod_shift);
                        }
                    }
                } else {
                    self.select_position(0, 0, mod_shift);
                }
                if let Some(offset) = self.select_focus_scroll() {
                    commands.push(Command::Iced(
                        scrollable::scroll_to(
                            self.scrollable_id.clone(),
                            AbsoluteOffset {
                                x: Some(offset.x),
                                y: Some(offset.y),
                            },
                        )
                        .into(),
                    ));
                }
                if let Some(id) = self.select_focus_id() {
                    commands.push(Command::Iced(widget::button::focus(id).into()));
                }
            }
            Message::ItemPageUp => {
                self.dehighlight_all();
                if let Some((row, col)) =
                    self.select_focus_pos_opt().or(self.select_first_pos_opt())
                {
                    if self.select_focus.is_none() {
                        self.select_position(row, col, mod_shift);
                    }

                    let rows_per_page = self.rows_per_page().max(1);
                    let target_row = row.saturating_sub(rows_per_page);

                    if !self.select_position(target_row, col, mod_shift) {
                        // Fall back to the first item at or after target_row
                        let best = self.items_opt.as_ref().and_then(|items| {
                            items
                                .iter()
                                .filter_map(|item| item.pos_opt.get())
                                .filter(|(r, _)| *r >= target_row)
                                .min()
                        });
                        if let Some((best_row, best_col)) = best {
                            self.select_position(best_row, best_col, mod_shift);
                        }
                    }
                } else {
                    self.select_position(0, 0, mod_shift);
                }
                if let Some(offset) = self.select_focus_scroll() {
                    commands.push(Command::Iced(
                        scrollable::scroll_to(
                            self.scrollable_id.clone(),
                            AbsoluteOffset {
                                x: Some(offset.x),
                                y: Some(offset.y),
                            },
                        )
                        .into(),
                    ));
                }
                if let Some(id) = self.select_focus_id() {
                    commands.push(Command::Iced(widget::button::focus(id).into()));
                }
            }
            Message::ItemLeft => {
                self.dehighlight_all();
                if self.gallery {
                    commands.append(&mut self.update(Message::GalleryPrevious, modifiers));
                } else {
                    if let Some((row, col)) =
                        self.select_focus_pos_opt().or(self.select_first_pos_opt())
                    {
                        if self.select_focus.is_none() {
                            // Select first item in current selection to focus it.
                            self.select_position(row, col, mod_shift);
                        }

                        // Try to select previous item in current row
                        if !col
                            .checked_sub(1)
                            .is_some_and(|col| self.select_position(row, col, mod_shift))
                        {
                            // Try to select last item in previous row
                            if !row.checked_sub(1).is_some_and(|row| {
                                let mut col = 0;
                                if let Some(ref items) = self.items_opt {
                                    for item in items {
                                        match item.pos_opt.get() {
                                            Some((item_row, item_col)) if item_row == row => {
                                                col = col.max(item_col);
                                            }
                                            _ => continue,
                                        }
                                    }
                                }
                                self.select_position(row, col, mod_shift)
                            }) {
                                // Ensure current item is still selected if there are no other items
                                self.select_position(row, col, mod_shift);
                            }
                        }
                    } else {
                        // Select first item
                        //TODO: select first in scroll
                        self.select_position(0, 0, mod_shift);
                    }
                    if let Some(offset) = self.select_focus_scroll() {
                        commands.push(Command::Iced(
                            scrollable::scroll_to(
                                self.scrollable_id.clone(),
                                AbsoluteOffset {
                                    x: Some(offset.x),
                                    y: Some(offset.y),
                                },
                            )
                            .into(),
                        ));
                    }
                    if let Some(id) = self.select_focus_id() {
                        commands.push(Command::Iced(widget::button::focus(id).into()));
                    }
                }
            }
            Message::ItemRight => {
                self.dehighlight_all();
                if self.gallery {
                    commands.append(&mut self.update(Message::GalleryNext, modifiers));
                } else {
                    if let Some((row, col)) =
                        self.select_focus_pos_opt().or(self.select_last_pos_opt())
                    {
                        if self.select_focus.is_none() {
                            // Select last item in current selection to focus it.
                            self.select_position(row, col, mod_shift);
                        }
                        // Try to select next item in current row
                        if !self.select_position(row, col + 1, mod_shift) {
                            // Try to select first item in next row
                            if !self.select_position(row + 1, 0, mod_shift) {
                                // Ensure current item is still selected if there are no other items
                                self.select_position(row, col, mod_shift);
                            }
                        }
                    } else {
                        // Select first item
                        //TODO: select first in scroll
                        self.select_position(0, 0, mod_shift);
                    }
                    if let Some(offset) = self.select_focus_scroll() {
                        commands.push(Command::Iced(
                            scrollable::scroll_to(
                                self.scrollable_id.clone(),
                                AbsoluteOffset {
                                    x: Some(offset.x),
                                    y: Some(offset.y),
                                },
                            )
                            .into(),
                        ));
                    }
                    if let Some(id) = self.select_focus_id() {
                        commands.push(Command::Iced(widget::button::focus(id).into()));
                    }
                }
            }
            Message::ItemUp => {
                self.dehighlight_all();
                if let Some(edit_location) = &mut self.edit_location {
                    edit_location.select(false);
                } else if self.gallery {
                    commands.append(&mut self.update(Message::GalleryPrevious, modifiers));
                } else {
                    if let Some((row, col)) =
                        self.select_focus_pos_opt().or(self.select_first_pos_opt())
                    {
                        if self.select_focus.is_none() {
                            // Select first item in current selection to focus it.
                            self.select_position(row, col, mod_shift);
                        }

                        //TODO: Shift modifier should select items in between
                        // Try to select item in last row
                        if !row
                            .checked_sub(1)
                            .is_some_and(|row| self.select_position(row, col, mod_shift))
                        {
                            // Ensure current item is still selected if there are no other items
                            self.select_position(row, col, mod_shift);
                        }
                    } else {
                        // Select first item
                        //TODO: select first in scroll
                        self.select_position(0, 0, mod_shift);
                    }
                    if let Some(offset) = self.select_focus_scroll() {
                        commands.push(Command::Iced(
                            scrollable::scroll_to(
                                self.scrollable_id.clone(),
                                AbsoluteOffset {
                                    x: Some(offset.x),
                                    y: Some(offset.y),
                                },
                            )
                            .into(),
                        ));
                    }
                    if let Some(id) = self.select_focus_id() {
                        commands.push(Command::Iced(widget::button::focus(id).into()));
                    }
                }
            }
            Message::Location(location) => {
                // Every sender of Message::Location passes a directory (breadcrumbs, mounts, and
                // on_nav_select, which routes favorited *files* to OpenFile off-thread), so no
                // is_dir stat is needed here on the GUI thread.
                cd = Some(location);
            }
            Message::LocationSelect(location, paths) => {
                // A typed/completed file path, resolved off-thread to its parent directory plus the
                // file to select. The parent is a directory, so navigating it needs no GUI-thread
                // stat.
                cd = Some(location);
                cd_selection = Some(paths);
            }
            Message::LocationUp => {
                // Sets location to the path's parent
                // Does nothing if path is root or location is Trash
                if let Location::Path(ref path) = self.location
                    && let Some(parent) = path.parent()
                {
                    cd = Some(Location::Path(parent.to_owned()));
                }
            }
            Message::Open(path_opt) => {
                match path_opt {
                    // The only sender (the preview Open button) routes directories through
                    // Message::Location, so an explicit path here is always a file to open.
                    Some(path) => {
                        commands.push(Command::OpenFile(vec![path]));
                    }
                    // Open selected items
                    None => {
                        enum ResolveResult {
                            Open(Option<PathBuf>),
                            OpenInTab(Option<PathBuf>),
                            OpenTrash,
                            OpenProperties,
                            Cd(Location),
                            Skip,
                        }
                        fn resolve_item(
                            item: &Item,
                            mode: &Mode,
                            is_only_one_selected: bool,
                        ) -> ResolveResult {
                            if !item.selected {
                                return ResolveResult::Skip;
                            }

                            let location = match &item.location_opt {
                                Some(l) => l,
                                None => return ResolveResult::OpenProperties,
                            };

                            let path_opt = location.path_opt();

                            if item.metadata.is_dir() {
                                match mode {
                                    Mode::App => {
                                        if is_only_one_selected {
                                            ResolveResult::Cd(location.clone())
                                        } else {
                                            ResolveResult::OpenInTab(path_opt.cloned())
                                        }
                                    }
                                    Mode::Desktop => match location {
                                        Location::Trash => ResolveResult::OpenTrash,
                                        _ => ResolveResult::Open(path_opt.cloned()),
                                    },
                                    Mode::Dialog(_) => {
                                        if is_only_one_selected {
                                            ResolveResult::Cd(location.clone())
                                        } else {
                                            ResolveResult::Skip
                                        }
                                    }
                                }
                            } else {
                                ResolveResult::Open(path_opt.cloned())
                            }
                        }
                        let mut open_files = Vec::new();
                        if let Some(items) = self.items_opt.as_ref() {
                            let selected_count = items.iter().filter(|i| i.selected).count();

                            for item in items.iter() {
                                match resolve_item(item, &self.mode, selected_count == 1) {
                                    ResolveResult::Open(Some(p)) => open_files.push(p),
                                    ResolveResult::OpenInTab(Some(p)) => {
                                        commands.push(Command::OpenInNewTab(p))
                                    }
                                    ResolveResult::Cd(loc) => cd = Some(loc),
                                    ResolveResult::OpenTrash => commands.push(Command::OpenTrash),
                                    ResolveResult::OpenProperties => {} //TODO: open properties?
                                    _ => {}
                                }
                            }
                        }
                        if !open_files.is_empty() {
                            commands.push(Command::OpenFile(open_files));
                        }
                    }
                }
            }
            Message::Reload => {
                //TODO: support keeping selected locations without paths
                let selected_paths = self
                    .selected_locations()
                    .into_iter()
                    .filter_map(Location::into_path_opt)
                    .collect();
                let location = self.location.clone();
                self.change_location(&location, None);
                commands.push(Command::ChangeLocation(
                    self.title(),
                    location,
                    Some(selected_paths),
                ));
            }
            Message::RightClick(_point_opt, click_i_opt) => {
                if mod_ctrl || mod_shift {
                    self.update(Message::Click(click_i_opt), modifiers);
                }
                if let Some(ref mut items) = self.items_opt
                    && !click_i_opt
                        .is_some_and(|click_i| items.get(click_i).is_some_and(|x| x.selected))
                {
                    // If item not selected, clear selection on other items
                    for (i, item) in items.iter_mut().enumerate() {
                        item.selected = Some(i) == click_i_opt;
                    }
                }
                //TODO: hack for clearing selecting when right clicking empty space
                self.last_right_click = click_i_opt;
            }
            Message::MiddleClick(click_i) => {
                if mod_ctrl || mod_shift {
                    self.update(Message::Click(Some(click_i)), modifiers);
                } else {
                    if let Some(ref mut items) = self.items_opt {
                        for (i, item) in items.iter_mut().enumerate() {
                            item.selected = i == click_i;
                        }
                        self.select_range = Some((click_i, click_i));
                    }
                    if let Some(clicked_item) =
                        self.items_opt.as_ref().and_then(|items| items.get(click_i))
                    {
                        if let Some(path) = clicked_item.path_opt() {
                            if clicked_item.metadata.is_dir() {
                                //cd = Some(Location::Path(path.clone()));
                                commands.push(Command::OpenInNewTab(path.clone()));
                            } else {
                                commands.push(Command::OpenFile(vec![path.clone()]));
                            }
                        } else {
                            log::warn!("no path for item {clicked_item:?}");
                        }
                    } else {
                        log::warn!("no item for click index {click_i:?}");
                    }
                }
            }
            Message::HighlightDeactivate(i) => {
                if let Some(item) = self.items_opt.as_mut().and_then(|f| f.get_mut(i)) {
                    item.highlighted = false;
                }
            }
            Message::HighlightActivate(i) => {
                if let Some(item) = self.items_opt.as_mut().and_then(|f| f.get_mut(i)) {
                    item.highlighted = true;
                }
            }
            Message::Resize(viewport) => {
                // Scroll to ensure focused item still in view
                if self.viewport_opt.map(|v| v.size()) != Some(viewport.size())
                    && let Some(offset) = self.select_focus_scroll()
                {
                    commands.push(Command::Iced(
                        scrollable::scroll_to(
                            self.scrollable_id.clone(),
                            AbsoluteOffset {
                                x: Some(offset.x),
                                y: Some(offset.y),
                            },
                        )
                        .into(),
                    ));
                }

                self.viewport_opt = Some(viewport);
            }
            Message::Scroll(viewport) => {
                self.scroll_opt = Some(viewport.absolute_offset());
            }
            Message::ScrollTab(scroll_speed) => {
                commands.push(Command::Iced(
                    scrollable::scroll_by(
                        self.scrollable_id.clone(),
                        AbsoluteOffset {
                            x: 0.0,
                            y: scroll_speed,
                        },
                    )
                    .into(),
                ));
            }
            Message::ScrollToFocused => {
                if let Some(offset) = self.select_focus_scroll() {
                    commands.push(Command::Iced(
                        scrollable::scroll_to(
                            self.scrollable_id.clone(),
                            AbsoluteOffset {
                                x: Some(offset.x),
                                y: Some(offset.y),
                            },
                        )
                        .into(),
                    ));
                }
            }
            Message::SearchContext(location, context) => {
                if location == self.location {
                    self.search_context = context.0;
                } else {
                    log::warn!(
                        "search context provided for {:?} instead of {:?}",
                        location,
                        self.location
                    );
                }
            }
            Message::SearchReady(finished) => {
                if let Some(context) = &mut self.search_context {
                    if let Some(items) = &mut self.items_opt {
                        if finished || context.ready.swap(false, atomic::Ordering::SeqCst) {
                            let duration = Instant::now();
                            while let Ok(item) = context.results_rx.try_recv() {
                                //TODO: combine this with column_sort logic, they must match!
                                // The Item is already built off-thread; trash results append,
                                // path results sort by modified time.
                                let index = if matches!(item.metadata, ItemMetadata::Trash { .. }) {
                                    items.len()
                                } else {
                                    let item_modified = item.metadata.modified();
                                    match items.binary_search_by(|other| {
                                        item_modified.cmp(&other.metadata.modified())
                                    }) {
                                        Ok(index) => index,
                                        Err(index) => index,
                                    }
                                };

                                if index < MAX_SEARCH_RESULTS {
                                    items.insert(index, item);
                                }
                                // Ensure that updates make it to the GUI in a timely manner
                                if !finished && duration.elapsed() >= MAX_SEARCH_LATENCY {
                                    break;
                                }
                            }
                        }
                        if items.len() >= MAX_SEARCH_RESULTS {
                            items.truncate(MAX_SEARCH_RESULTS);
                            if let Some(last_modified) =
                                items.last().and_then(|item| item.metadata.modified())
                            {
                                *context.last_modified_opt.write().unwrap() = Some(last_modified);
                            }
                        }
                    } else {
                        log::warn!("search ready but items array is empty");
                    }
                }
                if finished {
                    self.search_context = None;
                }
            }
            Message::SelectAll => {
                self.select_all();
                if self.select_focus.take().is_some() {
                    // Unfocus currently focused button
                    commands.push(Command::Iced(
                        widget::button::focus(widget::Id::unique()).into(),
                    ));
                }
            }
            Message::SelectFirst => {
                if self.select_position(0, 0, mod_shift) {
                    if let Some(offset) = self.select_focus_scroll() {
                        commands.push(Command::Iced(
                            scrollable::scroll_to(
                                self.scrollable_id.clone(),
                                AbsoluteOffset {
                                    x: Some(offset.x),
                                    y: Some(offset.y),
                                },
                            )
                            .into(),
                        ));
                    }
                    if let Some(id) = self.select_focus_id() {
                        commands.push(Command::Iced(widget::button::focus(id).into()));
                    }
                }
            }
            Message::SelectLast => {
                if let Some(ref items) = self.items_opt
                    && let Some(last_pos) = items.iter().filter_map(|item| item.pos_opt.get()).max()
                    && self.select_position(last_pos.0, last_pos.1, mod_shift)
                {
                    if let Some(offset) = self.select_focus_scroll() {
                        commands.push(Command::Iced(
                            scrollable::scroll_to(
                                self.scrollable_id.clone(),
                                AbsoluteOffset {
                                    x: Some(offset.x),
                                    y: Some(offset.y),
                                },
                            )
                            .into(),
                        ));
                    }
                    if let Some(id) = self.select_focus_id() {
                        commands.push(Command::Iced(widget::button::focus(id).into()));
                    }
                }
            }
            Message::SetOpenWith(mime, id) => {
                commands.push(Command::SetOpenWith(mime, id));
            }
            Message::SetPermissions(path, mode) => {
                commands.push(Command::SetPermissions(path, mode));
            }
            Message::ShiftPermissions(path_mode_opt, shift, bits) => match path_mode_opt {
                Some((path, mode)) => commands.push(Command::SetPermissions(
                    path,
                    set_mode_part(mode, shift, bits),
                )),
                // Shift permissions on all selected items
                None => {
                    let mut permissions = Vec::new();
                    for item in self.items_opt().map_or(Vec::new(), |items| {
                        items.iter().filter(|item| item.selected).collect()
                    }) {
                        // Read the mode from cached metadata only; a gvfs item would need a
                        // blocking network stat (and chmod over the network would not apply
                        // anyway), so such items are skipped rather than stat-ed here.
                        #[cfg(unix)]
                        if let (Some(path), Some(mode)) = (
                            item.path_opt(),
                            item.cached_metadata().map(|metadata| metadata.mode()),
                        ) {
                            permissions.push((path.clone(), set_mode_part(mode, shift, bits)));
                        }
                    }
                    commands.push(Command::SetMultiplePermissions(permissions));
                }
            },
            Message::SetSort(heading_option, dir) => {
                if !matches!(self.location, Location::Search(..)) {
                    self.sort_name = heading_option;
                    self.sort_direction = dir;
                    if !matches!(self.location, Location::Desktop(..)) {
                        commands.push(Command::SetSort(
                            self.location.normalize().to_string(),
                            heading_option,
                            self.sort_direction,
                        ));
                    }
                }
            }
            Message::TabComplete(path, completions) => {
                if let Some(edit_location) = &mut self.edit_location
                    && edit_location.location.path_opt() == Some(&path)
                {
                    edit_location.completions = Some(completions);
                    commands.push(Command::Iced(
                        widget::text_input::focus(self.edit_location_id.clone()).into(),
                    ));
                }
            }
            Message::Thumbnail(path, thumbnail, work_id) => {
                if let Some(ref mut items) = self.items_opt {
                    let location = Location::Path(path);
                    for item in items.iter_mut() {
                        if item.location_opt.as_ref() == Some(&location) && item.work_id == work_id
                        {
                            if let ItemThumbnail::Image(_, Some(dimensions)) = &thumbnail {
                                item.image_dimensions = Some(*dimensions);
                            }
                            let handle_opt = match &thumbnail {
                                ItemThumbnail::NotImage => None,
                                ItemThumbnail::Image(handle, _) => Some(widget::icon::Handle {
                                    symbolic: false,
                                    data: widget::icon::Data::Image(handle.clone()),
                                }),
                                ItemThumbnail::Svg(handle) => Some(widget::icon::Handle {
                                    symbolic: false,
                                    data: widget::icon::Data::Svg(handle.clone()),
                                }),
                                //TODO: text thumbnails?
                                ItemThumbnail::Text(_text) => None,
                            };
                            if let Some(handle) = handle_opt {
                                item.icon_handle_grid.clone_from(&handle);
                                item.icon_handle_list.clone_from(&handle);
                                item.icon_handle_list_condensed = handle;
                            }
                            item.thumbnail_opt = Some(thumbnail);
                            break;
                        }
                    }
                }
            }
            Message::ImageDecodeFailed(path, generation) => {
                self.large_image_manager.store_error_with_generation(
                    path,
                    "Failed to decode image".into(),
                    generation,
                );
            }
            Message::ImageDecoded(path, width, height, pixels, display_size, generation) => {
                // Create handle from pre-decoded RGBA data (fast!)
                let handle = widget::image::Handle::from_rgba(width, height, pixels);

                // Store decoded image handle if generation still matches (not superseded)
                self.large_image_manager.store_decoded_with_generation(
                    path,
                    handle,
                    display_size,
                    generation,
                );
            }
            Message::ToggleSort(heading_option) => {
                if !matches!(self.location, Location::Search(..)) {
                    let heading_sort = if self.sort_name == heading_option {
                        !self.sort_direction
                    } else {
                        // Default modified to descending, and others to ascending.
                        heading_option != HeadingOptions::Modified
                    };

                    if !matches!(self.location, Location::Desktop(..)) {
                        commands.push(Command::SetSort(
                            self.location.normalize().to_string(),
                            heading_option,
                            heading_sort,
                        ));
                    }

                    self.sort_direction = heading_sort;
                    self.sort_name = heading_option;
                }
            }
            Message::Drop(Some((to, mut from))) => {
                self.dnd_hovered = None;
                match to {
                    Location::Desktop(to, ..)
                    | Location::Path(to)
                    | Location::Network(_, _, Some(to)) => {
                        // A dropped file already living directly in `to` would be copied onto
                        // itself; comparing each path's parent to `to` skips those without
                        // listing the directory, which would block the GUI thread on a slow mount.
                        from.paths.retain(|p| p.parent() != Some(to.as_path()));
                        if from.paths.is_empty() {
                            log::info!("All dropped files already in target directory.");
                            return commands;
                        }
                        commands.push(Command::DropFiles(to, from));
                    }
                    Location::Trash if matches!(from.kind, ClipboardKind::Cut { .. }) => {
                        commands.push(Command::Delete(from.paths));
                    }
                    _ => {
                        log::warn!("{:?} to {:?} is not supported.", from.kind, to);
                    }
                }
            }
            Message::Drop(None) => {
                self.dnd_hovered = None;
            }
            Message::DndHover(loc) => {
                if self
                    .dnd_hovered
                    .as_ref()
                    .is_some_and(|(l, i)| *l == loc && i.elapsed() > HOVER_DURATION)
                {
                    cd = Some(loc);
                }
            }
            Message::DndEnter(loc) => {
                self.dnd_hovered = Some((loc.clone(), Instant::now()));
                if loc != self.location {
                    commands.push(Command::Iced(
                        cosmic::Task::future(async move {
                            tokio::time::sleep(HOVER_DURATION).await;
                            Message::DndHover(loc)
                        })
                        .into(),
                    ));
                }
            }
            Message::DndLeave(loc) => {
                if Some(&loc) == self.dnd_hovered.as_ref().map(|(l, _)| l) {
                    self.dnd_hovered = None;
                }
            }
            Message::WindowDrag => {
                commands.push(Command::WindowDrag);
            }
            Message::WindowToggleMaximize => {
                commands.push(Command::WindowToggleMaximize);
            }
            Message::ZoomIn => {
                commands.push(Command::Action(Action::ZoomIn));
            }
            Message::ZoomOut => {
                commands.push(Command::Action(Action::ZoomOut));
            }
            Message::DirectorySize(path, dir_size, work_id) => {
                let location = Location::Path(path);
                if let Some(ref mut item) = self.parent_item_opt
                    && item.location_opt.as_ref() == Some(&location)
                    && item.work_id == work_id
                {
                    item.dir_size.clone_from(&dir_size);
                }
                if let Some(ref mut items) = self.items_opt {
                    for item in items.iter_mut() {
                        if item.location_opt.as_ref() == Some(&location) && item.work_id == work_id
                        {
                            item.dir_size = dir_size;
                            break;
                        }
                    }
                }
            }
            Message::DirectoryChildren(path, children, work_id) => {
                let location = Location::Path(path.clone());
                if !self.items_opt.as_ref().is_some_and(|items| {
                    items.iter().any(|item| {
                        item.location_opt.as_ref() == Some(&location) && item.work_id == work_id
                    })
                }) {
                    return commands;
                }
                match children {
                    Some(children) => {
                        if let Some(ref mut items) = self.items_opt {
                            for item in items.iter_mut() {
                                if item.location_opt.as_ref() == Some(&location)
                                    && item.work_id == work_id
                                {
                                    item.metadata.set_children_count(children);
                                    break;
                                }
                            }
                        }
                        self.resort_if_ordered_by(&[HeadingOptions::Size]);
                    }
                    None => {
                        self.children_count_failed.insert(path);
                    }
                }
            }
            Message::Checksums(path, checksum_state, work_id) => {
                let location = Location::Path(path);
                if let Some(ref mut item) = self.parent_item_opt
                    && item.location_opt.as_ref() == Some(&location)
                    && item.work_id == work_id
                {
                    item.checksums = checksum_state.clone();
                }
                if let Some(ref mut items) = self.items_opt {
                    for item in items.iter_mut() {
                        if item.location_opt.as_ref() == Some(&location) && item.work_id == work_id
                        {
                            item.checksums = checksum_state;
                            break;
                        }
                    }
                }
            }
            Message::CalculateChecksums(path) => {
                let location = Location::Path(path.clone());
                let item = self
                    .items_opt
                    .as_mut()
                    .and_then(|items| {
                        items
                            .iter_mut()
                            .find(|item| item.location_opt.as_ref() == Some(&location))
                    })
                    .or_else(|| {
                        self.parent_item_opt
                            .as_deref_mut()
                            .filter(|item| item.location_opt.as_ref() == Some(&location))
                    });
                let Some(item) = item else {
                    return commands;
                };
                item.checksums = ChecksumState::Calculating;
                let work_id = item.work_id;
                commands.push(Command::Iced(
                    cosmic::Task::future(async move {
                        match calculate_checksums(&path).await {
                            Ok(checksums) => Message::Checksums(
                                path,
                                ChecksumState::Calculated(checksums),
                                work_id,
                            ),
                            Err(err) => {
                                Message::Checksums(path, ChecksumState::Error(err), work_id)
                            }
                        }
                    })
                    .into(),
                ));
            }
            Message::CopyChecksum(value) => {
                commands.push(Command::Iced(cosmic::iced::clipboard::write(value).into()));
            }
        }

        // Scroll to top if needed
        if self.scroll_opt.is_none() {
            let offset = AbsoluteOffset { x: 0.0, y: 0.0 };
            self.scroll_opt = Some(offset);
            commands.push(Command::Iced(
                scrollable::scroll_to(
                    self.scrollable_id.clone(),
                    AbsoluteOffset {
                        x: Some(0.0),
                        y: Some(0.0),
                    },
                )
                .into(),
            ));
        }

        // Change directory if requested
        if let Some(mut location) = cd {
            location = location.normalize();
            if matches!(self.mode, Mode::Desktop) {
                match location {
                    Location::Path(path) => {
                        commands.push(Command::OpenFile(vec![path]));
                    }
                    Location::Trash => {
                        commands.push(Command::OpenTrash);
                    }
                    _ => {}
                }
            } else {
                // `cd` always holds a directory: its senders decide file-vs-directory from cached
                // metadata or an off-thread stat (a typed/completed file arrives redirected to its
                // parent via Message::LocationSelect, which carries the file to select). Stat-ing
                // here would freeze the GUI on slow mounts.
                if location != self.location || cd_selection.is_some() {
                    let selected_paths = cd_selection
                        .or_else(|| self.location.path_opt().map(|path| vec![path.clone()]));
                    self.change_location(&location, history_i_opt);
                    commands.push(Command::ChangeLocation(
                        self.title(),
                        location,
                        selected_paths,
                    ));
                }
            }
        }

        commands
    }

    pub(crate) const fn sort_options(&self) -> SortOptions {
        match self.location {
            Location::Search(..) => (HeadingOptions::Modified, false, false),
            _ => (
                self.sort_name,
                self.sort_direction,
                self.config.folders_first,
            ),
        }
    }

    /// Drops the cached sort order if the view is ordered by one of `changed`, the orderings
    /// whose keys the caller has just altered.
    fn resort_if_ordered_by(&mut self, changed: &[HeadingOptions]) {
        if changed.contains(&self.sort_options().0) {
            self.sort_cache.get_mut().take();
        }
    }

    /// Whether the subscription should read `item`'s child count. A visible row shows it;
    /// ordering by size additionally reads every directory's count, however far off screen,
    /// because an unread count sorts as zero and reshuffles the listing as it arrives.
    fn fetches_children_count(&self, item: &Item, visible: bool) -> bool {
        (visible || self.sort_options().0 == HeadingOptions::Size)
            && item.metadata.children_count_pending()
            && item
                .path_opt()
                .is_some_and(|path| !self.children_count_failed.contains(path))
    }

    /// Whether anything would render a thumbnail for `item`.
    ///
    /// A text preview is rendered only by the gallery and by the preview pane, which shows one
    /// item at a time. Building one shapes the file under the process-wide font system, which
    /// every text layout on the GUI thread also waits on.
    fn thumbnail_is_displayed(&self, item: &Item, preview: bool) -> bool {
        item.mime.type_() != mime::TEXT || self.gallery || (preview && item.selected)
    }

    /// Sorted item indices, recomputed only when the item set or the sort options change.
    ///
    /// `view` runs on every frame and every layout pass, so collating thousands of names per
    /// call would dominate scrolling; the order is stable between those events.
    fn column_sort(&self) -> Option<Vec<(usize, &Item)>> {
        let items = self.items_opt.as_ref()?;
        let sort_options = self.sort_options();
        let mut cache = self.sort_cache.borrow_mut();
        if !cache
            .as_ref()
            .is_some_and(|(cached, order)| *cached == sort_options && order.len() == items.len())
        {
            *cache = Some((sort_options, Self::sorted_indices(items, sort_options)));
        }
        let (_, order) = cache.as_ref()?;
        Some(order.iter().map(|&i| (i, &items[i])).collect())
    }

    fn sorted_indices(
        items: &[Item],
        (sort_name, sort_direction, folders_first): SortOptions,
    ) -> Vec<usize> {
        let check_reverse = |ord: Ordering, sort: bool| {
            if sort { ord } else { ord.reverse() }
        };
        fn sort_by(
            items: &[Item],
            order: &mut [usize],
            compare: impl Fn(&Item, &Item) -> Ordering,
        ) {
            order.sort_by(|&a, &b| compare(&items[a], &items[b]));
        }
        let mut order: Vec<usize> = (0..items.len()).collect();
        match sort_name {
            HeadingOptions::Size => {
                sort_by(items, &mut order, |a, b| {
                    // entries take precedence over size
                    let get_size = |x: &Item| match &x.metadata {
                        ItemMetadata::Path {
                            metadata,
                            children_opt,
                        } => {
                            if metadata.is_dir() {
                                (true, children_opt.unwrap_or_default() as u64)
                            } else {
                                (false, metadata.len())
                            }
                        }
                        ItemMetadata::Trash { metadata, .. } => match metadata.size {
                            trash::TrashItemSize::Entries(entries) => (true, entries as u64),
                            trash::TrashItemSize::Bytes(bytes) => (false, bytes),
                        },
                        ItemMetadata::SimpleDir { entries } => (true, *entries),
                        ItemMetadata::SimpleFile { size } => (false, *size),
                        #[cfg(feature = "gvfs")]
                        ItemMetadata::GvfsPath {
                            size_opt,
                            children_opt,
                            is_dir,
                            ..
                        } => {
                            if *is_dir {
                                (true, children_opt.unwrap_or_default() as u64)
                            } else {
                                (false, size_opt.unwrap_or_default())
                            }
                        }
                    };
                    let (a_is_entry, a_size) = get_size(a);
                    let (b_is_entry, b_size) = get_size(b);

                    //TODO: use folders_first?
                    match (a_is_entry, b_is_entry) {
                        (true, false) => Ordering::Less,
                        (false, true) => Ordering::Greater,
                        _ => check_reverse(a_size.cmp(&b_size), sort_direction),
                    }
                });
            }
            HeadingOptions::Name => sort_by(items, &mut order, |a, b| {
                if folders_first {
                    match (a.metadata.is_dir(), b.metadata.is_dir()) {
                        (true, false) => Ordering::Less,
                        (false, true) => Ordering::Greater,
                        _ => check_reverse(
                            LANGUAGE_SORTER.compare(&a.display_name, &b.display_name),
                            sort_direction,
                        ),
                    }
                } else {
                    check_reverse(
                        LANGUAGE_SORTER.compare(&a.display_name, &b.display_name),
                        sort_direction,
                    )
                }
            }),
            HeadingOptions::Modified => {
                sort_by(items, &mut order, |a, b| {
                    let a_modified = a.metadata.modified();
                    let b_modified = b.metadata.modified();
                    if folders_first {
                        match (a.metadata.is_dir(), b.metadata.is_dir()) {
                            (true, false) => Ordering::Less,
                            (false, true) => Ordering::Greater,
                            _ => check_reverse(a_modified.cmp(&b_modified), sort_direction),
                        }
                    } else {
                        check_reverse(a_modified.cmp(&b_modified), sort_direction)
                    }
                });
            }
            HeadingOptions::TrashedOn => {
                let time_deleted = |x: &Item| match &x.metadata {
                    ItemMetadata::Trash { entry, .. } => Some(entry.time_deleted),
                    _ => None,
                };

                sort_by(items, &mut order, |a, b| {
                    let a_time_deleted = time_deleted(a);
                    let b_time_deleted = time_deleted(b);
                    if folders_first {
                        match (a.metadata.is_dir(), b.metadata.is_dir()) {
                            (true, false) => Ordering::Less,
                            (false, true) => Ordering::Greater,
                            _ => check_reverse(a_time_deleted.cmp(&b_time_deleted), sort_direction),
                        }
                    } else {
                        check_reverse(b_time_deleted.cmp(&a_time_deleted), sort_direction)
                    }
                });
            }
        }
        order
    }

    fn dnd_dest<'a>(
        &self,
        location: &Location,
        element: impl Into<Element<'a, Message>>,
    ) -> Element<'a, Message> {
        let location1 = location.clone();
        let location2 = location.clone();
        let location3 = location.clone();
        let is_dnd_hovered = self.dnd_hovered.as_ref().map(|(l, _)| l) == Some(location);
        let mut container = widget::container(
            DndDestination::for_data::<ClipboardPaste>(element, move |data, action| {
                if let Some(mut data) = data {
                    if action == DndAction::Copy {
                        Message::Drop(Some((location1.clone(), data)))
                    } else if action == DndAction::Move {
                        data.kind = ClipboardKind::Cut { is_dnd: true };
                        Message::Drop(Some((location1.clone(), data)))
                    } else {
                        log::warn!("unsupported action: {action:?}");
                        Message::Drop(None)
                    }
                } else {
                    Message::Drop(None)
                }
            })
            .on_enter(move |_, _, _| Message::DndEnter(location2.clone()))
            .on_leave(move || Message::DndLeave(location3.clone())),
        );
        // Desktop will not show DnD indicator
        if is_dnd_hovered && !matches!(self.mode, Mode::Desktop) {
            container = container.style(|t| {
                let mut a = widget::container::Style::default();
                let t = t.cosmic();
                // todo use theme drop target color
                let mut bg = t.accent_color();
                bg.alpha = 0.2;
                a.background = Some(Color::from(bg).into());
                a.border = Border {
                    color: t.accent_color().into(),
                    width: 1.0,
                    radius: t.radius_s().into(),
                };
                a
            });
        }
        container.into()
    }

    pub fn gallery_view(&self) -> Element<'_, Message> {
        let cosmic_theme::Spacing {
            space_xxs,
            space_xs,
            space_m,
            ..
        } = theme::spacing();

        //TODO: display error messages when image not found?
        let mut name_opt = None;
        let mut element_opt: Option<Element<Message>> = None;
        if let Some(index) = self.select_focus
            && let Some(items) = &self.items_opt
            && let Some(item) = items.get(index)
        {
            name_opt = Some(widget::text::heading(&item.display_name));
            match item
                .thumbnail_opt
                .as_ref()
                .unwrap_or(&ItemThumbnail::NotImage)
            {
                ItemThumbnail::NotImage => {}
                ItemThumbnail::Image(handle, original_dims) => {
                    // Determine which image to show based on async decode state
                    let mut is_loading = false;
                    let mut error_msg_opt = None;
                    let image_handle = if let Some(path) = item.path_opt() {
                        if let Some(error_msg) = self.large_image_manager.get_error(path) {
                            error_msg_opt = Some(error_msg.clone());
                            handle.clone()
                        } else if self.large_image_manager.is_decoding(path) {
                            // Currently decoding (initial or re-decode) --> show cached/thumbnail with loading indicator
                            is_loading = true;
                            // Use decoded handle if available (re-decode), otherwise thumbnail (initial decode)
                            self.large_image_manager
                                .get_decoded(path)
                                .cloned()
                                .unwrap_or_else(|| handle.clone())
                        } else if let Some(decoded_handle) =
                            self.large_image_manager.get_decoded(path)
                        {
                            // Decoded and not currently decoding --> use it
                            decoded_handle.clone()
                        } else if let Some((w, h)) = original_dims {
                            // Check if image needs tiling
                            if should_use_tiling(*w, *h) {
                                // Large image --> show thumbnail only
                                handle.clone()
                            } else {
                                // Normal-sized image --> load full resolution directly
                                widget::image::Handle::from_path(path)
                            }
                        } else {
                            // No dimensions available --> show thumbnail
                            handle.clone()
                        }
                    } else {
                        handle.clone()
                    };

                    let content: cosmic::Element<'_, Message> =
                        if let Some(error_msg) = error_msg_opt {
                            widget::column::with_capacity(2)
                                .push(widget::image(image_handle))
                                .push(widget::text(format!("⚠ {}", error_msg)).size(13))
                                .padding(space_xs)
                                .align_x(cosmic::iced::Alignment::Center)
                                .into()
                        } else if is_loading {
                            widget::column::with_capacity(2)
                                .push(widget::image(image_handle))
                                .push(widget::text("Loading higher resolution...").size(14))
                                .padding(space_xs)
                                .align_x(cosmic::iced::Alignment::Center)
                                .into()
                        } else {
                            //TODO: use widget::image::viewer, when its zoom can be reset
                            crate::load_image::loaded_image(image_handle).into()
                        };

                    element_opt = Some(widget::container(content).center(Length::Fill).into());
                }
                ItemThumbnail::Svg(handle) => {
                    element_opt = Some(
                        widget::svg(handle.clone())
                            .width(Length::Fill)
                            .height(Length::Fill)
                            .into(),
                    );
                }
                ItemThumbnail::Text(text) => {
                    element_opt = Some(
                        widget::container(
                            widget::text_editor::text_editor(text)
                                .padding(space_xxs)
                                .style(text_editor_class),
                        )
                        .center(Length::Fill)
                        .into(),
                    );
                }
            }
        }

        let mut column = widget::column::with_capacity(2);
        column = column.push(widget::space::vertical().height(Length::Fixed(space_m.into())));
        {
            let mut row = widget::row::with_capacity(5).align_y(Alignment::Center);
            row = row.push(widget::space::horizontal());
            if let Some(name) = name_opt {
                row = row.push(name);
            }
            row = row.push(widget::space::horizontal());
            row = row.push(
                widget::button::icon(widget::icon::from_name("window-close-symbolic"))
                    .class(theme::Button::Standard)
                    .on_press(Message::Gallery(false)),
            );
            row = row.push(widget::space::horizontal().width(Length::Fixed(space_m.into())));
            // This mouse area provides window drag while the header bar is hidden
            let mouse_area = mouse_area::MouseArea::new(row)
                .on_press(|_| Message::WindowDrag)
                .on_double_click(|_| Message::WindowToggleMaximize);
            column = column.push(mouse_area);
        }
        {
            let mut row = widget::row::with_capacity(7).align_y(Alignment::Center);
            row = row.push(widget::space::horizontal().width(Length::Fixed(space_m.into())));
            row = row.push(
                widget::button::icon(widget::icon::from_name("go-previous-symbolic"))
                    .padding(space_xs)
                    .class(theme::Button::Standard)
                    .on_press(Message::GalleryPrevious),
            );
            row = row.push(widget::space::horizontal().width(Length::Fixed(space_xxs.into())));
            if let Some(element) = element_opt {
                row = row.push(element);
            } else {
                //TODO: what to do when no image?
                row = row.push(space::horizontal().width(Length::Fill));
                row = row.push(space::vertical().height(Length::Fill));
            }
            row = row.push(widget::space::horizontal().width(Length::Fixed(space_xxs.into())));
            row = row.push(
                widget::button::icon(widget::icon::from_name("go-next-symbolic"))
                    .padding(space_xs)
                    .class(theme::Button::Standard)
                    .on_press(Message::GalleryNext),
            );
            row = row.push(widget::space::horizontal().width(Length::Fixed(space_m.into())));
            column = column.push(row);
        }

        widget::container(column)
            .width(Length::Fill)
            .height(Length::Fill)
            .style(|theme| {
                let cosmic = theme.cosmic();
                let mut bg = cosmic.bg_color();
                bg.alpha = 0.75;
                widget::container::Style {
                    background: Some(Color::from(bg).into()),
                    ..Default::default()
                }
            })
            .into()
    }

    pub fn location_view(&self) -> Element<'_, Message> {
        //TODO: responsiveness is done in a hacky way, potentially move this to a custom widget?
        fn text_width<'a>(
            content: &'a str,
            font: font::Font,
            font_size: f32,
            line_height: f32,
        ) -> f32 {
            let text: text::Text<&'a str, font::Font> = text::Text {
                content,
                bounds: Size::INFINITE,
                size: font_size.into(),
                line_height: text::LineHeight::Absolute(line_height.into()),
                font,
                align_x: text::Alignment::Left,
                align_y: Vertical::Top,
                shaping: text::Shaping::default(),
                wrapping: text::Wrapping::None,
                ellipsize: text::Ellipsize::End(text::EllipsizeHeightLimit::Lines(1)),
            };
            graphics::text::Paragraph::with_text(text)
                .min_bounds()
                .width
        }
        fn text_width_body(content: &str) -> f32 {
            //TODO: should libcosmic set the font when using widget::text::body?
            text_width(content, font::default(), 14.0, 20.0)
        }
        fn text_width_heading(content: &str) -> f32 {
            text_width(content, font::semibold(), 14.0, 20.0)
        }

        let cosmic_theme::Spacing {
            space_xxxs,
            space_xxs,
            space_s,
            space_m,
            ..
        } = theme::spacing();

        let size = self.size_opt.get().unwrap_or(Size::new(0.0, 0.0));

        let mut row = widget::row::with_capacity(5)
            .align_y(Alignment::Center)
            .padding([space_xxxs, 0]);
        let mut w = 0.0;

        let mut prev_button =
            widget::button::custom(widget::icon::from_name("go-previous-symbolic").size(16))
                .padding(space_xxs)
                .class(theme::Button::Icon);
        if self.history_i > 0 && !self.history.is_empty() {
            prev_button = prev_button.on_press(Message::GoPrevious);
        }
        row = row.push(prev_button);
        w += f32::from(space_xxs).mul_add(2.0, 16.0);

        let mut next_button =
            widget::button::custom(widget::icon::from_name("go-next-symbolic").size(16))
                .padding(space_xxs)
                .class(theme::Button::Icon);
        if self.history_i + 1 < self.history.len() {
            next_button = next_button.on_press(Message::GoNext);
        }
        row = row.push(next_button);
        w += f32::from(space_xxs).mul_add(2.0, 16.0);

        row = row.push(widget::space::horizontal().width(Length::Fixed(space_s.into())));
        w += f32::from(space_s);

        //TODO: allow resizing?
        let name_width = 300.0;
        let modified_width = 200.0;
        let size_width = 100.0;
        let condensed = size.width < (name_width + modified_width + size_width);

        let (sort_name, sort_direction, _) = self.sort_options();
        let heading_item = |name, width, msg| {
            let mut row = widget::row::with_capacity(2)
                .align_y(Alignment::Center)
                .spacing(space_xxxs)
                .width(width);
            row = row.push(widget::text::heading(name));
            match (sort_name == msg, sort_direction) {
                (true, true) => {
                    row = row.push(widget::icon::from_name("pan-down-symbolic").size(16));
                }
                (true, false) => {
                    row = row.push(widget::icon::from_name("pan-up-symbolic").size(16));
                }
                _ => {}
            }
            //TODO: make it possible to resize with the mouse
            mouse_area::MouseArea::new(row)
                .on_press(move |_point_opt| Message::ToggleSort(msg))
                .into()
        };

        let heading_row = widget::row::with_children([
            heading_item(fl!("name"), Length::Fill, HeadingOptions::Name),
            if self.location.is_trash() {
                heading_item(
                    fl!("trashed-on"),
                    Length::Fixed(modified_width),
                    HeadingOptions::TrashedOn,
                )
            } else {
                heading_item(
                    fl!("modified"),
                    Length::Fixed(modified_width),
                    HeadingOptions::Modified,
                )
            },
            heading_item(fl!("size"), Length::Fixed(size_width), HeadingOptions::Size),
        ])
        .align_y(Alignment::Center)
        .height(Length::Fixed((space_m + 4).into()))
        .padding([0, space_xxs]);

        let accent_rule =
            rule::horizontal(1).class(theme::Rule::Custom(Box::new(|theme| rule::Style {
                color: theme.cosmic().accent_color().into(),
                radius: 0.0.into(),
                fill_mode: rule::FillMode::Full,
                snap: true,
            })));
        let heading_rule = widget::container(rule::horizontal(1))
            .padding([0, theme::active().cosmic().corner_radii.radius_xs[0] as u16]);

        if let Some(edit_location) = &self.edit_location {
            let mut text_input = None;

            //TODO: allow editing other locations
            if let Location::Network(ref uri, ..) = edit_location.location {
                let location = edit_location.location.clone();
                text_input = Some(
                    widget::text_input("", uri.clone())
                        .id(self.edit_location_id.clone())
                        .on_input(move |input| {
                            Message::EditLocation(Some(location.with_uri(input).into()))
                        })
                        .on_submit(|_| Message::EditLocationSubmit)
                        .line_height(1.0),
                );
            } else if let Some(resolved_location) = edit_location.resolve()
                && let Some(path) = resolved_location.path_opt().cloned()
            {
                text_input = Some(
                    widget::text_input("", path.to_string_lossy().into_owned())
                        .id(self.edit_location_id.clone())
                        .on_input(move |input| {
                            Message::EditLocation(Some(
                                resolved_location.with_path(PathBuf::from(input)).into(),
                            ))
                        })
                        .on_submit(|_| Message::EditLocationSubmit)
                        .on_tab(Message::EditLocationTab)
                        .on_unfocus(Message::EditLocation(None))
                        .line_height(1.0),
                );
            }
            if let Some(text_input) = text_input {
                row = row.push(
                    widget::button::custom(
                        widget::icon::from_name("window-close-symbolic").size(16),
                    )
                    .on_press(Message::EditLocation(None))
                    .padding(space_xxs)
                    .class(theme::Button::Icon),
                );
                let mut popover =
                    widget::popover(text_input).position(widget::popover::Position::Bottom);
                if let Some(completions) = &edit_location.completions
                    && !completions.is_empty()
                {
                    let mut column =
                        widget::column::with_capacity(completions.len()).padding(space_xxs);
                    for (i, (name, _path)) in completions.iter().enumerate() {
                        let selected = edit_location.selected == Some(i);
                        column = column.push(
                            widget::button::custom(widget::text::body(name))
                                //TODO: match to design
                                .class(if selected {
                                    theme::Button::Standard
                                } else {
                                    theme::Button::HeaderBar
                                })
                                .on_press(Message::EditLocationComplete(i))
                                .padding(space_xxs)
                                .width(Length::Fill),
                        );
                    }
                    popover = popover.popup(
                        widget::container(column)
                            .class(theme::Container::Dropdown)
                            //TODO: This is a hack to get the popover to be the right width
                            .max_width(size.width - 140.0),
                    );
                }
                row = row.push(popover);
                let mut column = widget::column::with_capacity(4).padding([0, space_s]);
                column = column.push(row);
                column = column.push(accent_rule);
                if self.config.view == View::List && !condensed {
                    column = column.push(heading_row);
                    column = column.push(heading_rule);
                }
                return column.into();
            }
        } else if let Some(path) = self.location.path_opt() {
            row = row.push(
                crate::mouse_area::MouseArea::new(
                    widget::button::custom(widget::icon::from_name("edit-symbolic").size(16))
                        .padding(space_xxs)
                        .class(theme::Button::Icon)
                        .on_press(Message::EditLocation(Some(self.location.clone().into()))),
                )
                .on_middle_press(move |_| Message::OpenInNewTab(path.clone())),
            );
            w += f32::from(space_xxs).mul_add(2.0, 16.0);
        }

        let mut children: Vec<Element<_>> = Vec::new();
        match &self.location {
            Location::Desktop(path, ..)
            | Location::Path(path)
            | Location::Search(SearchLocation::Path(path), ..) => {
                let excess_str = "...";
                let excess_width = text_width_body(excess_str);
                for (index, ancestor) in path.ancestors().enumerate() {
                    let (name, found_home) = folder_name(ancestor);
                    let (name_width, name_text) = if children.is_empty() {
                        (
                            text_width_heading(&name),
                            widget::text::heading(name)
                                .wrapping(text::Wrapping::None)
                                .ellipsize(text::Ellipsize::End(
                                    text::EllipsizeHeightLimit::Lines(1),
                                )),
                        )
                    } else {
                        children.push(
                            widget::icon::from_name("go-next-symbolic")
                                .size(16)
                                .icon()
                                .into(),
                        );
                        w += 16.0;
                        (
                            text_width_body(&name),
                            widget::text::body(name).wrapping(text::Wrapping::None),
                        )
                    };

                    // Add padding for mouse area
                    w += 2.0 * f32::from(space_xxxs);

                    let mut row = widget::row::with_capacity(2)
                        .align_y(Alignment::Center)
                        .spacing(space_xxxs);
                    //TODO: figure out why this hardcoded offset is needed after the first item is ellipsed
                    let overflow_offset = 64.0;
                    let overflow = w + name_width + overflow_offset > size.width && index > 0;
                    if overflow {
                        row = row.push(widget::text::body(excess_str));
                        w += excess_width;
                    } else {
                        row = row.push(name_text);
                        w += name_width;
                    }

                    let location = self.location.with_path(ancestor.to_path_buf());
                    let mouse_area = crate::mouse_area::MouseArea::new(
                        widget::button::custom(row)
                            .padding(space_xxxs)
                            .class(if self.location_context_menu_index == Some(index) {
                                theme::Button::LinkActive
                            } else {
                                theme::Button::Link
                            })
                            .on_press(if ancestor == path {
                                Message::EditLocation(Some(self.location.clone().into()))
                            } else {
                                Message::Location(location.clone())
                            }),
                    );

                    let mouse_area = if let Location::Path(_) = &self.location {
                        mouse_area
                            .on_middle_press(move |_| Message::OpenInNewTab(ancestor.to_path_buf()))
                    } else {
                        mouse_area
                    };

                    // Each breadcrumb carries the menu for its own ancestor index
                    let mut context_menu =
                        widget::context_menu(mouse_area, Some(menu::location_context_menu(index)))
                            .on_open(Message::LocationContextMenuIndex(Some(index)))
                            .on_close(Message::LocationContextMenuIndex(None))
                            .on_surface_action(Message::Surface);
                    if let Some(window_id) = self.window_id {
                        context_menu = context_menu.window_id(window_id);
                    }
                    children.push(self.dnd_dest(&location, context_menu));

                    if found_home || overflow {
                        break;
                    }
                }
                children.reverse();
            }
            Location::Trash | Location::Search(SearchLocation::Trash, ..) => {
                children.push(
                    widget::button::custom(widget::text::heading(fl!("trash")))
                        .padding(space_xxxs)
                        .on_press(Message::Location(Location::Trash))
                        .class(theme::Button::Text)
                        .into(),
                );
            }
            Location::Recents | Location::Search(SearchLocation::Recents, ..) => {
                children.push(
                    widget::button::custom(widget::text::heading(fl!("recents")))
                        .padding(space_xxxs)
                        .on_press(Message::Location(Location::Recents))
                        .class(theme::Button::Text)
                        .into(),
                );
            }
            Location::Network(uri, display_name, path) => {
                children.push(
                    widget::button::custom(widget::text::heading(display_name))
                        .padding(space_xxxs)
                        .on_press(Message::Location(Location::Network(
                            uri.clone(),
                            display_name.clone(),
                            path.clone(),
                        )))
                        .class(theme::Button::Text)
                        .into(),
                );
            }
        }

        row = row.extend(children);
        let mut column = widget::column::with_capacity(4).padding([0, space_s]);
        column = column.push(row);
        column = column.push(accent_rule);

        if self.config.view == View::List && !condensed {
            column = column.push(heading_row);
            column = column.push(heading_rule);
        }

        column.into()
    }

    pub fn empty_view(&self, has_hidden: bool) -> Element<'_, Message> {
        let cosmic_theme::Spacing { space_xxs, .. } = theme::spacing();

        mouse_area::MouseArea::new(widget::column::with_children([widget::container(
            match self.mode {
                Mode::App | Mode::Dialog(_) => widget::column::with_children([
                    widget::icon::from_name("folder-symbolic")
                        .size(64)
                        .icon()
                        .into(),
                    widget::text::body(if has_hidden {
                        fl!("empty-folder-hidden")
                    } else if matches!(self.location, Location::Search(..)) {
                        fl!("no-results")
                    } else {
                        fl!("empty-folder")
                    })
                    .into(),
                ]),
                Mode::Desktop => widget::column::with_capacity(0),
            }
            .align_x(Alignment::Center)
            .spacing(space_xxs),
        )
        .center(Length::Fill)
        .into()]))
        .on_press(|_| Message::Click(None))
        .into()
    }

    pub fn grid_view(
        &self,
    ) -> (
        Option<Element<'static, Message>>,
        Element<'_, Message>,
        bool,
    ) {
        let cosmic_theme::Spacing {
            space_xxs,
            space_xxxs,
            ..
        } = theme::spacing();

        let TabConfig {
            show_hidden,
            mut icon_sizes,
            ..
        } = self.config;

        let mut grid_spacing = space_xxs;
        if let Location::Desktop(_path, _output, desktop_config) = &self.location {
            icon_sizes.grid = desktop_config.icon_size;
            grid_spacing = desktop_config.grid_spacing_for(space_xxs);
        }

        let text_height = 3 * 20; // 3 lines of text
        let item_width = (3 * space_xxs + icon_sizes.grid() + 3 * space_xxs) as usize;
        let item_height =
            (space_xxxs + icon_sizes.grid() + space_xxxs + text_height + space_xxxs) as usize;

        let (width, height) = match self.size_opt.get() {
            Some(size) => (
                (size.width.floor() as usize)
                    .saturating_sub(2 * (space_xxs as usize))
                    .max(item_width),
                (size.height.floor() as usize).max(item_height),
            ),
            None => (item_width, item_height),
        };

        let (cols, column_spacing) = {
            let width_m1 = width.saturating_sub(item_width);
            let cols_m1 = width_m1 / (item_width + grid_spacing as usize);
            let cols = cols_m1 + 1;
            let spacing = width_m1
                .checked_div(cols_m1)
                .unwrap_or(0)
                .saturating_sub(item_width);
            (cols, spacing as u16)
        };

        let rows = {
            let height_m1 = height.saturating_sub(item_height);
            let rows_m1 = height_m1 / (item_height + grid_spacing as usize);
            rows_m1 + 1
        };

        //TODO: move to function
        let visible_rect = {
            // Use cached content height to clamp scroll offset after resize
            let max_scroll_y = self
                .content_height_opt
                .get()
                .map(|ch| (ch - height as f32).max(0.0))
                .unwrap_or(f32::MAX);
            let scroll_y = self
                .scroll_opt
                .map(|o| o.y.min(max_scroll_y).max(0.0))
                .unwrap_or(0.0);
            let point = Point::new(0.0, scroll_y);
            let size = self.size_opt.get().unwrap_or_else(|| Size::new(0.0, 0.0));
            Rectangle::new(point, size)
        };

        let mut grid = widget::grid()
            .column_spacing(column_spacing)
            .row_spacing(grid_spacing)
            .padding(space_xxs.into());
        let mut dnd_items: Vec<(usize, (usize, usize), &Item)> = Vec::new();
        let mut drag_w_i = usize::MAX;
        let mut drag_n_i = usize::MAX;
        let mut drag_e_i = 0;
        let mut drag_s_i = 0;

        let mut column = widget::column::with_capacity(4);
        if let Some(items) = self.column_sort() {
            let mut count = 0;
            let mut col = 0;
            let mut row = 0;
            let mut page_row = 0;
            let mut hidden = 0;
            let mut grid_elements = Vec::new();
            for &(i, item) in &items {
                if !show_hidden && item.hidden {
                    item.pos_opt.set(None);
                    item.rect_opt.set(None);
                    hidden += 1;
                    continue;
                }
                item.pos_opt.set(Some((row, col)));
                let item_rect = Rectangle::new(
                    Point::new(
                        (col * (item_width + column_spacing as usize) + space_xxs as usize) as f32,
                        (row * (item_height + grid_spacing as usize) + space_xxs as usize) as f32,
                    ),
                    Size::new(item_width as f32, item_height as f32),
                );
                item.rect_opt.set(Some(item_rect));

                //TODO: error if the row or col is already set?
                while grid_elements.len() <= row {
                    grid_elements.push(Vec::new());
                }

                // Only build elements if visible (for performance)
                if item_rect.intersects(&visible_rect) {
                    //TODO: one focus group per grid item (needs custom widget)
                    let buttons: Vec<Element<Message>> = vec![
                        widget::button::custom(
                            item.icon(&item.icon_handle_grid, icon_sizes.grid()),
                        )
                        .padding(space_xxxs)
                        .class(button_style(
                            item.selected,
                            item.highlighted,
                            item.cut,
                            false,
                            false,
                            false,
                        ))
                        .into(),
                        widget::tooltip(
                            widget::button::custom(Item::grid_display_name(&item.display_name))
                                .id(item.button_id.clone())
                                .padding([0, space_xxxs])
                                .class(button_style(
                                    item.selected,
                                    item.highlighted,
                                    item.cut,
                                    true,
                                    true,
                                    matches!(self.mode, Mode::Desktop),
                                )),
                            widget::text::body(&item.name),
                            widget::tooltip::Position::Bottom,
                        )
                        .into(),
                    ];

                    let mut column = widget::column::with_capacity(buttons.len())
                        .align_x(Alignment::Center)
                        .height(Length::Fixed(item_height as f32))
                        .width(Length::Fixed(item_width as f32));
                    for button in buttons {
                        column = column.push(
                            mouse_area::MouseArea::new(button)
                                .on_right_press_no_capture()
                                .on_right_press(move |point_opt| {
                                    Message::RightClick(point_opt, Some(i))
                                }),
                        );
                    }

                    let column: Element<Message> =
                        if item.metadata.is_dir() && item.location_opt.is_some() {
                            self.dnd_dest(&item.location_opt.clone().unwrap(), column)
                        } else {
                            column.into()
                        };

                    if item.selected {
                        dnd_items.push((i, (row, col), item));
                        drag_w_i = drag_w_i.min(col);
                        drag_n_i = drag_n_i.min(row);
                        drag_e_i = drag_e_i.max(col);
                        drag_s_i = drag_s_i.max(row);
                    }
                    let mouse_area = crate::mouse_area::MouseArea::new(column)
                        .on_press(move |_| Message::Click(Some(i)))
                        .on_double_click(move |_| Message::DoubleClick(Some(i)))
                        .on_release(move |_| Message::ClickRelease(Some(i)))
                        .on_middle_press(move |_| Message::MiddleClick(i))
                        .on_enter(move || Message::HighlightActivate(i))
                        .on_exit(move || Message::HighlightDeactivate(i));
                    grid_elements[row].push(Element::from(mouse_area));
                }

                count += 1;
                if matches!(self.mode, Mode::Desktop) {
                    row += 1;
                    if row >= page_row + rows {
                        row = 0;
                        col += 1;
                    }
                    if col >= cols {
                        col = 0;
                        page_row += rows;
                        row = page_row;
                    }
                } else {
                    col += 1;
                    if col >= cols {
                        col = 0;
                        row += 1;
                    }
                }
            }

            // Rows are laid out at a fixed pitch, so the runs of rows above and below the
            // viewport can be replaced by one spacer each, outside the grid. Rows the grid does
            // carry keep their place: a spacer stands in for any hole between them.
            let row_pitch = (item_height + grid_spacing as usize) as f32;
            let first_row = grid_elements.iter().position(|row| !row.is_empty());
            let last_row = grid_elements.iter().rposition(|row| !row.is_empty());
            let rows_above = first_row.unwrap_or(grid_elements.len());
            let rows_below = last_row.map_or(0, |last| grid_elements.len() - last - 1);

            if let (Some(first_row), Some(last_row)) = (first_row, last_row) {
                for row_elements in grid_elements.drain(first_row..=last_row) {
                    if row_elements.is_empty() {
                        grid =
                            grid.push(space::vertical().height(Length::Fixed(item_height as f32)));
                    }
                    for element in row_elements {
                        grid = grid.push(element);
                    }
                    grid = grid.insert_row();
                }
            }

            if rows_above > 0 {
                column = column
                    .push(space::vertical().height(Length::Fixed(rows_above as f32 * row_pitch)));
            }

            if count == 0 {
                return (None, self.empty_view(hidden > 0), false);
            }

            column = column.push(grid);
            if rows_below > 0 {
                column = column
                    .push(space::vertical().height(Length::Fixed(rows_below as f32 * row_pitch)));
            }

            //TODO: HACK If we don't reach the bottom of the view, go ahead and add a spacer to do that
            {
                let mut max_bottom = 0;
                for (_, item) in items {
                    if let Some(rect) = item.rect_opt.get() {
                        let bottom = (rect.y + rect.height).ceil() as usize;
                        if bottom > max_bottom {
                            max_bottom = bottom;
                        }
                    }
                }

                // Cache content height for scroll clamping on next frame
                self.content_height_opt.set(Some(max_bottom as f32));

                let top_deduct = 7 * (space_xxs as usize);

                self.item_view_size_opt
                    .set(self.size_opt.get().map(|s| Size {
                        width: s.width,
                        height: s.height - top_deduct as f32,
                    }));

                let spacer_height = height.saturating_sub(max_bottom + top_deduct);
                if spacer_height > 0 {
                    column = column.push(widget::container(
                        space::vertical().height(Length::Fixed(spacer_height as f32)),
                    ));
                }
            }
        }

        let drag_list = (!dnd_items.is_empty()).then(|| {
            let mut dnd_grid = widget::grid()
                .column_spacing(column_spacing)
                .row_spacing(grid_spacing)
                .padding(space_xxs.into());

            let mut dnd_item_i = 0;
            for r in drag_n_i..=drag_s_i {
                dnd_grid = dnd_grid.insert_row();
                for c in drag_w_i..=drag_e_i {
                    let Some((i, (row, col), item)) = dnd_items.get(dnd_item_i) else {
                        break;
                    };
                    if *row == r && *col == c {
                        let buttons = vec![
                            widget::button::custom(
                                item.icon(&item.icon_handle_grid, icon_sizes.grid()),
                            )
                            .on_press(Message::Click(Some(*i)))
                            .padding(space_xxxs)
                            .class(button_style(
                                item.selected,
                                item.highlighted,
                                item.cut,
                                false,
                                false,
                                false,
                            )),
                            widget::button::custom(Item::grid_display_name(
                                item.display_name.clone(),
                            ))
                            .id(item.button_id.clone())
                            .on_press(Message::Click(Some(*i)))
                            .padding([0, space_xxxs])
                            .class(button_style(
                                item.selected,
                                item.highlighted,
                                item.cut,
                                true,
                                true,
                                false,
                            )),
                        ];

                        let column =
                            widget::column::with_children(buttons.into_iter().map(Element::from))
                                .align_x(Alignment::Center)
                                .height(Length::Fixed(item_height as f32))
                                .width(Length::Fixed(item_width as f32));

                        dnd_grid = dnd_grid.push(column);
                        dnd_item_i += 1;
                    } else {
                        dnd_grid = dnd_grid.push(
                            widget::container(space::vertical().height(item_width as f32))
                                .height(Length::Fixed(item_height as f32)),
                        );
                    }
                }
            }
            Element::from(dnd_grid)
        });

        let mouse_area = mouse_area::MouseArea::new(column.width(Length::Fill))
            .on_press(|_| Message::Click(None))
            .on_auto_scroll(Message::AutoScroll)
            .on_drag(move |rect_opt| self.on_drag(rect_opt))
            .on_drag_end(|_| Message::DragEnd)
            .show_drag_rect(self.mode.multiple())
            .on_release(|_| Message::ClickRelease(None));

        (drag_list, mouse_area.into(), true)
    }

    pub fn list_view(
        &self,
    ) -> (
        Option<Element<'static, Message>>,
        Element<'_, Message>,
        bool,
    ) {
        let cosmic_theme::Spacing {
            space_s, space_xxs, ..
        } = theme::spacing();

        let TabConfig {
            show_hidden,
            icon_sizes,
            ..
        } = self.config;

        let size = self.size_opt.get().unwrap_or_else(|| Size::new(0.0, 0.0));
        //TODO: allow resizing?
        let name_width = 300.0;
        let modified_width = 200.0;
        let size_width = 100.0;
        let condensed = size.width < (name_width + modified_width + size_width);
        let is_search = matches!(self.location, Location::Search(..));
        let icon_size = if condensed || is_search {
            icon_sizes.list_condensed()
        } else {
            icon_sizes.list()
        };
        let row_height = icon_size + 2 * space_xxs;

        let mut column = widget::column::with_capacity(3);
        let mut y: f32 = 0.0;

        let rule_padding = theme::active().cosmic().corner_radii.radius_xs[0] as u16;

        //TODO: move to function
        let visible_rect = {
            // Use cached content height to clamp scroll offset after resize
            let max_scroll_y = self
                .content_height_opt
                .get()
                .map(|ch| (ch - size.height).max(0.0))
                .unwrap_or(f32::MAX);
            let scroll_y = self
                .scroll_opt
                .map(|o| o.y.min(max_scroll_y).max(0.0))
                .unwrap_or(0.0);
            let point = Point::new(0.0, scroll_y);
            let size = self.size_opt.get().unwrap_or_else(|| Size::new(0.0, 0.0));
            Rectangle::new(point, size)
        };

        let mut drag_items = Vec::new();
        if let Some(items) = self.column_sort() {
            let mut count = 0;
            let mut hidden = 0;
            // Accumulating off-screen rows keeps widget count proportional to visible rows.
            let mut culled_height = 0.0;
            for (i, item) in items {
                if item.hidden && !show_hidden {
                    item.pos_opt.set(None);
                    item.rect_opt.set(None);
                    hidden += 1;
                    continue;
                }

                let separator_height = if count > 0 { 1.0 } else { 0.0 };

                item.pos_opt.set(Some((count, 0)));
                let item_rect = Rectangle::new(
                    Point::new(f32::from(space_s), y + separator_height),
                    Size::new(size.width - f32::from(2 * space_s), f32::from(row_height)),
                );
                item.rect_opt.set(Some(item_rect));

                count += 1;
                y += separator_height + f32::from(row_height);

                if !item_rect.intersects(&visible_rect) {
                    culled_height += separator_height + f32::from(row_height);
                    continue;
                }

                if culled_height > 0.0 {
                    column = column.push(space::vertical().height(culled_height));
                    culled_height = 0.0;
                }
                if separator_height > 0.0 {
                    column = column
                        .push(widget::container(rule::horizontal(1)).padding([0, rule_padding]));
                }

                let button_row = {
                    let modified_text = match &item.metadata {
                        ItemMetadata::Path { metadata, .. } => match metadata.modified() {
                            Ok(time) => self.format_time(time).to_string(),
                            Err(_) => String::new(),
                        },
                        ItemMetadata::Trash { entry, .. } => FormatTime::from_secs(
                            entry.time_deleted,
                            &self.date_time_formatter,
                            &self.time_formatter,
                        )
                        .map(|t| t.to_string())
                        .unwrap_or_default(),
                        #[cfg(feature = "gvfs")]
                        ItemMetadata::GvfsPath { .. } => match item.metadata.modified() {
                            Some(mtime) => self.format_time(mtime).to_string(),
                            None => String::new(),
                        },
                        _ => String::new(),
                    };

                    let size_text = match &item.metadata {
                        ItemMetadata::Path {
                            metadata,
                            children_opt,
                        } => {
                            if metadata.is_dir() {
                                //TODO: translate
                                if let Some(children) = children_opt {
                                    if *children == 1 {
                                        format!("{children} item")
                                    } else {
                                        format!("{children} items")
                                    }
                                } else {
                                    String::new()
                                }
                            } else {
                                format_size(metadata.len())
                            }
                        }
                        ItemMetadata::Trash { metadata, .. } => match metadata.size {
                            trash::TrashItemSize::Entries(entries) => {
                                //TODO: translate
                                if entries == 1 {
                                    format!("{entries} item")
                                } else {
                                    format!("{entries} items")
                                }
                            }
                            trash::TrashItemSize::Bytes(bytes) => format_size(bytes),
                        },
                        ItemMetadata::SimpleDir { entries } => {
                            //TODO: translate
                            if *entries == 1 {
                                format!("{entries} item")
                            } else {
                                format!("{entries} items")
                            }
                        }
                        ItemMetadata::SimpleFile { size } => format_size(*size),
                        #[cfg(feature = "gvfs")]
                        ItemMetadata::GvfsPath {
                            size_opt,
                            children_opt,
                            is_dir,
                            ..
                        } => {
                            if *is_dir {
                                // Children are not counted on remote filesystems
                                match children_opt {
                                    //TODO: translate
                                    Some(1) => "1 item".to_string(),
                                    Some(child_count) => format!("{child_count} items"),
                                    None => String::new(),
                                }
                            } else {
                                format_size(size_opt.unwrap_or_default())
                            }
                        }
                    };

                    let row = if condensed {
                        widget::row::with_children([
                            item.icon(&item.icon_handle_list_condensed, icon_size),
                            widget::column::with_children([
                                Item::list_display_name(item.display_name.clone()).into(),
                                //TODO: translate?
                                widget::text::caption(format!("{modified_text} - {size_text}"))
                                    .into(),
                            ])
                            .into(),
                        ])
                        .height(Length::Fixed(f32::from(row_height)))
                        .align_y(Alignment::Center)
                        .spacing(space_xxs)
                    } else if is_search {
                        widget::row::with_children([
                            item.icon(&item.icon_handle_list_condensed, icon_size),
                            widget::column::with_children([
                                Item::list_display_name(item.display_name.clone())
                                .ellipsize(text::Ellipsize::Middle(text::EllipsizeHeightLimit::Lines(
                                    1,
                                )))
                                .into(),
                                widget::text::caption(match item.path_opt() {
                                    Some(path) => path.display().to_string(),
                                    None => String::new(),
                                })
                                .ellipsize(text::Ellipsize::Middle(text::EllipsizeHeightLimit::Lines(
                                    1,
                                )))
                                .into(),
                            ])
                            .width(Length::Fill)
                            .into(),
                            widget::text::body(modified_text.clone())
                                .width(Length::Fixed(modified_width))
                                .into(),
                            widget::text::body(size_text.clone())
                                .width(Length::Fixed(size_width))
                                .into(),
                        ])
                        .height(Length::Fixed(f32::from(row_height)))
                        .align_y(Alignment::Center)
                        .spacing(space_xxs)
                    } else {
                        widget::row::with_children([
                            item.icon(&item.icon_handle_list, icon_size),
                            Item::list_display_name(item.display_name.clone())
                                .width(Length::Fill)
                                .into(),
                            widget::text::body(modified_text.clone())
                                .width(Length::Fixed(modified_width))
                                .into(),
                            widget::text::body(size_text.clone())
                                .width(Length::Fixed(size_width))
                                .into(),
                        ])
                        .height(Length::Fixed(f32::from(row_height)))
                        .align_y(Alignment::Center)
                        .spacing(space_xxs)
                    };

                    let button =
                        |row| {
                            let mouse_area = crate::mouse_area::MouseArea::new(
                                widget::button::custom(row)
                                    .width(Length::Fill)
                                    .id(item.button_id.clone())
                                    .padding([0, space_xxs])
                                    .class(button_style(
                                        item.selected,
                                        item.highlighted,
                                        item.cut,
                                        true,
                                        true,
                                        false,
                                    )),
                            )
                            .on_press(move |_| Message::Click(Some(i)))
                            .on_double_click(move |_| Message::DoubleClick(Some(i)))
                            .on_release(move |_| Message::ClickRelease(Some(i)))
                            .on_middle_press(move |_| Message::MiddleClick(i))
                            .on_enter(move || Message::HighlightActivate(i))
                            .on_exit(move || Message::HighlightDeactivate(i));

                            mouse_area.on_right_press_no_capture().on_right_press(
                                move |point_opt| Message::RightClick(point_opt, Some(i)),
                            )
                        };

                    let button_row = button(row.into());
                    let button_row: Element<_> = if item.metadata.is_dir()
                        && let Some(location) = item.location_opt.as_ref()
                    {
                        self.dnd_dest(location, button_row)
                    } else {
                        button_row.into()
                    };

                    if item.selected || !drag_items.is_empty() {
                        let dnd_row = if !item.selected {
                            Element::from(
                                space::vertical().height(Length::Fixed(f32::from(row_height))),
                            )
                        } else if condensed {
                            widget::row::with_children([
                                item.icon(&item.icon_handle_list_condensed, icon_size),
                                widget::column::with_children([
                                    Item::list_display_name(item.display_name.clone()).into(),
                                    //TODO: translate?
                                    widget::text::body(format!("{modified_text} - {size_text}"))
                                        .into(),
                                ])
                                .into(),
                            ])
                            .align_y(Alignment::Center)
                            .spacing(space_xxs)
                            .into()
                        } else if is_search {
                            widget::row::with_children([
                                item.icon(&item.icon_handle_list_condensed, icon_size),
                                widget::column::with_children([
                                    Item::list_display_name(item.display_name.clone())
                                    .ellipsize(text::Ellipsize::Middle(text::EllipsizeHeightLimit::Lines(
                                        1,
                                    )))
                                    .into(),
                                    widget::text::caption(match item.path_opt() {
                                        Some(path) => path.display().to_string(),
                                        None => String::new(),
                                    })
                                    .ellipsize(text::Ellipsize::Middle(text::EllipsizeHeightLimit::Lines(
                                        1,
                                    )))
                                    .into(),
                                ])
                                .width(Length::Fill)
                                .into(),
                                widget::text::body(modified_text.clone())
                                    .width(Length::Fixed(modified_width))
                                    .into(),
                                widget::text::body(size_text.clone())
                                    .width(Length::Fixed(size_width))
                                    .into(),
                            ])
                            .align_y(Alignment::Center)
                            .spacing(space_xxs)
                            .into()
                        } else {
                            widget::row::with_children([
                                item.icon(&item.icon_handle_list, icon_size),
                                Item::list_display_name(item.display_name.clone())
                                    .width(Length::Fill)
                                    .into(),
                                widget::text(modified_text)
                                    .width(Length::Fixed(modified_width))
                                    .into(),
                                widget::text::body(size_text)
                                    .width(Length::Fixed(size_width))
                                    .into(),
                            ])
                            .align_y(Alignment::Center)
                            .spacing(space_xxs)
                            .into()
                        };
                        if item.selected {
                            drag_items.push(
                                widget::container(button(dnd_row))
                                    .width(Length::Shrink)
                                    .into(),
                            );
                        } else {
                            drag_items.push(dnd_row);
                        }
                    }

                    button_row
                };

                column = column.push(button_row);
            }

            if culled_height > 0.0 {
                column = column.push(space::vertical().height(culled_height));
            }

            if count == 0 {
                return (None, self.empty_view(hidden > 0), false);
            }

            // Cache content height for scroll clamping on next frame
            self.content_height_opt.set(Some(y));
        }
        //TODO: HACK If we don't reach the bottom of the view, go ahead and add a spacer to do that
        {
            let top_deduct = (if condensed || is_search { 6 } else { 9 }) * space_xxs;

            self.item_view_size_opt
                .set(self.size_opt.get().map(|s| Size {
                    width: s.width,
                    height: s.height - f32::from(top_deduct),
                }));

            let spacer_height = size.height - y - f32::from(top_deduct);
            if spacer_height > 0. {
                column = column.push(widget::container(space::vertical().height(spacer_height)));
            }
        }
        let drag_col = (!drag_items.is_empty())
            .then(|| Element::from(widget::column::with_children(drag_items)));

        let mouse_area = mouse_area::MouseArea::new(column.padding([0, space_s]))
            .with_id(Id::new("list-view"))
            .on_press(|_| Message::Click(None))
            .on_auto_scroll(Message::AutoScroll)
            .on_drag(move |rect_opt| self.on_drag(rect_opt))
            .on_drag_end(|_| Message::DragEnd)
            .show_drag_rect(self.mode.multiple())
            .on_release(|_| Message::ClickRelease(None));

        (drag_col, mouse_area.into(), true)
    }

    pub fn view_responsive<'a>(
        &'a self,
        key_binds: &'a HashMap<KeyBind, Action>,
        modifiers: &'a Modifiers,
        size: Size,
        clipboard_paste_available: bool,
        context_actions: &'a [ContextActionPreset],
    ) -> Element<'a, Message> {
        // Update cached size
        self.size_opt.set(Some(size));

        let cosmic_theme::Spacing {
            space_xxxs,
            space_xxs,
            space_xs,
            ..
        } = theme::spacing();

        let location_view_opt = if matches!(self.mode, Mode::Desktop) {
            None
        } else {
            Some(self.location_view())
        };
        let (drag_list, mut item_view, can_scroll) = match self.config.view {
            View::Grid => self.grid_view(),
            View::List => self.list_view(),
        };
        item_view = widget::container(item_view).width(Length::Fill).into();
        let files = self
            .items_opt
            .as_ref()
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| {
                        if item.selected {
                            item.path_opt().cloned()
                        } else {
                            None
                        }
                    })
                    .collect::<Box<[PathBuf]>>()
            })
            .unwrap_or_default();
        let item_view =
            DndSource::<Message, ClipboardCopy>::with_id(item_view, Id::new("tab-view"));

        let view = self.config.view;
        let item_view = match drag_list {
            Some(drag_list) if self.selected_clicked => {
                let drag_list = RcElementWrapper::new(drag_list);
                item_view
                    .drag_content(move || {
                        ClipboardCopy::new(crate::clipboard::ClipboardKind::Copy, &files)
                    })
                    .drag_icon(move |_| {
                        let state: tree::State = Widget::<Message, _, _>::state(&drag_list);
                        (
                            Element::from(drag_list.clone()).map(|_m| ()),
                            state,
                            match view {
                                // offset by grid padding so that we grab the top left corner of the item in the drag grid.
                                View::Grid => Vector::new(
                                    f32::from(space_xxs).mul_add(-3.0, -f32::from(space_xxxs)),
                                    -4. * f32::from(space_xxxs),
                                ),
                                View::List => Vector::ZERO,
                            },
                        )
                    })
            }
            _ => item_view,
        };

        let tab_location = self.location.clone();
        let mouse_area = mouse_area::MouseArea::new(item_view)
            .on_press(move |_point_opt| Message::Click(None))
            .on_release(|_| Message::ClickRelease(None))
            .on_resize(Message::Resize)
            .on_back_press(move |_point_opt| Message::GoPrevious)
            .on_forward_press(move |_point_opt| Message::GoNext)
            .on_scroll(|delta| respond_to_scroll_direction(delta, modifiers))
            .on_right_press(|_| Message::RightClickBackground);

        let items_area: Element<'_, Message> = if can_scroll {
            // FIXME: new responsive widget will remove the state from the scrollable
            // id_container with custom id forces the state to be extracted in a diff
            // pre-processing step
            widget::id_container(
                widget::scrollable(mouse_area)
                    .id(self.scrollable_id.clone())
                    .on_scroll(Message::Scroll)
                    .width(Length::Fill)
                    .height(Length::Fill),
                widget::Id::new(format!("{}-scrollable", self.scrollable_id)),
            )
            .into()
        } else {
            mouse_area.into()
        };

        // Wrap the scrollable, not its content, so the popup anchors in window coordinates
        let mut context_menu = widget::context_menu(
            items_area,
            Some(self.context_menu_snapshot.view(|| {
                menu::context_menu(
                    self,
                    key_binds,
                    modifiers,
                    clipboard_paste_available,
                    context_actions,
                )
            })),
        )
        .item_width(cosmic::widget::menu::ItemWidth::Uniform(360))
        .on_open(Message::ContextMenuOpen(true))
        .on_close(Message::ContextMenuOpen(false))
        .on_surface_action(Message::Surface);
        if let Some(window_id) = self.window_id {
            context_menu = context_menu.window_id(window_id);
        }

        let mut tab_column = widget::column::with_capacity(3);
        if let Some(location_view) = location_view_opt {
            tab_column = tab_column.push(location_view);
        }
        tab_column = tab_column.push(context_menu);
        match &self.location {
            Location::Trash | Location::Search(SearchLocation::Trash, ..) => {
                if let Some(items) = self.items_opt()
                    && !items.is_empty()
                {
                    tab_column = tab_column.push(
                        widget::layer_container(widget::row::with_children([
                            widget::space::horizontal().into(),
                            widget::button::standard(fl!("empty-trash"))
                                .on_press(Message::EmptyTrash)
                                .into(),
                        ]))
                        .padding([space_xxs, space_xs])
                        .layer(cosmic_theme::Layer::Primary)
                        .apply(widget::container)
                        .padding([0, 0, 7, 0]),
                    );
                }
            }
            Location::Recents | Location::Search(SearchLocation::Recents, ..) => {
                if let Some(items) = self.items_opt()
                    && !items.is_empty()
                {
                    tab_column = tab_column.push(
                        widget::layer_container(widget::row::with_children([
                            widget::space::horizontal().into(),
                            widget::button::standard(fl!("clear-recents-history"))
                                .on_press(Message::ClearRecents)
                                .into(),
                        ]))
                        .padding([space_xxs, space_xs])
                        .layer(cosmic_theme::Layer::Primary)
                        .apply(widget::container)
                        .padding([0, 0, 7, 0]),
                    );
                }
            }
            Location::Network(uri, _display_name, _path) if uri == "network:///" => {
                tab_column = tab_column.push(
                    widget::layer_container(widget::row::with_children([
                        widget::space::horizontal().into(),
                        widget::button::standard(fl!("add-network-drive"))
                            .on_press(Message::AddNetworkDrive)
                            .into(),
                    ]))
                    .padding([space_xxs, space_xs])
                    .layer(cosmic_theme::Layer::Primary)
                    .apply(widget::container)
                    .padding([0, 0, 7, 0]),
                );
            }
            _ => {}
        }
        let mut tab_view = widget::container(tab_column)
            .height(Length::Fill)
            .width(Length::Fill);

        // Desktop will not show DnD indicator
        if self.dnd_hovered.as_ref().map(|(l, _)| l) == Some(&tab_location)
            && !matches!(self.mode, Mode::Desktop)
        {
            tab_view = tab_view.style(|t| {
                let mut a = widget::container::Style::default();
                let c = t.cosmic();
                a.border = cosmic::iced::core::Border {
                    color: (c.accent_color()).into(),
                    width: 1.,
                    radius: c.radius_0().into(),
                };
                a
            });
        }

        let tab_location_2 = self.location.clone();
        let tab_location_3 = self.location.clone();
        let dnd_dest = DndDestination::for_data(tab_view, move |data, action| {
            if let Some(mut data) = data {
                if action == DndAction::Copy {
                    Message::Drop(Some((tab_location.clone(), data)))
                } else if action == DndAction::Move {
                    data.kind = ClipboardKind::Cut { is_dnd: true };
                    Message::Drop(Some((tab_location.clone(), data)))
                } else {
                    log::warn!("unsupported action: {action:?}");
                    Message::Drop(None)
                }
            } else {
                Message::Drop(None)
            }
        })
        .on_enter(move |_, _, _| Message::DndEnter(tab_location_2.clone()))
        .on_leave(move || Message::DndLeave(tab_location_3.clone()));

        dnd_dest.into()
    }
    pub fn multi_preview_view<'a>(
        &'a self,
        mime_app_cache_opt: Option<&'a mime_app::MimeAppCache>,
    ) -> Element<'a, Message> {
        let cosmic_theme::Spacing {
            space_xxxs,
            space_m,
            ..
        } = theme::spacing();

        let mut column = widget::column::with_capacity(4).spacing(space_m);

        let handle = widget::icon::from_name("text-x-generic")
            .size(IconSizes::default().grid())
            .handle();

        let icon = widget::icon::icon(handle.clone())
            .content_fit(ContentFit::Contain)
            .size(IconSizes::default().grid());

        let icon_container1 = widget::container(icon.clone()).padding(padding::bottom(10).left(10));
        let icon_container2 =
            widget::container(icon.clone()).padding(padding::top(5).bottom(5).left(5).right(5));
        let icon_container3 = widget::container(icon).padding(padding::top(10).right(10));
        let stack = stack![icon_container1, icon_container2, icon_container3];

        column = column.push(
            widget::container(stack)
                .center_x(Length::Fill)
                .max_height(THUMBNAIL_SIZE as f32),
        );

        let selected_items: Vec<&Item> = self.items_opt().map_or(Vec::new(), |items| {
            items
                .iter()
                .filter(|item| {
                    if item.selected {
                        item.location_opt
                            .as_ref()
                            .and_then(Location::path_opt)
                            .is_some()
                    } else {
                        false
                    }
                })
                .collect()
        });

        let mut details = widget::column::with_capacity(3).spacing(space_xxxs);
        details = details.push(widget::text::body(fl!(
            "items",
            items = selected_items.len()
        )));

        let mut total_size: u64 = 0;
        let mut mime_type_counts: BTreeMap<String, u64> = BTreeMap::new();
        let mut user_name: BTreeSet<String> = BTreeSet::new();
        let mut mode_user: BTreeSet<u32> = BTreeSet::new();
        let mut group_name: BTreeSet<String> = BTreeSet::new();
        let mut mode_group: BTreeSet<u32> = BTreeSet::new();
        let mut mode_other: BTreeSet<u32> = BTreeSet::new();
        let mut calculating_dir_size = false;
        let mut dir_size_error: Option<String> = None;

        for item in selected_items.iter() {
            *mime_type_counts.entry(item.mime.to_string()).or_insert(0) += 1;

            if item.metadata.is_dir() {
                match &item.dir_size {
                    DirSize::Calculating(_) => {
                        calculating_dir_size = true;
                    }
                    DirSize::Directory(size) => {
                        total_size = total_size.saturating_add(*size);
                    }
                    DirSize::NotDirectory => (),
                    DirSize::Error(err) => {
                        dir_size_error = Some(err.clone());
                    }
                };
            } else if let Some(size) = item.metadata.file_size() {
                total_size = total_size.saturating_add(size);
            }

            // Ownership and permissions need full metadata, which only a local path caches: a
            // network item would have to be stat-ed here, on the GUI thread, every frame.
            #[cfg(unix)]
            if let Some(metadata) = item.cached_metadata() {
                let mode = metadata.mode();
                user_name.insert(
                    uzers::get_user_by_uid(metadata.uid())
                        .and_then(|user| user.name().to_str().map(ToOwned::to_owned))
                        .unwrap_or_default(),
                );
                mode_user.insert(get_mode_part(mode, MODE_SHIFT_USER));
                group_name.insert(
                    uzers::get_group_by_gid(metadata.gid())
                        .and_then(|group| group.name().to_str().map(ToOwned::to_owned))
                        .unwrap_or_default(),
                );
                mode_group.insert(get_mode_part(mode, MODE_SHIFT_GROUP));
                mode_other.insert(get_mode_part(mode, MODE_SHIFT_OTHER));
            }
        }
        let mut mime_types: Vec<(String, u64)> = mime_type_counts.into_iter().collect();
        mime_types.sort_by(|(_, v1), (_, v2)| v2.cmp(v1));

        // Limit the number of displayed mime types
        let limit = usize::min(10, mime_types.len());

        let mut mime_type_strings: Vec<String> = mime_types[..limit]
            .iter()
            .map(|(mime, count)| format!("{} ({})", mime, count))
            .collect();

        if mime_types.len() > limit {
            mime_type_strings.push("...".to_string());
        }

        details = details.push(widget::text::body(fl!(
            "type",
            mime = mime_type_strings.join(", ")
        )));

        let size = {
            if calculating_dir_size {
                fl!("calculating")
            } else if let Some(error) = dir_size_error {
                error
            } else {
                format_size(total_size)
            }
        };

        details = details.push(widget::text::body(fl!("item-size", size = size)));

        column = column.push(details);

        column = column.push(widget::button::standard(fl!("open")).on_press(Message::Open(None)));

        let mut settings = Vec::new();
        // Only allow modifying open-with if all mime types are the same
        if mime_types.len() == 1
            && let Some(mime) = mime_types
                .first()
                .and_then(|(mime, _)| mime.parse::<Mime>().ok())
            && let Some(mime_app_cache) = mime_app_cache_opt
        {
            let mime_apps = mime_app_cache.get_apps_for_mime(&mime, false);
            if !mime_apps.is_empty() {
                let mime_closure = mime.clone();
                let (names, icons) = mime_apps
                    .iter()
                    .map(|(app, _)| (Cow::Owned(app.name.clone()), app.icon()))
                    .collect::<(Vec<_>, Vec<_>)>();
                settings.push(
                    widget::settings::item::builder(fl!("open-with")).control(
                        Element::from(
                            widget::dropdown(
                                names,
                                mime_apps.iter().position(|(x, _)| x.is_default(&mime)),
                                move |index| (index, mime_closure.clone()),
                            )
                            .icons(Cow::Owned(icons)),
                        )
                        .map(move |(index, mime)| {
                            let mime_app = &mime_apps[index].0;
                            Message::SetOpenWith(mime, mime_app.id.clone())
                        }),
                    ),
                );
            }
        }

        #[cfg(unix)]
        {
            // Only return mode part if it's the only one
            fn selected_mode_part(mut modes: BTreeSet<u32>) -> Option<usize> {
                match (modes.pop_first(), modes.pop_first()) {
                    (Some(mode), None) => Some(mode.try_into().unwrap()),
                    _ => None,
                }
            }

            // Convert a limited number of values from a set into a comma separated list
            fn join_set(set: BTreeSet<String>) -> String {
                let limit = 5;
                let mut title = set.into_iter().collect::<Vec<String>>();
                if title.len() > limit {
                    title.truncate(limit);
                    title.push("...".to_string());
                }
                title.join(", ")
            }

            let mode_part_user = selected_mode_part(mode_user);
            settings.push(
                widget::settings::item::builder(join_set(user_name))
                    .description(fl!("owner"))
                    .control(
                        widget::dropdown(
                            Cow::Borrowed(MODE_NAMES.as_slice()),
                            mode_part_user,
                            move |selected| {
                                Message::ShiftPermissions(
                                    None,
                                    MODE_SHIFT_USER,
                                    selected.try_into().unwrap(),
                                )
                            },
                        )
                        .placeholder(fl!("mixed")),
                    ),
            );

            let mode_part_group = selected_mode_part(mode_group);
            settings.push(
                widget::settings::item::builder(join_set(group_name))
                    .description(fl!("group"))
                    .control(
                        widget::dropdown(
                            Cow::Borrowed(MODE_NAMES.as_slice()),
                            mode_part_group,
                            move |selected| {
                                Message::ShiftPermissions(
                                    None,
                                    MODE_SHIFT_GROUP,
                                    selected.try_into().unwrap(),
                                )
                            },
                        )
                        .placeholder(fl!("mixed")),
                    ),
            );

            let mode_part_other = selected_mode_part(mode_other);
            settings.push(
                widget::settings::item::builder(fl!("other")).control(
                    widget::dropdown(
                        Cow::Borrowed(MODE_NAMES.as_slice()),
                        mode_part_other,
                        move |selected| {
                            Message::ShiftPermissions(
                                None,
                                MODE_SHIFT_OTHER,
                                selected.try_into().unwrap(),
                            )
                        },
                    )
                    .placeholder(fl!("mixed")),
                ),
            );
        }

        if !settings.is_empty() {
            let mut section = widget::settings::section();
            section = section.extend(settings);
            column = column.push(section);
        }

        column.into()
    }
    pub fn view<'a>(
        &'a self,
        key_binds: &'a HashMap<KeyBind, Action>,
        modifiers: &'a Modifiers,
        clipboard_paste_available: bool,
        context_actions: &'a [ContextActionPreset],
    ) -> Element<'a, Message> {
        widget::responsive(move |size| {
            widget::id_container(
                self.view_responsive(
                    key_binds,
                    modifiers,
                    size,
                    clipboard_paste_available,
                    context_actions,
                ),
                Id::new(format!(
                    "tab-{}-{}",
                    self.scrollable_id, self.location_title
                )),
            )
            .into()
        })
        .into()
    }

    pub fn subscription(&self, preview: bool) -> Subscription<Message> {
        //TODO: how many thumbnail loads should be in flight at once?
        let jobs = self.thumb_config.jobs.get() as usize;
        let mut subscriptions = Vec::with_capacity(2 * jobs + 3);

        if let Some(items) = &self.items_opt {
            //TODO: move to function
            let visible_rect = {
                let point = match self.scroll_opt {
                    Some(offset) => Point::new(0.0, offset.y),
                    None => Point::new(0.0, 0.0),
                };
                let size = self.size_opt.get().unwrap_or_else(|| Size::new(0.0, 0.0));
                Rectangle::new(point, size)
            };

            let mut thumbnails = 0;
            let mut children_counts = 0;
            for item in items {
                // Items with no determined rect (which should include hidden items) are never
                // visible.
                let visible = item
                    .rect_opt
                    .get()
                    .is_some_and(|rect| rect.intersects(&visible_rect));

                let Some(path) = item.path_opt() else {
                    continue;
                };

                if children_counts < jobs && self.fetches_children_count(item, visible) {
                    children_counts += 1;
                    subscriptions.push(children_count_subscription(path.clone(), item.work_id));
                }

                if !visible {
                    continue;
                }

                if item.thumbnail_opt.is_some() {
                    // Skip items that already have a mime type and thumbnail
                    continue;
                }

                if !self.thumbnail_is_displayed(item, preview) {
                    continue;
                }

                let can_thumbnail = match item.metadata {
                    ItemMetadata::Path { .. } => true,
                    #[cfg(feature = "gvfs")]
                    ItemMetadata::GvfsPath { .. } => true,
                    _ => false,
                };
                if can_thumbnail && thumbnails < jobs {
                    let path = path.clone();
                    let metadata = item.metadata.clone();
                    let mime = item.mime.clone();
                    let max_jobs = jobs;
                    let max_mb = u64::from(self.thumb_config.max_mem_mb.get());
                    let max_size = u64::from(self.thumb_config.max_size_mb.get());

                    // Determine effective memory budget based on image size
                    let (effective_max_mb, effective_jobs) = if mime.type_() == mime::IMAGE {
                        match item.image_dimensions {
                            Some((width, height)) => {
                                let (_use_dedicated, eff_mb, eff_jobs) =
                                    should_use_dedicated_worker(width, height, max_mb, max_jobs);
                                (eff_mb, eff_jobs)
                            }
                            None => (max_mb, max_jobs),
                        }
                    } else {
                        (max_mb, max_jobs)
                    };

                    #[derive(Clone)]
                    struct Wrapper {
                        path: PathBuf,
                        work_id: u64,
                        metadata: ItemMetadata,
                        mime: mime::Mime,
                        effective_max_mb: u64,
                        effective_jobs: usize,
                        max_size: u64,
                    }

                    impl Hash for Wrapper {
                        fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
                            self.path.hash(state);
                            self.work_id.hash(state);
                        }
                    }

                    subscriptions.push(Subscription::run_with(
                        Wrapper {
                            path: path.clone(),
                            work_id: item.work_id,
                            metadata,
                            mime,
                            effective_max_mb,
                            effective_jobs,
                            max_size,
                        },
                        |wrapper| {
                            let Wrapper {
                                path,
                                work_id,
                                metadata,
                                mime,
                                effective_max_mb,
                                effective_jobs,
                                max_size,
                            } = wrapper.clone();
                            stream::channel(
                                1,
                                move |mut output: futures::channel::mpsc::Sender<_>| async move {
                                    while crate::operation::is_actively_writing_to(&path) {
                                        crate::operation::actively_writing_tick().await;
                                    }

                                    let message = {
                                        let path = path.clone();

                                        // Acquire semaphore permit
                                        let _permit = THUMB_SEMAPHORE.acquire().await.unwrap();

                                        tokio::task::spawn_blocking(move || {
                                            let start = Instant::now();
                                            let thumbnail = ItemThumbnail::new(
                                                &path,
                                                metadata,
                                                mime,
                                                THUMBNAIL_SIZE,
                                                effective_max_mb,
                                                effective_jobs,
                                                max_size,
                                            );
                                            log::debug!(
                                                "thumbnailed {} in {:?}",
                                                path.display(),
                                                start.elapsed()
                                            );
                                            Message::Thumbnail(path, thumbnail, work_id)
                                        })
                                        .await
                                        .unwrap()
                                    };

                                    match output.send(message).await {
                                        Ok(()) => {}
                                        Err(err) => {
                                            log::warn!(
                                                "failed to send thumbnail for {}: {}",
                                                path.display(),
                                                err
                                            );
                                        }
                                    }

                                    std::future::pending().await
                                },
                            )
                        },
                    ));
                    thumbnails += 1;
                }

                if thumbnails >= jobs && children_counts >= jobs {
                    break;
                }
            }

            if preview {
                // Load directory size for selected items

                let mut selected_items: Vec<&Item> =
                    items.iter().filter(|item| item.selected).collect();

                if selected_items.is_empty()
                    && let Some(p) = self.parent_item_opt.as_ref()
                {
                    selected_items.push(p)
                }
                for item in selected_items {
                    // Item must have a path
                    if let Some(path) = item.path_opt().cloned() {
                        // Item must be calculating directory size
                        if let DirSize::Calculating(controller) = &item.dir_size {
                            struct Wrapper {
                                path: PathBuf,
                                work_id: u64,
                                controller: Controller,
                            }
                            impl Hash for Wrapper {
                                fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
                                    self.path.hash(state);
                                    self.work_id.hash(state);
                                }
                            }
                            subscriptions.push(Subscription::run_with(
                                Wrapper { path: path.clone(), work_id: item.work_id, controller: controller.clone() },
                                |Wrapper { path, work_id, controller }| {
                                    let path = path.clone();
                                    let work_id = *work_id;
                                    let controller = controller.clone();
                                    stream::channel(1, move |mut output: futures::channel::mpsc::Sender<_>| async move {
                                        let message = {
                                            let start = Instant::now();
                                            match calculate_dir_size(&path, controller).await {
                                                Ok(size) => {
                                                    log::debug!(
                                                        "calculated directory size of {} in {:?}",
                                                        path.display(),
                                                        start.elapsed()
                                                    );
                                                    Message::DirectorySize(
                                                        path.clone(),
                                                        DirSize::Directory(size),
                                                        work_id,
                                                    )
                                                }
                                                Err(err) => {
                                                    log::warn!(
                                                        "failed to calculate directory size of {}: {}",
                                                        path.display(),
                                                        err
                                                    );
                                                    Message::DirectorySize(
                                                        path.clone(),
                                                        DirSize::Error(err.to_string()),
                                                        work_id,
                                                    )
                                                }
                                            }
                                        };

                                        match output.send(message).await {
                                            Ok(()) => {}
                                            Err(err) => {
                                                log::warn!(
                                                    "failed to send directory size for {}: {}",
                                                    path.display(),
                                                    err
                                                );
                                            }
                                        }

                                        std::future::pending().await
                                    })
                                }
                            ));
                        }
                    }
                }
            }
        }

        // Load search items incrementally
        if let Location::Search(search_location, term, show_hidden, start) = &self.location {
            let location = self.location.clone();
            let search_location = search_location.clone();
            let term = term.clone();
            let show_hidden = *show_hidden;
            let start = *start;
            #[derive(Debug, Hash, Clone)]
            struct Wrapper {
                location: Location,
                search_location: SearchLocation,
                term: String,
                show_hidden: bool,
                start: Instant,
            }

            subscriptions.push(Subscription::run_with(
                Wrapper {
                    location: location.clone(),
                    search_location: search_location.clone(),
                    term: term.clone(),
                    show_hidden,
                    start,
                },
                |wrapper| {
                    let wrapper = wrapper.clone();
                    stream::channel(
                        2,
                        move |mut output: futures::channel::mpsc::Sender<Message>| async move {
                            let Wrapper {
                                location,
                                search_location,
                                term,
                                show_hidden,
                                start,
                            } = wrapper;
                            //TODO: optimal size?
                            let (results_tx, results_rx) = mpsc::channel(65536);

                            let ready = Arc::new(atomic::AtomicBool::new(false));
                            let last_modified_opt = Arc::new(RwLock::new(None));
                            output
                                .send(Message::SearchContext(
                                    location.clone(),
                                    SearchContextWrapper(Some(SearchContext {
                                        results_rx,
                                        ready: ready.clone(),
                                        last_modified_opt: last_modified_opt.clone(),
                                    })),
                                ))
                                .await
                                .unwrap();

                            let (watch_tx, mut watch_rx) = tokio::sync::watch::channel(true);
                            {
                                tokio::task::spawn_blocking(move || {
                                    scan_search(
                                        &search_location,
                                        &term,
                                        show_hidden,
                                        move |search_item| -> bool {
                                            // Don't send if the result is too old
                                            if let Some(last_modified) =
                                                *last_modified_opt.read().unwrap()
                                                && let SearchItem::Path(_, _, ref metadata) =
                                                    search_item
                                            {
                                                if let Ok(modified) = metadata.modified() {
                                                    if modified < last_modified {
                                                        return true;
                                                    }
                                                } else {
                                                    return true;
                                                }
                                            }

                                            // item_from_search_item does read_dir, gvfs stats and
                                            // image-header reads per result; build the Item here on
                                            // the worker thread so the GUI-thread SearchReady drain
                                            // only inserts it.
                                            //TODO: use correct IconSizes
                                            let item = item_from_search_item(
                                                search_item,
                                                IconSizes::default(),
                                            );
                                            match results_tx.blocking_send(item) {
                                                Ok(()) => {
                                                    if ready.swap(true, atomic::Ordering::SeqCst) {
                                                        true
                                                    } else {
                                                        // Wake up update method
                                                        watch_tx.send(false).is_ok()
                                                    }
                                                }
                                                Err(_) => false,
                                            }
                                        },
                                    );
                                    log::info!(
                                        "searched for {:?} in {} in {:?}",
                                        term,
                                        search_location,
                                        start.elapsed(),
                                    );
                                });
                            }

                            while watch_rx.changed().await.is_ok() {
                                let is_ready = *watch_rx.borrow_and_update();
                                let _ = output.send(Message::SearchReady(is_ready)).await;
                            }

                            // Send final ready
                            let _ = output.send(Message::SearchReady(true)).await;

                            std::future::pending().await
                        },
                    )
                },
            ));
        }

        if let Some(path) = self
            .edit_location
            .as_ref()
            .and_then(|x| x.location.path_opt())
            .cloned()
        {
            subscriptions.push(Subscription::run_with(
                ("tab_complete", path.clone()),
                |(_, path)| {
                    let path = path.clone();
                    stream::channel(
                        1,
                        |mut output: futures::channel::mpsc::Sender<_>| async move {
                            let message = {
                                let path = path.clone();
                                tokio::task::spawn_blocking(move || {
                                    let start = Instant::now();
                                    match tab_complete(&path) {
                                        Ok(completions) => {
                                            log::info!(
                                                "tab completed {} in {:?}",
                                                path.display(),
                                                start.elapsed()
                                            );
                                            Message::TabComplete(path.clone(), completions)
                                        }
                                        Err(err) => {
                                            log::warn!(
                                                "failed to tab complete {}: {}",
                                                path.display(),
                                                err
                                            );
                                            Message::TabComplete(path.clone(), Vec::new())
                                        }
                                    }
                                })
                                .await
                                .unwrap()
                            };

                            match output.send(message).await {
                                Ok(()) => {}
                                Err(err) => {
                                    log::warn!(
                                        "failed to send tab completion for {}: {}",
                                        path.display(),
                                        err
                                    );
                                }
                            }

                            std::future::pending().await
                        },
                    )
                },
            ));
        }

        Subscription::batch(subscriptions)
    }

    const fn format_time(&self, time: SystemTime) -> FormatTime<'_> {
        format_time(time, &self.date_time_formatter, &self.time_formatter)
    }

    fn on_drag<'a>(&self, rect_opt: Option<Rectangle>) -> Option<Message> {
        let rect = rect_opt?;
        // We only want to publish a drag message if the overlapped items of the drag rect change,
        // otherwise a view rebuild is triggered on every drag event.
        let changed = self.items_opt.as_ref().is_some_and(|items| {
            items.iter().any(|item| {
                let overlaps = item.rect_opt.get().is_some_and(|r| r.intersects(&rect));
                overlaps != item.overlaps_drag_rect
            })
        });
        changed.then_some(Message::Drag(Some(rect)))
    }
}

fn children_count_subscription(path: PathBuf, work_id: u64) -> Subscription<Message> {
    struct Wrapper {
        path: PathBuf,
        work_id: u64,
    }

    impl Hash for Wrapper {
        fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
            self.path.hash(state);
            self.work_id.hash(state);
        }
    }

    Subscription::run_with(Wrapper { path, work_id }, |wrapper| {
        let path = wrapper.path.clone();
        let work_id = wrapper.work_id;
        stream::channel(
            1,
            move |mut output: futures::channel::mpsc::Sender<_>| async move {
                let children = {
                    let dir = path.clone();
                    tokio::task::spawn_blocking(move || {
                        fs::read_dir(&dir)
                            .inspect_err(|err| {
                                log::warn!("failed to read directory {}: {}", dir.display(), err);
                            })
                            .ok()
                            .map(Iterator::count)
                    })
                    .await
                    .unwrap_or_else(|err| {
                        log::warn!("failed to count children of {}: {}", path.display(), err);
                        None
                    })
                };
                let message = Message::DirectoryChildren(path.clone(), children, work_id);

                match output.send(message).await {
                    Ok(()) => {}
                    Err(err) => {
                        log::warn!("failed to send child count for {}: {}", path.display(), err);
                    }
                }

                std::future::pending().await
            },
        )
    })
}

pub fn respond_to_scroll_direction(delta: ScrollDelta, modifiers: &Modifiers) -> Option<Message> {
    if !modifiers.control() {
        return None;
    }

    let delta_y = match delta {
        ScrollDelta::Lines { y, .. } => y,
        ScrollDelta::Pixels { y, .. } => y,
    };

    if delta_y > 0.0 {
        return Some(Message::ZoomIn);
    }

    if delta_y < 0.0 {
        return Some(Message::ZoomOut);
    }

    None
}

fn text_editor_class(
    theme: &cosmic::Theme,
    status: cosmic::widget::text_editor::Status,
) -> cosmic::iced::widget::text_editor::Style {
    let cosmic = theme.cosmic();
    let container = theme.current_container();

    let mut background: cosmic::iced::Color = container.component.base.into();
    background.a = 0.25;
    let selection = cosmic.accent.base.into();
    let value = cosmic.palette.neutral_9.into();
    let mut placeholder = cosmic.palette.neutral_9;
    placeholder.alpha = 0.7;
    let placeholder = placeholder.into();

    match status {
        cosmic::iced::widget::text_editor::Status::Active
        | cosmic::iced::widget::text_editor::Status::Disabled => {
            cosmic::iced::widget::text_editor::Style {
                background: background.into(),
                border: cosmic::iced::Border {
                    radius: cosmic.corner_radii.radius_m.into(),
                    width: 2.0,
                    color: container.component.divider.into(),
                },
                placeholder,
                value,
                selection,
            }
        }
        cosmic::iced::widget::text_editor::Status::Hovered
        | cosmic::iced::widget::text_editor::Status::Focused { .. } => {
            cosmic::iced::widget::text_editor::Style {
                background: background.into(),
                border: cosmic::iced::Border {
                    radius: cosmic.corner_radii.radius_m.into(),
                    width: 2.0,
                    color: cosmic::iced::Color::from(cosmic.accent.base),
                },
                placeholder,
                value,
                selection,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::BufWriter;
    use std::path::{Path, PathBuf};
    use std::{fs, io};

    use cosmic::iced::mouse::ScrollDelta;
    use cosmic::iced::runtime::keyboard::Modifiers;
    use cosmic::widget;
    use log::{debug, trace};
    use mime_guess::mime;
    use tempfile::TempDir;
    use test_log::test;

    use cosmic::iced::advanced::widget::Tree;
    use cosmic::iced::widget::scrollable::AbsoluteOffset;
    use cosmic::iced::{Point, Rectangle, Size, Subscription};

    use super::{
        ChecksumState, Command, DirSize, EntryRefresh, FileChecksums, HeadingOptions, Instant,
        Item, ItemMetadata, ItemThumbnail, Location, Message, SearchLocation,
        TEXT_PREVIEW_MAX_BYTES, Tab, folder_name, item_from_entry, item_from_path,
        item_from_trash_entry, respond_to_scroll_direction, scan_path,
    };
    use crate::app::test_utils::{
        NAME_LEN, NUM_DIRS, NUM_FILES, NUM_HIDDEN, NUM_NESTED, assert_eq_tab_path,
        assert_eq_tab_path_contents, empty_fs, eq_path_item, filter_dirs, read_dir_sorted,
        simple_fs, tab_click_new,
    };
    use crate::config::{IconSizes, TabConfig, ThumbCfg};
    use crate::tab::View;

    // Boilerplate for tab tests. Checks if simulated clicks selected items.
    fn tab_selects_item(
        clicks: &[usize],
        modifiers: Modifiers,
        expected_selected: &[bool],
    ) -> io::Result<()> {
        let (_fs, mut tab) = tab_click_new(NUM_FILES, NUM_NESTED, NUM_DIRS, NUM_NESTED, NAME_LEN)?;

        // Simulate clicks by triggering Message::Click
        for &click in clicks {
            debug!("Emitting Message::Click(Some({click})) with modifiers: {modifiers:?}");
            tab.update(Message::Click(Some(click)), modifiers);
        }

        let items = tab
            .items_opt
            .as_deref()
            .expect("tab should be populated with items");

        for (i, (&expected, actual)) in expected_selected.iter().zip(items).enumerate() {
            assert_eq!(
                expected,
                actual.selected,
                "expected index {i} to be {}",
                if expected {
                    "selected but it was deselected"
                } else {
                    "deselected but it was selected"
                }
            );
        }

        Ok(())
    }

    fn tab_history() -> io::Result<(TempDir, Tab, Vec<PathBuf>)> {
        let fs = simple_fs(NUM_FILES, NUM_NESTED, NUM_DIRS, NUM_NESTED, NAME_LEN)?;
        let path = fs.path();
        let mut tab = Tab::new(
            Location::Path(path.into()),
            TabConfig::default(),
            ThumbCfg::default(),
            None,
            widget::Id::unique(),
            None,
        );

        // All directories (simple_fs only produces one nested layer)
        let dirs: Vec<PathBuf> = {
            let top_level = filter_dirs(path)?;
            let mut result = Vec::new();
            for dir in top_level {
                let nested_dirs = filter_dirs(&dir)?;
                result.push(dir);
                result.extend(nested_dirs);
            }
            result
        };
        assert!(
            dirs.len() == NUM_DIRS + NUM_DIRS * NUM_NESTED,
            "Sanity check: Have {} dirs instead of {}",
            dirs.len(),
            NUM_DIRS + NUM_DIRS * NUM_NESTED
        );

        debug!("Building history by emitting Message::Location");
        for dir in &dirs {
            debug!(
                "Emitting Message::Location(Location::Path(\"{}\"))",
                dir.display()
            );
            tab.update(
                Message::Location(Location::Path(dir.clone())),
                Modifiers::empty(),
            );
        }
        trace!("Tab history: {:?}", tab.history);

        Ok((fs, tab, dirs))
    }

    #[test]
    fn scan_path_succeeds_on_valid_path() -> io::Result<()> {
        let fs = simple_fs(NUM_FILES, NUM_HIDDEN, NUM_DIRS, NUM_NESTED, NAME_LEN)?;
        let path = fs.path();

        // Read directory entries and sort as cosmic-files does
        let entries = read_dir_sorted(path)?;

        debug!("Calling scan_path(\"{}\")", path.display());
        let actual = scan_path(&path.to_owned(), IconSizes::default());

        // scan_path shouldn't skip any entries
        assert_eq!(entries.len(), actual.len());

        // Correct files should be scanned
        assert!(
            entries
                .into_iter()
                .zip(actual.into_iter())
                .all(|(path, item)| eq_path_item(&path, &item))
        );

        Ok(())
    }

    #[test]
    fn tab_listing_matches_directory_contents() -> io::Result<()> {
        let fs = simple_fs(NUM_FILES, NUM_HIDDEN, NUM_DIRS, NUM_NESTED, NAME_LEN)?;
        let path = fs.path();

        let mut tab = test_tab(&fs);
        tab.set_items(scan_path(&path.to_owned(), IconSizes::default()));

        // Pins name, path, directory-ness, hidden flag and position for every entry, so a
        // scan that dropped, duplicated or reordered entries fails here.
        assert_eq_tab_path_contents(&tab, path);
        Ok(())
    }

    #[test]
    fn scan_path_returns_empty_vec_for_invalid_path() -> io::Result<()> {
        let fs = simple_fs(NUM_FILES, NUM_NESTED, NUM_DIRS, NUM_NESTED, NAME_LEN)?;
        let path = fs.path();

        // A nonexisting path within the temp dir
        let invalid_path = path.join("ferris");
        assert!(!invalid_path.exists());

        debug!("Calling scan_path(\"{}\")", invalid_path.display());
        let actual = scan_path(&invalid_path, IconSizes::default());

        assert!(actual.is_empty());

        Ok(())
    }

    #[test]
    fn scan_path_empty_dir_returns_empty_vec() -> io::Result<()> {
        let fs = empty_fs()?;
        let path = fs.path();

        debug!("Calling scan_path(\"{}\")", path.display());
        let actual = scan_path(&path.to_owned(), IconSizes::default());

        assert_eq!(0, path.read_dir()?.count());
        assert!(actual.is_empty());

        Ok(())
    }

    fn test_tab(fs: &TempDir) -> Tab {
        tab_at(Location::Path(fs.path().to_owned()))
    }

    fn tab_at(location: Location) -> Tab {
        Tab::new(
            location,
            TabConfig::default(),
            ThumbCfg::default(),
            None,
            widget::Id::unique(),
            None,
        )
    }

    #[test]
    fn an_entry_named_in_dot_hidden_is_marked_hidden() -> io::Result<()> {
        let fs = TempDir::new()?;
        let dir = fs.path().to_owned();
        fs::write(dir.join(".hidden"), "secret.txt\n")?;
        fs::write(dir.join("secret.txt"), b"x")?;
        fs::write(dir.join("public.txt"), b"x")?;

        let items = scan_path(&dir, IconSizes::default());

        let hidden = |name: &str| {
            items
                .iter()
                .find(|item| item.name == name)
                .unwrap_or_else(|| panic!("{name} should be listed"))
                .hidden
        };
        assert!(hidden("secret.txt"));
        assert!(!hidden("public.txt"));
        Ok(())
    }

    #[test]
    fn begin_rescan_if_idle_starts_a_scan_when_idle() -> io::Result<()> {
        let fs = empty_fs()?;
        let mut tab = test_tab(&fs);

        assert!(!tab.scans.defer_if_running());
        let request = tab.scans.begin();
        assert_eq!(tab.scans.finish(request), Some(false));

        Ok(())
    }

    #[test]
    fn begin_rescan_if_idle_coalesces_concurrent_watcher_events() -> io::Result<()> {
        let fs = empty_fs()?;
        let mut tab = test_tab(&fs);

        assert!(!tab.scans.defer_if_running());
        let request = tab.scans.begin();
        for _ in 0..10 {
            assert!(tab.scans.defer_if_running());
        }

        assert_eq!(tab.scans.finish(request), Some(true));

        Ok(())
    }

    #[test]
    fn finish_rescan_with_no_pending_events_does_not_request_another_scan() -> io::Result<()> {
        let fs = empty_fs()?;
        let mut tab = test_tab(&fs);

        let request = tab.scans.begin();
        assert_eq!(tab.scans.finish(request), Some(false));

        Ok(())
    }

    #[test]
    fn finish_rescan_allows_a_fresh_scan_to_start_afterwards() -> io::Result<()> {
        let fs = empty_fs()?;
        let mut tab = test_tab(&fs);

        let request = tab.scans.begin();
        assert_eq!(tab.scans.finish(request), Some(false));
        assert!(!tab.scans.defer_if_running());
        let next = tab.scans.begin();
        assert_eq!(tab.scans.finish(next), Some(false));

        Ok(())
    }

    #[test]
    fn cached_metadata_reflects_scan_without_restat() -> io::Result<()> {
        let fs = simple_fs(NUM_FILES, NUM_NESTED, NUM_DIRS, NUM_NESTED, NAME_LEN)?;
        let path = fs.path().to_owned();
        let items = scan_path(&path, IconSizes::default());
        assert!(!items.is_empty());

        for item in &items {
            let cached = item.cached_metadata();
            assert!(cached.is_some(), "local scanned item should cache metadata");
            assert_eq!(cached.unwrap().is_dir(), item.metadata.is_dir());
        }

        drop(fs);
        for item in &items {
            if let Some(item_path) = item.path_opt() {
                assert!(!item_path.exists());
            }
            assert!(item.cached_metadata().is_some());
        }

        Ok(())
    }

    #[test]
    fn folder_name_derives_last_path_component() {
        // No '.' or '_' in the component, so Item::display_name leaves it untouched.
        let (name, found_home) = folder_name("/srv/data/Projects");
        assert_eq!(name, "Projects");
        assert!(!found_home);
    }

    #[test]
    fn folder_name_does_not_stat_the_path() {
        let missing = "/this/path/does/not/exist/Documents";
        assert!(!Path::new(missing).exists());

        let (name, found_home) = folder_name(missing);
        assert_eq!(name, "Documents");
        assert!(!found_home);
    }

    #[test]
    fn folder_name_flags_the_home_directory() {
        let home = crate::home_dir();
        // The "/" fallback (no resolvable home) has no final component to match; only assert
        // the flag when home resolves to a named directory.
        if home.file_name().is_some() {
            let (_name, found_home) = folder_name(&home);
            assert!(found_home);
        }
    }

    #[test]
    fn normalize_preserves_parent_segments_without_filesystem_resolution() {
        let normalized =
            Location::Path(PathBuf::from("/no/such/dir/../sibling/./leaf")).normalize();
        assert_eq!(
            normalized.path_opt().unwrap(),
            Path::new("/no/such/dir/../sibling/leaf")
        );
    }

    #[test]
    fn normalize_marks_directories_with_a_trailing_separator() {
        // The trailing separator keeps a location's string form (the persisted per-folder
        // sort-setting key) stable, and is added without a stat.
        let normalized = Location::Path(PathBuf::from("/no/such/dir")).normalize();
        assert_eq!(normalized.to_string(), "/no/such/dir/");
    }

    #[test]
    fn tab_location_select_navigates_and_carries_selection() -> io::Result<()> {
        // A typed/completed file resolves off-thread to (parent dir, file) and arrives as
        // Message::LocationSelect. The tab must navigate to the directory with no GUI-thread stat
        // and forward the file to select once the off-thread rescan completes.
        let fs = simple_fs(NUM_FILES, NUM_NESTED, NUM_DIRS, NUM_NESTED, NAME_LEN)?;
        let path = fs.path();
        let dir = filter_dirs(path)?
            .next()
            .expect("temp directory should have at least one directory");
        let file = dir.join("selected.txt");
        fs::write(&file, b"x")?;

        let mut tab = Tab::new(
            Location::Path(path.to_owned()),
            TabConfig::default(),
            ThumbCfg::default(),
            None,
            widget::Id::unique(),
            None,
        );

        let commands = tab.update(
            Message::LocationSelect(Location::Path(dir.clone()), vec![file.clone()]),
            Modifiers::empty(),
        );

        assert_eq_tab_path(&tab, &dir);
        let carries_selection = commands.iter().any(|command| {
            matches!(command, Command::ChangeLocation(_, _, Some(selection)) if *selection == vec![file.clone()])
        });
        assert!(
            carries_selection,
            "LocationSelect should forward the file selection to Command::ChangeLocation"
        );
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn normalize_keeps_symlinks_unresolved() -> io::Result<()> {
        // normalize() must not resolve symlinks: doing so would stat-walk every component and
        // block the GUI thread. The kernel follows the link when the directory is read, so
        // navigation is unaffected and the clicked path is preserved.
        let temp = TempDir::new()?;
        let real = temp.path().join("real");
        fs::create_dir(&real)?;
        let link = temp.path().join("link");
        std::os::unix::fs::symlink(&real, &link)?;

        let normalized = Location::Path(link.clone()).normalize();
        // Path equality ignores the trailing separator, so this asserts the link was NOT
        // rewritten to `real`.
        assert_eq!(normalized.path_opt().unwrap(), link.as_path());
        Ok(())
    }

    #[test]
    fn normalize_expands_a_leading_tilde() {
        let home = dirs::home_dir().expect("a home directory");

        let normalized = Location::Path(PathBuf::from("~/Documents")).normalize();

        assert_eq!(normalized.path_opt().unwrap(), &home.join("Documents"));

        let up = Location::Path(PathBuf::from("~/..")).normalize();
        assert_eq!(up.path_opt().unwrap(), &home.join(".."));
    }

    #[test]
    fn a_search_tab_cannot_say_whether_a_name_in_its_root_is_taken() -> io::Result<()> {
        let fs = TempDir::new()?;
        let root = fs.path().to_owned();

        let listing = tab_at(Location::Path(root.clone()));
        let searching = tab_at(Location::Search(
            SearchLocation::Path(root.clone()),
            "report".to_string(),
            false,
            Instant::now(),
        ));

        // Both tabs answer to the same path, but a search tab's entries come from anywhere
        // below it, so a name it holds may belong to a subdirectory rather than the root.
        assert!(listing.lists_directory(&root));
        assert!(!searching.lists_directory(&root));
        Ok(())
    }

    #[test]
    fn selected_paths_report_their_directory_flag_without_restating() -> io::Result<()> {
        let fs = TempDir::new()?;
        let file = fs.path().join("note.txt");
        fs::write(&file, b"x")?;
        let dir = fs.path().join("folder");
        fs::create_dir(&dir)?;

        let mut tab = tab_at(Location::Path(fs.path().to_owned()));
        tab.items_opt = Some(scan_path(&fs.path().to_owned(), IconSizes::default()));
        for item in tab.items_opt.as_mut().unwrap() {
            item.selected = true;
        }
        // Removing the entries leaves any stat-based answer with nothing to read.
        fs::remove_file(&file)?;
        fs::remove_dir(&dir)?;

        let mut selected = tab.selected_paths_with_dir();
        selected.sort();

        assert_eq!(selected, vec![(dir, true), (file, false)]);
        Ok(())
    }

    #[test]
    fn tab_location_changes_location() -> io::Result<()> {
        let fs = simple_fs(NUM_FILES, NUM_NESTED, NUM_DIRS, NUM_NESTED, NAME_LEN)?;
        let path = fs.path();

        // Next directory in temp directory
        // This does not have to be sorted
        let next_dir = filter_dirs(path)?
            .next()
            .expect("temp directory should have at least one directory");

        let mut tab = Tab::new(
            Location::Path(path.to_owned()),
            TabConfig::default(),
            ThumbCfg::default(),
            None,
            widget::Id::unique(),
            None,
        );
        debug!(
            "Emitting Message::Location(Location::Path(\"{}\"))",
            next_dir.display()
        );
        tab.update(
            Message::Location(Location::Path(next_dir.clone())),
            Modifiers::empty(),
        );

        // Validate that the tab's path updated
        // NOTE: `items_opt` is set to None with Message::Location so this ONLY checks for equal paths
        // If item contents are NOT None then this needs to be reevaluated for correctness
        assert_eq_tab_path(&tab, &next_dir);
        assert!(
            tab.items_opt.is_none(),
            "Tab's `items` is not None which means this test needs to be updated"
        );

        Ok(())
    }

    #[test]
    fn tab_click_single_selects_item() -> io::Result<()> {
        // Select the second directory with no keys held down
        tab_selects_item(&[1], Modifiers::empty(), &[false, true])
    }

    #[test]
    fn tab_click_double_opens_folder() -> io::Result<()> {
        let (fs, mut tab) = tab_click_new(NUM_FILES, NUM_NESTED, NUM_DIRS, NUM_NESTED, NAME_LEN)?;
        let path = fs.path();

        // Simulate double clicking second directory
        debug!("Emitting double click Message::DoubleClick(Some(1))");
        tab.update(Message::DoubleClick(Some(1)), Modifiers::empty());

        // Path to second directory
        let second_dir = read_dir_sorted(path)?
            .into_iter()
            .filter(|p| p.is_dir())
            .nth(1)
            .expect("should be at least two directories");

        // Location should have changed to second_dir
        assert_eq_tab_path(&tab, &second_dir);

        Ok(())
    }

    #[test]
    fn tab_click_ctrl_selects_multiple() -> io::Result<()> {
        // Select the first and second directory by holding down ctrl
        tab_selects_item(&[0, 1], Modifiers::CTRL, &[true, true])
    }

    #[test]
    fn tab_gonext_moves_forward_in_history() -> io::Result<()> {
        let (fs, mut tab, dirs) = tab_history()?;
        let path = fs.path();

        // Rewind to the start
        for _ in 0..dirs.len() {
            debug!("Emitting Message::GoPrevious to rewind to the start",);
            tab.update(Message::GoPrevious, Modifiers::empty());
        }
        assert_eq_tab_path(&tab, path);

        // Back to the future. Directories should be in the order they were opened.
        for dir in dirs {
            debug!("Emitting Message::GoNext",);
            tab.update(Message::GoNext, Modifiers::empty());
            assert_eq_tab_path(&tab, &dir);
        }

        Ok(())
    }

    #[test]
    fn tab_goprev_moves_backward_in_history() -> io::Result<()> {
        let (fs, mut tab, dirs) = tab_history()?;
        let path = fs.path();

        for dir in dirs.into_iter().rev() {
            assert_eq_tab_path(&tab, &dir);
            debug!("Emitting Message::GoPrevious",);
            tab.update(Message::GoPrevious, Modifiers::empty());
        }
        assert_eq_tab_path(&tab, path);

        Ok(())
    }

    #[test]
    fn tab_scroll_up_with_ctrl_modifier_zooms() -> io::Result<()> {
        let message_maybe =
            respond_to_scroll_direction(ScrollDelta::Pixels { x: 0.0, y: 1.0 }, &Modifiers::CTRL);
        assert!(message_maybe.is_some());
        assert!(matches!(message_maybe.unwrap(), Message::ZoomIn));
        Ok(())
    }

    #[test]
    fn tab_scroll_up_without_ctrl_modifier_does_not_zoom() -> io::Result<()> {
        let message_maybe = respond_to_scroll_direction(
            ScrollDelta::Pixels { x: 0.0, y: 1.0 },
            &Modifiers::empty(),
        );
        assert!(message_maybe.is_none());
        Ok(())
    }

    #[test]
    fn tab_scroll_down_with_ctrl_modifier_zooms() -> io::Result<()> {
        let message_maybe =
            respond_to_scroll_direction(ScrollDelta::Pixels { x: 0.0, y: -1.0 }, &Modifiers::CTRL);
        assert!(message_maybe.is_some());
        assert!(matches!(message_maybe.unwrap(), Message::ZoomOut));
        Ok(())
    }

    #[test]
    fn tab_scroll_down_without_ctrl_modifier_does_not_zoom() -> io::Result<()> {
        let message_maybe = respond_to_scroll_direction(
            ScrollDelta::Pixels { x: 0.0, y: -1.0 },
            &Modifiers::empty(),
        );
        assert!(message_maybe.is_none());
        Ok(())
    }
    #[test]
    fn tab_empty_history_does_nothing_on_prev_next() -> io::Result<()> {
        let fs = simple_fs(0, NUM_NESTED, NUM_DIRS, 0, NAME_LEN)?;
        let path = fs.path();
        let mut tab = Tab::new(
            Location::Path(path.into()),
            TabConfig::default(),
            ThumbCfg::default(),
            None,
            widget::Id::unique(),
            None,
        );

        // Tab's location shouldn't change if GoPrev or GoNext is triggered
        debug!("Emitting Message::GoPrevious",);
        tab.update(Message::GoPrevious, Modifiers::empty());
        assert_eq_tab_path(&tab, path);

        debug!("Emitting Message::GoNext",);
        tab.update(Message::GoNext, Modifiers::empty());
        assert_eq_tab_path(&tab, path);

        Ok(())
    }

    #[test]
    fn tab_locationup_moves_up_hierarchy() -> io::Result<()> {
        let fs = simple_fs(0, NUM_NESTED, NUM_DIRS, 0, NAME_LEN)?;
        let path = fs.path();
        let mut next_dir = filter_dirs(path)?
            .next()
            .expect("should be at least one directory");

        let mut tab = Tab::new(
            Location::Path(next_dir.clone()),
            TabConfig::default(),
            ThumbCfg::default(),
            None,
            widget::Id::unique(),
            None,
        );
        // This will eventually yield false once root is hit
        while next_dir.pop() {
            debug!("Emitting Message::LocationUp",);
            tab.update(Message::LocationUp, Modifiers::empty());
            assert_eq_tab_path(&tab, &next_dir);
        }

        Ok(())
    }

    #[test]
    fn sort_long_number_file_names() -> io::Result<()> {
        let fs = empty_fs()?;
        let path = fs.path();

        // Create files with names 255 characters long that only contain a single number
        // Example: 0000...0 for 255 characters
        // https://en.wikipedia.org/wiki/Filename#Comparison_of_filename_limitations
        let mut base_nums: Vec<_> = ('0'..='9').collect();
        fastrand::shuffle(&mut base_nums);
        debug!("Shuffled numbers for paths: {base_nums:?}");
        let paths: Vec<_> = base_nums
            .iter()
            .copied()
            .map(|base| path.join(std::iter::repeat_n(base, 255).collect::<String>()))
            .collect();

        for (file, base) in paths.iter().zip(base_nums.into_iter()) {
            trace!("Creating long file name for {base}");
            fs::File::create(file)?;
        }

        debug!("Creating tab for directory of long file names");
        Tab::new(
            Location::Path(path.into()),
            TabConfig::default(),
            ThumbCfg::default(),
            None,
            widget::Id::unique(),
            None,
        );

        Ok(())
    }

    #[test]
    fn mode_calculations() {
        use super::{
            MODE_SHIFT_GROUP, MODE_SHIFT_OTHER, MODE_SHIFT_USER, get_mode_part, set_mode_part,
        };
        for user in 0..=7 {
            for group in 0..=7 {
                for other in 0..=7 {
                    let mode = (user << MODE_SHIFT_USER)
                        | (group << MODE_SHIFT_GROUP)
                        | (other << MODE_SHIFT_OTHER);
                    assert_eq!(format!("{mode:03o}"), format!("{user:o}{group:o}{other:o}"),);
                    assert_eq!(get_mode_part(mode, MODE_SHIFT_USER), user);
                    assert_eq!(get_mode_part(mode, MODE_SHIFT_GROUP), group);
                    assert_eq!(get_mode_part(mode, MODE_SHIFT_OTHER), other);

                    let mode_no_user = (group << MODE_SHIFT_GROUP) | (other << MODE_SHIFT_OTHER);
                    assert_eq!(
                        format!("{mode_no_user:03o}"),
                        format!("0{group:o}{other:o}")
                    );
                    assert_eq!(set_mode_part(mode_no_user, MODE_SHIFT_USER, user), mode);

                    let mode_no_group = (user << MODE_SHIFT_USER) | (other << MODE_SHIFT_OTHER);
                    assert_eq!(
                        format!("{mode_no_group:03o}"),
                        format!("{user:o}0{other:o}")
                    );
                    assert_eq!(set_mode_part(mode_no_group, MODE_SHIFT_GROUP, group), mode);

                    let mode_no_other = (user << MODE_SHIFT_USER) | (group << MODE_SHIFT_GROUP);
                    assert_eq!(
                        format!("{mode_no_other:03o}"),
                        format!("{user:o}{group:o}0")
                    );
                    assert_eq!(set_mode_part(mode_no_other, MODE_SHIFT_OTHER, other), mode);
                }
            }
        }
    }

    #[test]
    fn item_thumbnail_text_preview_small_utf8_returns_text() -> io::Result<()> {
        let dir = TempDir::new()?;
        let path = dir.path().join("preview.txt");
        fs::write(&path, "Hello, world!")?;
        let metadata = fs::metadata(&path)?;
        let item_metadata = ItemMetadata::Path {
            metadata,
            children_opt: None,
        };
        let thumb = ItemThumbnail::new(
            &path,
            item_metadata,
            mime::TEXT_PLAIN,
            128,
            100 * 1024 * 1024,
            1,
            8,
        );
        assert!(
            matches!(thumb, ItemThumbnail::Text(_)),
            "small text file should produce Text thumbnail"
        );
        Ok(())
    }

    #[test]
    fn a_standalone_preview_item_carries_no_shaped_text() -> io::Result<()> {
        let dir = TempDir::new()?;
        let path = dir.path().join("notes.txt");
        fs::write(&path, "hello")?;

        let item = item_from_path(&path, IconSizes::default()).expect("the file should resolve");

        // App::context_drawer clones the custom preview item on every rebuild, and cloning a text
        // preview re-shapes it under the process-wide font system. Only the tab subscription
        // builds those, and it reaches listed items alone.
        assert!(
            !matches!(item.thumbnail_opt, Some(ItemThumbnail::Text(_))),
            "a preview item built outside a listing must not carry a shaped text preview"
        );
        Ok(())
    }

    #[test]
    fn a_text_preview_is_capped_at_what_the_preview_can_show() -> io::Result<()> {
        let dir = TempDir::new()?;
        let path = dir.path().join("big.log");
        let line = "the quick brown fox jumps over the lazy dog\n";
        fs::write(&path, line.repeat(TEXT_PREVIEW_MAX_BYTES / line.len() * 4))?;
        let item_metadata = ItemMetadata::Path {
            metadata: fs::metadata(&path)?,
            children_opt: None,
        };

        let thumb = ItemThumbnail::new(&path, item_metadata, mime::TEXT_PLAIN, 128, 100, 1, 8);

        let ItemThumbnail::Text(content) = thumb else {
            panic!("a text file should produce a text preview");
        };
        assert!(
            content.text().len() <= TEXT_PREVIEW_MAX_BYTES + 1,
            "a preview shapes at most {TEXT_PREVIEW_MAX_BYTES} bytes, got {}",
            content.text().len()
        );
        Ok(())
    }

    #[test]
    fn item_thumbnail_text_preview_empty_file_returns_not_image() -> io::Result<()> {
        let dir = TempDir::new()?;
        let path = dir.path().join("empty.txt");
        fs::File::create(&path)?;
        let metadata = fs::metadata(&path)?;
        let item_metadata = ItemMetadata::Path {
            metadata,
            children_opt: None,
        };
        let thumb = ItemThumbnail::new(
            &path,
            item_metadata,
            mime::TEXT_PLAIN,
            128,
            100 * 1024 * 1024,
            1,
            8,
        );
        assert!(
            matches!(thumb, ItemThumbnail::NotImage),
            "empty text file should produce NotImage (no read)"
        );
        Ok(())
    }

    #[test]
    fn item_thumbnail_text_preview_invalid_utf8_uses_valid_prefix() -> io::Result<()> {
        let dir = TempDir::new()?;
        let path = dir.path().join("invalid_utf8.txt");
        // Valid UTF-8 "ab" then invalid byte sequence then "c"
        fs::write(&path, b"ab\xff\xfe\xfdc")?;
        let metadata = fs::metadata(&path)?;
        let item_metadata = ItemMetadata::Path {
            metadata,
            children_opt: None,
        };
        let thumb = ItemThumbnail::new(
            &path,
            item_metadata,
            mime::TEXT_PLAIN,
            128,
            100 * 1024 * 1024,
            1,
            8,
        );
        match &thumb {
            ItemThumbnail::Text(content) => {
                // Text editor content may add a trailing newline
                assert_eq!(content.text().trim_end(), "ab");
            }
            _ => panic!(
                "expected Text thumbnail with valid prefix only, got {:?}",
                thumb
            ),
        }
        Ok(())
    }

    fn write_test_png(path: &Path, width: u32, height: u32) -> io::Result<()> {
        let mut encoder = png::Encoder::new(BufWriter::new(fs::File::create(path)?), width, height);
        encoder.set_color(png::ColorType::Grayscale);
        encoder.set_depth(png::BitDepth::Eight);
        encoder
            .write_header()
            .and_then(|mut writer| writer.write_image_data(&vec![0; (width * height) as usize]))
            .map_err(io::Error::other)
    }

    #[test]
    fn cached_thumbnails_do_not_invent_original_dimensions() -> io::Result<()> {
        use crate::thumbnail_cacher::{CachedThumbnail, ThumbnailCacher, ThumbnailSize};
        let dir = TempDir::new()?;
        let path = dir.path().join("document.pdf");
        fs::write(&path, b"unsupported original")?;
        let cache = ThumbnailCacher::new(&path, ThumbnailSize::from_pixel_size(64))
            .map_err(io::Error::other)?;
        let cached_path = cache
            .update_with_image(image::DynamicImage::new_rgba8(7, 3))
            .map_err(|err| io::Error::other(err.to_string()))?
            .to_path_buf();
        assert!(matches!(
            cache.get_cached_thumbnail(),
            CachedThumbnail::Valid(_)
        ));
        let item = item_from_path(&path, IconSizes::default()).unwrap();
        let thumbnail = ItemThumbnail::new(&path, item.metadata, item.mime, 64, 64, 1, 64);
        fs::remove_file(cached_path)?;
        assert!(matches!(thumbnail, ItemThumbnail::Image(_, None)));
        Ok(())
    }

    #[test]
    fn review_a_local_thumbnail_populates_image_details() -> io::Result<()> {
        let dir = TempDir::new()?;
        let path = dir.path().join("photo.png");
        write_test_png(&path, 7, 3)?;
        let mut tab = test_tab(&dir);
        tab.set_items(scan_path(&dir.path().to_path_buf(), IconSizes::default()));
        let item = &tab.items_opt().unwrap()[0];
        let work_id = item.work_id;
        let thumbnail = ItemThumbnail::new(
            &path,
            item.metadata.clone(),
            item.mime.clone(),
            64,
            64,
            1,
            64,
        );
        tab.update(
            Message::Thumbnail(path, thumbnail, work_id),
            Modifiers::empty(),
        );
        assert_eq!(tab.items_opt().unwrap()[0].image_dimensions, Some((7, 3)));
        Ok(())
    }

    #[test]
    fn stale_thumbnail_dimensions_cannot_populate_a_replacement() -> io::Result<()> {
        let dir = TempDir::new()?;
        let path = dir.path().join("photo.png");
        write_test_png(&path, 7, 3)?;
        let mut tab = test_tab(&dir);
        tab.set_items(scan_path(&dir.path().to_path_buf(), IconSizes::default()));
        let old = tab.items_opt().unwrap()[0].work_id;
        tab.set_items(scan_path(&dir.path().to_path_buf(), IconSizes::default()));
        tab.update(
            Message::Thumbnail(
                path.clone(),
                ItemThumbnail::Image(widget::image::Handle::from_path(path), Some((7, 3))),
                old,
            ),
            Modifiers::empty(),
        );
        assert_eq!(tab.items_opt().unwrap()[0].image_dimensions, None);
        Ok(())
    }

    #[test]
    fn trash_item_reads_image_dimensions_from_the_trashed_file() -> io::Result<()> {
        let fs_dir = TempDir::new()?;
        let trashed = fs_dir.path().join("photo.png");
        write_test_png(&trashed, 7, 3)?;

        let entry = trash::TrashItem {
            id: trashed.into_os_string(),
            name: std::ffi::OsString::from("photo.png"),
            // The path the file was deleted from no longer holds it.
            original_parent: fs_dir.path().join("albums"),
            time_deleted: 0,
        };
        let item = item_from_trash_entry(
            entry,
            trash::TrashItemMetadata {
                size: trash::TrashItemSize::Bytes(0),
            },
            IconSizes::default(),
        );

        assert_eq!(item.image_dimensions, Some((7, 3)));
        Ok(())
    }

    fn tab_with_files(count: usize) -> io::Result<(TempDir, Tab)> {
        let dir = tempfile::tempdir()?;
        for i in 0..count {
            fs::write(dir.path().join(format!("file{i:05}.txt")), b"x")?;
        }
        let mut tab = Tab::new(
            Location::Path(dir.path().into()),
            TabConfig::default(),
            ThumbCfg::default(),
            None,
            widget::Id::unique(),
            None,
        );
        tab.set_items(scan_path(&dir.path().to_path_buf(), IconSizes::default()));
        tab.size_opt.set(Some(Size::new(800.0, 300.0)));
        Ok((dir, tab))
    }

    #[test]
    fn capped_search_keeps_new_results_in_modified_order() -> io::Result<()> {
        let dir = TempDir::new()?;
        let mut tab = test_tab(&dir);
        tab.location = Location::Search(
            SearchLocation::Path(dir.path().into()),
            String::new(),
            false,
            Instant::now(),
        );
        tab.set_items(Vec::new());
        let (sender, results_rx) = tokio::sync::mpsc::channel(super::MAX_SEARCH_RESULTS + 1);
        tab.search_context = Some(super::SearchContext {
            results_rx,
            ready: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true)),
            last_modified_opt: std::sync::Arc::new(std::sync::RwLock::new(None)),
        });
        let make_item = |i: usize| -> io::Result<Item> {
            let path = dir.path().join(format!("result{i:04}"));
            let file = std::fs::File::create(&path)?;
            file.set_times(std::fs::FileTimes::new().set_modified(
                std::time::UNIX_EPOCH + std::time::Duration::from_secs((i / 2) as u64 + 1),
            ))?;
            Ok(item_from_path(path, IconSizes::default()).unwrap())
        };
        for i in 0..super::MAX_SEARCH_RESULTS {
            sender.try_send(make_item(i)?).unwrap();
        }
        for _ in 0..super::MAX_SEARCH_RESULTS {
            if tab.items_opt().unwrap().len() == super::MAX_SEARCH_RESULTS {
                break;
            }
            tab.search_context
                .as_ref()
                .unwrap()
                .ready
                .store(true, std::sync::atomic::Ordering::SeqCst);
            tab.update(Message::SearchReady(false), Modifiers::empty());
        }
        let before = tab.column_sort().unwrap();
        assert_eq!(before.len(), super::MAX_SEARCH_RESULTS);
        sender
            .try_send(make_item(super::MAX_SEARCH_RESULTS)?)
            .unwrap();
        tab.update(Message::SearchReady(true), Modifiers::empty());
        let after = tab.column_sort().unwrap();
        assert_eq!(after.len(), super::MAX_SEARCH_RESULTS);
        assert_eq!(after[0].1.name, "result0200");
        assert!(
            after
                .windows(2)
                .all(|pair| pair[0].1.metadata.modified() >= pair[1].1.metadata.modified())
        );
        Ok(())
    }

    #[test]
    fn sort_order_survives_the_cache() -> io::Result<()> {
        let (_dir, mut tab) = tab_with_files(50)?;

        let ascending: Vec<String> = tab
            .column_sort()
            .expect("tab should be populated")
            .iter()
            .map(|(_, item)| item.name.clone())
            .collect();

        tab.update(
            Message::SetSort(HeadingOptions::Name, false),
            Modifiers::empty(),
        );
        let descending: Vec<String> = tab
            .column_sort()
            .expect("tab should be populated")
            .iter()
            .map(|(_, item)| item.name.clone())
            .collect();

        assert_ne!(ascending, descending, "the cache outlived a sort change");
        assert_eq!(
            ascending,
            descending.iter().rev().cloned().collect::<Vec<_>>()
        );
        Ok(())
    }

    #[test]
    fn a_metadata_refresh_only_discards_the_sort_order_that_reads_it() -> io::Result<()> {
        let (dir, mut tab) = tab_with_files(4)?;
        let path = dir.path().join("file00000.txt");

        for (sort, discarded) in [
            (HeadingOptions::Name, false),
            (HeadingOptions::Size, true),
            (HeadingOptions::Modified, true),
        ] {
            tab.update(Message::SetSort(sort, true), Modifiers::empty());
            tab.column_sort().expect("tab should be populated");
            assert!(
                tab.refresh_entries(vec![refreshed_entry(&tab, &path)]),
                "every refreshed entry is still listed"
            );
            assert_eq!(
                tab.sort_cache.borrow().is_none(),
                discarded,
                "sorting by {sort:?} should{} rebuild the order for new metadata",
                if discarded { "" } else { " not" }
            );
        }
        Ok(())
    }

    #[test]
    fn a_child_count_only_discards_the_sort_order_that_reads_it() -> io::Result<()> {
        let dir = tempfile::tempdir()?;
        fs::create_dir(dir.path().join("subdir"))?;
        let mut tab = Tab::new(
            Location::Path(dir.path().into()),
            TabConfig::default(),
            ThumbCfg::default(),
            None,
            widget::Id::unique(),
            None,
        );
        tab.set_items(scan_path(&dir.path().to_path_buf(), IconSizes::default()));

        let counted = Message::DirectoryChildren(
            dir.path().join("subdir"),
            Some(3),
            entry_work_id(&tab, &dir.path().join("subdir")),
        );
        for (sort, discarded) in [(HeadingOptions::Name, false), (HeadingOptions::Size, true)] {
            tab.update(Message::SetSort(sort, true), Modifiers::empty());
            tab.column_sort().expect("tab should be populated");
            tab.update(counted.clone(), Modifiers::empty());
            assert_eq!(
                tab.sort_cache.borrow().is_none(),
                discarded,
                "sorting by {sort:?} should{} rebuild the order for a child count",
                if discarded { "" } else { " not" }
            );
        }
        Ok(())
    }

    #[test]
    fn a_text_preview_is_built_only_where_one_is_rendered() -> io::Result<()> {
        let dir = tempfile::tempdir()?;
        fs::write(dir.path().join("notes.txt"), b"hello")?;
        fs::write(dir.path().join("photo.png"), b"")?;
        let mut tab = Tab::new(
            Location::Path(dir.path().into()),
            TabConfig::default(),
            ThumbCfg::default(),
            None,
            widget::Id::unique(),
            None,
        );
        tab.set_items(scan_path(&dir.path().to_path_buf(), IconSizes::default()));

        let displayed = |tab: &Tab, name: &str, preview: bool| {
            let items = tab.items_opt().expect("tab should be populated");
            let item = items
                .iter()
                .find(|item| item.name == name)
                .unwrap_or_else(|| panic!("{name} should be listed"));
            tab.thumbnail_is_displayed(item, preview)
        };

        assert!(
            displayed(&tab, "photo.png", false),
            "a row renders its own thumbnail"
        );
        assert!(
            !displayed(&tab, "notes.txt", false),
            "nothing renders a text preview with the pane closed"
        );
        assert!(
            !displayed(&tab, "notes.txt", true),
            "the pane renders the selected item, not every row"
        );

        for item in tab.items_opt.as_mut().expect("tab should be populated") {
            item.selected = item.name == "notes.txt";
        }
        assert!(displayed(&tab, "notes.txt", true));

        tab.gallery = true;
        assert!(
            displayed(&tab, "notes.txt", false),
            "the gallery renders a text preview of its own"
        );
        Ok(())
    }

    fn tree_len(tree: &Tree) -> usize {
        1 + tree.children.iter().map(tree_len).sum::<usize>()
    }

    fn list_view_tree(tab: &Tab) -> Tree {
        Tree::new(&tab.list_view().1)
    }

    #[test]
    fn list_view_widget_count_follows_viewport_not_directory_size() -> io::Result<()> {
        let (_small_dir, small) = tab_with_files(20)?;
        let (_large_dir, large) = tab_with_files(2000)?;

        let small_len = tree_len(&list_view_tree(&small));
        let large_len = tree_len(&list_view_tree(&large));

        assert!(
            large_len < small_len * 2,
            "a 2000 entry directory built {large_len} widgets against {small_len} for 20 entries"
        );
        Ok(())
    }

    #[test]
    fn grid_view_widget_count_follows_viewport_not_directory_size() -> io::Result<()> {
        let (_small_dir, mut small) = tab_with_files(20)?;
        let (_large_dir, mut large) = tab_with_files(2000)?;
        small.config.view = View::Grid;
        large.config.view = View::Grid;

        let small_len = tree_len(&Tree::new(&small.grid_view().1));
        let large_len = tree_len(&Tree::new(&large.grid_view().1));

        assert!(
            large_len < small_len * 2,
            "a 2000 entry directory built {large_len} widgets against {small_len} for 20 entries"
        );
        Ok(())
    }

    #[test]
    fn grid_view_content_height_covers_every_row() -> io::Result<()> {
        let (_dir, mut tab) = tab_with_files(500)?;
        tab.config.view = View::Grid;

        for scroll in [0.0, 500.0, 5000.0] {
            tab.scroll_opt = Some(AbsoluteOffset { x: 0.0, y: scroll });
            let _ = tab.grid_view();
            let content_height = tab
                .content_height_opt
                .get()
                .expect("grid_view should record the height of its content");
            let last_bottom = tab
                .items_opt()
                .expect("tab should be populated")
                .iter()
                .filter_map(|item| item.rect_opt.get())
                .map(|rect| rect.y + rect.height)
                .fold(0.0_f32, f32::max);
            assert_eq!(content_height, last_bottom.ceil(), "at scroll {scroll}");
        }
        Ok(())
    }

    #[test]
    fn list_view_content_height_covers_every_row() -> io::Result<()> {
        let (_dir, tab) = tab_with_files(500)?;
        let _ = tab.list_view();

        let content_height = tab
            .content_height_opt
            .get()
            .expect("list_view should record the height of its content");
        let items = tab.items_opt().expect("tab should be populated");
        let last_bottom = items
            .iter()
            .filter_map(|item| item.rect_opt.get())
            .map(|rect| rect.y + rect.height)
            .fold(0.0_f32, f32::max);

        assert_eq!(content_height, last_bottom);
        Ok(())
    }

    #[test]
    fn scrolled_row_keeps_its_button_id() -> io::Result<()> {
        let (_dir, mut tab) = tab_with_files(500)?;

        let mut tree = list_view_tree(&tab);
        tab.scroll_opt = Some(AbsoluteOffset { x: 0.0, y: 600.0 });
        let mut scrolled = tab.list_view().1;
        tree.diff(scrolled.as_widget_mut());

        let visible_rect = Rectangle::new(Point::new(0.0, 600.0), Size::new(800.0, 300.0));
        let on_screen: Vec<&Item> = tab
            .items_opt()
            .expect("tab should be populated")
            .iter()
            .filter(|item| {
                item.rect_opt
                    .get()
                    .is_some_and(|rect| rect.intersects(&visible_rect))
            })
            .collect();
        assert!(
            !on_screen.is_empty(),
            "scrolling should leave rows on screen"
        );

        for item in on_screen {
            assert!(
                tree.find(&item.button_id).is_some(),
                "{} lost its button id after scrolling",
                item.name
            );
        }
        Ok(())
    }

    /// Arrow-keying to a row the viewport culled names a button that does not exist yet;
    /// the scroll from select_focus_scroll has to bring it into the rebuilt tree.
    #[test]
    fn focusing_a_culled_row_scrolls_it_into_the_tree() -> io::Result<()> {
        let (_dir, mut tab) = tab_with_files(500)?;
        let mut tree = list_view_tree(&tab);

        tab.select_focus = Some(400);
        let focus_id = tab
            .select_focus_id()
            .expect("the focused item should have an id");
        assert!(
            tree.find(&focus_id).is_none(),
            "row 400 should start off screen with no widget"
        );

        tab.select_focus_scroll()
            .expect("focusing an off-screen row should move the scroll");
        let mut scrolled = tab.list_view().1;
        tree.diff(scrolled.as_widget_mut());

        assert!(
            tree.find(&focus_id).is_some(),
            "the focused row is missing from the scrolled tree"
        );
        Ok(())
    }

    #[test]
    fn scan_path_leaves_child_counts_for_the_visible_rows() -> io::Result<()> {
        let fs = simple_fs(NUM_FILES, NUM_NESTED, NUM_DIRS, NUM_NESTED, NAME_LEN)?;
        let items = scan_path(&fs.path().to_path_buf(), IconSizes::default());

        let dirs: Vec<&Item> = items.iter().filter(|item| item.metadata.is_dir()).collect();
        assert!(!dirs.is_empty(), "fixture should contain directories");
        for dir in dirs {
            assert_eq!(
                None,
                dir.metadata.children_count(),
                "{} was counted during the scan",
                dir.name
            );
        }
        Ok(())
    }

    #[test]
    fn size_ordering_reads_child_counts_for_off_screen_rows() -> io::Result<()> {
        let fs = simple_fs(NUM_FILES, NUM_NESTED, NUM_DIRS, NUM_NESTED, NAME_LEN)?;
        let mut tab = Tab::new(
            Location::Path(fs.path().into()),
            TabConfig::default(),
            ThumbCfg::default(),
            None,
            widget::Id::unique(),
            None,
        );
        tab.set_items(scan_path(&fs.path().to_path_buf(), IconSizes::default()));
        let dir_index = tab
            .items_opt()
            .expect("tab should be populated")
            .iter()
            .position(|item| item.metadata.is_dir())
            .expect("fixture should contain directories");

        assert!(
            !tab.fetches_children_count(&tab.items_opt().expect("populated")[dir_index], false),
            "a name-ordered listing reads counts for visible rows only"
        );

        tab.sort_name = HeadingOptions::Size;
        assert!(
            tab.fetches_children_count(&tab.items_opt().expect("populated")[dir_index], false),
            "ordering by size must read every directory's count, or unread counts sort as zero"
        );
        Ok(())
    }

    #[test]
    fn directory_children_message_fills_in_the_count() -> io::Result<()> {
        let fs = simple_fs(NUM_FILES, NUM_NESTED, NUM_DIRS, NUM_NESTED, NAME_LEN)?;
        let mut tab = Tab::new(
            Location::Path(fs.path().into()),
            TabConfig::default(),
            ThumbCfg::default(),
            None,
            widget::Id::unique(),
            None,
        );
        tab.set_items(scan_path(&fs.path().to_path_buf(), IconSizes::default()));

        let dir_path = tab
            .items_opt()
            .expect("tab should be populated")
            .iter()
            .find(|item| item.metadata.is_dir())
            .and_then(|item| item.path_opt().cloned())
            .expect("fixture should contain a directory");

        tab.update(
            Message::DirectoryChildren(dir_path.clone(), Some(7), entry_work_id(&tab, &dir_path)),
            Modifiers::empty(),
        );

        let counted = tab
            .items_opt()
            .expect("tab should be populated")
            .iter()
            .find(|item| item.path_opt() == Some(&dir_path))
            .and_then(|item| item.metadata.children_count().copied());
        assert_eq!(Some(7), counted);
        Ok(())
    }

    /// chmod fixes permissions without touching mtime or size, so an error carried across
    /// rescans on that fingerprint would never be retried however the directory changes.
    #[test]
    fn rescan_retries_a_failed_directory_size() -> io::Result<()> {
        let (dir, mut tab) = tab_with_files(2)?;
        let path = dir.path().join("file00000.txt");

        for item in tab.items_opt.as_mut().expect("tab should be populated") {
            if item.path_opt() == Some(&path) {
                item.dir_size = DirSize::Error("permission denied".to_string());
            }
        }
        tab.set_items(scan_path(&dir.path().to_path_buf(), IconSizes::default()));

        let item = tab
            .items_opt()
            .expect("tab should be populated")
            .iter()
            .find(|item| item.path_opt() == Some(&path))
            .expect("entry should survive the rescan");
        assert!(
            !matches!(item.dir_size, DirSize::Error(_)),
            "a failed size computation survived the rescan"
        );
        Ok(())
    }

    /// A rescan's (modified, size) fingerprint cannot detect writes within mtime granularity.
    #[test]
    fn a_same_size_rewrite_with_equal_mtime_keeps_the_old_thumbnail() -> io::Result<()> {
        let (dir, mut tab) = tab_with_files(2)?;
        let path = dir.path().join("file00000.txt");
        tab.update(
            Message::Thumbnail(
                path.clone(),
                ItemThumbnail::NotImage,
                entry_work_id(&tab, &path),
            ),
            Modifiers::empty(),
        );
        let modified = fs::metadata(&path)?.modified()?;

        fs::write(&path, b"y")?;
        fs::OpenOptions::new()
            .write(true)
            .open(&path)?
            .set_modified(modified)?;

        tab.set_items(scan_path(&dir.path().to_path_buf(), IconSizes::default()));

        let item = tab
            .items_opt()
            .expect("tab should be populated")
            .iter()
            .find(|item| item.path_opt() == Some(&path))
            .expect("entry should survive the rescan");
        assert!(
            item.thumbnail_opt.is_some(),
            "an equal fingerprint carries results across a rewrite; content is not consulted"
        );
        Ok(())
    }

    #[test]
    fn rescan_keeps_the_thumbnail_of_an_unchanged_entry() -> io::Result<()> {
        let (dir, mut tab) = tab_with_files(4)?;
        let path = dir.path().join("file00000.txt");

        tab.update(
            Message::Thumbnail(
                path.clone(),
                ItemThumbnail::NotImage,
                entry_work_id(&tab, &path),
            ),
            Modifiers::empty(),
        );
        tab.set_items(scan_path(&dir.path().to_path_buf(), IconSizes::default()));

        let thumbnailed = tab
            .items_opt()
            .expect("tab should be populated")
            .iter()
            .find(|item| item.path_opt() == Some(&path))
            .expect("entry should survive the rescan");
        assert!(
            thumbnailed.thumbnail_opt.is_some(),
            "rescan discarded a thumbnail for an unchanged entry"
        );
        Ok(())
    }

    #[test]
    fn rescan_drops_the_thumbnail_of_a_rewritten_entry() -> io::Result<()> {
        let (dir, mut tab) = tab_with_files(4)?;
        let path = dir.path().join("file00000.txt");

        tab.update(
            Message::Thumbnail(
                path.clone(),
                ItemThumbnail::NotImage,
                entry_work_id(&tab, &path),
            ),
            Modifiers::empty(),
        );
        fs::write(&path, b"rewritten with a different length")?;
        tab.set_items(scan_path(&dir.path().to_path_buf(), IconSizes::default()));

        let rewritten = tab
            .items_opt()
            .expect("tab should be populated")
            .iter()
            .find(|item| item.path_opt() == Some(&path))
            .expect("entry should survive the rescan");
        assert!(
            rewritten.thumbnail_opt.is_none(),
            "rescan kept a thumbnail for an entry whose content changed"
        );
        Ok(())
    }

    #[test]
    fn refresh_entries_updates_size_without_a_rescan() -> io::Result<()> {
        let (dir, mut tab) = tab_with_files(4)?;
        let path = dir.path().join("file00000.txt");
        fs::write(&path, b"grown considerably")?;

        assert!(tab.refresh_entries(vec![refreshed_entry(&tab, &path)]));

        let refreshed = tab
            .items_opt()
            .expect("tab should be populated")
            .iter()
            .find(|item| item.path_opt() == Some(&path))
            .expect("entry should still be listed");
        assert_eq!(Some(18), refreshed.metadata.file_size());
        Ok(())
    }

    #[test]
    fn refresh_entries_reports_an_entry_missing_from_the_listing() -> io::Result<()> {
        let (dir, mut tab) = tab_with_files(4)?;
        let unlisted = dir.path().join("appeared_since_the_scan.txt");
        fs::write(&unlisted, b"new")?;

        assert!(
            !tab.refresh_entries(vec![refreshed_entry(&tab, &unlisted)]),
            "an entry the listing does not hold should ask the caller for a rescan"
        );
        Ok(())
    }

    #[test]
    fn refresh_entries_reports_a_vanished_entry() -> io::Result<()> {
        let (dir, mut tab) = tab_with_files(4)?;
        let path = dir.path().join("file00000.txt");

        assert!(
            !tab.refresh_entries(vec![EntryRefresh {
                work_id: Some(entry_work_id(&tab, &path)),
                path,
                item_opt: None
            }]),
            "a vanished entry should ask the caller for a rescan"
        );
        Ok(())
    }

    #[test]
    fn a_trashed_file_reports_its_size() {
        let item = item_from_trash_entry(
            trash::TrashItem {
                id: std::ffi::OsString::from("/trash/files/report.pdf"),
                name: std::ffi::OsString::from("report.pdf"),
                original_parent: PathBuf::from("/home/user/documents"),
                time_deleted: 0,
            },
            trash::TrashItemMetadata {
                size: trash::TrashItemSize::Bytes(1234),
            },
            IconSizes::default(),
        );

        assert!(!item.metadata.is_dir());
        assert_eq!(item.metadata.file_size(), Some(1234));
    }

    /// Submit `typed` through the location bar and return what the off-thread resolver is
    /// handed: the location to resolve, and whether the text was a URI.
    fn submitted_location(tab: &mut Tab, typed: &str) -> (Location, bool) {
        tab.edit_location = Some(Location::Path(PathBuf::from(typed)).into());
        tab.update(Message::EditLocationSubmit, Modifiers::empty())
            .into_iter()
            .find_map(|command| match command {
                Command::EditLocationSubmit(edit_location, typed_uri) => {
                    Some((edit_location.location, typed_uri))
                }
                _ => None,
            })
            .expect("submitting the location bar should defer resolution off-thread")
    }

    #[test]
    fn a_typed_uri_submits_as_a_network_location() {
        let mut tab = tab_at(Location::Path(PathBuf::from("/tmp")));

        let (location, typed_uri) = submitted_location(&mut tab, "sftp://host/share");

        assert!(matches!(location, Location::Network(..)));
        assert!(
            typed_uri,
            "a URI has nothing on disk to resolve against, so the resolver has to be told to \
             navigate to it as entered"
        );
    }

    #[test]
    fn a_typed_path_submits_as_a_path() {
        let mut tab = tab_at(Location::Path(PathBuf::from("/tmp")));

        let (location, typed_uri) = submitted_location(&mut tab, "/tmp/reports");

        assert!(matches!(location, Location::Path(..)));
        assert!(!typed_uri);
    }

    fn entry_work_id(tab: &Tab, path: &Path) -> u64 {
        tab.items_opt()
            .unwrap()
            .iter()
            .find(|item| item.path_opt().is_some_and(|p| p == path))
            .unwrap()
            .work_id
    }

    fn refreshed_entry(tab: &Tab, path: &Path) -> EntryRefresh {
        EntryRefresh {
            path: path.to_path_buf(),
            work_id: tab
                .items_opt()
                .unwrap()
                .iter()
                .find(|item| item.path_opt().is_some_and(|p| p == path))
                .map(|item| item.work_id),
            item_opt: fs::metadata(path).ok().map(|metadata| {
                item_from_entry(
                    path.to_path_buf(),
                    path.file_name().unwrap().to_string_lossy().into_owned(),
                    metadata,
                    IconSizes::default(),
                )
            }),
        }
    }

    #[test]
    fn an_observed_write_invalidates_equal_fingerprint_results() -> io::Result<()> {
        let (dir, mut tab) = tab_with_files(1)?;
        let path = dir.path().join("file00000.txt");
        let modified = filetime::FileTime::from_last_modification_time(&fs::metadata(&path)?);
        let item = &mut tab.items_opt.as_mut().unwrap()[0];
        let fingerprint = item.fingerprint();
        item.thumbnail_opt = Some(ItemThumbnail::NotImage);
        item.checksums = ChecksumState::Calculated(FileChecksums {
            sha256: "old".into(),
        });
        item.selected = true;
        let button_id = item.button_id.clone();
        fs::write(&path, b"y")?;
        filetime::set_file_mtime(&path, modified)?;
        let refreshed = refreshed_entry(&tab, &path);
        assert_eq!(
            refreshed.item_opt.as_ref().unwrap().fingerprint(),
            fingerprint
        );
        assert!(tab.refresh_entries(vec![refreshed]));
        let item = &tab.items_opt().unwrap()[0];
        assert!(item.thumbnail_opt.is_none());
        assert!(matches!(item.checksums, ChecksumState::NotCalculated));
        assert!(item.selected);
        assert_eq!(item.button_id, button_id);
        Ok(())
    }

    #[test]
    fn a_content_refresh_discards_derived_results_before_and_after_rescan() -> io::Result<()> {
        let (dir, mut tab) = tab_with_files(1)?;
        let path = dir.path().join("file00000.txt");
        let item = &mut tab.items_opt.as_mut().unwrap()[0];
        item.thumbnail_opt = Some(ItemThumbnail::NotImage);
        item.checksums = ChecksumState::Calculated(FileChecksums {
            sha256: "old result".into(),
        });
        item.image_dimensions = Some((900, 800));
        item.selected = true;
        let button_id = item.button_id.clone();
        fs::write(&path, b"a longer document")?;
        assert!(tab.refresh_entries(vec![refreshed_entry(&tab, &path)]));
        let item = &tab.items_opt().unwrap()[0];
        assert!(item.thumbnail_opt.is_none());
        assert!(matches!(item.checksums, ChecksumState::NotCalculated));
        assert_eq!(item.image_dimensions, None);
        assert!(item.selected);
        assert_eq!(item.button_id, button_id);
        tab.set_items(scan_path(&dir.path().to_path_buf(), IconSizes::default()));
        assert!(tab.items_opt().unwrap()[0].thumbnail_opt.is_none());
        Ok(())
    }

    #[test]
    fn stale_entry_results_cannot_populate_a_replacement() -> io::Result<()> {
        let (dir, mut tab) = tab_with_files(1)?;
        let path = dir.path().join("file00000.txt");
        let old = entry_work_id(&tab, &path);
        tab.set_items(scan_path(&dir.path().to_path_buf(), IconSizes::default()));
        tab.update(
            Message::Thumbnail(path.clone(), ItemThumbnail::NotImage, old),
            Modifiers::empty(),
        );
        tab.update(
            Message::Checksums(path.clone(), ChecksumState::Error("stale".into()), old),
            Modifiers::empty(),
        );
        tab.update(
            Message::DirectorySize(path.clone(), DirSize::Directory(999), old),
            Modifiers::empty(),
        );
        tab.update(
            Message::DirectoryChildren(path.clone(), None, old),
            Modifiers::empty(),
        );
        let item = &tab.items_opt().unwrap()[0];
        assert!(item.thumbnail_opt.is_none());
        assert!(matches!(item.checksums, ChecksumState::NotCalculated));
        assert!(matches!(item.dir_size, DirSize::NotDirectory));
        assert!(!tab.children_count_failed.contains(&path));
        let current = item.work_id;
        tab.update(
            Message::Thumbnail(path, ItemThumbnail::NotImage, current),
            Modifiers::empty(),
        );
        assert!(tab.items_opt().unwrap()[0].thumbnail_opt.is_some());
        Ok(())
    }

    #[test]
    fn a_later_refresh_request_supersedes_earlier_results() -> io::Result<()> {
        let (dir, mut tab) = tab_with_files(1)?;
        let path = dir.path().join("file00000.txt");
        tab.refresh_requests(vec![path.clone()]);
        let old = refreshed_entry(&tab, &path);
        fs::write(&path, b"latest document")?;
        tab.refresh_requests(vec![path.clone()]);
        let current = refreshed_entry(&tab, &path);
        assert!(tab.refresh_entries(vec![old]));
        assert!(tab.refresh_entries(vec![current]));
        assert_eq!(tab.items_opt().unwrap()[0].metadata.file_size(), Some(15));
        Ok(())
    }

    #[test]
    fn rescan_recalculates_recursive_size_after_a_descendant_changes() -> io::Result<()> {
        let dir = tempfile::tempdir()?;
        let child = dir.path().join("child");
        fs::create_dir(&child)?;
        fs::write(child.join("file"), b"x")?;
        let mut tab = tab_at(Location::Path(dir.path().to_path_buf()));
        tab.set_items(scan_path(&dir.path().to_path_buf(), IconSizes::default()));
        let fingerprint = tab.items_opt().unwrap()[0].fingerprint();
        tab.items_opt.as_mut().unwrap()[0].dir_size = DirSize::Directory(1);
        fs::write(child.join("file"), b"a longer child")?;
        tab.set_items(scan_path(&dir.path().to_path_buf(), IconSizes::default()));
        let item = &tab.items_opt().unwrap()[0];
        assert_eq!(item.fingerprint(), fingerprint);
        assert!(matches!(item.dir_size, DirSize::Calculating(_)));
        Ok(())
    }

    #[cfg(not(feature = "wgpu"))]
    #[test]
    fn virtual_rows_match_actual_layout_at_each_scroll_position() -> io::Result<()> {
        use cosmic::iced::advanced::widget::{Operation, operation::Focusable};
        use cosmic::iced::advanced::{Layout, layout};
        #[derive(Default)]
        struct ButtonBounds(Vec<(widget::Id, Rectangle)>);
        impl Operation for ButtonBounds {
            fn traverse(&mut self, visit: &mut dyn FnMut(&mut dyn Operation)) {
                visit(self);
            }
            fn focusable(
                &mut self,
                id: Option<&widget::Id>,
                bounds: Rectangle,
                _: &mut dyn Focusable,
            ) {
                if let Some(id) = id {
                    self.0.push((id.clone(), bounds));
                }
            }
        }
        let renderer =
            cosmic::Renderer::new(cosmic::iced::Font::default(), cosmic::iced::Pixels(14.0));
        let (_dir, mut tab) = tab_with_files(500)?;
        tab.config.show_hidden = false;
        tab.items_opt.as_mut().unwrap()[2].hidden = true;
        for view in [View::List, View::Grid] {
            tab.config.view = view;
            for width in [800.0, 400.0] {
                tab.size_opt.set(Some(Size::new(width, 300.0)));
                match view {
                    View::List => {
                        let _ = tab.list_view();
                    }
                    View::Grid => {
                        let _ = tab.grid_view();
                    }
                }
                let last_scroll = (tab.content_height_opt.get().unwrap() - 300.0).max(0.0);
                for scroll in [0.0, 500.0, last_scroll] {
                    tab.scroll_opt = Some(AbsoluteOffset { x: 0.0, y: scroll });
                    let mut element = match view {
                        View::List => tab.list_view().1,
                        View::Grid => tab.grid_view().1,
                    };
                    let mut tree = Tree::new(&element);
                    let node = element.as_widget_mut().layout(
                        &mut tree,
                        &renderer,
                        &layout::Limits::new(Size::ZERO, Size::new(width, f32::INFINITY)),
                    );
                    let mut bounds = ButtonBounds::default();
                    element.as_widget_mut().operate(
                        &mut tree,
                        Layout::new(&node),
                        &renderer,
                        &mut bounds,
                    );
                    let viewport = Rectangle::new(Point::new(0.0, scroll), Size::new(width, 300.0));
                    let visible: Vec<_> = tab
                        .items_opt()
                        .unwrap()
                        .iter()
                        .filter(|item| {
                            item.rect_opt
                                .get()
                                .is_some_and(|rect| rect.intersects(&viewport))
                        })
                        .collect();
                    assert!(!visible.is_empty());
                    for item in visible {
                        let bounds = bounds
                            .0
                            .iter()
                            .find(|(id, _)| *id == item.button_id)
                            .map(|(_, bounds)| *bounds)
                            .unwrap_or_else(|| {
                                panic!("{} has no button in the laid-out viewport", item.name)
                            });
                        let expected = item.rect_opt.get().unwrap();
                        match view {
                            View::List => {
                                assert_eq!(bounds.y, expected.y, "{} at scroll {scroll}", item.name)
                            }
                            View::Grid => assert!(
                                bounds.y >= expected.y
                                    && bounds.y + bounds.height <= expected.y + expected.height,
                                "{}: {bounds:?} outside {expected:?}",
                                item.name
                            ),
                        }
                    }
                    let height = tab.content_height_opt.get().unwrap();
                    assert_eq!(node.size().height, height, "{view:?} at scroll {scroll}");
                }
            }
        }
        Ok(())
    }

    fn subscription_ids(subscription: Subscription<Message>) -> Vec<u64> {
        use cosmic::iced::advanced::subscription::{Hasher, into_recipes};
        use std::hash::Hasher as _;
        into_recipes(subscription)
            .into_iter()
            .map(|recipe| {
                let mut hash = Hasher::default();
                recipe.hash(&mut hash);
                hash.finish()
            })
            .collect()
    }

    #[test]
    fn rescans_restart_child_counts_and_directory_size_work() -> io::Result<()> {
        let dir = tempfile::tempdir()?;
        fs::create_dir(dir.path().join("child"))?;
        let mut tab = tab_at(Location::Path(dir.path().to_path_buf()));
        tab.set_items(scan_path(&dir.path().to_path_buf(), IconSizes::default()));
        tab.size_opt.set(Some(Size::new(800.0, 300.0)));
        tab.items_opt.as_mut().unwrap()[0].selected = true;
        let controller = match &tab.items_opt().unwrap()[0].dir_size {
            DirSize::Calculating(controller) => controller.clone(),
            _ => panic!("directory should need a size calculation"),
        };
        let _ = tab.list_view();
        let old = subscription_ids(tab.subscription(true));
        tab.set_items(scan_path(&dir.path().to_path_buf(), IconSizes::default()));
        let _ = tab.list_view();
        let current = subscription_ids(tab.subscription(true));
        assert!(controller.is_cancelled());
        assert_eq!(old.len(), 3);
        assert_eq!(current.len(), old.len());
        assert!(old.iter().all(|identity| !current.contains(identity)));
        match &tab.items_opt().unwrap()[0].dir_size {
            DirSize::Calculating(controller) => assert!(!controller.is_cancelled()),
            _ => panic!("replacement should need a size calculation"),
        }
        Ok(())
    }

    #[test]
    fn only_current_child_counts_and_sizes_are_applied() -> io::Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("child");
        fs::create_dir(&path)?;
        let mut tab = tab_at(Location::Path(dir.path().to_path_buf()));
        tab.set_items(scan_path(&dir.path().to_path_buf(), IconSizes::default()));
        let old = entry_work_id(&tab, &path);
        tab.set_items(scan_path(&dir.path().to_path_buf(), IconSizes::default()));
        let current = entry_work_id(&tab, &path);
        tab.update(
            Message::DirectoryChildren(path.clone(), Some(77), old),
            Modifiers::empty(),
        );
        tab.update(
            Message::DirectorySize(path.clone(), DirSize::Error("cancelled".into()), old),
            Modifiers::empty(),
        );
        let item = &tab.items_opt().unwrap()[0];
        assert_eq!(item.metadata.children_count(), None);
        assert!(matches!(item.dir_size, DirSize::Calculating(_)));
        tab.update(
            Message::DirectoryChildren(path.clone(), Some(4), current),
            Modifiers::empty(),
        );
        tab.update(
            Message::DirectorySize(path, DirSize::Directory(100), current),
            Modifiers::empty(),
        );
        let item = &tab.items_opt().unwrap()[0];
        assert_eq!(item.metadata.children_count(), Some(&4));
        assert!(matches!(item.dir_size, DirSize::Directory(100)));
        Ok(())
    }

    #[test]
    fn a_current_checksum_result_is_accepted() -> io::Result<()> {
        let (dir, mut tab) = tab_with_files(1)?;
        let path = dir.path().join("file00000.txt");
        let current = entry_work_id(&tab, &path);
        tab.update(
            Message::Checksums(
                path,
                ChecksumState::Calculated(FileChecksums {
                    sha256: "fresh".into(),
                }),
                current,
            ),
            Modifiers::empty(),
        );
        match &tab.items_opt().unwrap()[0].checksums {
            ChecksumState::Calculated(checksums) => assert_eq!(checksums.sha256, "fresh"),
            _ => panic!("current checksum result should be stored"),
        }
        Ok(())
    }

    #[test]
    fn external_thumbnail_without_image_dimensions_does_not_start_a_decode() -> io::Result<()> {
        let (_dir, mut tab) = tab_with_files(1)?;
        tab.gallery = true;
        tab.select_focus = Some(0);
        tab.items_opt.as_mut().unwrap()[0].thumbnail_opt = Some(ItemThumbnail::Image(
            widget::image::Handle::from_rgba(1, 1, vec![0; 4]),
            None,
        ));
        assert!(tab.trigger_async_decode().is_empty());
        Ok(())
    }
}
