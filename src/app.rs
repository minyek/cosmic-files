// Copyright 2023 System76 <info@system76.com>
// SPDX-License-Identifier: GPL-3.0-only

use cosmic::app::{self, Core, Task, context_drawer};
use cosmic::core::Auto;
use cosmic::cosmic_config::{self, ConfigSet};
use cosmic::iced::clipboard::dnd::DndAction;
use cosmic::iced::core::SmolStr;
use cosmic::iced::core::widget::operation::focusable::unfocus;
use cosmic::iced::futures::{self, SinkExt};
use cosmic::iced::keyboard::key::Physical;
use cosmic::iced::keyboard::{Event as KeyEvent, Key, Modifiers};
#[cfg(all(feature = "wayland", feature = "desktop-applet"))]
use cosmic::iced::platform_specific::shell::wayland::commands::overlap_notify::overlap_notify;
use cosmic::iced::runtime::{clipboard, task};
use cosmic::iced::widget::button::focus;
use cosmic::iced::widget::scrollable;
use cosmic::iced::widget::scrollable::AbsoluteOffset;
use cosmic::iced::window::{self, Event as WindowEvent, Id as WindowId};
use cosmic::iced::{self, Alignment, Event, Length, Size, Subscription, event, mouse, stream};
#[cfg(all(feature = "wayland", feature = "desktop-applet"))]
use cosmic::iced::{
    Limits, Point, Rectangle,
    event::wayland::{Event as WaylandEvent, OutputEvent, OverlapNotifyEvent},
    platform_specific::runtime::wayland::layer_surface::{
        IcedMargin, IcedOutput, SctkLayerSurfaceSettings,
    },
    platform_specific::shell::wayland::commands::layer_surface::{
        Anchor, KeyboardInteractivity, Layer, destroy_layer_surface,
    },
};
use cosmic::widget::about::About;
use cosmic::widget::dnd_destination::DragId;
use cosmic::widget::menu::action::MenuAction;
use cosmic::widget::menu::key_bind::KeyBind;
use cosmic::widget::segmented_button::{self, Entity, ReorderEvent};
use cosmic::widget::{self, icon, settings, space};
use cosmic::{Application, ApplicationExt, Element, cosmic_theme, executor, surface, theme};
#[cfg(feature = "desktop")]
use i18n_embed::LanguageLoader;
use mime_guess::Mime;
use notify_debouncer_full::notify::event::{EventKind, ModifyKind};
use notify_debouncer_full::notify::{self, RecommendedWatcher};
use notify_debouncer_full::{DebouncedEvent, Debouncer, RecommendedCache, new_debouncer};
use rustc_hash::{FxHashMap, FxHashSet};
use slotmap::Key as SlotMapKey;
use std::any::TypeId;
use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::future::Future;
use std::num::NonZeroU16;
use std::path::{Path, PathBuf};
use std::pin::Pin;
#[cfg(feature = "notify")]
use std::sync::Mutex;
use std::sync::{Arc, LazyLock};
use std::time::{self, Duration, Instant};
use std::{env, fmt, fs, io, process};
use tokio::sync::mpsc;
use trash::TrashItem;
#[cfg(all(feature = "wayland", feature = "desktop-applet"))]
use wayland_client::{Proxy, protocol::wl_output::WlOutput};

use crate::clipboard::{
    ClipboardCache, ClipboardCopy, ClipboardKind, ClipboardPaste, ClipboardPasteImage,
    ClipboardPasteText, ClipboardPasteVideo,
};
use crate::config::{
    AppTheme, Config, DesktopConfig, Favorite, IconSizes, State, TIME_CONFIG_ID, TabConfig,
    TimeConfig, TypeToSearch,
};
use crate::dialog::{Dialog, DialogKind, DialogMessage, DialogResult, DialogSettings};
use crate::key_bind::key_binds;
use crate::localize::LANGUAGE_SORTER;
use crate::mime_app::{self, MimeApp, MimeAppCache, MimeAppMatch};
use crate::mounter::{
    MOUNTERS, MounterAuth, MounterItem, MounterItems, MounterKey, MounterMessage,
};
use crate::operation::{
    Controller, Operation, OperationError, OperationErrorType, OperationSelection, ReplaceResult,
};
use crate::spawn_detached::spawn_detached;
use crate::tab::{
    self, HOVER_DURATION, HeadingOptions, ItemMetadata, Location, SORT_OPTION_FALLBACK,
    SearchLocation, Tab,
};
use crate::trash::{Trash, TrashExt};
use crate::zoom::{zoom_in_view, zoom_out_view, zoom_to_default};
use crate::{FxOrderMap, context_action, fl, home_dir, menu, mime_icon};

static PERMANENT_DELETE_BUTTON_ID: LazyLock<widget::Id> =
    LazyLock::new(|| widget::Id::new("permanent-delete-button"));

static DELETE_TRASH_BUTTON_ID: LazyLock<widget::Id> =
    LazyLock::new(|| widget::Id::new("delete-trash-button"));

static CONFIRM_OPEN_WITH_BUTTON_ID: LazyLock<widget::Id> =
    LazyLock::new(|| widget::Id::new("confirm-open-with-button"));

static CONFIRM_CONTEXT_ACTION_BUTTON_ID: LazyLock<widget::Id> =
    LazyLock::new(|| widget::Id::new("confirm-context-action-button"));

static EMPTY_TRASH_BUTTON_ID: LazyLock<widget::Id> =
    LazyLock::new(|| widget::Id::new("empty-trash-button"));

static SET_EXECUTABLE_AND_LAUNCH_CONFIRM_BUTTON_ID: LazyLock<widget::Id> =
    LazyLock::new(|| widget::Id::new("set-executable-and-launch-confirm-button"));

static FAVORITE_PATH_ERROR_REMOVE_BUTTON_ID: LazyLock<widget::Id> =
    LazyLock::new(|| widget::Id::new("favorite-path-error-remove-button"));

static MOUNT_ERROR_TRY_AGAIN_BUTTON_ID: LazyLock<widget::Id> =
    LazyLock::new(|| widget::Id::new("mount-error-try-again-button"));

pub(crate) static REPLACE_BUTTON_ID: LazyLock<widget::Id> =
    LazyLock::new(|| widget::Id::new("replace-button"));

/// Most recent completed/failed operations to retain for the history view.
const MAX_OPERATION_HISTORY: usize = 256;

/// Drop the oldest entries once an operation-history map exceeds [`MAX_OPERATION_HISTORY`]. The
/// maps are keyed by an ascending operation id, so the smallest keys are the oldest.
fn cap_operation_history<V>(history: &mut BTreeMap<u64, V>) {
    while history.len() > MAX_OPERATION_HISTORY {
        history.pop_first();
    }
}

/// The retry an ExtractPassword dialog resolves to, or `None` when the failed operation
/// cannot be retried: evicted from the capped history, or not an extract.
fn retry_extract_with_password<C, E>(
    failed_operations: &BTreeMap<u64, (Operation, C, E)>,
    id: u64,
    password: String,
) -> Option<Operation> {
    let (Operation::Extract { to, paths, .. }, _, _) = failed_operations.get(&id)? else {
        return None;
    };
    Some(Operation::Extract {
        to: to.clone(),
        paths: paths.clone(),
        password: Some(password),
    })
}

/// The text a failed operation shows and copies, or `None` when it has left the
/// failed-operations history.
fn failed_operation_text<C, E: fmt::Display>(
    failed_operations: &BTreeMap<u64, (Operation, C, E)>,
    id: u64,
) -> Option<String> {
    let (operation, _, err) = failed_operations.get(&id)?;
    //TODO: nice description of error
    Some(format!("{operation:#?}\n{err}"))
}

/// The same text for several failed operations, skipping any that have left the history.
fn failed_operations_text<C, E: fmt::Display>(
    failed_operations: &BTreeMap<u64, (Operation, C, E)>,
    ids: &[u64],
) -> String {
    ids.iter()
        .filter_map(|id| failed_operation_text(failed_operations, *id))
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// What a batch of watcher events did to a tab's listing.
enum ListingChange {
    /// Entries were added, removed or renamed: the listing has to be read again.
    Structural,
    /// Listed entries were written to: only their own metadata is out of date.
    Entries(Vec<PathBuf>),
    /// Nothing the tab displays changed.
    None,
}

/// Work out what a debounced batch of watcher events means for the tab listing `dir`.
fn classify_events(events: &[DebouncedEvent], dir: &Path) -> ListingChange {
    let mut written = Vec::new();
    for event in events {
        let touches_dir = event.paths.iter().any(|path| path.starts_with(dir));
        if !touches_dir {
            continue;
        }
        match event.kind {
            // Opening or reading a file leaves the listing alone.
            EventKind::Access(_) => {}
            EventKind::Modify(ModifyKind::Data(_) | ModifyKind::Metadata(_)) => {
                if event.paths.iter().any(|path| {
                    path.parent() == Some(dir)
                        && path.file_name().is_some_and(|name| name == ".hidden")
                }) {
                    return ListingChange::Structural;
                }
                written.extend(
                    event
                        .paths
                        .iter()
                        .filter(|path| path.parent() == Some(dir))
                        .cloned(),
                );
            }
            _ => return ListingChange::Structural,
        }
    }
    if written.is_empty() {
        ListingChange::None
    } else {
        ListingChange::Entries(written)
    }
}

#[derive(Clone, Debug)]
pub enum Mode {
    App,
    Desktop,
}

#[derive(Clone, Debug)]
pub struct Flags {
    pub config_handler: Option<cosmic_config::Config>,
    pub config: Config,
    pub state_handler: Option<cosmic_config::Config>,
    pub state: State,
    pub mode: Mode,
    pub locations: Vec<Location>,
    pub uris: Vec<url::Url>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Action {
    About,
    AddToSidebar,
    Compress,
    Copy,
    CopyPath,
    CopyTo,
    Cut,
    CosmicSettingsDesktop,
    CosmicSettingsDisplays,
    CosmicSettingsWallpaper,
    DesktopViewOptions,
    Delete,
    EditHistory,
    EditLocation,
    Eject,
    EmptyTrash,
    #[cfg(feature = "desktop")]
    ExecEntryAction(u64, usize),
    ExtractHere,
    ExtractTo,
    Gallery,
    HistoryNext,
    HistoryPrevious,
    ItemDown,
    ItemLeft,
    ItemPageDown,
    ItemPageUp,
    ItemRight,
    ItemUp,
    LocationUp,
    MoveTo,
    NewFile,
    NewFolder,
    Open,
    OpenInNewTab,
    OpenInNewWindow,
    OpenItemLocation,
    OpenTerminal,
    OpenWith,
    RunContextAction(usize),
    Paste,
    PermanentlyDelete,
    Preview,
    Reload,
    RemoveFromRecents,
    Rename,
    RestoreFromTrash,
    SearchActivate,
    SelectFirst,
    SelectLast,
    SelectAll,
    SetSort(HeadingOptions, bool),
    Settings,
    TabClose,
    TabNew,
    TabNext,
    TabPrev,
    TabViewGrid,
    TabViewList,
    ToggleFoldersFirst,
    ToggleShowHidden,
    ToggleSort(HeadingOptions),
    WindowClose,
    WindowNew,
    ZoomDefault,
    ZoomIn,
    ZoomOut,
    Recents,
}

impl Action {
    const fn message(&self, entity_opt: Option<Entity>) -> Message {
        match self {
            Self::About => Message::ToggleContextPage(ContextPage::About),
            Self::AddToSidebar => Message::AddToSidebar(entity_opt),
            Self::Compress => Message::Compress(entity_opt),
            Self::Copy => Message::Copy(entity_opt),
            Self::CopyPath => Message::CopyPath(entity_opt),
            Self::CopyTo => Message::CopyTo(entity_opt),
            Self::Cut => Message::Cut(entity_opt),
            Self::CosmicSettingsDesktop => Message::CosmicSettings("desktop"),
            Self::CosmicSettingsDisplays => Message::CosmicSettings("displays"),
            Self::CosmicSettingsWallpaper => Message::CosmicSettings("wallpaper"),
            Self::Delete => Message::Delete(entity_opt),
            Self::DesktopViewOptions => Message::DesktopViewOptions,
            Self::EditHistory => Message::ToggleContextPage(ContextPage::EditHistory),
            Self::EditLocation => Message::TabMessage(entity_opt, tab::Message::EditLocationEnable),
            Self::Eject => Message::Eject,
            Self::EmptyTrash => Message::TabMessage(None, tab::Message::EmptyTrash),
            Self::ExtractHere => Message::ExtractHere(entity_opt),
            Self::ExtractTo => Message::ExtractTo(entity_opt),
            #[cfg(feature = "desktop")]
            Self::ExecEntryAction(snapshot, action) => Message::TabMessage(
                entity_opt,
                tab::Message::ExecEntryAction(*snapshot, *action),
            ),
            Self::Gallery => Message::TabMessage(entity_opt, tab::Message::GalleryToggle),
            Self::HistoryNext => Message::TabMessage(entity_opt, tab::Message::GoNext),
            Self::HistoryPrevious => Message::TabMessage(entity_opt, tab::Message::GoPrevious),
            Self::ItemDown => Message::TabMessage(entity_opt, tab::Message::ItemDown),
            Self::ItemLeft => Message::TabMessage(entity_opt, tab::Message::ItemLeft),
            Self::ItemPageDown => Message::TabMessage(entity_opt, tab::Message::ItemPageDown),
            Self::ItemPageUp => Message::TabMessage(entity_opt, tab::Message::ItemPageUp),
            Self::ItemRight => Message::TabMessage(entity_opt, tab::Message::ItemRight),
            Self::ItemUp => Message::TabMessage(entity_opt, tab::Message::ItemUp),
            Self::LocationUp => Message::TabMessage(entity_opt, tab::Message::LocationUp),
            Self::MoveTo => Message::MoveTo(entity_opt),
            Self::NewFile => Message::NewItem(entity_opt, false),
            Self::NewFolder => Message::NewItem(entity_opt, true),
            Self::Open => Message::TabMessage(entity_opt, tab::Message::Open(None)),
            Self::OpenInNewTab => Message::OpenInNewTab(entity_opt),
            Self::OpenInNewWindow => Message::OpenInNewWindow(entity_opt),
            Self::OpenItemLocation => Message::OpenItemLocation(entity_opt),
            Self::OpenTerminal => Message::OpenTerminal(entity_opt),
            Self::OpenWith => Message::OpenWithDialog(entity_opt),
            Self::RunContextAction(action) => {
                Message::TabMessage(entity_opt, tab::Message::RunContextAction(*action))
            }
            Self::Paste => Message::Paste(entity_opt),
            Self::PermanentlyDelete => Message::PermanentlyDelete(entity_opt),
            Self::Preview => Message::Preview(entity_opt),
            Self::Reload => Message::TabMessage(entity_opt, tab::Message::Reload),
            Self::RemoveFromRecents => Message::RemoveFromRecents(entity_opt),
            Self::Rename => Message::Rename(entity_opt),
            Self::RestoreFromTrash => Message::RestoreFromTrash(entity_opt),
            Self::SearchActivate => Message::SearchActivate,
            Self::SelectAll => Message::TabMessage(entity_opt, tab::Message::SelectAll),
            Self::SelectFirst => Message::TabMessage(entity_opt, tab::Message::SelectFirst),
            Self::SelectLast => Message::TabMessage(entity_opt, tab::Message::SelectLast),
            Self::SetSort(sort, dir) => {
                Message::TabMessage(entity_opt, tab::Message::SetSort(*sort, *dir))
            }
            Self::Settings => Message::ToggleContextPage(ContextPage::Settings),
            Self::TabClose => Message::TabClose(entity_opt),
            Self::TabNew => Message::TabNew,
            Self::TabNext => Message::TabNext,
            Self::TabPrev => Message::TabPrev,
            Self::TabViewGrid => Message::TabView(entity_opt, tab::View::Grid),
            Self::TabViewList => Message::TabView(entity_opt, tab::View::List),
            Self::ToggleFoldersFirst => Message::ToggleFoldersFirst,
            Self::ToggleShowHidden => Message::ToggleShowHidden,
            Self::ToggleSort(sort) => {
                Message::TabMessage(entity_opt, tab::Message::ToggleSort(*sort))
            }
            Self::WindowClose => Message::WindowClose,
            Self::WindowNew => Message::WindowNew,
            Self::ZoomDefault => Message::ZoomDefault(entity_opt),
            Self::ZoomIn => Message::ZoomIn(entity_opt),
            Self::ZoomOut => Message::ZoomOut(entity_opt),
            Self::Recents => Message::Recents,
        }
    }
}

impl MenuAction for Action {
    type Message = Message;

    fn message(&self) -> Message {
        self.message(None)
    }
}

#[derive(Clone, Debug)]
pub struct PreviewItem(pub Box<tab::Item>);

impl PartialEq for PreviewItem {
    fn eq(&self, other: &Self) -> bool {
        self.0.location_opt == other.0.location_opt
    }
}

impl Eq for PreviewItem {}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PreviewKind {
    Custom(PreviewItem),
    Location(Location),
    Selected,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum NavMenuAction {
    ClearRecents,
    EmptyTrash,
    Open(segmented_button::Entity),
    OpenWith(segmented_button::Entity),
    OpenInNewTab(segmented_button::Entity),
    OpenInNewWindow(segmented_button::Entity),
    Preview(segmented_button::Entity),
    RunContextAction(segmented_button::Entity, usize),
    RemoveFromSidebar(segmented_button::Entity),
    ChangeSidebarLabel(segmented_button::Entity),
}

impl MenuAction for NavMenuAction {
    type Message = cosmic::Action<Message>;

    fn message(&self) -> Self::Message {
        cosmic::Action::App(Message::NavMenuAction(*self))
    }
}

/// How a set of paths is to be opened, decided by sniffing their MIME types.
#[derive(Clone, Debug)]
pub enum OpenTargets {
    /// Every path is a supported archive, in the order they were given, for the extract dialog.
    Archives(Vec<PathBuf>),
    /// Paths grouped by MIME type, so one app can be passed a whole group at once.
    ByMime(FxHashMap<Mime, Vec<PathBuf>>),
}

impl OpenTargets {
    /// Sorts `paths` into what can open them, given the `mimes` sniffed for them in the same
    /// order. Grouping loses the caller's order, so the all-archive case keeps `paths` whole:
    /// the extract dialog presents them in the order they were selected.
    fn classify(paths: Vec<PathBuf>, mimes: Vec<Mime>) -> Self {
        let supported_archive_types = crate::archive::SUPPORTED_ARCHIVE_TYPES;
        if mimes
            .iter()
            .all(|mime| supported_archive_types.iter().copied().any(|t| *mime == t))
        {
            return Self::Archives(paths);
        }
        let mut groups: FxHashMap<Mime, Vec<PathBuf>> = FxHashMap::default();
        for (mime, path) in mimes.into_iter().zip(paths) {
            groups.entry(mime).or_default().push(path);
        }
        Self::ByMime(groups)
    }
}

fn write_unique_file(to: &Path, name: &str, data: &[u8]) -> io::Result<PathBuf> {
    use std::io::Write;

    let name_path = Path::new(name);
    let stem = name_path.file_stem().unwrap_or_default().to_string_lossy();
    let extension = name_path.extension().map(|value| value.to_string_lossy());
    for index in 0u64.. {
        let candidate = if index == 0 {
            name.to_string()
        } else {
            let copied = format!("{stem} ({} {index})", fl!("copy_noun"));
            extension.as_ref().map_or_else(
                || copied.clone(),
                |extension| format!("{copied}.{extension}"),
            )
        };
        let path = to.join(candidate);
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(mut file) => {
                file.write_all(data)?;
                return Ok(path);
            }
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists => {}
            Err(err) => return Err(err),
        }
    }
    Err(io::Error::other("failed to allocate a pasted file name"))
}

/// Messages that are used specifically by our [`App`].
#[derive(Clone, Debug)]
pub enum Message {
    AddToSidebar(Option<Entity>),
    AppTheme(AppTheme),
    CloseToast(widget::ToastId),
    Compress(Option<Entity>),
    Config(Config),
    Copy(Option<Entity>),
    CopyPath(Option<Entity>),
    CopyText(String),
    CopyTo(Option<Entity>),
    CopyToResult(DialogResult),
    CosmicSettings(&'static str),
    Cut(Option<Entity>),
    Delete(Option<Entity>),
    DeleteClassified {
        trash_paths: Vec<PathBuf>,
        dialog_paths: Vec<PathBuf>,
    },
    DesktopConfig(DesktopConfig),
    DesktopViewOptions,
    DesktopDialogs(bool),
    DialogCancel,
    DialogComplete,
    Eject,
    FileDialogMessage(DialogMessage),
    DialogPush(DialogPage, Option<widget::Id>),
    DialogUpdate(DialogPage),
    DialogUpdateComplete(DialogPage),
    ExtractHere(Option<Entity>),
    ExtractTo(Option<Entity>),
    ExtractToResult(DialogResult),
    #[cfg(all(feature = "wayland", feature = "desktop-applet"))]
    Focused(window::Id),
    Key(window::Id, Modifiers, Key, Physical, Option<SmolStr>),
    LaunchUrl(String),
    MaybeExit,
    PersistenceFlushed,
    MimeAppCacheLoaded(widget::Id, MimeAppCache),
    ModifiersChanged(window::Id, Modifiers),
    MounterItems(MounterKey, MounterItems),
    MountResult(MounterKey, MounterItem, Result<bool, String>),
    Mouse(window::Id, mouse::Button),
    MoveTo(Option<Entity>),
    MoveToResult(DialogResult),
    NavBarClose(Entity),
    NavBarContext(Entity),
    NavContextInfoLoaded(Entity, widget::Id, NavContextInfo),
    NavMenuAction(NavMenuAction),
    NavMenuOpenWith(PathBuf, Box<tab::Item>),
    NavSelectChecked {
        entity: Entity,
        tab_entity: Entity,
        request: widget::Id,
        exists: bool,
        is_dir: bool,
    },
    NavigationResolved {
        entity: Entity,
        request: widget::Id,
        target: Option<(Location, Option<PathBuf>)>,
    },
    NetworkAuth(MounterKey, String, MounterAuth, mpsc::Sender<MounterAuth>),
    NetworkDriveInput(String),
    NetworkDriveOpenEntityAfterMount {
        entity: Entity,
    },
    NavigationMounted {
        entity: Entity,
        tab_entity: Entity,
        request: widget::Id,
    },
    NetworkDriveOpenTabAfterMount {
        location: Location,
    },
    NetworkDriveSubmit,
    NetworkResult(MounterKey, String, Result<bool, String>),
    NewItem(Option<Entity>, bool),
    #[cfg(feature = "notify")]
    Notification(Arc<Mutex<notify_rust::NotificationHandle>>),
    NotifyEvents(Vec<DebouncedEvent>),
    NotifyWatcher(WatcherWrapper),
    OpenTerminal(Option<Entity>),
    OpenFileClassified(OpenTargets),
    OpenLaunchLocations(Vec<(Location, Option<Vec<PathBuf>>)>),
    OpenInNewTab(Option<Entity>),
    OpenInNewWindow(Option<Entity>),
    OpenItemLocation(Option<Entity>),
    OpenWithBrowse,
    OpenWithDialog(Option<Entity>),
    OpenWithSelection(usize),
    OpenWithSearchClear,
    #[cfg(all(feature = "wayland", feature = "desktop-applet"))]
    Overlap(window::Id, OverlapNotifyEvent),
    Paste(Option<Entity>),
    PasteContents(PathBuf, ClipboardPaste),
    PasteImage(PathBuf),
    PasteImageContents(PathBuf, ClipboardPasteImage),
    PasteText(PathBuf),
    PasteTextContents(PathBuf, ClipboardPasteText),
    PasteVideo(PathBuf),
    PasteVideoContents(PathBuf, ClipboardPasteVideo),
    CheckClipboard,
    CheckClipboardImage,
    CheckClipboardVideo,
    CheckClipboardText,
    RetryCheckClipboard(ClipboardCache),
    ClipboardCached(ClipboardCache),
    PendingCancel(u64),
    PendingCancelAll,
    PendingComplete(u64, OperationSelection),
    PendingDismiss,
    PendingError(u64, OperationError),
    PendingResults(Vec<(u64, OperationSelection)>, Vec<(u64, OperationError)>),
    PendingPause(u64, bool),
    PendingPauseAll(bool),
    PermanentlyDelete(Option<Entity>),
    Preview(Option<Entity>),
    PreviewReady(widget::Id, Option<Entity>, Box<tab::Item>),
    ReloadMimeAppCache,
    ReorderTab(ReorderEvent),
    RescanRecents,
    RescanTrash,
    RemoveFromRecents(Option<Entity>),
    Rename(Option<Entity>),
    ReplaceResult(ReplaceResult),
    RestoreFromTrash(Option<Entity>),
    SaveSortNames,
    ScrollTab(i16),
    SearchActivate,
    SearchClear,
    SearchInput(String),
    SetShowDetails(bool),
    SetShowRecents(bool),
    SetTypeToSearch(TypeToSearch),
    SystemThemeModeChange,
    Size(window::Id, Size),
    TabActivate(Entity),
    TabNext,
    TabPrev,
    TabClose(Option<Entity>),
    TabConfig(TabConfig),
    TabMessage(Option<Entity>, tab::Message),
    TabNew,
    TabRefreshEntries(Entity, Location, Vec<tab::EntryRefresh>),
    TabRescan(
        Entity,
        u64,
        Location,
        Option<Box<tab::Item>>,
        Vec<tab::Item>,
        Option<Vec<PathBuf>>,
    ),
    TabRescanFailed(Entity, u64),
    TabView(Option<Entity>, tab::View),
    TimeConfigChange(TimeConfig),
    ToggleContextPage(ContextPage),
    ToggleFoldersFirst,
    ToggleShowHidden,
    TrashStateRefreshed(widget::Id, bool),
    Undo(usize),
    UndoTrash(widget::ToastId, Arc<[PathBuf]>),
    UndoTrashStart(Vec<TrashItem>),
    WatcherUpdated(WatcherWrapper, FxHashSet<PathBuf>, FxHashSet<PathBuf>),
    WindowClose,
    WindowCloseRequested(window::Id),
    WindowMaximize(window::Id, bool),
    WindowNew,
    ZoomDefault(Option<Entity>),
    ZoomIn(Option<Entity>),
    ZoomOut(Option<Entity>),
    DndHoverLocTimeout(Location),
    DndHoverTabTimeout(Entity),
    DndEnterNav(Entity),
    DndExitNav,
    DndEnterTab(Entity, Vec<String>),
    DndExitTab,
    DndDropTab(Entity, Option<ClipboardPaste>, DndAction),
    DndDropNav(Entity, Option<ClipboardPaste>, DndAction),
    Recents,
    #[cfg(all(feature = "wayland", feature = "desktop-applet"))]
    OutputEvent(OutputEvent, WlOutput),
    Cosmic(app::Action),
    None,
    Surface(surface::Action<Message>),
    CutPaths(Vec<PathBuf>),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ContextPage {
    About,
    EditHistory,
    NetworkDrive,
    Preview(Option<Entity>, PreviewKind),
    Settings,
}

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum ArchiveType {
    Tgz,
    #[default]
    Zip,
}

impl ArchiveType {
    pub const fn all() -> &'static [Self] {
        &[Self::Tgz, Self::Zip]
    }

    pub const fn extension(&self) -> &str {
        match self {
            Self::Tgz => ".tgz",
            Self::Zip => ".zip",
        }
    }
}

impl AsRef<str> for ArchiveType {
    fn as_ref(&self) -> &str {
        self.extension()
    }
}

#[derive(Clone, Debug)]
pub enum DialogPage {
    Compress {
        paths: Box<[PathBuf]>,
        to: PathBuf,
        name: String,
        archive_type: ArchiveType,
        password: Option<String>,
    },
    EmptyTrash,
    FailedOperation(u64),
    FailedOperations(Vec<u64>),
    ExtractPassword {
        id: u64,
        password: String,
    },
    MountError {
        mounter_key: MounterKey,
        item: MounterItem,
        error: String,
    },
    NetworkAuth {
        mounter_key: MounterKey,
        uri: String,
        auth: MounterAuth,
        auth_tx: mpsc::Sender<MounterAuth>,
    },
    NetworkError {
        mounter_key: MounterKey,
        uri: String,
        error: String,
    },
    NewItem {
        parent: PathBuf,
        name: String,
        dir: bool,
    },
    RunContextAction {
        action: usize,
        paths: Box<[PathBuf]>,
    },
    OpenWith {
        path: PathBuf,
        mime: mime_guess::Mime,
        selected: usize,
        store_opt: Option<Arc<MimeApp>>,
        search_app_name: String,
    },
    PermanentlyDelete {
        paths: Box<[PathBuf]>,
    },
    DeleteTrash {
        items: Vec<TrashItem>,
    },
    ChangeSidebarLabel {
        entity: Entity,
        label: String,
    },
    RenameItem {
        from: PathBuf,
        parent: PathBuf,
        name: String,
        dir: bool,
    },
    Replace {
        from: Box<tab::Item>,
        to: Box<tab::Item>,
        multiple: bool,
        apply_to_all: bool,
        conflict_count: usize,
        tx: mpsc::Sender<ReplaceResult>,
    },
    SetExecutableAndLaunch {
        path: PathBuf,
    },
    FavoritePathError {
        path: PathBuf,
        entity: Entity,
    },
}

pub struct DialogPages {
    pages: VecDeque<DialogPage>,
}

impl Default for DialogPages {
    fn default() -> Self {
        Self::new()
    }
}

impl DialogPages {
    pub const fn new() -> Self {
        Self {
            pages: VecDeque::new(),
        }
    }

    pub fn front(&self) -> Option<&DialogPage> {
        self.pages.front()
    }

    pub fn front_mut(&mut self) -> Option<&mut DialogPage> {
        self.pages.front_mut()
    }

    pub fn push_back(&mut self, page: DialogPage) -> Task<Message> {
        let task = if self.pages.is_empty() {
            Task::done(cosmic::Action::App(Message::DesktopDialogs(true)))
        } else {
            Task::none()
        };
        self.pages.push_back(page);
        task
    }

    pub fn push_front(&mut self, page: DialogPage) -> Task<Message> {
        let task = if self.pages.is_empty() {
            Task::done(cosmic::Action::App(Message::DesktopDialogs(true)))
        } else {
            Task::none()
        };
        self.pages.push_front(page);
        task
    }

    #[must_use]
    pub fn pop_front(&mut self) -> Option<(DialogPage, Task<Message>)> {
        let page = self.pages.pop_front()?;
        let task = if self.pages.is_empty() {
            Task::done(cosmic::Action::App(Message::DesktopDialogs(false)))
        } else {
            Task::none()
        };
        Some((page, task))
    }

    pub fn update_front(&mut self, page: DialogPage) {
        if !self.pages.is_empty() {
            self.pages[0] = page;
        }
    }
}

pub struct FavoriteIndex(usize);

pub struct MounterData(MounterKey, MounterItem);

#[derive(Clone, Debug)]
pub enum WindowKind {
    Desktop(Entity),
    DesktopViewOptions,
    Dialogs(widget::Id),
    FileDialog(Option<Box<[PathBuf]>>),
    Preview(Option<Entity>, PreviewKind),
}

pub struct WatcherWrapper {
    watcher_opt: Option<Debouncer<RecommendedWatcher, RecommendedCache>>,
}

impl Clone for WatcherWrapper {
    fn clone(&self) -> Self {
        Self { watcher_opt: None }
    }
}

impl fmt::Debug for WatcherWrapper {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WatcherWrapper").finish()
    }
}

impl PartialEq for WatcherWrapper {
    fn eq(&self, _other: &Self) -> bool {
        false
    }
}

struct Window {
    kind: WindowKind,
    modifiers: Modifiers,
}

/// Filesystem facts about the sidebar entry whose context menu is open, computed off-thread
/// when the menu opens: `nav_context_menu` is rebuilt every frame and must never stat the
/// entry's path on the GUI thread.
#[derive(Clone, Copy, Debug, Default)]
pub struct NavContextInfo {
    is_file: bool,
    is_dir: bool,
    has_recents: bool,
}

impl Window {
    fn new(kind: WindowKind) -> Self {
        Self {
            kind,
            modifiers: Modifiers::empty(),
        }
    }
}

// The [`App`] stores application-specific state.
pub struct App {
    core: Core,
    about: About,
    nav_bar_context_id: segmented_button::Entity,
    nav_context_info: Option<(segmented_button::Entity, NavContextInfo)>,
    nav_context_request: crate::navigation::PendingRequest,
    navigation: crate::navigation::PendingRequest,
    preview_request: crate::navigation::PendingRequest,
    mime_cache_request: crate::navigation::PendingRequest,
    nav_model: segmented_button::SingleSelectModel,
    tab_model: segmented_button::Model<segmented_button::SingleSelect>,
    config_handler: Option<cosmic_config::Config>,
    state_handler: Option<cosmic_config::Config>,
    config: Config,
    state: State,
    mode: Mode,
    app_themes: Vec<String>,
    compio_tx: mpsc::Sender<Pin<Box<dyn Future<Output = ()> + Send>>>,
    context_page: ContextPage,
    dialog_pages: DialogPages,
    dialog_text_input: widget::Id,
    key_binds: HashMap<KeyBind, Action>,
    margin: FxHashMap<window::Id, (f32, f32, f32, f32)>,
    mime_app_cache: MimeAppCache,
    modifiers: Modifiers,
    mounter_items: FxHashMap<MounterKey, MounterItems>,
    must_save_sort_names: bool,
    network_drive_connecting: Option<(MounterKey, String)>,
    network_drive_input: String,
    #[cfg(feature = "notify")]
    notification_opt: Option<Arc<Mutex<notify_rust::NotificationHandle>>>,
    #[cfg(all(feature = "wayland", feature = "desktop-applet"))]
    overlap: FxHashMap<String, (window::Id, Rectangle)>,
    pending_operation_id: u64,
    pending_operations: BTreeMap<u64, (Operation, Controller)>,
    progress_operations: BTreeSet<u64>,
    complete_operations: BTreeMap<u64, Operation>,
    failed_operations: BTreeMap<u64, (Operation, Controller, String)>,
    scrollable_id: widget::Id,
    search_id: widget::Id,
    size: Option<Size>,
    #[cfg(all(feature = "wayland", feature = "desktop-applet"))]
    layer_sizes: FxHashMap<window::Id, Size>,
    #[cfg(all(feature = "wayland", feature = "desktop-applet"))]
    surface_ids: FxHashMap<WlOutput, WindowId>,
    #[cfg(all(feature = "wayland", feature = "desktop-applet"))]
    surface_names: FxHashMap<WindowId, String>,
    toasts: widget::toaster::Toasts<Message>,
    watcher_opt: Option<(
        Debouncer<RecommendedWatcher, RecommendedCache>,
        FxHashSet<PathBuf>,
    )>,
    // Set when update_watcher is called while a watch update is already running off-thread, so
    // the watched-path set is reconciled once the in-flight update finishes.
    watcher_update_pending: bool,
    windows: FxHashMap<window::Id, Window>,
    nav_dnd_hover: Option<(Location, Instant)>,
    tab_dnd_hover: Option<(Entity, Instant)>,
    type_select_prefix: String,
    type_select_last_key: Option<Instant>,
    nav_drag_id: DragId,
    tab_drag_id: DragId,
    auto_scroll_speed: Option<i16>,
    file_dialog_opt: Option<Dialog<Message>>,
    clipboard_cache: ClipboardCache,
}

impl App {
    fn nav_context_items(&self, entity: Entity) -> Vec<widget::menu::Item<NavMenuAction, String>> {
        let favorite_index_opt = self.nav_model.data::<FavoriteIndex>(entity);
        let location_opt = self.nav_model.data::<Location>(entity);

        let info = self
            .nav_context_info
            .filter(|(cached_entity, _)| *cached_entity == entity)
            .map(|(_, info)| info);

        let mut items = Vec::with_capacity(7);

        if info.is_some_and(|info| info.is_file) {
            items.push(cosmic::widget::menu::Item::Button(
                fl!("open"),
                None,
                NavMenuAction::Open(entity),
            ));
            items.push(cosmic::widget::menu::Item::Button(
                fl!("menu-open-with"),
                None,
                NavMenuAction::OpenWith(entity),
            ));
        } else if info.is_some_and(|info| info.is_dir)
            || location_opt.is_some_and(|location| location.path_opt().is_none())
        {
            items.push(cosmic::widget::menu::Item::Button(
                fl!("open-in-new-tab"),
                None,
                NavMenuAction::OpenInNewTab(entity),
            ));
            items.push(cosmic::widget::menu::Item::Button(
                fl!("open-in-new-window"),
                None,
                NavMenuAction::OpenInNewWindow(entity),
            ));
        }
        if location_opt.and_then(Location::path_opt).is_some() {
            let action_items: Vec<_> = self
                .config
                .context_actions
                .iter()
                .enumerate()
                .filter(|(_, action)| {
                    action.selection == crate::context_action::ContextActionSelection::Any
                        || info.is_some_and(|info| {
                            (info.is_file || info.is_dir)
                                && action.matches_selection(1, usize::from(info.is_dir))
                        })
                })
                .map(|(i, action)| {
                    cosmic::widget::menu::Item::Button(
                        action.name.clone(),
                        None,
                        NavMenuAction::RunContextAction(entity, i),
                    )
                })
                .collect();

            if !action_items.is_empty() {
                items.push(cosmic::widget::menu::Item::Divider);
                items.extend(action_items);
            }
        }
        items.push(cosmic::widget::menu::Item::Divider);
        if matches!(location_opt, Some(Location::Path(..))) {
            items.push(cosmic::widget::menu::Item::Button(
                fl!("show-details"),
                None,
                NavMenuAction::Preview(entity),
            ));
        }
        items.push(cosmic::widget::menu::Item::Divider);
        if favorite_index_opt.is_some() {
            items.push(cosmic::widget::menu::Item::Button(
                fl!("change-sidebar-label"),
                None,
                NavMenuAction::ChangeSidebarLabel(entity),
            ));
            items.push(cosmic::widget::menu::Item::Button(
                fl!("remove-from-sidebar"),
                None,
                NavMenuAction::RemoveFromSidebar(entity),
            ));
        }

        if matches!(location_opt, Some(Location::Recents))
            && info.is_some_and(|info| info.has_recents)
        {
            items.push(cosmic::widget::menu::Item::Button(
                fl!("clear-recents-history"),
                None,
                NavMenuAction::ClearRecents,
            ));
        }

        if matches!(location_opt, Some(Location::Trash)) && !Trash::is_empty_cached() {
            items.push(cosmic::widget::menu::Item::Button(
                fl!("empty-trash"),
                None,
                NavMenuAction::EmptyTrash,
            ));
        }
        items
    }

    /// Returns true if the clipboard cache contains pasteable content
    fn clipboard_has_content(&self) -> bool {
        !matches!(self.clipboard_cache, ClipboardCache::Empty)
    }

    fn push_dialog(&mut self, page: DialogPage, focus_id: Option<widget::Id>) -> Task<Message> {
        let t = self.dialog_pages.push_back(page);
        if let Some(focus_id) = focus_id {
            Task::batch([t, focus(focus_id)])
        } else {
            t
        }
    }

    fn open_file(paths: &[impl AsRef<Path>]) -> Task<Message> {
        // Associate all paths to its MIME type. This allows handling paths as groups if
        // possible, such as launching a single video player that is passed every path.
        //
        // mime_for_path stats and reads each file's header to sniff its type, which blocks on
        // slow/network mounts; classify off-thread and launch in Message::OpenFileClassified.
        let paths: Vec<PathBuf> = paths.iter().map(|path| path.as_ref().to_owned()).collect();
        Task::future(async move {
            let targets = tokio::task::spawn_blocking(move || {
                let mimes: Vec<Mime> = paths
                    .iter()
                    .map(|path| mime_icon::mime_for_path(path, None, false))
                    .collect();
                OpenTargets::classify(paths, mimes)
            })
            .await
            .unwrap_or_else(|err| {
                log::warn!("failed to classify paths for opening: {err}");
                OpenTargets::ByMime(FxHashMap::default())
            });
            cosmic::action::app(Message::OpenFileClassified(targets))
        })
    }

    fn open_classified(&mut self, targets: OpenTargets) -> Task<Message> {
        let groups = match targets {
            // Use extract to dialog if all selected paths are supported archives
            OpenTargets::Archives(paths) => return self.extract_to(&paths),
            OpenTargets::ByMime(groups) => groups,
        };
        let mut tasks = Vec::new();

        'outer: for (mime, paths) in groups {
            log::debug!("Attempting to launch app\n\tfor: {mime}\n\twith: {paths:?}");

            // First launch apps that can be launched directly
            if mime == "application/x-desktop" {
                #[cfg(feature = "desktop")]
                {
                    // DesktopEntry::from_path reads the .desktop file, which blocks on a slow/removable
                    // mount; parse off-thread (the spawn_detached launch is already non-blocking).
                    tasks.push(Task::future(async move {
                        let _ = tokio::task::spawn_blocking(move || {
                            Self::launch_desktop_entries(&paths);
                        })
                        .await;
                        cosmic::action::none()
                    }));
                    continue;
                }
            } else if mime == "application/x-executable" || mime == "application/vnd.appimage" {
                // Try opening executable
                for path in paths {
                    let mut command = std::process::Command::new(&path);
                    match spawn_detached(&mut command) {
                        Ok(()) => {}
                        Err(err) => match err.kind() {
                            io::ErrorKind::PermissionDenied => {
                                // If permission is denied, try marking as executable, then running
                                tasks.push(self.push_dialog(
                                    DialogPage::SetExecutableAndLaunch { path },
                                    Some(SET_EXECUTABLE_AND_LAUNCH_CONFIRM_BUTTON_ID.clone()),
                                ));
                            }
                            _ => {
                                log::warn!("failed to execute {}: {}", path.display(), err);
                            }
                        },
                    }
                }
                continue;
            }

            // Try mime apps, which should be faster than xdg-open
            if self.launch_from_mime_cache(&mime, &paths) {
                continue;
            }

            // loop through subclasses if available
            if let Some(mime_sub_classes) = mime_icon::parent_mime_types(&mime) {
                for sub_class in mime_sub_classes {
                    if self.launch_from_mime_cache(&sub_class, &paths) {
                        continue 'outer;
                    }
                }
            }

            // Fall back to using open crate
            let mut opened = Vec::with_capacity(paths.len());
            for path in paths {
                match open::that_detached(&path) {
                    Ok(()) => opened.push(path),
                    Err(err) => {
                        log::warn!("failed to open {}: {}", path.display(), err);
                    }
                }
            }
            self.record_recently_used(opened);
        }

        Task::batch(tasks)
    }

    #[cfg(feature = "desktop")]
    fn launch_desktop_entries(paths: &[impl AsRef<Path>]) {
        use cosmic::desktop::fde::DesktopEntry;
        let locales = cosmic::desktop::fde::get_languages_from_env();

        for path in paths.iter().map(AsRef::as_ref) {
            match DesktopEntry::from_path::<&str>(path, None) {
                Ok(entry) => match entry.exec() {
                    Some(exec) => {
                        match mime_app::exec_to_command(
                            exec,
                            entry.name(&locales).as_deref().unwrap_or_default(),
                            Some(path),
                            &[] as &[&str; 0],
                        ) {
                            Some(commands) => {
                                let cwd_opt = entry.desktop_entry("Path");

                                for mut command in commands {
                                    if let Some(cwd) = cwd_opt {
                                        command.current_dir(cwd);
                                    }

                                    if let Err(err) = spawn_detached(&mut command) {
                                        log::warn!("failed to execute {}: {}", path.display(), err);
                                    }
                                }
                            }
                            None => {
                                log::warn!(
                                    "failed to parse {}: invalid Desktop Entry/Exec",
                                    path.display()
                                );
                            }
                        }
                    }
                    None => {
                        log::warn!(
                            "failed to parse {}: missing Desktop Entry/Exec",
                            path.display()
                        );
                    }
                },
                Err(err) => {
                    log::warn!("failed to parse {}: {}", path.display(), err);
                }
            }
        }
    }

    fn record_recently_used(&self, paths: Vec<PathBuf>) {
        if self.config.show_recents {
            crate::recents::record_recently_used(paths, Self::APP_ID);
        }
    }

    fn clear_recently_used() {
        crate::recents::clear_recently_used();
    }

    fn launch_from_mime_cache<P>(&self, mime: &Mime, paths: &[P]) -> bool
    where
        P: std::fmt::Debug + AsRef<Path> + AsRef<std::ffi::OsStr>,
    {
        for app in self.mime_app_cache.get(mime) {
            let Some(commands) = app.command(paths) else {
                continue;
            };
            let len = commands.len();

            for (i, mut command) in commands.into_iter().enumerate() {
                match spawn_detached(&mut command) {
                    Ok(()) => {
                        self.record_recently_used(
                            paths
                                .iter()
                                .map(|path| <P as AsRef<Path>>::as_ref(path).to_path_buf())
                                .collect(),
                        );

                        return true;
                    }
                    Err(err) => {
                        // More than one command: The app doesn't support lists of paths so each command
                        // is associated with one instance
                        //
                        // One command: Attempted to launch one app with multiple paths
                        let path = if len > 1 {
                            format!("{:?}", paths.get(i))
                        } else {
                            format!("{paths:?}")
                        };
                        log::warn!("failed to open {:?} with {:?}: {}", path, app.id, err);
                    }
                }
            }
        }

        // No app matched for mimes and paths
        false
    }

    #[cfg(feature = "desktop")]
    fn exec_entry_action(command: &str) {
        let mut exec = shlex::Shlex::new(command);
        if let Some(cmd) = exec.next().filter(|cmd| !cmd.contains('=')) {
            let mut proc = tokio::process::Command::new(cmd);
            proc.args(exec.filter(|arg| !arg.starts_with('%')));
            let _ = proc.spawn();
        }
    }

    fn destination_selection_dialog(
        &mut self,
        paths: &[impl AsRef<Path>],
        on_result: impl Fn(DialogResult) -> Message + 'static,
        title: impl Into<String>,
        accept_label: impl AsRef<str>,
    ) -> Task<Message> {
        if let Some(destination) = paths
            .first()
            .and_then(|first| first.as_ref().parent())
            .map(Path::to_path_buf)
        {
            let mut tasks = Vec::new();
            if let Some(old_dialog) = self.file_dialog_opt.take() {
                let old_id = old_dialog.window_id();
                self.windows.remove(&old_id);
                tasks.push(window::close(old_id));
            }
            let (mut dialog, dialog_task) = Dialog::new(
                DialogSettings::new()
                    .kind(DialogKind::OpenFolder)
                    .path(destination),
                Message::FileDialogMessage,
                on_result,
            );
            let set_title_task = dialog.set_title(title);
            dialog.set_accept_label(accept_label);
            self.windows.insert(
                dialog.window_id(),
                Window::new(WindowKind::FileDialog(Some(
                    paths.iter().map(|x| x.as_ref().to_path_buf()).collect(),
                ))),
            );
            self.file_dialog_opt = Some(dialog);
            tasks.push(set_title_task);
            tasks.push(dialog_task);
            Task::batch(tasks)
        } else {
            Task::none()
        }
    }

    fn extract_to(&mut self, paths: &[impl AsRef<Path>]) -> Task<Message> {
        self.destination_selection_dialog(
            paths,
            Message::ExtractToResult,
            fl!("extract-to-title"),
            fl!("extract-here"),
        )
    }

    fn move_to(&mut self, paths: &[impl AsRef<Path>]) -> Task<Message> {
        self.destination_selection_dialog(
            paths,
            Message::MoveToResult,
            fl!("move-to-title"),
            fl!("move-to-button-label"),
        )
    }

    fn copy_to(&mut self, paths: &[impl AsRef<Path>]) -> Task<Message> {
        self.destination_selection_dialog(
            paths,
            Message::CopyToResult,
            fl!("copy-to-title"),
            fl!("copy-to-button-label"),
        )
    }

    #[cfg(all(feature = "wayland", feature = "desktop-applet"))]
    fn handle_overlap(&mut self) {
        let mut overlaps: FxHashMap<_, _> = self
            .windows
            .keys()
            .map(|k| (*k, (0., 0., 0., 0.)))
            .collect();
        let mut sorted_overlaps: Box<[_]> = self.overlap.values().collect();
        sorted_overlaps
            .sort_by(|a, b| (b.1.width * b.1.height).total_cmp(&(a.1.width * b.1.height)));

        for (w_id, overlap) in sorted_overlaps {
            let Some((bl, br, tl, tr, size)) = self.layer_sizes.get(w_id).map(|s| {
                (
                    Rectangle::new(
                        Point::new(0., s.height / 2.),
                        Size::new(s.width / 2., s.height / 2.),
                    ),
                    Rectangle::new(
                        Point::new(s.width / 2., s.height / 2.),
                        Size::new(s.width / 2., s.height / 2.),
                    ),
                    Rectangle::new(Point::new(0., 0.), Size::new(s.width / 2., s.height / 2.)),
                    Rectangle::new(
                        Point::new(s.width / 2., 0.),
                        Size::new(s.width / 2., s.height / 2.),
                    ),
                    *s,
                )
            }) else {
                continue;
            };
            let tl = tl.intersects(overlap);
            let tr = tr.intersects(overlap);
            let bl = bl.intersects(overlap);
            let br = br.intersects(overlap);
            let Some((top, left, bottom, right)) = overlaps.get_mut(w_id) else {
                continue;
            };
            if tl && tr {
                *top += overlap.height;
            }
            if tl && bl {
                *left += overlap.width;
            }
            if bl && br {
                *bottom += overlap.height;
            }
            if tr && br {
                *right += overlap.width;
            }

            let min_dim =
                if overlap.width / size.width.max(1.) > overlap.height / size.height.max(1.) {
                    (0., overlap.height)
                } else {
                    (overlap.width, 0.)
                };
            if tl && !(tr || bl) {
                *top += min_dim.1;
                *left += min_dim.0;
            }
            if tr && !(tl || br) {
                *top += min_dim.1;
                *right += min_dim.0;
            }
            if bl && !(br || tl) {
                *bottom += min_dim.1;
                *left += min_dim.0;
            }
            if br && !(bl || tr) {
                *bottom += min_dim.1;
                *right += min_dim.0;
            }
        }
        self.margin = overlaps;
    }

    fn open_tab_entity(
        &mut self,
        location: Location,
        activate: bool,
        selection_paths: Option<Vec<PathBuf>>,
        scrollable_id: widget::Id,
        window_id: Option<window::Id>,
    ) -> (Entity, Task<Message>) {
        if activate {
            self.navigation.invalidate();
            self.preview_request.invalidate();
        }
        let mut tab = Tab::new(
            location.clone(),
            self.config.tab,
            self.config.thumb_cfg,
            Some(&self.state.sort_names),
            scrollable_id,
            window_id,
        );
        tab.mode = match self.mode {
            Mode::App => tab::Mode::App,
            Mode::Desktop => {
                tab.config.view = tab::View::Grid;
                tab::Mode::Desktop
            }
        };

        let entity = self
            .tab_model
            .insert()
            .text(tab.title())
            .data(tab)
            .closable();

        let entity = if activate {
            entity.activate().id()
        } else {
            entity.id()
        };

        let mut tasks = Vec::with_capacity(4);
        if activate {
            tasks.push(task::widget(unfocus()));
        }
        tasks.push(self.update_title());
        tasks.push(self.update_watcher());
        tasks.push(self.update_tab(entity, location, selection_paths));
        (entity, Task::batch(tasks))
    }

    /// Open the tab shown when no location was asked for: the working directory, or home.
    fn open_default_tab(&mut self) -> Task<Message> {
        let path = env::current_dir().unwrap_or_else(|_| home_dir());
        self.open_tab(Location::Path(path), true, None)
    }

    fn open_tab(
        &mut self,
        location: Location,
        activate: bool,
        selection_paths: Option<Vec<PathBuf>>,
    ) -> Task<Message> {
        self.open_tab_entity(
            location,
            activate,
            selection_paths,
            self.scrollable_id.clone(),
            None,
        )
        .1
    }

    // This wrapper ensures that local folders use trash and remote folders permanently delete with a dialog
    fn delete(paths: impl IntoIterator<Item = PathBuf>) -> Task<Message> {
        Self::delete_confirming(paths, Vec::new())
    }

    /// Deletes `paths`, trashing those the Trash can take, and asks before deleting the rest and
    /// every path in `dialog_paths` permanently.
    fn delete_confirming(
        paths: impl IntoIterator<Item = PathBuf>,
        dialog_paths: Vec<PathBuf>,
    ) -> Task<Message> {
        let paths: Vec<PathBuf> = paths.into_iter().collect();
        // Classifying trash vs permanent-delete stats each path, which blocks on slow mounts;
        // do it off-thread and finish in Message::DeleteClassified.
        Task::future(async move {
            let (trash_paths, dialog_paths) = tokio::task::spawn_blocking(move || {
                let mut trash_paths = Vec::new();
                let mut dialog_paths = dialog_paths;
                for path in paths {
                    let can_trash = match path.symlink_metadata() {
                        Ok(metadata) => tab::can_trash(&metadata),
                        Err(err) => {
                            log::warn!("failed to get metadata for {}: {}", path.display(), err);
                            false
                        }
                    };
                    if can_trash {
                        trash_paths.push(path);
                    } else {
                        dialog_paths.push(path);
                    }
                }
                (trash_paths, dialog_paths)
            })
            .await
            .unwrap_or_else(|err| {
                log::warn!("failed to classify paths for deletion: {err}");
                (Vec::new(), Vec::new())
            });
            cosmic::action::app(Message::DeleteClassified {
                trash_paths,
                dialog_paths,
            })
        })
    }

    fn delete_classified(
        &mut self,
        trash_paths: Vec<PathBuf>,
        dialog_paths: Vec<PathBuf>,
    ) -> Task<Message> {
        let mut tasks = Vec::new();
        if !dialog_paths.is_empty() {
            tasks.push(self.update(Message::DialogPush(
                DialogPage::PermanentlyDelete {
                    paths: dialog_paths.into_boxed_slice(),
                },
                Some(PERMANENT_DELETE_BUTTON_ID.clone()),
            )));
        }
        if !trash_paths.is_empty() {
            tasks.push(self.operation(Operation::Delete { paths: trash_paths }));
        }
        Task::batch(tasks)
    }

    fn operation(&mut self, operation: Operation) -> Task<Message> {
        let id = self.pending_operation_id;
        let controller = Controller::default();
        let compio_tx = self.compio_tx.clone();

        self.pending_operation_id += 1;
        if operation.show_progress_notification() {
            self.progress_operations.insert(id);
        }
        self.pending_operations
            .insert(id, (operation.clone(), controller.clone()));

        // Use a task to send operations to the compio runtime thread.
        cosmic::Task::stream(cosmic::iced::stream::channel(4, move |msg_tx| async move {
            let (tx, rx) = tokio::sync::oneshot::channel();

            let msg_tx = Arc::new(tokio::sync::Mutex::new(msg_tx));

            let msg_tx_clone = msg_tx.clone();

            _ = compio_tx
                .send(Box::pin(async move {
                    let msg = match operation.perform(&msg_tx_clone, controller).await {
                        Ok(result_paths) => Message::PendingComplete(id, result_paths),
                        Err(err) => Message::PendingError(id, err),
                    };

                    _ = tx.send(msg);
                }))
                .await;

            if let Ok(msg) = rx.await {
                let _ = msg_tx.lock().await.send(msg).await;
            }
        }))
        .map(cosmic::Action::App)
    }

    /// Will join operations together into a single task that will return a single
    /// Message::PendingResults message when all operations are complete.
    fn join_operations(&mut self, operations: Vec<Operation>) -> Task<Message> {
        Task::batch(
            operations
                .into_iter()
                .map(|operation| self.operation(operation)),
        )
        .collect()
        .map(|messages| {
            let results = messages.into_iter().fold(
                Message::PendingResults(Vec::new(), Vec::new()),
                |mut acc, message| {
                    if let Message::PendingResults(completed, errors) = &mut acc {
                        match message {
                            cosmic::Action::App(Message::PendingComplete(id, selection)) => {
                                completed.push((id, selection));
                            }
                            cosmic::Action::App(Message::PendingError(id, err)) => {
                                errors.push((id, err));
                            }
                            _ => {}
                        }
                    }
                    acc
                },
            );
            cosmic::Action::App(results)
        })
    }

    fn handle_completed_operations(
        &mut self,
        completed: Vec<(u64, OperationSelection)>,
    ) -> Task<Message> {
        let mut commands = Vec::with_capacity(4 * completed.len());
        let mut op_sel = OperationSelection::default();
        for (id, op_sel_pending) in completed {
            op_sel.ignored.extend(op_sel_pending.ignored);
            op_sel.selected.extend(op_sel_pending.selected);
            if let Some((op, _)) = self.pending_operations.remove(&id) {
                // Show toast for some operations
                if let Some(description) = op.toast() {
                    if let Operation::Delete { ref paths } = op {
                        let paths: Arc<[PathBuf]> = Arc::from(paths.as_slice());
                        commands.push(
                            self.toasts
                                .push(
                                    widget::toaster::Toast::new(description)
                                        .action(fl!("undo"), move |tid| {
                                            Message::UndoTrash(tid, paths.clone())
                                        }),
                                )
                                .map(cosmic::Action::App),
                        );
                    } else {
                        commands.push(
                            self.toasts
                                .push(widget::toaster::Toast::new(description))
                                .map(cosmic::Action::App),
                        );
                    }
                }

                // If a favorite for a path has been renamed or moved, update it.
                if let Operation::Rename { ref from, ref to } = op {
                    if self.update_favorites([(from, to)].as_slice()) {
                        commands.push(self.update_config());
                    }
                } else if let Operation::Move {
                    ref paths, ref to, ..
                } = op
                {
                    let path_changes: Box<[_]> = paths
                        .iter()
                        .filter_map(|from| from.file_name().map(|name| (from, to.join(name))))
                        .collect();
                    if self.update_favorites(&path_changes) {
                        commands.push(self.update_config());
                    }
                }

                if matches!(op, Operation::RemoveFromRecents { .. }) {
                    commands.push(self.rescan_recents());
                }

                self.complete_operations.insert(id, op);
                cap_operation_history(&mut self.complete_operations);
            }
        }
        // Close progress notification if all relevant operations are finished
        if !self
            .pending_operations
            .values()
            .any(|(op, _)| op.show_progress_notification())
        {
            self.progress_operations.clear();
        }
        // Potentially show a notification
        commands.push(self.update_notification());
        // Rescan and select based on operation
        commands.push(self.rescan_operation_selection(op_sel));
        // Manually rescan any trash tabs after any operation is completed
        commands.push(self.rescan_trash());
        // An operation may have changed the trash; refresh its cached state off-thread.
        commands.push(Self::refresh_trash_state());

        Task::batch(commands)
    }

    fn handle_operation_errors(&mut self, errors: Vec<(u64, OperationError)>) -> Task<Message> {
        let mut tasks = Vec::new();
        let mut failed = Vec::new();
        for (id, err) in errors.into_iter() {
            if let Some((op, controller)) = self.pending_operations.remove(&id) {
                // Only show dialog if not cancelled
                if !controller.is_cancelled() {
                    match err.kind {
                        OperationErrorType::Generic(_) => failed.push(id),
                        OperationErrorType::PasswordRequired => {
                            tasks.push(self.dialog_pages.push_back(DialogPage::ExtractPassword {
                                id,
                                password: String::new(),
                            }));
                        }
                    }
                }

                // Remove from progress
                self.progress_operations.remove(&id);
                self.failed_operations
                    .insert(id, (op, controller, err.to_string()));
                cap_operation_history(&mut self.failed_operations);
            }
        }
        if !failed.is_empty() {
            tasks.push(
                self.dialog_pages
                    .push_back(DialogPage::FailedOperations(failed)),
            );
            tasks.push(widget::text_input::focus(self.dialog_text_input.clone()));
        }

        // Close progress notification if all relevant operations are finished
        if !self
            .pending_operations
            .values()
            .any(|(op, _)| op.show_progress_notification())
        {
            self.progress_operations.clear();
        }
        // Manually rescan any trash tabs after any operation is completed
        tasks.push(self.rescan_trash());
        Task::batch(tasks)
    }

    fn remove_window(&mut self, id: &window::Id) {
        if let Some(window) = self.windows.remove(id)
            && let WindowKind::Desktop(entity) = window.kind
        {
            // Remove the tab from the tab model
            self.tab_model.remove(entity);
        }
    }

    fn rescan_operation_selection(&mut self, op_sel: OperationSelection) -> Task<Message> {
        log::info!("rescan_operation_selection {op_sel:?}");
        let entity = self.tab_model.active();
        let Some(tab) = self.tab_model.data::<Tab>(entity) else {
            return Task::none();
        };
        let Some(items) = tab.items_opt() else {
            return Task::none();
        };
        for item in items {
            if item.selected {
                if let Some(path) = item.path_opt()
                    && (op_sel.selected.contains(path) || op_sel.ignored.contains(path))
                {
                    // Ignore if path in selected or ignored paths
                    continue;
                }

                // Return if there is a previous selection not matching
                return Task::none();
            }
        }
        self.update_tab(entity, tab.location.clone(), Some(op_sel.selected))
    }

    fn update_tab(
        &mut self,
        entity: Entity,
        location: Location,
        selection_paths: Option<Vec<PathBuf>>,
    ) -> Task<Message> {
        if let Location::Search(_, term, ..) = location {
            self.search_set(entity, Some(term), selection_paths)
        } else {
            self.rescan_tab(entity, location, selection_paths)
        }
    }

    fn rescan_tab(
        &mut self,
        entity: Entity,
        location: Location,
        selection_paths: Option<Vec<PathBuf>>,
    ) -> Task<Message> {
        log::info!("rescan_tab {entity:?} {location:?} {selection_paths:?}");
        let Some(tab) = self.tab_model.data_mut::<Tab>(entity) else {
            return Task::none();
        };
        let request = tab.scans.begin();
        let icon_sizes = self.config.tab.icon_sizes;
        let mounter_items = self.mounter_items.clone();

        Task::future(async move {
            let location2 = location.clone();
            match tokio::task::spawn_blocking(move || location2.scan(icon_sizes)).await {
                Ok((parent_item_opt, mut items)) => {
                    #[cfg(feature = "gvfs")]
                    {
                        let mounter_paths: Box<[_]> = mounter_items
                            .values()
                            .flatten()
                            .filter_map(MounterItem::path)
                            .collect();
                        if !mounter_paths.is_empty() {
                            for item in &mut items {
                                item.is_mount_point =
                                    item.path_opt().is_some_and(|p| mounter_paths.contains(p));
                            }
                        }
                    }

                    cosmic::action::app(Message::TabRescan(
                        entity,
                        request,
                        location,
                        parent_item_opt,
                        items,
                        selection_paths,
                    ))
                }
                Err(err) => {
                    log::warn!("failed to rescan: {err}");
                    cosmic::action::app(Message::TabRescanFailed(entity, request))
                }
            }
        })
    }

    fn rescan_tab_if_idle(&mut self, entity: Entity, location: Location) -> Option<Task<Message>> {
        if self
            .tab_model
            .data_mut::<Tab>(entity)?
            .scans
            .defer_if_running()
        {
            return None;
        }
        Some(self.update_tab(entity, location, None))
    }

    fn refresh_tab_entries(
        &mut self,
        entity: Entity,
        paths: Vec<PathBuf>,
    ) -> Option<Task<Message>> {
        let tab = self.tab_model.data_mut::<Tab>(entity)?;
        let location = tab.location.clone();
        let sizes = tab.config.icon_sizes;
        let requests = tab.refresh_requests(paths);
        Some(Task::future(async move {
            let refreshed = tokio::task::spawn_blocking(move || {
                requests
                    .into_iter()
                    .map(|(path, work_id)| {
                        let item_opt = fs::metadata(&path).ok().and_then(|metadata| {
                            let name = path.file_name()?.to_str()?.to_owned();
                            Some(tab::item_from_entry(path.clone(), name, metadata, sizes))
                        });
                        tab::EntryRefresh {
                            path,
                            work_id,
                            item_opt,
                        }
                    })
                    .collect()
            })
            .await
            .inspect_err(|err| log::warn!("failed to refresh watched entries: {err}"))
            .unwrap_or_default();
            cosmic::action::app(Message::TabRefreshEntries(entity, location, refreshed))
        }))
    }

    fn rescan_trash(&mut self) -> Task<Message> {
        let needs_reload: Box<[_]> = self
            .tab_model
            .iter()
            .filter_map(|entity| {
                let tab = self.tab_model.data::<Tab>(entity)?;
                tab.location
                    .is_trash()
                    .then_some((entity, tab.location.clone()))
            })
            .collect();

        let commands = needs_reload
            .into_iter()
            .filter_map(|(entity, location)| self.rescan_tab_if_idle(entity, location));

        Task::batch(commands)
    }

    fn refresh_trash_state() -> Task<Message> {
        let request = Trash::begin_refresh();
        Task::future(async move {
            match tokio::task::spawn_blocking(Trash::is_empty).await {
                Ok(is_empty) => {
                    cosmic::action::app(Message::TrashStateRefreshed(request, is_empty))
                }
                Err(err) => {
                    log::warn!("failed to read trash state: {err}");
                    cosmic::action::none()
                }
            }
        })
    }

    /// Rebuild the mime-app cache off-thread. Scanning every desktop entry (and resolving the
    /// default terminal) blocks, so it must never run inline in `update`/`view`/`init`;
    /// [`Message::MimeAppCacheLoaded`] swaps in the freshly built cache.
    fn refresh_mime_app_cache(&mut self) -> Task<Message> {
        let request = self.mime_cache_request.begin();
        Task::future(async move {
            match tokio::task::spawn_blocking(MimeAppCache::new).await {
                Ok(cache) => cosmic::action::app(Message::MimeAppCacheLoaded(request, cache)),
                Err(err) => {
                    log::warn!("failed to reload mime app cache: {err}");
                    cosmic::action::none()
                }
            }
        })
    }

    /// Build the [`tab::Item`] for `path` off the GUI thread and hand it to `to_message`.
    /// `item_from_path` does an `fs::metadata` plus a `read_dir`/gvfs stat, which blocks on a
    /// dead or slow mount.
    fn item_from_path_task(
        path: PathBuf,
        to_message: impl FnOnce(PathBuf, Box<tab::Item>) -> Message + Send + 'static,
    ) -> Task<Message> {
        Task::future(async move {
            match tokio::task::spawn_blocking(move || {
                let item = tab::item_from_path(&path, IconSizes::default());
                (path, item)
            })
            .await
            {
                Ok((path, Ok(item))) => cosmic::action::app(to_message(path, Box::new(item))),
                Ok((path, Err(err))) => {
                    log::warn!("failed to get item from path {}: {}", path.display(), err);
                    cosmic::action::none()
                }
                Err(err) => {
                    log::warn!("failed to get item from path: {err}");
                    cosmic::action::none()
                }
            }
        })
    }

    fn write_pasted_file(
        to: PathBuf,
        base_name: String,
        data: impl AsRef<[u8]> + Send + 'static,
        label: &'static str,
    ) -> Task<Message> {
        Task::future(async move {
            match tokio::task::spawn_blocking(move || {
                write_unique_file(&to, &base_name, data.as_ref())
            })
            .await
            {
                Ok(Ok(path)) => log::info!("pasted {label} saved to {}", path.display()),
                Ok(Err(err)) => log::error!("failed to save pasted {label}: {err}"),
                Err(err) => log::error!("failed to save pasted {label}: {err}"),
            }
            cosmic::action::none()
        })
    }

    fn rescan_recents(&mut self) -> Task<Message> {
        let needs_reload: Box<[_]> = self
            .tab_model
            .iter()
            .filter_map(|entity| {
                let tab = self.tab_model.data::<Tab>(entity)?;
                tab.location
                    .is_recents()
                    .then_some((entity, tab.location.clone()))
            })
            .collect();

        let commands = needs_reload
            .into_iter()
            .filter_map(|(entity, location)| self.rescan_tab_if_idle(entity, location));

        Task::batch(commands)
    }

    fn search_get(&self) -> Option<&str> {
        let entity = self.tab_model.active();
        let tab = self.tab_model.data::<Tab>(entity)?;
        match &tab.location {
            Location::Search(_, term, ..) => Some(term),
            _ => None,
        }
    }

    fn search_set_active(&mut self, term_opt: Option<String>) -> Task<Message> {
        let entity = self.tab_model.active();
        self.search_set(entity, term_opt, None)
    }

    fn search_set(
        &mut self,
        tab: Entity,
        term_opt: Option<String>,
        selection_paths: Option<Vec<PathBuf>>,
    ) -> Task<Message> {
        self.navigation.invalidate();
        self.preview_request.invalidate();
        let mut title_location_opt = None;
        if let Some(tab) = self.tab_model.data_mut::<Tab>(tab) {
            let location_opt = match term_opt {
                Some(term) => {
                    let search_location = if let Some(path) = tab.location.path_opt() {
                        Some(SearchLocation::Path(path.clone()))
                    } else if tab.location.is_recents() {
                        Some(SearchLocation::Recents)
                    } else if tab.location.is_trash() {
                        Some(SearchLocation::Trash)
                    } else {
                        None
                    };

                    search_location.map(|search_location| {
                        (
                            Location::Search(
                                search_location,
                                term,
                                tab.config.show_hidden,
                                Instant::now(),
                            ),
                            true,
                        )
                    })
                }
                None => match &tab.location {
                    Location::Search(search_location, ..) => match search_location {
                        SearchLocation::Path(path) => Some((Location::Path(path.clone()), false)),
                        SearchLocation::Recents => Some((Location::Recents, false)),
                        SearchLocation::Trash => Some((Location::Trash, false)),
                    },
                    _ => None,
                },
            };
            if let Some((location, focus_search)) = location_opt {
                tab.change_location(&location, None);
                title_location_opt = Some((tab.title(), tab.location.clone(), focus_search));
            }
        }
        if let Some((title, location, focus_search)) = title_location_opt {
            self.tab_model.text_set(tab, title);
            return Task::batch([
                self.update_title(),
                self.update_watcher(),
                self.rescan_tab(tab, location, selection_paths),
                if focus_search {
                    widget::text_input::focus(self.search_id.clone())
                } else {
                    Task::none()
                },
            ]);
        }
        Task::none()
    }

    fn selected_paths(
        &self,
        entity_opt: Option<Entity>,
    ) -> impl Iterator<Item = PathBuf> + use<'_> {
        let entity = entity_opt.unwrap_or_else(|| self.tab_model.active());
        self.tab_model
            .data::<Tab>(entity)
            .into_iter()
            .flat_map(|tab| {
                tab.selected_locations()
                    .into_iter()
                    .filter_map(Location::into_path_opt)
            })
    }

    // Selected paths paired with their cached directory flag, read from already-scanned
    // metadata so deciding dir-vs-file never stats the filesystem on the GUI thread.
    fn selected_paths_with_dir(&self, entity_opt: Option<Entity>) -> Vec<(PathBuf, bool)> {
        let entity = entity_opt.unwrap_or_else(|| self.tab_model.active());
        self.tab_model
            .data::<Tab>(entity)
            .map(Tab::selected_paths_with_dir)
            .unwrap_or_default()
    }

    /// Whether `name` already exists in `dir`, with its directory flag, read from the
    /// already-scanned listing of a tab showing `dir`: the dialog builders run in `view` every
    /// frame and must not block on a slow/network mount. `None` means absent, or that no open
    /// tab is showing `dir`; the create/rename operation remains the authoritative collision
    /// guard.
    fn existing_entry_in_dir(&self, dir: &Path, name: &str) -> Option<bool> {
        self.tab_model.iter().find_map(|entity| {
            let tab = self.tab_model.data::<Tab>(entity)?;
            tab.lists_directory(dir)
                .then(|| tab.find_entry_is_dir(name))
                .flatten()
        })
    }

    fn set_cut(&mut self, entity_opt: Option<Entity>) {
        let entity = entity_opt.unwrap_or_else(|| self.tab_model.active());
        if let Some(tab) = self.tab_model.data_mut::<Tab>(entity) {
            tab.cut_selected();
        }
    }

    fn update_config(&mut self) -> Task<Message> {
        self.update_nav_model();
        // Tabs are collected first to placate the borrowck
        let tabs: Box<[_]> = self.tab_model.iter().collect();
        // Update main conf and each tab with the new config
        let commands = std::iter::once(cosmic::command::set_theme(self.config.app_theme.theme()))
            .chain(tabs.into_iter().map(|entity| {
                self.update(Message::TabMessage(
                    Some(entity),
                    tab::Message::Config(self.config.tab),
                ))
            }));
        Task::batch(commands)
    }

    fn update_desktop(&mut self) -> Task<Message> {
        let needs_reload: Box<[_]> = (self.tab_model.iter())
            .filter_map(|entity| {
                let tab = self.tab_model.data::<Tab>(entity)?;
                if let Location::Desktop(path, output, _) = &tab.location {
                    Some((
                        entity,
                        Location::Desktop(path.clone(), output.clone(), self.config.desktop),
                    ))
                } else {
                    None
                }
            })
            .collect();

        let mut commands = Vec::with_capacity(needs_reload.len());
        for (entity, location) in needs_reload {
            if let Some(tab) = self.tab_model.data_mut::<Tab>(entity) {
                tab.location = location.clone();
            }
            commands.push(self.update_tab(entity, location, None));
        }
        Task::batch(commands)
    }

    fn activate_nav_model_location(&mut self, location: &Location) {
        let nav_bar_id = self.nav_model.iter().find(|&id| {
            self.nav_model
                .data::<Location>(id)
                .is_some_and(|l| l == location)
        });

        if let Some(id) = nav_bar_id {
            self.nav_model.activate(id);
        } else {
            let active = self.nav_model.active();
            segmented_button::Selectable::deactivate(&mut self.nav_model, active);
        }
    }

    fn update_nav_model(&mut self) {
        let mut nav_model = segmented_button::ModelBuilder::default();

        if self.config.show_recents {
            nav_model = nav_model.insert(|b| {
                b.text(fl!("recents"))
                    .icon(icon::from_name("document-open-recent-symbolic"))
                    .data(Location::Recents)
            });
        }

        for (favorite_i, favorite) in self.config.favorites.iter().enumerate() {
            if let Some(path) = favorite.path_opt() {
                let name = favorite.display_name().unwrap_or_else(|| fl!("filesystem"));
                nav_model = nav_model.insert(move |b| {
                    // Favorites are directories; use the folder icon without stat-ing the
                    // path, which would block the GUI thread on slow/network mounts.
                    b.text(name.clone())
                        .icon(icon::icon(tab::folder_icon_symbolic(&path, 16)).size(16))
                        .data(match favorite {
                            Favorite::Network { uri, name, path } => {
                                Location::Network(uri.clone(), name.clone(), Some(path.to_owned()))
                            }
                            _ => Location::Path(path.clone()),
                        })
                        .data(FavoriteIndex(favorite_i))
                });
            }
        }

        nav_model = nav_model.insert(|b| {
            b.text(fl!("trash"))
                .icon(icon::icon(Trash::icon_symbolic(16)))
                .data(Location::Trash)
                .divider_above()
        });

        if !MOUNTERS.is_empty() {
            nav_model = nav_model.insert(|b| {
                b.text(fl!("networks"))
                    .icon(icon::icon(
                        icon::from_name("network-workgroup-symbolic")
                            .size(16)
                            .handle(),
                    ))
                    .data(Location::Network(
                        "network:///".to_string(),
                        fl!("networks"),
                        None,
                    ))
                    .divider_above()
            });
        }

        // Collect all mounter items
        let mut nav_items = Vec::new();
        for (key, items) in &self.mounter_items {
            nav_items.extend(items.iter().map(|item| (*key, item)));
        }
        // Sort by name lexically
        nav_items.sort_by(|a, b| LANGUAGE_SORTER.compare(&a.1.name(), &b.1.name()));
        // Add items to nav model
        for (i, (key, item)) in nav_items.into_iter().enumerate() {
            nav_model = nav_model.insert(|mut b| {
                b = b.text(item.name()).data(MounterData(key, item.clone()));
                let uri = item.uri();
                if let Some(path) = item.path() {
                    if item.is_remote() {
                        b = b.data(Location::Network(uri, item.name(), Some(path)));
                    } else {
                        b = b.data(Location::Path(path));
                    }
                } else if !uri.is_empty() {
                    b = b.data(Location::Network(uri, item.name(), None));
                }
                if let Some(icon) = item.icon(true) {
                    b = b.icon(icon::icon(icon).size(16));
                }
                if item.is_mounted() {
                    b = b.closable();
                }
                if i == 0 {
                    b = b.divider_above();
                }
                b
            });
        }

        self.nav_model = nav_model.build();

        let tab_entity = self.tab_model.active();
        if let Some(tab) = self.tab_model.data::<Tab>(tab_entity) {
            self.activate_nav_model_location(&tab.location.clone());
        }
    }

    fn update_notification(&mut self) -> Task<Message> {
        // Handle closing notification if there are no operations
        if self.pending_operations.is_empty() {
            #[cfg(feature = "notify")]
            if let Some(notification_arc) = self.notification_opt.take() {
                return Task::future(async move {
                    tokio::task::spawn_blocking(move || {
                        //TODO: this is nasty
                        let notification_mutex = Arc::try_unwrap(notification_arc).unwrap();
                        let notification = notification_mutex.into_inner().unwrap();
                        notification.close();
                    })
                    .await
                    .unwrap();
                    cosmic::action::app(Message::MaybeExit)
                });
            }
        }

        Task::none()
    }

    fn update_title(&mut self) -> Task<Message> {
        let window_title = match self.tab_model.text(self.tab_model.active()) {
            Some(tab_title) => format!("{tab_title} — {}", fl!("cosmic-files")),
            None => fl!("cosmic-files"),
        };
        if let Some(window_id) = self.core.main_window_id() {
            self.set_window_title(window_title, window_id)
        } else {
            Task::none()
        }
    }

    /// The directories the open tabs show — the set the watcher should be watching.
    fn open_directory_paths(&self) -> FxHashSet<PathBuf> {
        self.tab_model
            .iter()
            .filter_map(|entity| {
                let tab = self.tab_model.data::<Tab>(entity)?;
                tab.location.path_opt().cloned()
            })
            .collect()
    }

    fn leave_unmounted_paths(&mut self, unmounted: &[PathBuf]) -> Task<Message> {
        let mut commands = Vec::new();
        {
            let home_location = Location::Path(home_dir());
            let entities: Box<[_]> = self.tab_model.iter().collect();
            for entity in entities {
                let title_opt = self.tab_model.data_mut::<Tab>(entity).and_then(|tab| {
                    unmounted
                        .iter()
                        .any(|unmounted| {
                            tab.location
                                .path_opt()
                                .is_some_and(|location| location.starts_with(unmounted))
                        })
                        .then(|| {
                            tab.change_location(&home_location, None);
                            tab.title()
                        })
                });
                if let Some(title) = title_opt {
                    self.navigation.invalidate();
                    self.tab_model.text_set(entity, title);
                    commands.push(self.update_tab(entity, home_location.clone(), None));
                }
            }
            if !commands.is_empty() {
                commands.push(self.update_title());
                commands.push(self.update_watcher());
            }
        }

        Task::batch(commands)
    }

    fn update_watcher(&mut self) -> Task<Message> {
        let Some((watcher, old_paths)) = self.watcher_opt.take() else {
            // A watch update is already running off-thread; reconcile once it finishes.
            self.watcher_update_pending = true;
            return Task::none();
        };

        let new_paths = self.open_directory_paths();

        // watch()/unwatch() call inotify_add_watch, which resolves the path and blocks on a
        // cold/slow mount; run them off-thread and restore the watcher in WatcherUpdated.
        Task::future(async move {
            let requested = new_paths.clone();
            let watcher = tokio::task::spawn_blocking(move || {
                crate::watcher::reconcile(watcher, old_paths, new_paths)
            })
            .await;

            match watcher {
                Ok((watcher, new_paths)) => cosmic::action::app(Message::WatcherUpdated(
                    WatcherWrapper {
                        watcher_opt: Some(watcher),
                    },
                    new_paths,
                    requested,
                )),
                Err(err) => {
                    log::warn!("failed to update watcher: {err}");
                    cosmic::action::none()
                }
            }
        })
    }

    fn network_drive(&self) -> Element<'_, Message> {
        let cosmic_theme::Spacing {
            space_xxs, space_m, ..
        } = theme::spacing();
        let mut table = widget::column::with_capacity(8);
        for (i, line) in fl!("network-drive-schemes").lines().enumerate() {
            let mut row = widget::row::with_capacity(2);
            for part in line.split(',') {
                row = row.push(
                    widget::container(if i == 0 {
                        widget::text::heading(part.to_string())
                    } else {
                        widget::text::body(part.to_string())
                    })
                    .width(Length::Fill)
                    .padding(space_xxs),
                );
            }
            table = table.push(row);
            if i == 0 {
                table = table.push(widget::divider::horizontal::light());
            }
        }
        widget::column::with_children([
            widget::text::body(fl!("network-drive-description")).into(),
            table.into(),
        ])
        .spacing(space_m)
        .into()
    }

    fn desktop_view_options(&self) -> Element<'_, Message> {
        let cosmic_theme::Spacing {
            space_m, space_l, ..
        } = theme::spacing();
        let config = self.config.desktop;

        let show_on_desktop = settings::section()
            .title(fl!("show-on-desktop"))
            .add(
                settings::item::builder(fl!("desktop-folder-content")).toggler(
                    config.show_content,
                    move |show_content| {
                        Message::DesktopConfig(DesktopConfig {
                            show_content,
                            ..config
                        })
                    },
                ),
            )
            .add(settings::item::builder(fl!("mounted-drives")).toggler(
                config.show_mounted_drives,
                move |show_mounted_drives| {
                    Message::DesktopConfig(DesktopConfig {
                        show_mounted_drives,
                        ..config
                    })
                },
            ))
            .add(settings::item::builder(fl!("trash-folder-icon")).toggler(
                config.show_trash,
                move |show_trash| {
                    Message::DesktopConfig(DesktopConfig {
                        show_trash,
                        ..config
                    })
                },
            ));

        let icon_size = config.icon_size;
        let grid_spacing = config.grid_spacing;
        let icon_size_and_spacing = settings::section()
            .title(fl!("icon-size-and-spacing"))
            .add(
                settings::item::builder(fl!("icon-size"))
                    .description(format!("{icon_size}%"))
                    .control(
                        widget::slider(50..=500, icon_size.get(), move |new_value| {
                            Message::DesktopConfig(DesktopConfig {
                                icon_size: NonZeroU16::new(new_value).unwrap_or(icon_size),
                                ..config
                            })
                        })
                        .step(25u16),
                    ),
            )
            .add(
                settings::item::builder(fl!("grid-spacing"))
                    .description(format!("{grid_spacing}%"))
                    .control(
                        widget::slider(50..=500, grid_spacing.get(), move |new_value| {
                            Message::DesktopConfig(DesktopConfig {
                                grid_spacing: NonZeroU16::new(new_value).unwrap_or(grid_spacing),
                                ..config
                            })
                        })
                        .step(25u16),
                    ),
            );

        widget::column::with_capacity(2)
            .padding([0, space_l, space_l, space_l])
            .spacing(space_m)
            .push(show_on_desktop)
            .push(icon_size_and_spacing)
            .into()
    }

    fn edit_history(&self) -> Element<'_, Message> {
        let cosmic_theme::Spacing { space_m, .. } = theme::spacing();

        let mut children = Vec::new();

        //TODO: get height from theme?
        let progress_bar_height = Length::Fixed(4.0);

        if !self.pending_operations.is_empty() {
            let mut section = widget::settings::section().title(fl!("pending"));
            for (id, (op, controller)) in self.pending_operations.iter().rev() {
                let progress = controller.progress();
                section = section.add(widget::column::with_children([
                    widget::row::with_children([
                        widget::determinate_linear(progress)
                            .width(Length::Fill)
                            .girth(progress_bar_height)
                            .into(),
                        if controller.is_paused() {
                            widget::tooltip(
                                widget::button::icon(icon::from_name(
                                    "media-playback-start-symbolic",
                                ))
                                .on_press(Message::PendingPause(*id, false))
                                .padding(8),
                                widget::text::body(fl!("resume")),
                                widget::tooltip::Position::Top,
                            )
                            .into()
                        } else {
                            widget::tooltip(
                                widget::button::icon(icon::from_name(
                                    "media-playback-pause-symbolic",
                                ))
                                .on_press(Message::PendingPause(*id, true))
                                .padding(8),
                                widget::text::body(fl!("pause")),
                                widget::tooltip::Position::Top,
                            )
                            .into()
                        },
                        widget::tooltip(
                            widget::button::icon(icon::from_name("window-close-symbolic"))
                                .on_press(Message::PendingCancel(*id))
                                .padding(8),
                            widget::text::body(fl!("cancel")),
                            widget::tooltip::Position::Top,
                        )
                        .into(),
                    ])
                    .align_y(Alignment::Center)
                    .into(),
                    widget::text::body(op.pending_text(progress, controller.state())).into(),
                ]));
            }
            children.push(section.into());
        }

        if !self.failed_operations.is_empty() {
            let mut section = widget::settings::section().title(fl!("failed"));
            for (op, controller, error) in self.failed_operations.values().rev() {
                let progress = controller.progress();
                section = section.add(widget::column::with_children([
                    widget::text::body(op.pending_text(progress, controller.state())).into(),
                    widget::text::body(error).into(),
                ]));
            }
            children.push(section.into());
        }

        if !self.complete_operations.is_empty() {
            let mut section = widget::settings::section().title(fl!("complete"));
            for op in self.complete_operations.values().rev() {
                section = section.add(widget::text::body(op.completed_text()));
            }
            children.push(section.into());
        }

        if children.is_empty() {
            children.push(widget::text::body(fl!("no-history")).into());
        }

        widget::column::with_children(children)
            .spacing(space_m)
            .into()
    }

    fn preview<'a>(
        &'a self,
        entity_opt: &Option<Entity>,
        kind: &'a PreviewKind,
        context_drawer: bool,
    ) -> Element<'a, tab::Message> {
        let cosmic_theme::Spacing { space_l, .. } = theme::spacing();

        let mut children = Vec::with_capacity(1);
        let entity = entity_opt.unwrap_or_else(|| self.tab_model.active());
        let military_time = self.config.tab.military_time;
        match kind {
            PreviewKind::Custom(PreviewItem(item)) => {
                children.push(item.preview_view(Some(&self.mime_app_cache), military_time));
            }
            PreviewKind::Location(location) => {
                if let Some(tab) = self.tab_model.data::<Tab>(entity)
                    && let Some(items) = tab.items_opt()
                {
                    for item in items {
                        if item.location_opt.as_ref() == Some(location) {
                            children
                                .push(item.preview_view(Some(&self.mime_app_cache), military_time));
                            // Only show one property view to avoid issues like hangs when generating
                            // preview images on thousands of files
                            break;
                        }
                    }
                }
            }
            PreviewKind::Selected => {
                if let Some(tab) = self.tab_model.data::<Tab>(entity)
                    && let Some(items) = tab.items_opt()
                {
                    let preview_opt = {
                        let mut selected = items.iter().filter(|item| item.selected);

                        match (selected.next(), selected.next()) {
                            // At least two selected items
                            (Some(_), Some(_)) => {
                                Some(tab.multi_preview_view(Some(&self.mime_app_cache)))
                            }
                            // Exactly one selected item
                            (Some(item), None) => {
                                Some(item.preview_view(Some(&self.mime_app_cache), military_time))
                            }
                            // No selected items
                            _ => None,
                        }
                    };

                    if let Some(preview) = preview_opt {
                        children.push(preview);
                    }

                    if children.is_empty()
                        && let Some(item) = &tab.parent_item_opt
                    {
                        children.push(item.preview_view(Some(&self.mime_app_cache), military_time));
                    }
                }
            }
        }
        widget::column::with_children(children)
            .padding(if context_drawer {
                [0, 0, 0, 0]
            } else {
                [0, space_l, space_l, space_l]
            })
            .into()
    }

    fn settings(&self) -> Element<'_, Message> {
        let tab_config = self.config.tab;

        // TODO: Should dialog be updated here too?
        settings::view_column(vec![
            settings::section()
                .title(fl!("appearance"))
                .add({
                    let app_theme_selected = match self.config.app_theme {
                        AppTheme::Dark => 1,
                        AppTheme::Light => 2,
                        AppTheme::System => 0,
                    };
                    settings::item::builder(fl!("theme")).control(widget::dropdown(
                        &self.app_themes,
                        Some(app_theme_selected),
                        move |index| {
                            Message::AppTheme(match index {
                                1 => AppTheme::Dark,
                                2 => AppTheme::Light,
                                _ => AppTheme::System,
                            })
                        },
                    ))
                })
                .into(),
            settings::section()
                .title(fl!("type-to-search"))
                .add(
                    settings::item::builder(fl!("type-to-search-recursive")).radio(
                        TypeToSearch::Recursive,
                        Some(self.config.type_to_search),
                        Message::SetTypeToSearch,
                    ),
                )
                .add(
                    settings::item::builder(fl!("type-to-search-enter-path")).radio(
                        TypeToSearch::EnterPath,
                        Some(self.config.type_to_search),
                        Message::SetTypeToSearch,
                    ),
                )
                .add(settings::item::builder(fl!("type-to-search-select")).radio(
                    TypeToSearch::SelectByPrefix,
                    Some(self.config.type_to_search),
                    Message::SetTypeToSearch,
                ))
                .into(),
            settings::section()
                .title(fl!("other"))
                .add({
                    settings::item::builder(fl!("single-click")).toggler(
                        tab_config.single_click,
                        move |single_click| {
                            Message::TabConfig(TabConfig {
                                single_click,
                                ..tab_config
                            })
                        },
                    )
                })
                .add({
                    settings::item::builder(fl!("show-recents"))
                        .toggler(self.config.show_recents, Message::SetShowRecents)
                })
                .into(),
        ])
        .into()
    }

    // Update favorites based on renaming or moving dirs.
    fn update_favorites(&mut self, path_changes: &[(impl AsRef<Path>, impl AsRef<Path>)]) -> bool {
        let mut favorites_changed = false;
        let favorites = self
            .config
            .favorites
            .iter()
            .map(|favorite| {
                match favorite {
                    Favorite::Path(path) => {
                        for (from, to) in path_changes.iter().map(|(f, t)| (f.as_ref(), t.as_ref()))
                        {
                            if path.starts_with(from)
                                && let Ok(relative) = path.strip_prefix(from)
                            {
                                favorites_changed = true;
                                return Favorite::from_path(to.join(relative));
                            }
                        }
                    }
                    Favorite::Named { path, name } => {
                        for (from, to) in path_changes.iter().map(|(f, t)| (f.as_ref(), t.as_ref()))
                        {
                            if path.starts_with(from)
                                && let Ok(relative) = path.strip_prefix(from)
                            {
                                favorites_changed = true;
                                return Favorite::Named {
                                    path: to.join(relative),
                                    name: name.clone(),
                                };
                            }
                        }
                    }
                    _ => {}
                }
                favorite.clone()
            })
            .collect();

        if favorites_changed {
            if let Some(config_handler) = &self.config_handler {
                match self.config.set_favorites(config_handler, favorites) {
                    Ok(updated) => {
                        if updated {
                            return true;
                        }
                    }
                    Err(err) => {
                        log::warn!("failed to update favorites after moving directories: {err:?}",);
                    }
                }
            } else {
                self.config.favorites = favorites;
                log::warn!(
                    "failed to update favorites after moving directories: no config handler",
                );
            }
        }

        false
    }
}

/// Implement [`Application`] to integrate with COSMIC.
impl Application for App {
    /// Default async executor to use with the app.
    type Executor = executor::Default;

    /// Argument received
    type Flags = Flags;

    /// Message type specific to our [`App`].
    type Message = Message;

    /// The unique application ID to supply to the window manager.
    const APP_ID: &'static str = "com.system76.CosmicFiles";

    fn core(&self) -> &Core {
        &self.core
    }

    fn core_mut(&mut self) -> &mut Core {
        &mut self.core
    }

    /// Creates the application, and optionally emits command on initialize.
    fn init(mut core: Core, flags: Self::Flags) -> (Self, Task<Self::Message>) {
        core.window.context_is_overlay = false;
        match flags.mode {
            Mode::App => {
                core.window.show_context = flags.config.show_details;
            }
            Mode::Desktop => {
                core.window.content_container = false;
                core.window.show_window_menu = false;
                core.window.show_headerbar = false;
                core.window.sharp_corners = false;
                core.window.show_maximize = false;
                core.window.show_minimize = false;
                core.window.use_template = true;
            }
        }

        let app_themes = vec![fl!("match-desktop"), fl!("dark"), fl!("light")];

        let key_binds = key_binds(&match flags.mode {
            Mode::App => tab::Mode::App,
            Mode::Desktop => tab::Mode::Desktop,
        });

        // Create a dedicated thread for the compio runtime to handle operations on.
        // Supports io_uring on Linux, IOPC on Windows, and polling everywhere else.
        let (compio_tx, mut compio_rx) = mpsc::channel(1);
        let tokio_handle = tokio::runtime::Handle::current();
        std::thread::spawn(move || {
            let _tokio = tokio_handle.enter();
            compio::runtime::RuntimeBuilder::new()
                .build()
                .unwrap()
                .block_on(async move {
                    while let Some(task) = compio_rx.recv().await {
                        compio::runtime::spawn(task).detach();
                    }
                });
        });

        let about = About::default()
            .name(fl!("cosmic-files"))
            .icon(icon::from_name(Self::APP_ID))
            .version(env!("CARGO_PKG_VERSION"))
            .author("System76")
            .comments(fl!("comment"))
            .license("GPL-3.0-only")
            .license_url("https://spdx.org/licenses/GPL-3.0-only")
            .developers([("System76", "info@system76.com")])
            .links([
                (fl!("repository"), "https://github.com/pop-os/cosmic-files"),
                (
                    fl!("support"),
                    "https://github.com/pop-os/cosmic-files/issues",
                ),
            ]);

        if matches!(flags.mode, Mode::Desktop) {
            core.set_auto_blur(Auto::Window | Auto::Popup);
        }
        let mut app = Self {
            core,
            about,
            nav_bar_context_id: segmented_button::Entity::null(),
            nav_context_info: None,
            nav_context_request: crate::navigation::PendingRequest::default(),
            navigation: crate::navigation::PendingRequest::default(),
            preview_request: crate::navigation::PendingRequest::default(),
            mime_cache_request: crate::navigation::PendingRequest::default(),
            nav_model: segmented_button::ModelBuilder::default().build(),
            tab_model: segmented_button::ModelBuilder::default().build(),
            config_handler: flags.config_handler,
            state_handler: flags.state_handler,
            config: flags.config,
            state: flags.state,
            mode: flags.mode,
            app_themes,
            compio_tx,
            context_page: ContextPage::Preview(None, PreviewKind::Selected),
            dialog_pages: DialogPages::new(),
            dialog_text_input: widget::Id::new("Dialog Text Input"),
            key_binds,
            margin: FxHashMap::default(),
            // Populated off-thread so scanning every desktop entry never blocks startup.
            mime_app_cache: MimeAppCache::empty(),
            modifiers: Modifiers::empty(),
            mounter_items: FxHashMap::default(),
            must_save_sort_names: false,
            network_drive_connecting: None,
            network_drive_input: String::new(),
            #[cfg(feature = "notify")]
            notification_opt: None,
            #[cfg(all(feature = "wayland", feature = "desktop-applet"))]
            overlap: FxHashMap::default(),
            pending_operation_id: 0,
            pending_operations: BTreeMap::new(),
            progress_operations: BTreeSet::new(),
            complete_operations: BTreeMap::new(),
            failed_operations: BTreeMap::new(),
            scrollable_id: widget::Id::new("File Scrollable"),
            search_id: widget::Id::new("File Search"),
            size: None,
            #[cfg(all(feature = "wayland", feature = "desktop-applet"))]
            surface_ids: FxHashMap::default(),
            #[cfg(all(feature = "wayland", feature = "desktop-applet"))]
            surface_names: FxHashMap::default(),
            toasts: widget::toaster::Toasts::new(Message::CloseToast),
            watcher_opt: None,
            watcher_update_pending: false,
            windows: FxHashMap::default(),
            nav_dnd_hover: None,
            tab_dnd_hover: None,
            type_select_prefix: String::new(),
            type_select_last_key: None,
            nav_drag_id: DragId::new(),
            tab_drag_id: DragId::new(),
            auto_scroll_speed: None,
            file_dialog_opt: None,
            clipboard_cache: ClipboardCache::Empty,
            #[cfg(all(feature = "wayland", feature = "desktop-applet"))]
            layer_sizes: FxHashMap::default(),
        };

        let mut commands = vec![
            app.update_config(),
            app.update(Message::CheckClipboard),
            // Compute the trash empty/full state off-thread so the nav icon never blocks init.
            App::refresh_trash_state(),
            // Scan desktop entries off-thread so the mime-app cache never blocks init.
            app.refresh_mime_app_cache(),
        ];

        // Resolving each launch location stats it (a file argument opens its parent with the
        // file selected), which blocks init on a slow mount; do it off-thread and open the
        // tabs in Message::OpenLaunchLocations.
        let launch_locations = flags.locations;
        let has_launch_locations = !launch_locations.is_empty();
        if has_launch_locations {
            commands.push(Task::future(async move {
                let resolved = tokio::task::spawn_blocking(move || {
                    launch_locations
                        .into_iter()
                        .map(|location| {
                            let redirect =
                                location.path_opt().filter(|path| path.is_file()).and_then(
                                    |path| Some((path.parent()?.to_path_buf(), path.clone())),
                                );
                            match redirect {
                                Some((parent, file)) => (Location::Path(parent), Some(vec![file])),
                                None => (location, None),
                            }
                        })
                        .collect::<Vec<_>>()
                })
                .await
                .unwrap_or_default();
                cosmic::action::app(Message::OpenLaunchLocations(resolved))
            }));
        }
        for location in flags.uris {
            if let Some(e) = app.nav_model.iter().find(|e| {
                app.nav_model.data::<Location>(*e).is_some_and(
                    |l| matches!(l, Location::Network(uri, ..) if *uri == *location.as_str()),
                )
            }) {
                commands.push(cosmic::task::message(cosmic::Action::App(
                    Message::NetworkDriveOpenEntityAfterMount { entity: e },
                )));
            }
        }

        // Launch locations open off-thread, so tab_model is still empty here even when
        // locations were given; gate the default tab on whether any were passed.
        if !has_launch_locations && app.tab_model.entity_at(0).is_none() {
            commands.push(app.open_default_tab());
        }

        (app, Task::batch(commands))
    }

    fn nav_bar(&self) -> Option<Element<'_, cosmic::Action<Self::Message>>> {
        if !self.core.nav_bar_active() {
            return None;
        }

        let nav_model = self.nav_model()?;

        let mut nav = cosmic::widget::nav_bar(nav_model, |entity| {
            cosmic::Action::Cosmic(cosmic::app::Action::NavBar(entity))
        })
        .drag_id(self.nav_drag_id)
        .on_dnd_enter(|entity, _| cosmic::Action::App(Message::DndEnterNav(entity)))
        .on_dnd_leave(|_| cosmic::Action::App(Message::DndExitNav))
        .on_dnd_drop(|entity, data, action| {
            cosmic::Action::App(Message::DndDropNav(entity, data, action))
        })
        .on_context(|entity| cosmic::Action::App(Message::NavBarContext(entity)))
        .on_close(|entity| cosmic::Action::App(Message::NavBarClose(entity)))
        .on_middle_press(|entity| {
            cosmic::Action::App(Message::NavMenuAction(NavMenuAction::OpenInNewTab(entity)))
        })
        .context_menu(self.nav_context_menu())
        .close_icon(icon::from_name("media-eject-symbolic").size(16).icon());

        #[cfg(feature = "wayland")]
        {
            nav = nav
                .window_id_maybe(self.core().main_window_id())
                .on_surface_action(|action| cosmic::Action::Surface(action.flatten()))
        }

        let mut nav = nav.into_container();

        if !self.core.is_condensed() {
            nav = nav.max_width(280);
        }

        Some(Element::from(
            nav.width(Length::Shrink).height(Length::Fill),
        ))
    }

    fn nav_context_menu(&self) -> Option<Vec<widget::menu::Tree<cosmic::Action<Self::Message>>>> {
        let items = self
            .nav_model
            .iter()
            .map(|entity| self.nav_context_items(entity));

        Some(cosmic::widget::menu::nav_context(
            &HashMap::new(),
            items.collect(),
        ))
    }

    fn nav_model(&self) -> Option<&segmented_button::SingleSelectModel> {
        match self.mode {
            Mode::App => Some(&self.nav_model),
            Mode::Desktop => None,
        }
    }

    fn on_nav_select(&mut self, entity: Entity) -> Task<Self::Message> {
        let request = self.navigation.begin();
        let tab_entity = self.tab_model.active();
        self.nav_model.activate(entity);
        if let Some(location) = self.nav_model.data::<Location>(entity) {
            // Path-bearing favorites/mounts need an existence check (and dir-vs-file decision)
            // that stats the path; on a dead or slow network favorite that blocks the GUI
            // thread, so compute it off-thread and branch in Message::NavSelectChecked.
            if let Some(path) = location.path_opt().cloned() {
                return Task::future(async move {
                    let (exists, is_dir) = tokio::task::spawn_blocking(move || {
                        (path.try_exists().unwrap_or(false), path.is_dir())
                    })
                    .await
                    .unwrap_or((false, false));
                    cosmic::action::app(Message::NavSelectChecked {
                        entity,
                        tab_entity,
                        request,
                        exists,
                        is_dir,
                    })
                });
            }
            // Locations without a path (Trash, Recents, Desktop, ...) open with no I/O.
            let message = Message::TabMessage(None, tab::Message::Location(location.clone()));
            return self.update(message);
        }
        if let Some(data) = self.nav_model.data::<MounterData>(entity)
            && let Some(mounter) = MOUNTERS.get(&data.0)
        {
            return mounter
                .mount(data.1.clone())
                .map(|()| cosmic::action::none());
        }
        Task::none()
    }

    fn on_app_exit(&mut self) -> Option<Message> {
        Some(Message::WindowClose)
    }

    fn on_close_requested(&self, id: window::Id) -> Option<Self::Message> {
        Some(Message::WindowCloseRequested(id))
    }

    fn on_context_drawer(&mut self) -> Task<Self::Message> {
        if let ContextPage::Preview(..) = self.context_page {
            // Persist state of preview page
            if self.core.window.show_context != self.config.show_details {
                return self.update(Message::Preview(None));
            }
        }
        Task::none()
    }

    fn on_escape(&mut self) -> Task<Self::Message> {
        let entity = self.tab_model.active();

        // Close dialog if open
        if let Some((_page, task)) = self.dialog_pages.pop_front() {
            return task;
        }

        // Close gallery mode if open
        if let Some(tab) = self.tab_model.data_mut::<Tab>(entity)
            && tab.gallery
        {
            tab.gallery = false;
            return Task::none();
        }

        // Close menus and context panes in order per message
        // Why: It'd be weird to close everything all at once
        // Usually, the Escape key (for example) closes menus and panes one by one instead
        // of closing everything on one press
        if self.core.window.show_context {
            self.preview_request.invalidate();
            self.set_show_context(false);
            return cosmic::task::message(cosmic::action::app(Message::SetShowDetails(false)));
        }
        if let Some(tab) = self.tab_model.data_mut::<Tab>(entity) {
            if tab.edit_location.is_some() {
                tab.edit_location = None;
                return Task::none();
            }

            let had_focused_button = tab.select_focus_id().is_some();
            if tab.select_none() {
                if had_focused_button {
                    // Unfocus if there was a focused button
                    return widget::button::focus(widget::Id::unique());
                }
                return Task::none();
            }
        }

        if self.search_get().is_some() {
            // Close search if open
            return self.search_set_active(None);
        }

        Task::none()
    }

    /// Handle application events here.
    fn update(&mut self, message: Self::Message) -> Task<Self::Message> {
        // Helper for updating config values efficiently
        macro_rules! config_set {
            ($name: ident, $value: expr) => {
                match &self.config_handler {
                    Some(config_handler) => {
                        match paste::paste! { self.config.[<set_ $name>](config_handler, $value) } {
                            Ok(_) => {}
                            Err(err) => {
                                log::warn!(
                                    "failed to save config {:?}: {}",
                                    stringify!($name),
                                    err
                                );
                            }
                        }
                    }
                    None => {
                        self.config.$name = $value;
                        log::warn!(
                            "failed to save config {:?}: no config handler",
                            stringify!($name)
                        );
                    }
                }
            };
        }

        match message {
            Message::AddToSidebar(entity_opt) => {
                let mut favorites = self.config.favorites.clone();
                // check if the selected entity is in the current tab
                // else just use the selected entity and check its location
                let entity = entity_opt.unwrap_or_else(|| self.tab_model.active());

                for path in self.selected_paths(entity_opt) {
                    let is_network = self.tab_model.data::<Tab>(entity).and_then(|tab| {
                        let in_current_tab = tab
                            .location
                            .path_opt()
                            .zip(path.parent())
                            .is_some_and(|(t_path, parent)| parent == t_path);
                        let tab = if in_current_tab {
                            self.tab_model
                                .data::<Tab>(self.tab_model.active())
                                .unwrap_or(tab)
                        } else {
                            tab
                        };

                        let name = Location::Path(path.clone()).title();
                        if let Location::Network(uri, _, _) = tab
                            .items_opt
                            .as_ref()
                            .and_then(|items| items.iter().find(|&i| i.path_opt() == Some(&path)))
                            .unwrap()
                            .location_opt
                            .as_ref()
                            .unwrap()
                        {
                            Some((uri.clone(), name, path.clone()))
                        } else {
                            None
                        }
                    });
                    let name = Location::Path(path.clone()).title();
                    let favorite = if let Some((uri, _, _)) = is_network.clone() {
                        Favorite::Network { uri, name, path }
                    } else {
                        Favorite::from_path(path)
                    };
                    let favorite_path = favorite.path_opt();
                    if !favorites.iter().any(|f| f.path_opt() == favorite_path) {
                        favorites.push(favorite);
                    }
                }
                config_set!(favorites, favorites);
                return self.update_config();
            }
            Message::AppTheme(app_theme) => {
                config_set!(app_theme, app_theme);
                return self.update_config();
            }
            Message::Compress(entity_opt) => {
                let paths: Box<[_]> = self.selected_paths(entity_opt).collect();
                if let Some(current_path) = paths.first()
                    && let Some(destination) = current_path.parent().zip(current_path.file_stem())
                {
                    let to = destination.0.to_path_buf();
                    let name = destination.1.to_str().unwrap_or_default().to_string();
                    let archive_type = ArchiveType::default();
                    return self.push_dialog(
                        DialogPage::Compress {
                            paths,
                            to,
                            name,
                            archive_type,
                            password: None,
                        },
                        Some(self.dialog_text_input.clone()),
                    );
                }
            }
            Message::Config(config) => {
                if config != self.config {
                    log::info!("update config");
                    // Show details and military time are preserved for existing instances
                    let show_details = self.config.show_details;
                    let military_time = self.config.tab.military_time;
                    self.config = config;
                    self.config.show_details = show_details;
                    self.config.tab.military_time = military_time;
                    return self.update_config();
                }
            }
            Message::Copy(entity_opt) => {
                if let Some(entity) = entity_opt
                    && let Some(tab) = self.tab_model.data_mut::<Tab>(entity)
                {
                    tab.refresh_cut(&[]);
                }
                let paths = self.selected_paths(entity_opt);
                self.clipboard_cache = ClipboardCache::Files(ClipboardPaste {
                    paths: paths.map(|p| p.to_path_buf()).collect(),
                    kind: ClipboardKind::Copy,
                });
                let contents =
                    ClipboardCopy::new(ClipboardKind::Copy, self.selected_paths(entity_opt));
                return clipboard::write_data(contents);
            }
            Message::CopyPath(entity_opt) => {
                let paths = self.selected_paths(entity_opt);
                let path_strings: Vec<String> =
                    paths.into_iter().map(|p| p.display().to_string()).collect();
                let text = path_strings.join("\n");
                return clipboard::write(text);
            }
            Message::CopyText(text) => {
                return clipboard::write(text);
            }
            Message::CopyTo(entity_opt) => {
                let selected_paths: Box<[_]> = self.selected_paths(entity_opt).collect();
                return self.copy_to(&selected_paths);
            }
            Message::CopyToResult(result) => {
                match result {
                    DialogResult::Cancel => {}
                    DialogResult::Open(selected_paths) => {
                        let mut file_paths = None;
                        if let Some(file_dialog) = &self.file_dialog_opt
                            && let Some(window) = self.windows.remove(&file_dialog.window_id())
                            && let WindowKind::FileDialog(paths) = window.kind
                        {
                            file_paths = paths;
                        }
                        if let Some(file_paths) = file_paths
                            && !selected_paths.is_empty()
                        {
                            self.file_dialog_opt = None;
                            return self.operation(Operation::Copy {
                                paths: file_paths.to_vec(),
                                to: selected_paths[0].clone(),
                            });
                        }
                    }
                }
                self.file_dialog_opt = None;
            }
            Message::Cut(entity_opt) => {
                self.set_cut(entity_opt);
                let paths = self.selected_paths(entity_opt);
                self.clipboard_cache = ClipboardCache::Files(ClipboardPaste {
                    paths: paths.map(|p| p.to_path_buf()).collect(),
                    kind: ClipboardKind::Cut { is_dnd: false },
                });
                let contents = ClipboardCopy::new(
                    ClipboardKind::Cut { is_dnd: false },
                    self.selected_paths(entity_opt),
                );

                return clipboard::write_data(contents);
            }
            Message::CloseToast(id) => {
                self.toasts.remove(id);
            }
            Message::CosmicSettings(arg) => {
                //TODO: use special settings URL scheme instead?
                let mut command = process::Command::new("cosmic-settings");
                command.arg(arg);
                match spawn_detached(&mut command) {
                    Ok(()) => {}
                    Err(err) => {
                        log::warn!("failed to run cosmic-settings {arg}: {err}");
                    }
                }
            }
            Message::Delete(entity_opt) => {
                let entity = entity_opt.unwrap_or_else(|| self.tab_model.active());
                if let Some(tab) = self.tab_model.data::<Tab>(entity) {
                    if tab.location.is_trash() {
                        if let Some(items) = tab.items_opt() {
                            let mut trash_items = Vec::new();
                            for item in items {
                                if item.selected {
                                    if let ItemMetadata::Trash { entry, .. } = &item.metadata {
                                        trash_items.push(entry.clone());
                                    } else {
                                        //TODO: error on trying to permanently delete non-trash file?
                                    }
                                }
                            }
                            if !trash_items.is_empty() {
                                return self.update(Message::DialogPush(
                                    DialogPage::DeleteTrash { items: trash_items },
                                    Some(DELETE_TRASH_BUTTON_ID.clone()),
                                ));
                            }
                        }
                    } else {
                        let mut paths = Vec::new();
                        let mut dialog_paths = Vec::new();
                        let selected = tab
                            .items_opt()
                            .into_iter()
                            .flatten()
                            .filter(|item| item.selected);
                        for item in selected {
                            let Some(path) =
                                item.location_opt.clone().and_then(Location::into_path_opt)
                            else {
                                continue;
                            };
                            if item.metadata.can_trash() {
                                paths.push(path);
                            } else {
                                dialog_paths.push(path);
                            }
                        }
                        if !paths.is_empty() || !dialog_paths.is_empty() {
                            return Self::delete_confirming(paths, dialog_paths);
                        }
                    }
                }
            }
            Message::DeleteClassified {
                trash_paths,
                dialog_paths,
            } => {
                return self.delete_classified(trash_paths, dialog_paths);
            }
            Message::DesktopConfig(config) => {
                if config != self.config.desktop {
                    config_set!(desktop, config);
                    return self.update_desktop();
                }
            }
            Message::DesktopViewOptions => {
                let mut settings = window::Settings {
                    decorations: true,
                    min_size: Some(Size::new(360.0, 180.0)),
                    resizable: true,
                    size: Size::new(480.0, 444.0),
                    transparent: true,
                    ..Default::default()
                };

                #[cfg(target_os = "linux")]
                {
                    // Use the dialog ID to make it float
                    settings.platform_specific.application_id =
                        "com.system76.CosmicFilesDialog".to_string();
                }

                let (id, command) = window::open(settings);
                self.windows
                    .insert(id, Window::new(WindowKind::DesktopViewOptions));
                return command.map(|_id| cosmic::action::none());
            }
            Message::DesktopDialogs(show) => {
                if matches!(self.mode, Mode::Desktop) {
                    if show {
                        //TODO: would it be better to make this a layer surface?
                        let mut settings = window::Settings {
                            decorations: false,
                            level: window::Level::AlwaysOnTop,
                            max_size: Some(Size::new(1280.0, 640.0)),
                            min_size: Some(Size::new(320.0, 180.0)),
                            position: window::Position::Centered,
                            resizable: false,
                            size: Size::new(640.0, 320.0),
                            transparent: true,
                            ..Default::default()
                        };

                        #[cfg(target_os = "linux")]
                        {
                            // Use the dialog ID to make it float
                            settings.platform_specific.application_id =
                                "com.system76.CosmicFilesDialog".to_string();
                        }

                        let (id, command) = window::open(settings);
                        self.windows
                            .insert(id, Window::new(WindowKind::Dialogs(widget::Id::unique())));
                        return command.map(|_id| cosmic::Action::None);
                    }

                    let tasks = self
                        .windows
                        .iter()
                        .filter(|(_, window)| matches!(window.kind, WindowKind::Dialogs(_)))
                        .map(|(id, _)| window::close(*id));
                    return Task::batch(tasks);
                }
            }
            Message::DialogCancel => {
                if let Some((_page, task)) = self.dialog_pages.pop_front() {
                    return task;
                }
            }
            Message::DialogComplete => {
                if let Some((dialog_page, task)) = self.dialog_pages.pop_front() {
                    let mut tasks = vec![task];
                    match dialog_page {
                        DialogPage::Compress {
                            paths,
                            to,
                            name,
                            archive_type,
                            password,
                        } => {
                            let extension = archive_type.extension();
                            let name = format!("{name}{extension}");
                            let to = to.join(name);
                            tasks.push(self.operation(Operation::Compress {
                                paths: paths.into_vec(),
                                to,
                                archive_type,
                                password,
                            }));
                        }
                        DialogPage::EmptyTrash => {
                            tasks.push(self.operation(Operation::EmptyTrash));
                        }
                        DialogPage::FailedOperation(id) => {
                            log::warn!("TODO: retry operation {id}");
                        }
                        DialogPage::FailedOperations(_ids) => {
                            log::warn!("TODO: retry operations");
                        }
                        DialogPage::ExtractPassword { id, password } => {
                            // The failed operation may have been evicted from the capped history.
                            match retry_extract_with_password(&self.failed_operations, id, password)
                            {
                                Some(new_op) => tasks.push(self.operation(new_op)),
                                None => log::warn!("failed operation {id} not available to retry"),
                            }
                        }
                        DialogPage::MountError {
                            mounter_key,
                            item,
                            error: _,
                        } => {
                            if let Some(mounter) = MOUNTERS.get(&mounter_key) {
                                tasks.push(mounter.mount(item).map(|()| cosmic::action::none()));
                            }
                        }
                        DialogPage::NetworkAuth {
                            mounter_key: _,
                            uri: _,
                            auth,
                            auth_tx,
                        } => {
                            tasks.push(Task::future(async move {
                                auth_tx.send(auth).await.unwrap();
                                cosmic::action::none()
                            }));
                        }
                        DialogPage::NetworkError {
                            mounter_key: _,
                            uri,
                            error: _,
                        } => {
                            //TODO: re-use mounter_key?
                            tasks.push(self.update(Message::NetworkDriveInput(uri)));
                            tasks.push(self.update(Message::NetworkDriveSubmit));
                        }
                        DialogPage::NewItem { parent, name, dir } => {
                            let path = parent.join(name);
                            tasks.push(self.operation(if dir {
                                Operation::NewFolder { path }
                            } else {
                                Operation::NewFile { path }
                            }));
                        }
                        DialogPage::RunContextAction { action, paths } => {
                            context_action::run(&self.config.context_actions, action, &paths);
                        }
                        DialogPage::OpenWith {
                            path,
                            mime,
                            selected,
                            search_app_name,
                            ..
                        } => {
                            let mut available_apps =
                                self.mime_app_cache.get_apps_for_mime(&mime, true);
                            available_apps.retain(|(app, _)| {
                                app.name
                                    .to_lowercase()
                                    .trim()
                                    .contains(search_app_name.to_lowercase().as_str().trim())
                            });

                            if let Some((app, _)) = available_apps.get(selected) {
                                if let Some(mut command) =
                                    app.command(&[&path]).and_then(|v| v.into_iter().next())
                                {
                                    match spawn_detached(&mut command) {
                                        Ok(()) => {
                                            self.record_recently_used(vec![path.clone()]);
                                        }
                                        Err(err) => {
                                            log::warn!(
                                                "failed to open {} with {:?}: {}",
                                                path.display(),
                                                app.id,
                                                err
                                            );
                                        }
                                    }
                                } else {
                                    log::warn!(
                                        "failed to open {} with {:?}: failed to get command",
                                        path.display(),
                                        app.id
                                    );
                                }
                            }
                        }
                        DialogPage::PermanentlyDelete { paths } => {
                            tasks.push(self.operation(Operation::PermanentlyDelete { paths }));
                        }
                        DialogPage::DeleteTrash { items } => {
                            tasks.push(self.operation(Operation::DeleteTrash { items }));
                        }
                        DialogPage::ChangeSidebarLabel { entity, label } => {
                            if let Some(FavoriteIndex(favorite_i)) =
                                self.nav_model.data::<FavoriteIndex>(entity)
                            {
                                let mut favorites = self.config.favorites.clone();
                                if let Some(favorite) = favorites.get_mut(*favorite_i) {
                                    *favorite = favorite.with_label(label.trim());
                                    config_set!(favorites, favorites);
                                    tasks.push(self.update_config());
                                }
                            }
                        }
                        DialogPage::RenameItem {
                            from, parent, name, ..
                        } => {
                            let to = parent.join(name);
                            tasks.push(self.operation(Operation::Rename { from, to }));
                        }
                        DialogPage::Replace { .. } => {
                            log::warn!("replace dialog should be completed with replace result");
                        }
                        DialogPage::SetExecutableAndLaunch { path } => {
                            tasks.push(self.operation(Operation::SetExecutableAndLaunch { path }));
                        }
                        DialogPage::FavoritePathError { entity, .. } => {
                            if let Some(FavoriteIndex(favorite_i)) =
                                self.nav_model.data::<FavoriteIndex>(entity)
                            {
                                let mut favorites = self.config.favorites.clone();
                                favorites.remove(*favorite_i);
                                config_set!(favorites, favorites);
                                tasks.push(self.update_config());
                            }
                        }
                    }
                    return Task::batch(tasks);
                }
            }
            Message::DialogPush(dialog_page, focused_id) => {
                return self.push_dialog(dialog_page, focused_id);
            }
            Message::DialogUpdate(dialog_page) => {
                self.dialog_pages.update_front(dialog_page);
            }
            Message::DialogUpdateComplete(dialog_page) => {
                return Task::batch([
                    self.update(Message::DialogUpdate(dialog_page)),
                    self.update(Message::DialogComplete),
                ]);
            }
            Message::ExtractHere(entity_opt) => {
                let paths: Box<[_]> = self.selected_paths(entity_opt).collect();
                if let Some(destination) = paths
                    .first()
                    .and_then(|first| first.parent())
                    .map(Path::to_path_buf)
                {
                    return self.operation(Operation::Extract {
                        paths,
                        to: destination,
                        password: None,
                    });
                }
            }
            Message::ExtractTo(entity_opt) => {
                let selected_paths: Box<[_]> = self.selected_paths(entity_opt).collect();
                return self.extract_to(&selected_paths);
            }
            Message::ExtractToResult(result) => {
                match result {
                    DialogResult::Cancel => {}
                    DialogResult::Open(selected_paths) => {
                        let mut archive_paths = None;
                        if let Some(file_dialog) = &self.file_dialog_opt
                            && let Some(window) = self.windows.remove(&file_dialog.window_id())
                            && let WindowKind::FileDialog(paths) = window.kind
                        {
                            archive_paths = paths;
                        }
                        if let Some(archive_paths) = archive_paths
                            && !selected_paths.is_empty()
                        {
                            self.file_dialog_opt = None;
                            return self.operation(Operation::Extract {
                                paths: archive_paths,
                                to: selected_paths[0].clone(),
                                password: None,
                            });
                        }
                    }
                }
                self.file_dialog_opt = None;
            }
            Message::FileDialogMessage(dialog_message) => {
                if let Some(dialog) = &mut self.file_dialog_opt {
                    return dialog.update(dialog_message);
                }
            }
            Message::Key(window_id, modifiers, key, physical_key, text) => {
                #[cfg(all(feature = "wayland", feature = "desktop-applet"))]
                let in_surface_ids = self.surface_ids.values().any(|id| *id == window_id);
                #[cfg(not(all(feature = "wayland", feature = "desktop-applet")))]
                let in_surface_ids = false;
                if self.core.main_window_id() == Some(window_id) || in_surface_ids {
                    let entity = self.tab_model.active();
                    for (key_bind, action) in &self.key_binds {
                        if key_bind.matches(modifiers, &key, Some(&physical_key)) {
                            return self.update(action.message(Some(entity)));
                        }
                    }

                    // Uncaptured keys with only shift modifiers go to the search or location box
                    if matches!(self.mode, Mode::App)
                        && !modifiers.logo()
                        && !modifiers.control()
                        && !modifiers.alt()
                        && matches!(key, Key::Character(_))
                        && let Some(text) = text
                    {
                        match self.config.type_to_search {
                            TypeToSearch::Recursive => {
                                let mut term = self.search_get().unwrap_or_default().to_string();
                                term.push_str(&text);
                                return self.search_set_active(Some(term));
                            }
                            TypeToSearch::EnterPath => {
                                if let Some(tab) = self.tab_model.data_mut::<Tab>(entity) {
                                    let location = tab
                                        .edit_location
                                        .as_ref()
                                        .map_or_else(|| &tab.location, |x| &x.location);
                                    // Try to add text to end of location
                                    if let Location::Network(uri, ..) = location {
                                        let mut uri_string = uri.clone();
                                        uri_string.push_str(&text);
                                        tab.edit_location =
                                            Some(location.with_uri(uri_string).into());
                                    } else if let Some(path) = location.path_opt() {
                                        let mut path_string = path.to_string_lossy().into_owned();
                                        path_string.push_str(&text);
                                        tab.edit_location =
                                            Some(location.with_path(path_string.into()).into());
                                    }
                                }
                            }
                            TypeToSearch::SelectByPrefix => {
                                // Reset buffer if timeout elapsed
                                if let Some(last_key) = self.type_select_last_key
                                    && last_key.elapsed() >= tab::TYPE_SELECT_TIMEOUT
                                {
                                    self.type_select_prefix.clear();
                                }

                                // Accumulate character and select
                                self.type_select_prefix.push_str(&text.to_lowercase());
                                self.type_select_last_key = Some(Instant::now());

                                if let Some(tab) = self.tab_model.data_mut::<Tab>(entity) {
                                    tab.select_by_prefix(&self.type_select_prefix);
                                    if let Some(offset) = tab.select_focus_scroll() {
                                        return scrollable::scroll_to(
                                            tab.scrollable_id.clone(),
                                            AbsoluteOffset {
                                                x: Some(offset.x),
                                                y: Some(offset.y),
                                            },
                                        );
                                    }
                                }
                            }
                        }
                    }
                }
            }
            Message::MaybeExit => {
                if self.core.main_window_id().is_none() && self.pending_operations.is_empty() {
                    return Task::future(async {
                        crate::persistence::flush().await;
                        cosmic::action::app(Message::PersistenceFlushed)
                    });
                }
            }
            Message::PersistenceFlushed => {
                if self.core.main_window_id().is_none() && self.pending_operations.is_empty() {
                    process::exit(0);
                }
            }
            Message::LaunchUrl(url) => match open::that_detached(&url) {
                Ok(()) => {}
                Err(err) => {
                    log::warn!("failed to open {url:?}: {err}");
                }
            },
            Message::ModifiersChanged(window_id, modifiers) => {
                #[cfg(all(feature = "wayland", feature = "desktop-applet"))]
                let in_surface_ids = self.surface_ids.values().any(|id| *id == window_id);
                #[cfg(not(all(feature = "wayland", feature = "desktop-applet")))]
                let in_surface_ids = false;
                if self.core.main_window_id() == Some(window_id) || in_surface_ids {
                    self.modifiers = modifiers;
                }
                if let Some(window) = self.windows.get_mut(&window_id) {
                    window.modifiers = modifiers;
                }
            }
            Message::MounterItems(mounter_key, mounter_items) => {
                // Check for unmounted folders
                let mut unmounted = Vec::new();
                if let Some(old_items) = self.mounter_items.get(&mounter_key) {
                    for old_item in old_items {
                        if let Some(old_path) = old_item.path()
                            && old_item.is_mounted()
                        {
                            let mut still_mounted = false;
                            for item in &mounter_items {
                                if let Some(path) = item.path()
                                    && path == old_path
                                    && item.is_mounted()
                                {
                                    still_mounted = true;
                                    break;
                                }
                            }
                            if !still_mounted {
                                unmounted.push(old_path);
                            }
                        }
                    }
                }

                let mut commands = vec![self.leave_unmounted_paths(&unmounted)];

                // Insert new items
                self.mounter_items.insert(mounter_key, mounter_items);

                // Update nav bar
                //TODO: this could change favorites IDs while they are in use
                self.update_nav_model();

                // Update desktop tabs
                commands.push(self.update_desktop());

                return Task::batch(commands);
            }
            Message::MountResult(mounter_key, item, res) => match res {
                Ok(true) => {
                    log::info!("connected to {item:?}");
                    // Automatically navigate to the mounted location
                    if let Some(path) = item.path() {
                        let location = if item.is_remote() {
                            Location::Network(item.uri(), item.name(), Some(path))
                        } else {
                            Location::Path(path)
                        };
                        let message = Message::TabMessage(None, tab::Message::Location(location));
                        return self.update(message);
                    }
                }
                Ok(false) => {
                    log::info!("cancelled connection to {item:?}");
                }
                Err(error) => {
                    log::warn!("failed to connect to {item:?}: {error}");
                    return self.push_dialog(
                        DialogPage::MountError {
                            mounter_key,
                            item,
                            error,
                        },
                        Some(MOUNT_ERROR_TRY_AGAIN_BUTTON_ID.clone()),
                    );
                }
            },
            Message::Mouse(window_id, _button) => {
                // Close context menu when clicking outside.
                if self.core.main_window_id() == Some(window_id) {
                    return Task::none();
                }
            }
            Message::MoveTo(entity_opt) => {
                let selected_paths: Box<[_]> = self.selected_paths(entity_opt).collect();
                return self.move_to(&selected_paths);
            }
            Message::MoveToResult(result) => {
                match result {
                    DialogResult::Cancel => {}
                    DialogResult::Open(selected_paths) => {
                        let mut file_paths = None;
                        if let Some(file_dialog) = &self.file_dialog_opt
                            && let Some(window) = self.windows.remove(&file_dialog.window_id())
                            && let WindowKind::FileDialog(paths) = window.kind
                        {
                            file_paths = paths;
                        }
                        if let Some(file_paths) = file_paths
                            && !selected_paths.is_empty()
                        {
                            self.file_dialog_opt = None;
                            return self.operation(Operation::Move {
                                paths: file_paths.to_vec(),
                                to: selected_paths[0].clone(),
                                cross_device_copy: false,
                            });
                        }
                    }
                }
                self.file_dialog_opt = None;
            }
            Message::NetworkAuth(mounter_key, uri, auth, auth_tx) => {
                return self.push_dialog(
                    DialogPage::NetworkAuth {
                        mounter_key,
                        uri,
                        auth,
                        auth_tx,
                    },
                    Some(self.dialog_text_input.clone()),
                );
            }
            Message::NetworkDriveInput(input) => {
                self.network_drive_input = input;
            }
            Message::NetworkDriveSubmit => {
                //TODO: know which mounter to use for network drives
                if let Some((mounter_key, mounter)) = MOUNTERS.iter().next() {
                    self.network_drive_connecting =
                        Some((*mounter_key, self.network_drive_input.clone()));
                    return mounter
                        .network_drive(self.network_drive_input.clone())
                        .map(|_| cosmic::action::none());
                }
                log::warn!(
                    "no mounter found for connecting to {:?}",
                    self.network_drive_input
                );
            }
            Message::NetworkResult(mounter_key, uri, res) => {
                if self
                    .network_drive_connecting
                    .as_ref()
                    .is_some_and(|(m, u)| *m == mounter_key && *u == uri)
                {
                    self.network_drive_connecting = None;
                }
                match res {
                    Ok(true) => {
                        log::info!("connected to {uri:?}");
                        if matches!(self.context_page, ContextPage::NetworkDrive) {
                            self.set_show_context(false);
                        }
                    }
                    Ok(false) => {
                        log::info!("cancelled connection to {uri:?}");
                    }
                    Err(error) => {
                        log::warn!("failed to connect to {uri:?}: {error}");
                        return self.dialog_pages.push_back(DialogPage::NetworkError {
                            mounter_key,
                            uri,
                            error,
                        });
                    }
                }
            }
            Message::NewItem(entity_opt, dir) => {
                let entity = entity_opt.unwrap_or_else(|| self.tab_model.active());
                if let Some(tab) = self.tab_model.data_mut::<Tab>(entity)
                    && let Some(path) = tab.location.path_opt()
                {
                    return Task::batch([
                        self.dialog_pages.push_back(DialogPage::NewItem {
                            parent: path.clone(),
                            name: String::new(),
                            dir,
                        }),
                        widget::text_input::focus(self.dialog_text_input.clone()),
                    ]);
                }
            }
            #[cfg(feature = "notify")]
            Message::Notification(notification) => {
                self.notification_opt = Some(notification);
            }
            Message::NotifyEvents(events) => {
                log::debug!("{events:?}");

                let mut work = Vec::new();
                let entities: Box<[_]> = self.tab_model.iter().collect();
                for entity in entities {
                    if let Some(tab) = self.tab_model.data::<Tab>(entity)
                        && let Some(path) = tab.location.path_opt()
                    {
                        work.push((entity, tab.location.clone(), classify_events(&events, path)));
                    }
                }

                let commands =
                    work.into_iter()
                        .filter_map(|(entity, location, change)| match change {
                            ListingChange::Structural => self.rescan_tab_if_idle(entity, location),
                            ListingChange::Entries(paths) => {
                                self.refresh_tab_entries(entity, paths)
                            }
                            ListingChange::None => None,
                        });
                return Task::batch(commands);
            }
            Message::TabRefreshEntries(entity, location, refreshed) => {
                let Some(tab) = self.tab_model.data_mut::<Tab>(entity) else {
                    return Task::none();
                };
                if tab.location != location {
                    return Task::none();
                }
                if tab.refresh_entries(refreshed) {
                    return Task::none();
                }
                let location = tab.location.clone();
                return self
                    .rescan_tab_if_idle(entity, location)
                    .unwrap_or_else(Task::none);
            }
            Message::NotifyWatcher(mut watcher_wrapper) => match watcher_wrapper.watcher_opt.take()
            {
                Some(watcher) => {
                    self.watcher_opt = Some((watcher, FxHashSet::default()));
                    return self.update_watcher();
                }
                None => {
                    log::warn!("message did not contain notify watcher");
                }
            },
            Message::WatcherUpdated(mut watcher_wrapper, paths, requested) => {
                if let Some(watcher) = watcher_wrapper.watcher_opt.take() {
                    let stale = requested != self.open_directory_paths();
                    self.watcher_opt = Some((watcher, paths));
                    if self.watcher_update_pending || stale {
                        self.watcher_update_pending = false;
                        return self.update_watcher();
                    }
                }
            }
            Message::OpenTerminal(entity_opt) => {
                if let Some(terminal) = self.mime_app_cache.terminal() {
                    let mut paths = Box::from([]);
                    let entity = entity_opt.unwrap_or_else(|| self.tab_model.active());
                    if let Some(tab) = self.tab_model.data_mut::<Tab>(entity)
                        && let Some(path) = tab.location.path_opt()
                    {
                        if let Some(items) = tab.items_opt() {
                            paths = items
                                .iter()
                                .filter_map(
                                    |item| {
                                        if item.selected { item.path_opt() } else { None }
                                    },
                                )
                                .collect();
                        }
                        if paths.is_empty() {
                            paths = Box::from([path]);
                        }
                    }
                    for path in paths {
                        if let Some(mut command) = terminal
                            .command::<&str>(&[])
                            .and_then(|v| v.into_iter().next())
                        {
                            command.current_dir(path);
                            if let Err(err) = spawn_detached(&mut command) {
                                log::warn!(
                                    "failed to open {} with terminal {:?}: {}",
                                    path.display(),
                                    terminal.id,
                                    err
                                );
                            }
                        } else {
                            log::warn!("failed to get command for {:?}", terminal.id);
                        }
                    }
                }
            }
            Message::OpenFileClassified(targets) => {
                return self.open_classified(targets);
            }
            Message::OpenLaunchLocations(locations) => {
                if locations.is_empty() && self.tab_model.entity_at(0).is_none() {
                    // Resolving them failed; init left the default tab to this handler.
                    return self.open_default_tab();
                }
                return Task::batch(
                    locations
                        .into_iter()
                        .map(|(location, selection)| self.open_tab(location, true, selection)),
                );
            }
            Message::OpenInNewTab(entity_opt) => {
                let selected_paths: Box<[_]> = self
                    .selected_paths_with_dir(entity_opt)
                    .into_iter()
                    .filter_map(|(path, is_dir)| is_dir.then_some(path))
                    .collect();
                return Task::batch(
                    selected_paths
                        .into_iter()
                        .map(|path| self.open_tab(Location::Path(path), false, None)),
                );
            }
            Message::OpenInNewWindow(entity_opt) => match env::current_exe() {
                Ok(exe) => self
                    .selected_paths_with_dir(entity_opt)
                    .into_iter()
                    .filter_map(|(path, is_dir)| is_dir.then_some(path))
                    .for_each(|path| match process::Command::new(&exe).arg(path).spawn() {
                        Ok(_child) => {}
                        Err(err) => {
                            log::error!("failed to execute {}: {}", exe.display(), err);
                        }
                    }),
                Err(err) => {
                    log::error!("failed to get current executable path: {err}");
                }
            },
            Message::OpenItemLocation(entity_opt) => {
                let selected_paths: Box<[_]> = self.selected_paths(entity_opt).collect();
                return Task::batch(selected_paths.into_iter().filter_map(|path| {
                    path.parent()
                        .map(Path::to_path_buf)
                        .map(|parent| self.open_tab(Location::Path(parent), true, Some(vec![path])))
                }));
            }
            Message::OpenWithBrowse => match self.dialog_pages.pop_front() {
                Some((
                    DialogPage::OpenWith {
                        mime,
                        store_opt: Some(app),
                        ..
                    },
                    task,
                )) => {
                    let url = format!("mime:///{mime}");
                    // TODO: Support multiple URLs
                    if let Some(mut command) =
                        app.command(&[&url]).and_then(|v| v.into_iter().next())
                    {
                        if let Err(err) = spawn_detached(&mut command) {
                            log::warn!("failed to open {:?} with {:?}: {}", url, app.id, err);
                        }
                    } else {
                        log::warn!(
                            "failed to open {:?} with {:?}: failed to get command",
                            url,
                            app.id
                        );
                    }
                    return task;
                }
                Some((dialog_page, task)) => {
                    log::warn!("tried to open with browse from the wrong dialog");
                    return Task::batch([task, self.dialog_pages.push_front(dialog_page)]);
                }
                None => {}
            },
            Message::OpenWithDialog(entity_opt) => {
                let entity = entity_opt.unwrap_or_else(|| self.tab_model.active());
                if let Some(tab) = self.tab_model.data::<Tab>(entity)
                    && let Some(items) = tab.items_opt()
                {
                    for item in items {
                        if !item.selected {
                            continue;
                        }
                        let Some(path) = item.path_opt() else {
                            continue;
                        };
                        return Task::batch([
                            self.push_dialog(
                                DialogPage::OpenWith {
                                    path: path.clone(),
                                    mime: item.mime.clone(),
                                    selected: 0,
                                    store_opt: "x-scheme-handler/mime"
                                        .parse::<mime_guess::Mime>()
                                        .ok()
                                        .and_then(|mime| {
                                            self.mime_app_cache.get(&mime).first().cloned()
                                        }),
                                    search_app_name: String::new(),
                                },
                                Some(CONFIRM_OPEN_WITH_BUTTON_ID.clone()),
                            ),
                            widget::text_input::focus(self.dialog_text_input.clone()),
                        ]);
                    }
                }
            }
            Message::OpenWithSelection(index) => {
                if let Some(DialogPage::OpenWith { selected, .. }) = self.dialog_pages.front_mut() {
                    *selected = index;
                }
            }
            Message::OpenWithSearchClear => {
                if let Some(DialogPage::OpenWith {
                    search_app_name, ..
                }) = self.dialog_pages.front_mut()
                {
                    *search_app_name = String::new();
                }
            }
            Message::Paste(entity_opt) => {
                let entity = entity_opt.unwrap_or_else(|| self.tab_model.active());
                if let Some(tab) = self.tab_model.data_mut::<Tab>(entity)
                    && let Some(path) = tab.location.path_opt()
                {
                    let to = path.clone();

                    // Use cached clipboard data if available (needed for Wayland popups)
                    match &self.clipboard_cache {
                        ClipboardCache::Files(contents) => {
                            if contents.paths.is_empty() {
                                return iced::Task::future(tokio::time::sleep(
                                    std::time::Duration::from_millis(300),
                                ))
                                .discard()
                                .chain(
                                    clipboard::read_data::<ClipboardPaste>().map(
                                        move |contents_opt| match contents_opt {
                                            Some(contents) => cosmic::action::app(
                                                Message::PasteContents(to.clone(), contents),
                                            ),
                                            None => {
                                                cosmic::action::app(Message::PasteImage(to.clone()))
                                            }
                                        },
                                    ),
                                );
                            }
                            return self
                                .update(Message::PasteContents(to.clone(), contents.clone()));
                        }
                        ClipboardCache::Image(contents) => {
                            return self
                                .update(Message::PasteImageContents(to.clone(), contents.clone()));
                        }
                        ClipboardCache::Video(contents) => {
                            return self
                                .update(Message::PasteVideoContents(to.clone(), contents.clone()));
                        }
                        ClipboardCache::Text(contents) => {
                            return self
                                .update(Message::PasteTextContents(to.clone(), contents.clone()));
                        }
                        ClipboardCache::Empty => {
                            // Cache is empty, try reading from clipboard directly
                            // (works when triggered from main window, e.g., Ctrl+V)
                            return clipboard::read_data::<ClipboardPaste>().map(
                                move |contents_opt| match contents_opt {
                                    Some(contents) => cosmic::action::app(Message::PasteContents(
                                        to.clone(),
                                        contents,
                                    )),
                                    None => cosmic::action::app(Message::PasteImage(to.clone())),
                                },
                            );
                        }
                    }
                }
            }
            Message::PasteContents(to, mut contents) => {
                contents.paths.retain(|p| *p != to);
                if !contents.paths.is_empty() {
                    return match contents.kind {
                        ClipboardKind::Copy => self.operation(Operation::Copy {
                            paths: contents.paths,
                            to,
                        }),
                        ClipboardKind::Cut { is_dnd } => self.operation(Operation::Move {
                            paths: contents.paths,
                            to,
                            cross_device_copy: is_dnd,
                        }),
                    };
                }
            }
            Message::PasteImage(to) => {
                return clipboard::read_data::<ClipboardPasteImage>().map(move |contents_opt| {
                    match contents_opt {
                        Some(contents) => {
                            cosmic::action::app(Message::PasteImageContents(to.clone(), contents))
                        }
                        // No image data in clipboard, try video data
                        None => cosmic::action::app(Message::PasteVideo(to.clone())),
                    }
                });
            }
            Message::PasteImageContents(to, contents) => {
                let Some(extension) = contents.extension() else {
                    log::warn!(
                        "Ignoring paste: unknown image MIME type {:?}",
                        contents.mime_type
                    );
                    return Task::none();
                };

                // Generate unique filename for the pasted image
                let base_name = format!("{}.{}", fl!("pasted-image"), extension);
                return Self::write_pasted_file(to, base_name, contents.data, "image");
            }
            Message::PasteVideo(to) => {
                return clipboard::read_data::<ClipboardPasteVideo>().map(move |contents_opt| {
                    match contents_opt {
                        Some(contents) => {
                            cosmic::action::app(Message::PasteVideoContents(to.clone(), contents))
                        }
                        // No video data in clipboard, try text data
                        None => cosmic::action::app(Message::PasteText(to.clone())),
                    }
                });
            }
            Message::PasteVideoContents(to, contents) => {
                let Some(extension) = contents.extension() else {
                    log::warn!(
                        "Ignoring paste: unknown video MIME type {:?}",
                        contents.mime_type
                    );
                    return Task::none();
                };

                // Generate unique filename for the pasted video
                let base_name = format!("{}.{}", fl!("pasted-video"), extension);
                return Self::write_pasted_file(to, base_name, contents.data, "video");
            }
            Message::PasteText(to) => {
                return clipboard::read_data::<ClipboardPasteText>().map(move |contents_opt| {
                    match contents_opt {
                        Some(contents) => {
                            cosmic::action::app(Message::PasteTextContents(to.clone(), contents))
                        }
                        None => cosmic::action::none(),
                    }
                });
            }
            Message::PasteTextContents(to, contents) => {
                // Generate unique filename for the pasted text
                let base_name = format!("{}.txt", fl!("pasted-text"));
                return Self::write_pasted_file(to, base_name, contents.data, "text");
            }
            Message::CheckClipboard => {
                // Check if clipboard has any paste-able content and cache it
                return clipboard::read_data::<ClipboardPaste>().map(|contents_opt| {
                    match contents_opt {
                        Some(contents) if contents.paths.is_empty() => cosmic::action::app(
                            Message::RetryCheckClipboard(ClipboardCache::Files(contents)),
                        ),
                        Some(contents) => cosmic::action::app(Message::ClipboardCached(
                            ClipboardCache::Files(contents),
                        )),
                        _ => cosmic::action::app(Message::CheckClipboardImage),
                    }
                });
            }
            Message::CheckClipboardImage => {
                return clipboard::read_data::<ClipboardPasteImage>().map(|contents_opt| {
                    match contents_opt {
                        Some(contents) => cosmic::action::app(Message::ClipboardCached(
                            ClipboardCache::Image(contents),
                        )),
                        None => cosmic::action::app(Message::CheckClipboardVideo),
                    }
                });
            }
            Message::CheckClipboardVideo => {
                return clipboard::read_data::<ClipboardPasteVideo>().map(|contents_opt| {
                    match contents_opt {
                        Some(contents) => cosmic::action::app(Message::ClipboardCached(
                            ClipboardCache::Video(contents),
                        )),
                        None => cosmic::action::app(Message::CheckClipboardText),
                    }
                });
            }
            Message::CheckClipboardText => {
                return clipboard::read_data::<ClipboardPasteText>().map(|contents_opt| {
                    cosmic::action::app(Message::ClipboardCached(match contents_opt {
                        Some(contents) => ClipboardCache::Text(contents),
                        None => ClipboardCache::Empty,
                    }))
                });
            }
            Message::RetryCheckClipboard(cache) => {
                let mut cmds = Vec::new();
                cmds.push(self.update(Message::ClipboardCached(cache)));

                cmds.push(
                    iced::Task::future(tokio::time::sleep(Duration::from_millis(300)))
                        .discard()
                        .chain(
                            clipboard::read_data::<ClipboardPaste>().map(|contents_opt| {
                                match contents_opt {
                                    Some(contents) if !contents.paths.is_empty() => {
                                        cosmic::action::app(Message::ClipboardCached(
                                            ClipboardCache::Files(contents),
                                        ))
                                    }
                                    _ => cosmic::action::app(Message::CheckClipboardImage),
                                }
                            }),
                        ),
                );
                return Task::batch(cmds);
            }
            Message::ClipboardCached(cache) => {
                self.clipboard_cache = cache;
            }
            Message::PendingCancel(id) => {
                if let Some((_, controller)) = self.pending_operations.get(&id) {
                    controller.cancel();
                    self.progress_operations.remove(&id);
                }
            }
            Message::PendingCancelAll => {
                for (id, (_, controller)) in &self.pending_operations {
                    controller.cancel();
                    self.progress_operations.remove(id);
                }
            }
            Message::PendingComplete(id, op_sel) => {
                return self.handle_completed_operations(vec![(id, op_sel)]);
            }
            Message::PendingDismiss => {
                self.progress_operations.clear();
            }
            Message::PendingError(id, err) => {
                return self.handle_operation_errors(vec![(id, err)]);
            }
            Message::PendingResults(completed, errors) => {
                return Task::batch(vec![
                    self.handle_completed_operations(completed),
                    self.handle_operation_errors(errors),
                ]);
            }
            Message::PendingPause(id, pause) => {
                if let Some((_, controller)) = self.pending_operations.get(&id) {
                    if pause {
                        controller.pause();
                    } else {
                        controller.unpause();
                    }
                }
            }
            Message::PendingPauseAll(pause) => {
                for (_, controller) in self.pending_operations.values() {
                    if pause {
                        controller.pause();
                    } else {
                        controller.unpause();
                    }
                }
            }
            Message::PermanentlyDelete(entity_opt) => {
                let paths: Box<[_]> = self.selected_paths(entity_opt).collect();
                if !paths.is_empty() {
                    return self.push_dialog(
                        DialogPage::PermanentlyDelete { paths },
                        Some(PERMANENT_DELETE_BUTTON_ID.clone()),
                    );
                }
            }
            Message::Preview(entity_opt) => {
                self.preview_request.invalidate();
                match self.mode {
                    Mode::App => {
                        let show_details = !self.config.show_details;
                        self.context_page = ContextPage::Preview(None, PreviewKind::Selected);
                        self.core.window.show_context = show_details;
                        return cosmic::task::message(Message::SetShowDetails(show_details));
                    }
                    Mode::Desktop => {
                        let preview_kind = {
                            let mut selected_paths = self.selected_paths(entity_opt);
                            match (selected_paths.next(), selected_paths.next()) {
                                (Some(_), Some(_)) => Some(PreviewKind::Selected),
                                (Some(path), None) => {
                                    Some(PreviewKind::Location(Location::Path(path)))
                                }
                                _ => None,
                            }
                        };

                        if let Some(preview_kind) = preview_kind {
                            let mut settings = window::Settings {
                                decorations: true,
                                min_size: Some(Size::new(360.0, 180.0)),
                                resizable: true,
                                size: Size::new(480.0, 600.0),
                                transparent: true,
                                ..Default::default()
                            };

                            #[cfg(target_os = "linux")]
                            {
                                // Use the dialog ID to make it float
                                settings.platform_specific.application_id =
                                    "com.system76.CosmicFilesDialog".to_string();
                            }

                            let (id, command) = window::open(settings);
                            self.windows.insert(
                                id,
                                Window::new(WindowKind::Preview(entity_opt, preview_kind)),
                            );
                            return Task::batch([
                                self.update_desktop(), // Force re-calculating of directory sizes
                                command.map(|_id| cosmic::action::none()),
                            ]);
                        }
                    }
                }
            }
            Message::RemoveFromRecents(entity_opt) => {
                let paths: Box<[_]> = self.selected_paths(entity_opt).collect();
                return self.operation(Operation::RemoveFromRecents { paths });
            }
            Message::ReloadMimeAppCache => {
                return self.refresh_mime_app_cache();
            }
            Message::MimeAppCacheLoaded(request, cache) => {
                if !self.mime_cache_request.finish(&request) {
                    return Task::none();
                }
                self.mime_app_cache = cache;
            }
            Message::RescanRecents => {
                return self.rescan_recents();
            }
            Message::RescanTrash => {
                // Refresh the cached state off-thread (Message::TrashStateRefreshed repaints the icon)
                // and rescan tabs.
                return Task::batch([
                    Self::refresh_trash_state(),
                    self.rescan_trash(),
                    self.update_desktop(),
                ]);
            }
            Message::TrashStateRefreshed(request, is_empty) => {
                if !Trash::finish_refresh(&request, is_empty) {
                    return Task::none();
                }
                let maybe_entity = self.nav_model.iter().find(|&entity| {
                    self.nav_model
                        .data::<Location>(entity)
                        .is_some_and(|loc| matches!(loc, Location::Trash))
                });
                if let Some(entity) = maybe_entity {
                    self.nav_model
                        .icon_set(entity, icon::icon(Trash::icon_symbolic(16)));
                }
            }
            Message::Rename(entity_opt) => {
                let entity = entity_opt.unwrap_or_else(|| self.tab_model.active());
                if let Some(tab) = self.tab_model.data_mut::<Tab>(entity)
                    && let Some(items) = tab.items_opt()
                {
                    // Pair each selected path with its cached is_dir; re-stating here would
                    // block the GUI thread on a slow mount.
                    let selected: Box<[_]> = items
                        .iter()
                        .filter_map(|item| {
                            if item.selected {
                                Some((item.path_opt()?.to_path_buf(), item.metadata.is_dir()))
                            } else {
                                None
                            }
                        })
                        .collect();
                    if !selected.is_empty() {
                        //TODO: batch rename
                        let mut last_name = String::new();
                        let tasks: Vec<_> = selected
                            .into_iter()
                            .filter_map(|(path, dir)| {
                                let parent = path.parent()?.to_path_buf();
                                let name = path.file_name()?.to_str()?.to_string();
                                last_name = name.clone();
                                Some(self.dialog_pages.push_back(DialogPage::RenameItem {
                                    from: path,
                                    parent,
                                    name,
                                    dir,
                                }))
                            })
                            .collect();
                        let tasks = tasks.into_iter().chain([
                            widget::text_input::focus(self.dialog_text_input.clone()),
                            widget::text_input::select_until_last(
                                self.dialog_text_input.clone(),
                                &last_name,
                                '.',
                            ),
                        ]);
                        return Task::batch(tasks);
                    }
                }
            }
            Message::ReplaceResult(replace_result) => {
                if let Some((dialog_page, task)) = self.dialog_pages.pop_front() {
                    match dialog_page {
                        DialogPage::Replace { tx, .. } => {
                            return Task::future(async move {
                                let _ = tx.send(replace_result).await;
                                cosmic::action::none()
                            });
                        }
                        other => {
                            log::warn!("tried to send replace result to the wrong dialog");
                            return Task::batch([task, self.dialog_pages.push_front(other)]);
                        }
                    }
                }
            }
            Message::RestoreFromTrash(entity_opt) => {
                let mut trash_items = Vec::new();
                let entity = entity_opt.unwrap_or_else(|| self.tab_model.active());
                if let Some(tab) = self.tab_model.data_mut::<Tab>(entity)
                    && let Some(items) = tab.items_opt()
                {
                    for item in items {
                        if item.selected {
                            if let ItemMetadata::Trash { entry, .. } = &item.metadata {
                                trash_items.push(entry.clone());
                            } else {
                                //TODO: error on trying to restore non-trash file?
                            }
                        }
                    }
                }
                if !trash_items.is_empty() {
                    return self.operation(Operation::Restore { items: trash_items });
                }
            }
            Message::ScrollTab(scroll_speed) => {
                let entity = self.tab_model.active();
                return self.update(Message::TabMessage(
                    Some(entity),
                    tab::Message::ScrollTab(f32::from(scroll_speed) / 10.0),
                ));
            }
            Message::SearchActivate => {
                let mut tasks = vec![];

                if self.search_get().is_none() {
                    tasks.push(self.search_set_active(Some(String::new())));
                } else {
                    tasks.push(widget::text_input::focus(self.search_id.clone()));
                };

                return Task::batch(tasks);
            }
            Message::SearchClear => {
                return self.search_set_active(None);
            }
            Message::SearchInput(input) => {
                return self.search_set_active(Some(input));
            }
            Message::SetShowDetails(show_details) => {
                self.preview_request.invalidate();
                config_set!(show_details, show_details);
                return self.update_config();
            }
            Message::SetShowRecents(show_recents) => {
                config_set!(show_recents, show_recents);
                return self.update_config();
            }
            Message::SetTypeToSearch(type_to_search) => {
                config_set!(type_to_search, type_to_search);
                return self.update_config();
            }
            Message::SystemThemeModeChange => {
                return self.update_config();
            }
            Message::TabActivate(entity) => {
                self.preview_request.invalidate();
                self.navigation.invalidate();
                let mut tasks = vec![];

                // Activate new tab
                self.tab_model.activate(entity);
                if let Some(tab) = self.tab_model.data::<Tab>(entity) {
                    {
                        //Restore scroll
                        //TODO: why do scrollers with different IDs get the same scroll position?
                        let scroll = tab.scroll_opt.unwrap_or_default();
                        tasks.push(scrollable::scroll_to(
                            tab.scrollable_id.clone(),
                            AbsoluteOffset {
                                x: Some(scroll.x),
                                y: Some(scroll.y),
                            },
                        ));
                    }
                    self.activate_nav_model_location(&tab.location.clone());
                }
                tasks.push(self.update_title());
                return Task::batch(tasks);
            }
            Message::TabNext => {
                let len = self.tab_model.len();
                let pos = (self
                    .tab_model
                    .position(self.tab_model.active())
                    .expect("should always be at least one tab open")
                    + 1)
                    // Wraparound to 0 if i + 1 > num of tabs
                    % len as u16;

                let entity = self.tab_model.entity_at(pos);
                if let Some(entity) = entity {
                    return self.update(Message::TabActivate(entity));
                }
            }
            Message::TabPrev => {
                let pos = self
                    .tab_model
                    .position(self.tab_model.active())
                    .expect("should always be at least one tab open")
                    .checked_sub(1)
                    // Subtraction underflow => last tab; i.e. it wraps around
                    .unwrap_or_else(|| (self.tab_model.len() as u16).saturating_sub(1));

                let entity = self.tab_model.entity_at(pos);
                if let Some(entity) = entity {
                    return self.update(Message::TabActivate(entity));
                }
            }
            Message::TabClose(entity_opt) => {
                self.preview_request.invalidate();
                self.navigation.invalidate();
                let mut tasks = Vec::with_capacity(2);

                let entity = entity_opt.unwrap_or_else(|| self.tab_model.active());

                // If the last tab is closed, close the window
                // Otherwise, activate closest item
                if self.tab_model.len() == 1 {
                    tasks.push(Task::future(async move {
                        cosmic::action::app(Message::WindowClose)
                    }));
                } else if entity == self.tab_model.active()
                    && let Some(position) = self.tab_model.position(entity)
                {
                    let new_position = if position > 0 {
                        position - 1
                    } else {
                        position + 1
                    };

                    if let Some(new_entity) = self.tab_model.entity_at(new_position) {
                        tasks.push(self.update(Message::TabActivate(new_entity)));
                    }
                }

                // Remove item
                self.tab_model.remove(entity);

                tasks.push(self.update_watcher());

                return Task::batch(tasks);
            }
            Message::TabConfig(config) => {
                if config != self.config.tab {
                    config_set!(tab, config);
                    return self.update_config();
                }
            }
            Message::ToggleFoldersFirst => {
                let mut config = self.config.tab;
                config.folders_first = !config.folders_first;
                return self.update(Message::TabConfig(config));
            }
            Message::ToggleShowHidden => {
                let mut config = self.config.tab;
                config.show_hidden = !config.show_hidden;
                return self.update(Message::TabConfig(config));
            }
            Message::TabMessage(entity_opt, tab_message) => {
                if matches!(
                    &tab_message,
                    tab::Message::EditLocation(_) | tab::Message::EditLocationEnable
                ) {
                    self.navigation.invalidate();
                }
                let entity = entity_opt.unwrap_or_else(|| self.tab_model.active());
                // The context menu opens on right-button release, so refresh paste availability now
                let right_click = matches!(tab_message, tab::Message::RightClick(..));

                let tab_commands = match self.tab_model.data_mut::<Tab>(entity) {
                    Some(tab) => tab.update(tab_message, self.modifiers),
                    _ => Vec::new(),
                };

                let mut commands = Vec::new();
                if right_click {
                    commands.push(self.update(Message::CheckClipboard));
                }
                for tab_command in tab_commands {
                    match tab_command {
                        tab::Command::Action(action) => {
                            commands.push(self.update(action.message(Some(entity))));
                        }
                        tab::Command::AddNetworkDrive => {
                            self.context_page = ContextPage::NetworkDrive;
                            self.set_show_context(true);
                        }
                        tab::Command::AddToSidebar(path) => {
                            let mut favorites = self.config.favorites.clone();
                            let favorite = Favorite::from_path(path);
                            let favorite_path = favorite.path_opt();
                            if !favorites.iter().any(|f| f.path_opt() == favorite_path) {
                                favorites.push(favorite);
                            }
                            config_set!(favorites, favorites);
                            commands.push(self.update_config());
                        }
                        tab::Command::AutoScroll(scroll_speed) => {
                            // converting an f32 to an i16 here by multiplying by 10 and casting to i16
                            // further resolution isn't necessary
                            if let Some(scroll_speed_float) = scroll_speed {
                                self.auto_scroll_speed = Some((scroll_speed_float * 10.0) as i16);
                            } else {
                                self.auto_scroll_speed = None;
                            }
                        }
                        tab::Command::ChangeLocation(tab_title, tab_path, selection_paths) => {
                            self.preview_request.invalidate();
                            self.navigation.invalidate();
                            self.activate_nav_model_location(&tab_path);

                            self.tab_model.text_set(entity, tab_title);
                            // clear the prefix selection buffer when changing location
                            self.type_select_prefix.clear();
                            commands.push(Task::batch([
                                self.update_title(),
                                self.update_watcher(),
                                self.update_tab(entity, tab_path, selection_paths),
                            ]));
                        }
                        tab::Command::EditLocationSubmit(edit_location, typed_uri) => {
                            let request = self.navigation.begin();
                            commands.push(Task::future(async move {
                                let target = tokio::task::spawn_blocking(move || {
                                    crate::navigation::resolve_edit_location(
                                        edit_location,
                                        typed_uri,
                                    )
                                })
                                .await
                                .ok()
                                .flatten();
                                cosmic::action::app(Message::NavigationResolved {
                                    entity,
                                    request,
                                    target,
                                })
                            }));
                        }
                        tab::Command::ParseDesktopEntry(path, work_id) => {
                            #[cfg(feature = "desktop")]
                            {
                                let lang_id = crate::localize::LANGUAGE_LOADER.current_language();
                                let language = lang_id.language.as_str().to_string();
                                commands.push(Task::future(async move {
                                    let parsed = tokio::task::spawn_blocking(move || {
                                        let actions = cosmic::desktop::load_desktop_file(
                                            &[language],
                                            path.clone(),
                                        )
                                        .map(menu::DesktopEntryActions::from_entry);
                                        (path, actions)
                                    })
                                    .await
                                    .ok();
                                    match parsed {
                                        Some((path, actions)) => {
                                            cosmic::action::app(Message::TabMessage(
                                                Some(entity),
                                                tab::Message::DesktopEntryParsed(
                                                    path, work_id, actions,
                                                ),
                                            ))
                                        }
                                        None => cosmic::action::none(),
                                    }
                                }));
                            }
                            #[cfg(not(feature = "desktop"))]
                            {
                                let _ = (path, work_id);
                            }
                        }
                        tab::Command::Surface(action) => {
                            // re-type the tab's surface action the way its messages are re-typed
                            let action = action
                                .map(move |message| Message::TabMessage(Some(entity), message));
                            commands.push(self.update(Message::Surface(action)));
                        }
                        tab::Command::Delete(paths) => commands.push(Self::delete(paths)),
                        tab::Command::DropFiles(to, from) => {
                            commands.push(self.update(Message::PasteContents(to, from)));
                        }
                        tab::Command::ClearRecents => {
                            Self::clear_recently_used();
                        }
                        tab::Command::EmptyTrash => {
                            return self.push_dialog(
                                DialogPage::EmptyTrash,
                                Some(EMPTY_TRASH_BUTTON_ID.clone()),
                            );
                        }
                        #[cfg(feature = "desktop")]
                        tab::Command::ExecEntryAction(command) => {
                            Self::exec_entry_action(&command);
                        }
                        tab::Command::RunContextAction(action) => {
                            let paths: Box<[_]> = self.selected_paths(Some(entity)).collect();
                            if let Some(preset) = self.config.context_actions.get(action) {
                                if preset.confirm {
                                    commands.push(self.push_dialog(
                                        DialogPage::RunContextAction { action, paths },
                                        Some(CONFIRM_CONTEXT_ACTION_BUTTON_ID.clone()),
                                    ));
                                } else {
                                    context_action::run(
                                        &self.config.context_actions,
                                        action,
                                        &paths,
                                    );
                                }
                            } else {
                                log::warn!("invalid context action index `{action}`");
                            }
                        }
                        tab::Command::Iced(iced_command) => {
                            commands.push(iced_command.0.map(move |x| {
                                cosmic::action::app(Message::TabMessage(Some(entity), x))
                            }));
                        }
                        tab::Command::OpenFile(paths) => commands.push(Self::open_file(&paths)),
                        tab::Command::OpenInNewTab(path) => {
                            commands.push(self.open_tab(Location::Path(path), false, None));
                        }
                        tab::Command::OpenInNewWindow(path) => match env::current_exe() {
                            Ok(exe) => match process::Command::new(&exe).arg(path).spawn() {
                                Ok(_child) => {}
                                Err(err) => {
                                    log::error!("failed to execute {}: {}", exe.display(), err);
                                }
                            },
                            Err(err) => {
                                log::error!("failed to get current executable path: {err}");
                            }
                        },
                        tab::Command::OpenTrash => {
                            //TODO: use handler for x-scheme-handler/trash and open trash:///
                            let mut command = process::Command::new("cosmic-files");
                            command.arg("--trash");
                            match spawn_detached(&mut command) {
                                Ok(()) => {}
                                Err(err) => {
                                    log::warn!("failed to run cosmic-files --trash: {err}");
                                }
                            }
                        }
                        tab::Command::Preview(kind) => {
                            self.preview_request.invalidate();
                            self.context_page = ContextPage::Preview(Some(entity), kind);
                            self.set_show_context(true);
                        }
                        tab::Command::PreviewPath(path) => {
                            let request = self.preview_request.begin();
                            // A breadcrumb ancestor can live on a slow mount, so build its item
                            // off-thread and show it in Message::PreviewReady.
                            commands.push(Self::item_from_path_task(path, move |_, item| {
                                Message::PreviewReady(request, Some(entity), item)
                            }));
                        }
                        tab::Command::SetOpenWith(mime, id) => {
                            self.mime_cache_request.invalidate();
                            let completion = crate::mime_app::queue_default(mime, id);
                            commands.push(Task::future(async move {
                                match completion.await {
                                    Ok(Ok(())) => {}
                                    Ok(Err(err)) => {
                                        log::warn!("failed to set default handler: {err}")
                                    }
                                    Err(err) => log::warn!("failed to set default handler: {err}"),
                                }
                                cosmic::action::app(Message::ReloadMimeAppCache)
                            }));
                        }

                        tab::Command::SetPermissions(path, mode) => {
                            commands.push(self.operation(Operation::SetPermissions { path, mode }));
                        }
                        tab::Command::SetMultiplePermissions(permissions) => {
                            commands.push(
                                self.join_operations(
                                    permissions
                                        .into_iter()
                                        .map(|(path, mode)| Operation::SetPermissions {
                                            path,
                                            mode,
                                        })
                                        .collect(),
                                ),
                            );
                        }
                        tab::Command::WindowDrag => {
                            if let Some(window_id) = self.core.main_window_id() {
                                commands.push(window::drag(window_id));
                            }
                        }
                        tab::Command::WindowToggleMaximize => {
                            if let Some(window_id) = self.core.main_window_id() {
                                commands.push(window::toggle_maximize(window_id));
                            }
                        }
                        tab::Command::SetSort(location, heading_options, direction) => {
                            let default_sort = tab::SORT_OPTION_FALLBACK
                                .get(&location)
                                .copied()
                                .unwrap_or((HeadingOptions::Name, true));
                            let changed = if default_sort == (heading_options, direction) {
                                self.state.sort_names.remove(&location).is_some()
                            } else {
                                // force reordering of inserted values so new settings are not dropped in the truncation step
                                _ = self.state.sort_names.remove(&location);
                                _ = self
                                    .state
                                    .sort_names
                                    .insert(location, (heading_options, direction))
                                    .is_none_or(|old| old != (heading_options, direction));

                                const MAX_SORT_NAMES: usize = 999;
                                // TODO potentially configurable limit on max size?
                                if self.state.sort_names.len() > MAX_SORT_NAMES {
                                    // truncate is not a good fit because it drops the items at the end, which are newest...
                                    self.state.sort_names = self
                                        .state
                                        .sort_names
                                        .split_off(self.state.sort_names.len() - MAX_SORT_NAMES);
                                }

                                true
                            };

                            if !self.must_save_sort_names & changed {
                                self.must_save_sort_names = true;
                                return cosmic::Task::future(async move {
                                    tokio::time::sleep(Duration::from_secs(1)).await;
                                    cosmic::action::app(Message::SaveSortNames)
                                });
                            }
                        }
                    }
                }
                return Task::batch(commands);
            }
            Message::TabNew => {
                let active = self.tab_model.active();
                let location = match self.tab_model.data::<Tab>(active) {
                    Some(tab) => tab.location.clone(),
                    None => Location::Path(home_dir()),
                };
                return self.open_tab(location, true, None);
            }
            Message::TabRescanFailed(entity, request) => {
                let Some(tab) = self.tab_model.data_mut::<Tab>(entity) else {
                    return Task::none();
                };
                if tab.scans.finish(request) == Some(true) {
                    let location = tab.location.clone();
                    return self
                        .rescan_tab_if_idle(entity, location)
                        .unwrap_or_else(Task::none);
                }
            }
            Message::TabRescan(
                entity,
                request,
                mut location,
                parent_item_opt,
                items,
                selection_paths,
            ) => {
                location = location.normalize();
                let selection_paths_present = selection_paths.is_some();

                let mut matched = false;
                let mut rescan_pending = false;
                let mut current_location = None;

                if let Some(tab) = self.tab_model.data_mut::<Tab>(entity) {
                    let Some(pending) = tab.scans.finish(request) else {
                        return Task::none();
                    };
                    rescan_pending = pending;
                    tab.location = tab.location.normalize();
                    matched = location == tab.location;
                    current_location = Some(tab.location.clone());

                    if matched {
                        tab.parent_item_opt = parent_item_opt;
                        tab.set_items(items);
                        let location_str = location.to_string();
                        let sort = self
                            .state
                            .sort_names
                            .get(&location_str)
                            .or_else(|| SORT_OPTION_FALLBACK.get(&location_str))
                            .unwrap_or(&(HeadingOptions::Name, true));

                        tab.sort_name = sort.0;
                        tab.sort_direction = sort.1;

                        if let Some(selection_paths) = selection_paths {
                            tab.select_paths(selection_paths);
                        }
                    }
                }

                if !matched {
                    // Stale rescan superseded by a location change in the meantime; if a
                    // watcher event still wants a rescan, run it for wherever the tab is now.
                    return match current_location {
                        Some(location) if rescan_pending => self
                            .rescan_tab_if_idle(entity, location)
                            .unwrap_or_else(Task::none),
                        _ => Task::none(),
                    };
                }

                let mut tasks = Vec::with_capacity(3);

                if selection_paths_present {
                    // Ensure selected path is scrolled to after redraw
                    tasks.push(Task::done(cosmic::action::app(Message::TabMessage(
                        Some(entity),
                        tab::Message::ScrollToFocused,
                    ))));
                }

                tasks.push(clipboard::read_data::<ClipboardPaste>().map(|p| {
                    cosmic::action::app(Message::CutPaths(match p {
                        Some(s) => match s.kind {
                            ClipboardKind::Copy => Vec::new(),
                            ClipboardKind::Cut { .. } => s.paths,
                        },
                        None => Vec::new(),
                    }))
                }));

                if rescan_pending && let Some(location) = current_location {
                    tasks.extend(self.rescan_tab_if_idle(entity, location));
                }

                return Task::batch(tasks);
            }
            Message::TabView(entity_opt, view) => {
                let entity = entity_opt.unwrap_or_else(|| self.tab_model.active());
                if let Some(tab) = self.tab_model.data_mut::<Tab>(entity) {
                    if matches!(tab.mode, tab::Mode::Desktop) {
                        return Task::none();
                    }

                    tab.config.view = view;
                }
                let mut config = self.config.tab;
                config.view = view;
                return self.update(Message::TabConfig(config));
            }
            Message::CutPaths(paths) => {
                if let Some(tab) = self.tab_model.active_data_mut::<Tab>() {
                    tab.refresh_cut(&paths);
                }
            }
            Message::TimeConfigChange(time_config) => {
                self.config.tab.military_time = time_config.military_time;
                return self.update_config();
            }
            Message::ToggleContextPage(context_page) => {
                self.preview_request.invalidate();
                //TODO: ensure context menus are closed
                if self.context_page == context_page
                    || matches!(self.context_page, ContextPage::Preview(_, _))
                {
                    self.set_show_context(!self.core.window.show_context);
                } else {
                    self.set_show_context(true);
                }
                self.context_page = context_page;
                // Preview status is preserved across restarts
                if matches!(self.context_page, ContextPage::Preview(_, _)) {
                    return cosmic::task::message(cosmic::action::app(Message::SetShowDetails(
                        self.core.window.show_context,
                    )));
                }
            }
            Message::Undo(_id) => {
                // TODO: undo
            }
            Message::UndoTrash(id, recently_trashed) => {
                self.toasts.remove(id);

                let mut paths = Vec::with_capacity(recently_trashed.len());
                let icon_sizes = self.config.tab.icon_sizes;

                return cosmic::task::future(async move {
                    match tokio::task::spawn_blocking(move || Location::Trash.scan(icon_sizes))
                        .await
                    {
                        Ok((_parent_item_opt, items)) => {
                            for path in &*recently_trashed {
                                for item in &items {
                                    if let ItemMetadata::Trash { ref entry, .. } = item.metadata {
                                        let original_path = entry.original_path();
                                        if &original_path == path {
                                            paths.push(entry.clone());
                                        }
                                    }
                                }
                            }
                        }
                        Err(err) => {
                            log::warn!("failed to rescan: {err}");
                        }
                    }

                    Message::UndoTrashStart(paths)
                });
            }
            Message::UndoTrashStart(items) => {
                return self.operation(Operation::Restore { items });
            }
            Message::WindowClose => {
                if let Some(window_id) = self.core.main_window_id() {
                    self.core.set_main_window_id(None);
                    return Task::batch([
                        window::close(window_id),
                        Task::future(async move { cosmic::action::app(Message::MaybeExit) }),
                    ]);
                }
            }
            Message::WindowCloseRequested(id) => {
                self.remove_window(&id);
            }
            Message::WindowMaximize(id, maximized) => {
                return window::maximize(id, maximized);
            }
            Message::WindowNew => match env::current_exe() {
                Ok(exe) => {
                    // initialize command to spawn another instance of this application
                    let mut command = process::Command::new(&exe);

                    // make the new window open at the same location as the currently active tab by
                    // passing respective command line arguments
                    let entity = self.tab_model.active();
                    let active_tab_location =
                        self.tab_model.data::<Tab>(entity).map(|tab| &tab.location);
                    match active_tab_location {
                        Some(
                            Location::Desktop(path, ..)
                            | Location::Path(path)
                            | Location::Search(SearchLocation::Path(path), ..),
                        ) => {
                            command.arg(path);
                        }
                        Some(Location::Network(uri, ..)) => {
                            command.arg(uri);
                        }
                        Some(Location::Recents | Location::Search(SearchLocation::Recents, ..)) => {
                            command.arg("--recents");
                        }
                        Some(Location::Trash | Location::Search(SearchLocation::Trash, ..)) => {
                            command.arg("--trash");
                        }
                        None => {}
                    };

                    // spawn the new window
                    match command.spawn() {
                        Ok(_child) => {}
                        Err(err) => {
                            log::error!("failed to execute {}: {}", exe.display(), err);
                        }
                    }
                }
                Err(err) => {
                    log::error!("failed to get current executable path: {err}");
                }
            },
            Message::ZoomDefault(entity_opt) => {
                let entity = entity_opt.unwrap_or_else(|| self.tab_model.active());
                let mut config = self.config.tab;
                if let Some(tab) = self.tab_model.data::<Tab>(entity) {
                    zoom_to_default(tab.config.view, &mut config.icon_sizes);
                }
                return self.update(Message::TabConfig(config));
            }
            Message::ZoomIn(entity_opt) => {
                let entity = entity_opt.unwrap_or_else(|| self.tab_model.active());
                let mut config = self.config.tab;
                if let Some(tab) = self.tab_model.data::<Tab>(entity) {
                    zoom_in_view(tab.config.view, &mut config.icon_sizes);
                }
                return self.update(Message::TabConfig(config));
            }
            Message::ZoomOut(entity_opt) => {
                let entity = entity_opt.unwrap_or_else(|| self.tab_model.active());
                let mut config = self.config.tab;
                if let Some(tab) = self.tab_model.data::<Tab>(entity) {
                    zoom_out_view(tab.config.view, &mut config.icon_sizes);
                }
                return self.update(Message::TabConfig(config));
            }
            Message::DndEnterNav(entity) => {
                if let Some(location) = self.nav_model.data::<Location>(entity) {
                    self.nav_dnd_hover = Some((location.clone(), Instant::now()));
                    let location = location.clone();
                    return Task::perform(tokio::time::sleep(HOVER_DURATION), move |()| {
                        cosmic::Action::App(Message::DndHoverLocTimeout(location.clone()))
                    });
                }
            }
            Message::DndExitNav => {
                self.nav_dnd_hover = None;
            }
            Message::DndDropNav(entity, data, action) => {
                self.nav_dnd_hover = None;
                if let Some((location, data)) = self.nav_model.data::<Location>(entity).zip(data) {
                    let kind = match action {
                        DndAction::Move => ClipboardKind::Cut { is_dnd: true },
                        _ => ClipboardKind::Copy,
                    };
                    let ret = match location {
                        Location::Path(p) => self.update(Message::PasteContents(
                            p.clone(),
                            ClipboardPaste {
                                kind,
                                paths: data.paths,
                            },
                        )),
                        Location::Trash if matches!(action, DndAction::Move) => {
                            Self::delete(data.paths)
                        }
                        _ => {
                            log::warn!("Copy to trash is not supported.");
                            Task::none()
                        }
                    };
                    return ret;
                }
            }
            Message::DndHoverLocTimeout(location) => {
                if self
                    .nav_dnd_hover
                    .as_ref()
                    .is_some_and(|(loc, i)| *loc == location && i.elapsed() >= HOVER_DURATION)
                {
                    self.nav_dnd_hover = None;
                    self.navigation.invalidate();
                    let entity = self.tab_model.active();
                    let title_opt = match self.tab_model.data_mut::<Tab>(entity) {
                        Some(tab) => {
                            tab.change_location(&location, None);
                            Some(tab.title())
                        }
                        None => None,
                    };
                    if let Some(title) = title_opt {
                        self.tab_model.text_set(entity, title);
                        return Task::batch([
                            self.update_title(),
                            self.update_watcher(),
                            self.update_tab(entity, location, None),
                        ]);
                    }
                }
            }
            Message::DndEnterTab(entity, mimes) => {
                if mimes.iter().all(|m| m.as_str() != "x-cosmic-files/tab-dnd") {
                    self.tab_dnd_hover = Some((entity, Instant::now()));
                    return Task::perform(tokio::time::sleep(HOVER_DURATION), move |()| {
                        cosmic::Action::App(Message::DndHoverTabTimeout(entity))
                    });
                }
            }
            Message::DndExitTab => {
                self.nav_dnd_hover = None;
            }
            Message::DndDropTab(entity, data, action) => {
                self.nav_dnd_hover = None;
                if let Some((tab, data)) = self.tab_model.data::<Tab>(entity).zip(data) {
                    let kind = match action {
                        DndAction::Move => ClipboardKind::Cut { is_dnd: true },
                        _ => ClipboardKind::Copy,
                    };
                    let ret = match &tab.location {
                        Location::Trash if matches!(action, DndAction::Move) => {
                            Self::delete(data.paths)
                        }
                        _ => {
                            if let Some(path) = tab.location.path_opt() {
                                self.update(Message::PasteContents(
                                    path.clone(),
                                    ClipboardPaste {
                                        kind,
                                        paths: data.paths,
                                    },
                                ))
                            } else {
                                log::warn!("{:?} to {:?} is not supported.", action, tab.location);
                                Task::none()
                            }
                        }
                    };
                    return ret;
                }
            }
            Message::DndHoverTabTimeout(entity) => {
                if self
                    .tab_dnd_hover
                    .as_ref()
                    .is_some_and(|(e, i)| *e == entity && i.elapsed() >= HOVER_DURATION)
                {
                    self.tab_dnd_hover = None;
                    return self.update(Message::TabActivate(entity));
                }
            }
            Message::NavBarClose(entity) => {
                if let Some(data) = self.nav_model.data::<MounterData>(entity)
                    && let Some(mounter) = MOUNTERS.get(&data.0)
                {
                    return mounter
                        .unmount(data.1.clone())
                        .map(|()| cosmic::action::none());
                }
            }
            Message::NavBarContext(entity) => {
                self.nav_bar_context_id = entity;

                self.nav_context_info = None;
                let request = self.nav_context_request.begin();
                let location_opt = self.nav_model.data::<Location>(entity).cloned();
                let path_opt = location_opt
                    .as_ref()
                    .and_then(|location| location.path_opt().cloned());
                let is_recents = matches!(location_opt, Some(Location::Recents));
                let info_task = Task::future(async move {
                    let info = tokio::task::spawn_blocking(move || {
                        let (is_file, is_dir) = path_opt
                            .as_ref()
                            .map_or((false, false), |path| (path.is_file(), path.is_dir()));
                        NavContextInfo {
                            is_file,
                            is_dir,
                            has_recents: is_recents && tab::has_recents(),
                        }
                    })
                    .await
                    .unwrap_or_default();
                    cosmic::action::app(Message::NavContextInfoLoaded(entity, request, info))
                });

                let tab_entity = self.tab_model.active();
                if let Some(tab) = self.tab_model.data_mut::<Tab>(tab_entity) {
                    // Close location editing if enabled
                    tab.edit_location = None;
                }
                return info_task;
            }
            Message::NavContextInfoLoaded(entity, request, info) => {
                if self.nav_context_request.finish(&request) && entity == self.nav_bar_context_id {
                    self.nav_context_info = Some((entity, info));
                }
            }
            Message::NavigationResolved {
                entity,
                request,
                target,
            } => {
                if !self.navigation.finish(&request) {
                    return Task::none();
                }
                if let Some((location, selected)) = target {
                    let message = match selected {
                        Some(path) => tab::Message::LocationSelect(location, vec![path]),
                        None => tab::Message::Location(location),
                    };
                    return self.update(Message::TabMessage(Some(entity), message));
                }
            }
            Message::NavSelectChecked {
                entity,
                tab_entity,
                request,
                exists,
                is_dir,
            } => {
                if !self.navigation.is_current(&request)
                    || self.tab_model.data::<Tab>(tab_entity).is_none()
                {
                    return Task::none();
                }
                let Some(location) = self.nav_model.data::<Location>(entity).cloned() else {
                    return Task::none();
                };

                // A gvfs network favorite that isn't present: try to mount it, else report it.
                #[cfg(feature = "gvfs")]
                if let Location::Network(uri, name, Some(path)) = &location
                    && !exists
                {
                    let mut found = false;
                    if let Some(key) = self
                        .mounter_items
                        .iter()
                        .find_map(|(k, items)| {
                            items.iter().find_map(|item| {
                                found |= item.path().is_some_and(|p| path.starts_with(&p))
                                    || item.name() == *name
                                    || item.uri() == *uri;
                                (!item.is_mounted() && found).then_some(*k)
                            })
                        })
                        .or(if found {
                            None
                        } else {
                            // TODO do we need to choose the correct mounter?
                            self.mounter_items.keys().copied().next()
                        })
                        && let Some(mounter) = MOUNTERS.get(&key)
                    {
                        return mounter.network_drive(uri.clone()).map(move |mounted| {
                            if mounted {
                                cosmic::Action::App(Message::NavigationMounted {
                                    entity,
                                    tab_entity,
                                    request: request.clone(),
                                })
                            } else {
                                cosmic::action::none()
                            }
                        });
                    }

                    log::warn!(
                        "failed to open favorite, path does not exist: {}",
                        path.display()
                    );
                    return self.push_dialog(
                        DialogPage::FavoritePathError {
                            path: path.clone(),
                            entity,
                        },
                        Some(FAVORITE_PATH_ERROR_REMOVE_BUTTON_ID.clone()),
                    );
                }

                self.navigation.finish(&request);

                // Other path-bearing favorites: report if missing, otherwise open — a file
                // (local or on a mounted share) launches, a directory is navigated to.
                if let Some(path) = location.path_opt().cloned() {
                    if !exists {
                        log::warn!(
                            "failed to open favorite, path does not exist: {}",
                            path.display()
                        );
                        return self.push_dialog(
                            DialogPage::FavoritePathError { path, entity },
                            Some(FAVORITE_PATH_ERROR_REMOVE_BUTTON_ID.clone()),
                        );
                    }
                    if !is_dir {
                        return Self::open_file(&[path]);
                    }
                }

                return self.update(Message::TabMessage(
                    Some(tab_entity),
                    tab::Message::Location(location),
                ));
            }
            Message::NavMenuOpenWith(path, item) => {
                return self.push_dialog(
                    DialogPage::OpenWith {
                        path,
                        mime: item.mime,
                        selected: 0,
                        store_opt: "x-scheme-handler/mime"
                            .parse::<mime_guess::Mime>()
                            .ok()
                            .and_then(|mime| self.mime_app_cache.get(&mime).first().cloned()),
                        search_app_name: String::new(),
                    },
                    None,
                );
            }
            Message::PreviewReady(request, entity_opt, item) => {
                if !self.preview_request.finish(&request) {
                    return Task::none();
                }
                self.context_page =
                    ContextPage::Preview(entity_opt, PreviewKind::Custom(PreviewItem(item)));
                self.set_show_context(true);
            }
            Message::NavMenuAction(action) => match action {
                NavMenuAction::ClearRecents => Self::clear_recently_used(),
                NavMenuAction::EmptyTrash => {
                    return self
                        .push_dialog(DialogPage::EmptyTrash, Some(EMPTY_TRASH_BUTTON_ID.clone()));
                }
                NavMenuAction::Open(entity) => {
                    if let Some(path) = self
                        .nav_model
                        .data::<Location>(entity)
                        .and_then(Location::path_opt)
                        .cloned()
                    {
                        return Self::open_file(&[path]);
                    }
                }
                NavMenuAction::OpenWith(entity) => {
                    if let Some(path) = self
                        .nav_model
                        .data::<Location>(entity)
                        .and_then(Location::path_opt)
                        .cloned()
                    {
                        return Self::item_from_path_task(path, Message::NavMenuOpenWith);
                    }
                }
                NavMenuAction::RunContextAction(entity, action) => {
                    if let Some(path) = self
                        .nav_model
                        .data::<Location>(entity)
                        .and_then(Location::path_opt)
                        .cloned()
                    {
                        let paths = vec![path];
                        if let Some(preset) = self.config.context_actions.get(action) {
                            if preset.confirm {
                                return self.push_dialog(
                                    DialogPage::RunContextAction {
                                        action,
                                        paths: paths.into_boxed_slice(),
                                    },
                                    Some(CONFIRM_CONTEXT_ACTION_BUTTON_ID.clone()),
                                );
                            }
                            context_action::run(&self.config.context_actions, action, &paths);
                        } else {
                            log::warn!("invalid context action index `{action}`");
                        }
                    }
                }
                NavMenuAction::OpenInNewTab(entity) => {
                    let open_task = match self.nav_model.data::<Location>(entity) {
                        Some(Location::Network(uri, display_name, path)) => self.open_tab(
                            Location::Network(uri.clone(), display_name.clone(), path.clone()),
                            false,
                            None,
                        ),
                        Some(Location::Path(path)) => {
                            self.open_tab(Location::Path(path.clone()), false, None)
                        }
                        Some(Location::Recents) => self.open_tab(Location::Recents, false, None),
                        Some(Location::Trash) => self.open_tab(Location::Trash, false, None),
                        _ => Task::none(),
                    };

                    return open_task;
                }

                // Open the selected path in a new cosmic-files window.
                NavMenuAction::OpenInNewWindow(entity) => 'open_in_new_window: {
                    if let Some(location) = self.nav_model.data::<Location>(entity) {
                        match env::current_exe() {
                            Ok(exe) => {
                                let mut command = process::Command::new(&exe);
                                match location {
                                    Location::Path(path) => {
                                        command.arg(path);
                                    }
                                    Location::Trash => {
                                        command.arg("--trash");
                                    }
                                    Location::Network(uri, _, Some(_)) => {
                                        command.arg(uri);
                                    }
                                    Location::Network(..) => {
                                        command.arg("--network");
                                    }
                                    Location::Recents => {
                                        command.arg("--recents");
                                    }
                                    _ => {
                                        log::error!(
                                            "unsupported location for open in new window: {location:?}"
                                        );
                                        break 'open_in_new_window;
                                    }
                                }
                                match command.spawn() {
                                    Ok(_child) => {}
                                    Err(err) => {
                                        log::error!("failed to execute {}: {}", exe.display(), err);
                                    }
                                }
                            }
                            Err(err) => {
                                log::error!("failed to get current executable path: {err}");
                            }
                        }
                    }
                }

                NavMenuAction::Preview(entity) => {
                    if let Some(path) = self
                        .nav_model
                        .data::<Location>(entity)
                        .and_then(Location::path_opt)
                        .cloned()
                    {
                        // A dead or slow network favorite would freeze the GUI, so build its
                        // item off-thread and show it in Message::PreviewReady.
                        let request = self.preview_request.begin();
                        return Self::item_from_path_task(path, move |_, item| {
                            Message::PreviewReady(request, None, item)
                        });
                    }
                }

                NavMenuAction::RemoveFromSidebar(entity) => {
                    if let Some(FavoriteIndex(favorite_i)) =
                        self.nav_model.data::<FavoriteIndex>(entity)
                    {
                        let mut favorites = self.config.favorites.clone();
                        favorites.remove(*favorite_i);
                        config_set!(favorites, favorites);
                        return self.update_config();
                    }
                }

                NavMenuAction::ChangeSidebarLabel(entity) => {
                    if let Some(favorite) = self.nav_model.data::<FavoriteIndex>(entity).and_then(
                        |FavoriteIndex(favorite_i)| self.config.favorites.get(*favorite_i),
                    ) {
                        let label = favorite.display_name().unwrap_or_else(|| fl!("filesystem"));
                        return Task::batch([
                            self.dialog_pages
                                .push_back(DialogPage::ChangeSidebarLabel { entity, label }),
                            widget::text_input::focus(self.dialog_text_input.clone()),
                            widget::text_input::select_all(self.dialog_text_input.clone()),
                        ]);
                    }
                }
            },
            Message::Recents => {
                if self.config.show_recents {
                    return self.open_tab(Location::Recents, false, None);
                }
            }
            #[cfg(all(feature = "wayland", feature = "desktop-applet"))]
            Message::OutputEvent(output_event, output) => {
                match output_event {
                    OutputEvent::Created(output_info_opt) => {
                        let output_id = output.id();
                        log::info!("output {output_id}: created");

                        let surface_id = WindowId::unique();
                        if let Some(old_surface_id) =
                            self.surface_ids.insert(output.clone(), surface_id)
                        {
                            //TODO: remove old surface?
                            log::warn!(
                                "output {output_id}: already had surface ID {old_surface_id:?}"
                            );
                        }

                        let display = match output_info_opt {
                            Some(output_info) => match output_info.name {
                                Some(output_name) => {
                                    self.surface_names.insert(surface_id, output_name.clone());
                                    output_name
                                }
                                None => {
                                    log::warn!("output {output_id}: no output name");
                                    String::new()
                                }
                            },
                            None => {
                                log::warn!("output {output_id}: no output info");
                                String::new()
                            }
                        };

                        let (entity, command) = self.open_tab_entity(
                            Location::Desktop(crate::desktop_dir(), display, self.config.desktop),
                            false,
                            None,
                            widget::Id::unique(),
                            Some(surface_id),
                        );
                        self.windows
                            .insert(surface_id, Window::new(WindowKind::Desktop(entity)));
                        return Task::batch([
                            command,
                            cosmic::task::message(cosmic::Action::Surface(
                                cosmic::surface::action::app_layer_shell(
                                    |_| Default::default(),
                                    move |_: &mut App| SctkLayerSurfaceSettings {
                                        id: surface_id,
                                        layer: Layer::Bottom,
                                        keyboard_interactivity: KeyboardInteractivity::OnDemand,
                                        input_zone: None,
                                        anchor: Anchor::TOP
                                            | Anchor::BOTTOM
                                            | Anchor::LEFT
                                            | Anchor::RIGHT,
                                        output: IcedOutput::Output(output.clone()),
                                        namespace: "cosmic-files-applet".into(),
                                        size: Some((None, None)),
                                        margin: IcedMargin {
                                            top: 0,
                                            bottom: 0,
                                            left: 0,
                                            right: 0,
                                        },
                                        exclusive_zone: 0,
                                        size_limits: Limits::NONE.min_width(1.0).min_height(1.0),
                                    },
                                    None,
                                ),
                            )),
                            #[cfg(all(feature = "wayland", feature = "desktop-applet"))]
                            overlap_notify(surface_id, true),
                        ]);
                    }
                    OutputEvent::Removed => {
                        log::info!("output {}: removed", output.id());
                        match self.surface_ids.remove(&output) {
                            Some(surface_id) => {
                                self.remove_window(&surface_id);
                                self.surface_names.remove(&surface_id);
                                return destroy_layer_surface(surface_id);
                            }
                            None => {
                                log::warn!("output {}: no surface found", output.id());
                            }
                        }
                    }
                    OutputEvent::InfoUpdate(_output_info) => {
                        log::info!("output {}: info update", output.id());
                    }
                }
            }
            Message::Cosmic(cosmic) => {
                // Forward cosmic messages
                return Task::perform(async move { cosmic }, cosmic::action::cosmic);
            }
            Message::None => {}
            #[cfg(all(feature = "wayland", feature = "desktop-applet"))]
            Message::Overlap(w_id, overlap_notify_event) => match overlap_notify_event {
                OverlapNotifyEvent::OverlapLayerAdd {
                    identifier,
                    namespace,
                    logical_rect,
                    exclusive,
                    ..
                } => {
                    if exclusive > 0 || namespace == "Dock" || namespace == "Panel" {
                        self.overlap.insert(identifier, (w_id, logical_rect));
                        self.handle_overlap();
                    }
                }
                OverlapNotifyEvent::OverlapLayerRemove { identifier } => {
                    self.overlap.remove(&identifier);
                    self.handle_overlap();
                }
                _ => {}
            },
            Message::Size(window_id, size) => {
                if self.core.main_window_id() == Some(window_id) {
                    self.size = Some(size);
                } else {
                    #[cfg(all(feature = "wayland", feature = "desktop-applet"))]
                    self.layer_sizes.insert(window_id, size);
                }
            }
            Message::Eject => {
                #[cfg(feature = "gvfs")]
                {
                    let mut paths = self.selected_paths(None);
                    if let Some(p) = paths.next() {
                        {
                            for (k, mounter_items) in &self.mounter_items {
                                if let Some(mounter) = MOUNTERS.get(k)
                                    && let Some(item) = mounter_items
                                        .iter()
                                        .find(|&item| item.path().is_some_and(|path| path == p))
                                {
                                    return mounter
                                        .unmount(item.clone())
                                        .map(|()| cosmic::action::none());
                                }
                            }
                        }
                    }
                }
            }
            #[cfg(all(feature = "wayland", feature = "desktop-applet"))]
            Message::Focused(id) => {
                if let Some(w) = self.windows.get(&id) {
                    match &w.kind {
                        WindowKind::Desktop(entity) => self.tab_model.activate(*entity),
                        _ => {}
                    };
                }
                // Check clipboard when window gains focus
                // HACK: Wait a moment for the data to be available.
                return cosmic::task::future(async {
                    _ = tokio::time::sleep(Duration::from_millis(300)).await;
                    cosmic::action::app(Message::CheckClipboard)
                });
            }
            Message::Surface(action) => {
                return cosmic::task::message(cosmic::Action::Surface(action));
            }
            Message::SaveSortNames => {
                self.must_save_sort_names = false;
                if let Some(state_handler) = self.state_handler.as_ref()
                    && let Err(err) = state_handler
                        .set::<&FxOrderMap<String, (HeadingOptions, bool)>>(
                            "sort_names",
                            &self.state.sort_names,
                        )
                {
                    log::warn!("Failed to save sort names: {err:?}");
                }
            }
            Message::NavigationMounted {
                entity,
                tab_entity,
                request,
            } => {
                if !self.navigation.finish(&request)
                    || self.tab_model.active() != tab_entity
                    || self.tab_model.data::<Tab>(tab_entity).is_none()
                {
                    return Task::none();
                }
                return self.on_nav_select(entity);
            }
            Message::NetworkDriveOpenEntityAfterMount { entity } => {
                return self.on_nav_select(entity);
            }
            Message::NetworkDriveOpenTabAfterMount { location } => {
                return self.open_tab(location, false, None);
            }
            Message::ReorderTab(ReorderEvent {
                dragged,
                target,
                position,
            }) => {
                _ = self.tab_model.reorder(dragged, target, position);
            }
        }

        Task::none()
    }

    fn context_drawer(&self) -> Option<context_drawer::ContextDrawer<'_, Message>> {
        if !self.core.window.show_context {
            return None;
        }

        Some(match &self.context_page {
            ContextPage::About => context_drawer::about(
                &self.about,
                |url| Message::LaunchUrl(url.to_string()),
                Message::ToggleContextPage(ContextPage::About),
            ),
            ContextPage::EditHistory => context_drawer::context_drawer(
                self.edit_history(),
                Message::ToggleContextPage(ContextPage::EditHistory),
            )
            .title(fl!("edit-history")),
            ContextPage::NetworkDrive => {
                let mut text_input =
                    widget::text_input(fl!("enter-server-address"), &self.network_drive_input);
                let button = if self.network_drive_connecting.is_some() {
                    widget::button::standard(fl!("connecting"))
                } else {
                    text_input = text_input
                        .on_input(Message::NetworkDriveInput)
                        .on_submit(|_| Message::NetworkDriveSubmit);
                    widget::button::standard(fl!("connect")).on_press(Message::NetworkDriveSubmit)
                };
                context_drawer::context_drawer(
                    self.network_drive(),
                    Message::ToggleContextPage(ContextPage::NetworkDrive),
                )
                .title(fl!("add-network-drive"))
                .header(text_input)
                .footer(widget::row::with_children([
                    widget::space::horizontal().into(),
                    button.into(),
                ]))
            }
            ContextPage::Preview(entity_opt, kind) => {
                let entity = entity_opt.unwrap_or_else(|| self.tab_model.active());
                let actions = self
                    .tab_model
                    .data::<Tab>(entity)
                    .and_then(|tab| {
                        let mut selected = tab.items_opt()?.iter().filter(|item| item.selected);

                        match (selected.next(), selected.next()) {
                            // Exactly one item
                            (Some(item), None) => Some(
                                item.preview_actions()
                                    .map(move |x| Message::TabMessage(Some(entity), x)),
                            ),
                            // Zero or more than one item
                            _ => None,
                        }
                    })
                    .unwrap_or_else(|| widget::space::horizontal().into());
                context_drawer::context_drawer(
                    self.preview(entity_opt, kind, true)
                        .map(move |x| Message::TabMessage(Some(entity), x)),
                    Message::ToggleContextPage(ContextPage::Preview(Some(entity), kind.clone())),
                )
                .actions(actions)
            }
            ContextPage::Settings => context_drawer::context_drawer(
                self.settings(),
                Message::ToggleContextPage(ContextPage::Settings),
            )
            .title(fl!("settings")),
        })
    }

    fn dialog(&self) -> Option<Element<'_, Message>> {
        //TODO: should gallery view just be a dialog?
        let entity = self.tab_model.active();
        if let Some(tab) = self.tab_model.data::<Tab>(entity)
            && tab.gallery
        {
            return Some(
                tab.gallery_view()
                    .map(move |x| Message::TabMessage(Some(entity), x)),
            );
        }
        let dialog_page = self.dialog_pages.front()?;

        let cosmic_theme::Spacing {
            space_xxs, space_s, ..
        } = theme::spacing();

        let dialog = match dialog_page {
            DialogPage::Compress {
                paths,
                to,
                name,
                archive_type,
                password,
            } => {
                let mut dialog = widget::dialog().title(fl!("create-archive"));

                let complete_maybe = if name.is_empty() {
                    None
                } else if name == "." || name == ".." {
                    dialog = dialog.tertiary_action(widget::text::body(fl!(
                        "name-invalid",
                        filename = name.as_str()
                    )));
                    None
                } else if name.contains('/') {
                    dialog = dialog.tertiary_action(widget::text::body(fl!("name-no-slashes")));
                    None
                } else {
                    let extension = archive_type.extension();
                    let name = format!("{name}{extension}");
                    if self.existing_entry_in_dir(to, &name).is_some() {
                        dialog =
                            dialog.tertiary_action(widget::text::body(fl!("file-already-exists")));
                        None
                    } else {
                        if name.starts_with('.') {
                            dialog = dialog.tertiary_action(widget::text::body(fl!("name-hidden")));
                        }
                        Some(Message::DialogComplete)
                    }
                };

                let archive_types = ArchiveType::all();
                let selected = archive_types.iter().position(|&x| x == *archive_type);
                dialog = dialog
                    .primary_action(
                        widget::button::suggested(fl!("create"))
                            .on_press_maybe(complete_maybe.clone()),
                    )
                    .secondary_action(
                        widget::button::standard(fl!("cancel")).on_press(Message::DialogCancel),
                    )
                    .control(
                        widget::column::with_children([
                            widget::text::body(fl!("file-name")).into(),
                            widget::row::with_children([
                                widget::text_input("", name.as_str())
                                    .id(self.dialog_text_input.clone())
                                    .on_input(move |name| {
                                        Message::DialogUpdate(DialogPage::Compress {
                                            paths: paths.clone(),
                                            to: to.clone(),
                                            name,
                                            archive_type: *archive_type,
                                            password: password.clone(),
                                        })
                                    })
                                    .on_submit_maybe(
                                        complete_maybe.clone().map(|maybe| move |_| maybe.clone()),
                                    )
                                    .into(),
                                Element::from(widget::dropdown(
                                    archive_types,
                                    selected,
                                    move |index| index,
                                ))
                                .map(|index| {
                                    Message::DialogUpdate(DialogPage::Compress {
                                        paths: paths.clone(),
                                        to: to.clone(),
                                        name: name.clone(),
                                        archive_type: archive_types[index],
                                        password: password.clone(),
                                    })
                                }),
                            ])
                            .align_y(Alignment::Center)
                            .spacing(space_xxs)
                            .into(),
                        ])
                        .spacing(space_xxs),
                    );

                if *archive_type == ArchiveType::Zip {
                    let password_unwrapped = password.clone().unwrap_or_default();
                    dialog = dialog.control(widget::column::with_children([
                        widget::text::body(fl!("password")).into(),
                        widget::text_input("", password_unwrapped)
                            .password()
                            .on_input(move |password_unwrapped| {
                                Message::DialogUpdate(DialogPage::Compress {
                                    paths: paths.clone(),
                                    to: to.clone(),
                                    name: name.clone(),
                                    archive_type: *archive_type,
                                    password: Some(password_unwrapped),
                                })
                            })
                            .on_submit_maybe(complete_maybe.map(|maybe| move |_| maybe.clone()))
                            .into(),
                    ]));
                }

                dialog
            }
            DialogPage::EmptyTrash => widget::dialog()
                .title(fl!("empty-trash-title"))
                .body(fl!("empty-trash-warning"))
                .primary_action(
                    widget::button::suggested(fl!("empty-trash"))
                        .on_press(Message::DialogComplete)
                        .id(EMPTY_TRASH_BUTTON_ID.clone()),
                )
                .secondary_action(
                    widget::button::standard(fl!("cancel")).on_press(Message::DialogCancel),
                ),
            DialogPage::FailedOperation(id) => {
                //TODO: try next dialog page (making sure index is used by Dialog messages)?
                let error_text = failed_operation_text(&self.failed_operations, *id)?;
                widget::dialog()
                    .title("Failed operation")
                    .body(error_text.clone())
                    .icon(icon::from_name("dialog-error").size(64))
                    //TODO: retry action
                    .primary_action(
                        widget::button::standard(fl!("cancel")).on_press(Message::DialogCancel),
                    )
                    .secondary_action(
                        widget::button::standard(fl!("copy-to-clipboard"))
                            .on_press(Message::CopyText(error_text)),
                    )
            }
            DialogPage::FailedOperations(ids) => {
                let error_text = failed_operations_text(&self.failed_operations, ids);
                widget::dialog()
                    .title("Failed operations")
                    .body(error_text.clone())
                    .icon(icon::from_name("dialog-error").size(64))
                    //TODO: retry action
                    .primary_action(
                        widget::button::standard(fl!("cancel")).on_press(Message::DialogCancel),
                    )
                    .secondary_action(
                        widget::button::standard(fl!("copy-to-clipboard"))
                            .on_press(Message::CopyText(error_text)),
                    )
            }
            DialogPage::ExtractPassword { id, password } => widget::dialog()
                .title(fl!("extract-password-required"))
                .icon(icon::from_name("dialog-error").size(64))
                .control(
                    widget::text_input("", password)
                        .password()
                        .on_input(move |password| {
                            Message::DialogUpdate(DialogPage::ExtractPassword { id: *id, password })
                        })
                        .on_submit(|_| Message::DialogComplete)
                        .id(self.dialog_text_input.clone()),
                )
                .primary_action(
                    widget::button::suggested(fl!("extract-here"))
                        .on_press(Message::DialogComplete),
                )
                .secondary_action(
                    widget::button::standard(fl!("cancel")).on_press(Message::DialogCancel),
                ),
            DialogPage::MountError {
                mounter_key: _,
                item: _,
                error,
            } => widget::dialog()
                .title(fl!("mount-error"))
                .body(error)
                .icon(icon::from_name("dialog-error").size(64))
                .primary_action(
                    widget::button::standard(fl!("try-again"))
                        .on_press(Message::DialogComplete)
                        .id(MOUNT_ERROR_TRY_AGAIN_BUTTON_ID.clone()),
                )
                .secondary_action(
                    widget::button::standard(fl!("cancel")).on_press(Message::DialogCancel),
                ),
            DialogPage::NetworkAuth {
                mounter_key,
                uri,
                auth,
                auth_tx,
            } => {
                //TODO: use URI!
                let mut controls = widget::column::with_capacity(4);
                let mut id_assigned = false;

                if let Some(username) = &auth.username_opt {
                    //TODO: what should submit do?
                    let mut input = widget::text_input(fl!("username"), username)
                        .on_input(move |value| {
                            Message::DialogUpdate(DialogPage::NetworkAuth {
                                mounter_key: *mounter_key,
                                uri: uri.clone(),
                                auth: MounterAuth {
                                    username_opt: Some(value),
                                    ..auth.clone()
                                },
                                auth_tx: auth_tx.clone(),
                            })
                        })
                        .on_submit(|_| Message::DialogComplete);
                    if !id_assigned {
                        input = input.id(self.dialog_text_input.clone());
                        id_assigned = true;
                    }
                    controls = controls.push(input);
                }

                if let Some(domain) = &auth.domain_opt {
                    //TODO: what should submit do?
                    let mut input = widget::text_input(fl!("domain"), domain)
                        .on_input(move |value| {
                            Message::DialogUpdate(DialogPage::NetworkAuth {
                                mounter_key: *mounter_key,
                                uri: uri.clone(),
                                auth: MounterAuth {
                                    domain_opt: Some(value),
                                    ..auth.clone()
                                },
                                auth_tx: auth_tx.clone(),
                            })
                        })
                        .on_submit(|_| Message::DialogComplete);
                    if !id_assigned {
                        input = input.id(self.dialog_text_input.clone());
                        id_assigned = true;
                    }
                    controls = controls.push(input);
                }

                if let Some(password) = &auth.password_opt {
                    //TODO: what should submit do?
                    //TODO: button for showing password
                    let mut input = widget::secure_input(fl!("password"), password, None, true)
                        .on_input(move |value| {
                            Message::DialogUpdate(DialogPage::NetworkAuth {
                                mounter_key: *mounter_key,
                                uri: uri.clone(),
                                auth: MounterAuth {
                                    password_opt: Some(value),
                                    ..auth.clone()
                                },
                                auth_tx: auth_tx.clone(),
                            })
                        })
                        .on_submit(|_| Message::DialogComplete);
                    if !id_assigned {
                        input = input.id(self.dialog_text_input.clone());
                    }
                    controls = controls.push(input);
                }

                if let Some(remember) = &auth.remember_opt {
                    //TODO: what should submit do?
                    //TODO: button for showing password
                    controls = controls.push(
                        widget::checkbox(*remember)
                            .label(fl!("remember-password"))
                            .on_toggle(move |value| {
                                Message::DialogUpdate(DialogPage::NetworkAuth {
                                    mounter_key: *mounter_key,
                                    uri: uri.clone(),
                                    auth: MounterAuth {
                                        remember_opt: Some(value),
                                        ..auth.clone()
                                    },
                                    auth_tx: auth_tx.clone(),
                                })
                            }),
                    );
                }

                let mut parts = auth.message.splitn(2, '\n');
                let title = parts.next().unwrap_or_default();
                let body = parts.next().unwrap_or_default();

                let mut widget = widget::dialog()
                    .title(title)
                    .body(body)
                    .control(controls.spacing(space_s))
                    .primary_action(
                        widget::button::suggested(fl!("connect")).on_press(Message::DialogComplete),
                    )
                    .secondary_action(
                        widget::button::standard(fl!("cancel")).on_press(Message::DialogCancel),
                    );

                if let Some(_anonymous) = &auth.anonymous_opt {
                    widget = widget.tertiary_action(
                        widget::button::text(fl!("connect-anonymously")).on_press(
                            Message::DialogUpdateComplete(DialogPage::NetworkAuth {
                                mounter_key: *mounter_key,
                                uri: uri.clone(),
                                auth: MounterAuth {
                                    anonymous_opt: Some(true),
                                    ..auth.clone()
                                },
                                auth_tx: auth_tx.clone(),
                            }),
                        ),
                    );
                }

                widget
            }
            DialogPage::NetworkError {
                mounter_key: _,
                uri: _,
                error,
            } => widget::dialog()
                .title(fl!("network-drive-error"))
                .body(error)
                .icon(icon::from_name("dialog-error").size(64))
                .primary_action(
                    widget::button::standard(fl!("try-again")).on_press(Message::DialogComplete),
                )
                .secondary_action(
                    widget::button::standard(fl!("cancel")).on_press(Message::DialogCancel),
                ),
            DialogPage::NewItem { parent, name, dir } => {
                let mut dialog = widget::dialog().title(if *dir {
                    fl!("create-new-folder")
                } else {
                    fl!("create-new-file")
                });

                let complete_maybe = if name.is_empty() {
                    None
                } else if name == "." || name == ".." {
                    dialog = dialog.tertiary_action(widget::text::body(fl!(
                        "name-invalid",
                        filename = name.as_str()
                    )));
                    None
                } else if name.contains('/') {
                    dialog = dialog.tertiary_action(widget::text::body(fl!("name-no-slashes")));
                    None
                } else {
                    match self.existing_entry_in_dir(parent, name) {
                        Some(true) => {
                            dialog = dialog
                                .tertiary_action(widget::text::body(fl!("folder-already-exists")));
                            None
                        }
                        Some(false) => {
                            dialog = dialog
                                .tertiary_action(widget::text::body(fl!("file-already-exists")));
                            None
                        }
                        None => {
                            if name.starts_with('.') {
                                dialog =
                                    dialog.tertiary_action(widget::text::body(fl!("name-hidden")));
                            }
                            Some(Message::DialogComplete)
                        }
                    }
                };

                dialog
                    .primary_action(
                        widget::button::suggested(fl!("save"))
                            .on_press_maybe(complete_maybe.clone()),
                    )
                    .secondary_action(
                        widget::button::standard(fl!("cancel")).on_press(Message::DialogCancel),
                    )
                    .control(
                        widget::column::with_children([
                            widget::text::body(if *dir {
                                fl!("folder-name")
                            } else {
                                fl!("file-name")
                            })
                            .into(),
                            widget::text_input("", name.as_str())
                                .id(self.dialog_text_input.clone())
                                .on_input(move |name| {
                                    Message::DialogUpdate(DialogPage::NewItem {
                                        parent: parent.clone(),
                                        name,
                                        dir: *dir,
                                    })
                                })
                                .on_submit_maybe(complete_maybe.map(|maybe| move |_| maybe.clone()))
                                .into(),
                        ])
                        .spacing(space_xxs),
                    )
            }
            DialogPage::RunContextAction { action, paths } => {
                let name = self
                    .config
                    .context_actions
                    .get(*action)
                    .map_or_else(|| fl!("context-action"), |preset| preset.name.clone());

                widget::dialog()
                    .title(fl!("context-action-confirm-title", name = name))
                    .body(fl!("context-action-confirm-warning", items = paths.len()))
                    .icon(icon::from_name("dialog-error").size(64))
                    .primary_action(
                        widget::button::suggested(fl!("run"))
                            .on_press(Message::DialogComplete)
                            .id(CONFIRM_CONTEXT_ACTION_BUTTON_ID.clone()),
                    )
                    .secondary_action(
                        widget::button::standard(fl!("cancel")).on_press(Message::DialogCancel),
                    )
            }
            DialogPage::OpenWith {
                path,
                mime,
                selected,
                store_opt,
                search_app_name,
                ..
            } => {
                let name = match path.file_name() {
                    Some(file_name) => file_name.to_str(),
                    None => path.as_os_str().to_str(),
                };

                let mut column = widget::list_column();
                let mut available_apps = self.mime_app_cache.get_apps_for_mime(mime, true);
                available_apps.retain(|(app, _)| {
                    app.name
                        .to_lowercase()
                        .trim()
                        .contains(search_app_name.to_lowercase().as_str().trim())
                });
                let item_height = 32.0;
                let mut displayed_default = false;
                let mut last_kind = MimeAppMatch::Exact;
                for (i, &(app, kind)) in available_apps.iter().enumerate() {
                    if kind != last_kind {
                        match kind {
                            MimeAppMatch::Related => {
                                column = column.add(widget::text::heading(fl!("related-apps")));
                            }
                            MimeAppMatch::Other => {
                                column = column.add(widget::text::heading(fl!("other-apps")));
                            }
                            _ => {}
                        }
                        last_kind = kind;
                    }
                    column = column.add(
                        widget::mouse_area(
                            widget::button::custom(
                                widget::row::with_children([
                                    icon(app.icon()).size(32).into(),
                                    if app.is_default(mime) && !displayed_default {
                                        displayed_default = true;
                                        widget::text::body(fl!(
                                            "default-app",
                                            name = Some(app.name.as_str())
                                        ))
                                        .into()
                                    } else {
                                        widget::text::body(app.name.clone()).into()
                                    },
                                    widget::space::horizontal().into(),
                                    if *selected == i {
                                        icon::from_name("checkbox-checked-symbolic").size(16).into()
                                    } else {
                                        widget::space::horizontal()
                                            .width(Length::Fixed(16.0))
                                            .into()
                                    },
                                ])
                                .spacing(space_s)
                                .height(Length::Fixed(item_height))
                                .align_y(Alignment::Center),
                            )
                            .width(Length::Fill)
                            .class(theme::Button::MenuItem)
                            .force_enabled(true),
                        )
                        .on_press(Message::OpenWithSelection(i))
                        .on_double_press(Message::DialogComplete),
                    );
                }

                let mut dialog = widget::dialog()
                    .title(fl!("open-with-title", name = name))
                    .primary_action(
                        widget::button::suggested(fl!("open"))
                            .on_press(Message::DialogComplete)
                            .id(CONFIRM_OPEN_WITH_BUTTON_ID.clone()),
                    )
                    .secondary_action(
                        widget::button::standard(fl!("cancel")).on_press(Message::DialogCancel),
                    )
                    .control(
                        widget::text_input::search_input(
                            fl!("search-application"),
                            search_app_name,
                        )
                        .id(self.dialog_text_input.clone())
                        .on_clear(Message::OpenWithSearchClear)
                        .on_input(move |search_app_name| {
                            Message::DialogUpdate(DialogPage::OpenWith {
                                path: path.clone(),
                                mime: mime.clone(),
                                selected: *selected,
                                store_opt: store_opt.clone(),
                                search_app_name,
                            })
                        })
                        .on_submit(|_| Message::DialogComplete),
                    )
                    .control(widget::scrollable(column).height({
                        let max_size = self
                            .size
                            .map_or(480.0, |size| (size.height - 256.0).min(480.0));
                        // (32 (item_height) + 5.0 (custom button padding)) + (space_xxs (list item spacing) * 2)
                        let scrollable_height = available_apps.len() as f32
                            * f32::from(space_xxs).mul_add(2.0, item_height + 5.0);

                        if scrollable_height > max_size {
                            Length::Fixed(max_size)
                        } else {
                            Length::Shrink
                        }
                    }));

                if let Some(app) = store_opt {
                    dialog = dialog.tertiary_action(
                        widget::button::text(fl!("browse-store", store = app.name.as_str()))
                            .on_press(Message::OpenWithBrowse),
                    );
                }

                dialog
            }
            DialogPage::PermanentlyDelete { paths } => {
                let target = if paths.len() == 1 {
                    format!(
                        "\"{}\"",
                        paths[0].file_name().map_or_else(
                            || paths[0].to_string_lossy(),
                            std::ffi::OsStr::to_string_lossy
                        )
                    )
                } else {
                    fl!("selected-items", items = paths.len())
                };

                widget::dialog()
                    .title(fl!("permanently-delete-question"))
                    .primary_action(
                        widget::button::destructive(fl!("delete"))
                            .on_press(Message::DialogComplete)
                            .id(PERMANENT_DELETE_BUTTON_ID.clone()),
                    )
                    .secondary_action(
                        widget::button::standard(fl!("cancel")).on_press(Message::DialogCancel),
                    )
                    .control(widget::text(fl!(
                        "permanently-delete-warning",
                        target = target
                    )))
            }
            DialogPage::DeleteTrash { items } => {
                let target = if items.len() == 1 {
                    format!("\"{}\"", items[0].name.to_string_lossy())
                } else {
                    fl!("selected-items", items = items.len())
                };

                widget::dialog()
                    .title(fl!("permanently-delete-question"))
                    .primary_action(
                        widget::button::destructive(fl!("delete"))
                            .on_press(Message::DialogComplete)
                            .id(DELETE_TRASH_BUTTON_ID.clone()),
                    )
                    .secondary_action(
                        widget::button::standard(fl!("cancel")).on_press(Message::DialogCancel),
                    )
                    .control(widget::text(fl!(
                        "permanently-delete-warning",
                        target = target
                    )))
            }
            DialogPage::ChangeSidebarLabel { entity, label } => {
                let entity = *entity;
                let complete_maybe = if label.trim().is_empty() {
                    None
                } else {
                    Some(Message::DialogComplete)
                };

                widget::dialog()
                    .title(fl!("change-sidebar-label"))
                    .primary_action(
                        widget::button::suggested(fl!("save"))
                            .on_press_maybe(complete_maybe.clone()),
                    )
                    .secondary_action(
                        widget::button::standard(fl!("cancel")).on_press(Message::DialogCancel),
                    )
                    .control(
                        widget::column::with_children([
                            widget::text::body(fl!("sidebar-label")).into(),
                            widget::text_input("", label.as_str())
                                .id(self.dialog_text_input.clone())
                                .on_input(move |label| {
                                    Message::DialogUpdate(DialogPage::ChangeSidebarLabel {
                                        entity,
                                        label,
                                    })
                                })
                                .on_submit_maybe(complete_maybe.map(|maybe| move |_| maybe.clone()))
                                .into(),
                        ])
                        .spacing(space_xxs),
                    )
            }
            DialogPage::RenameItem {
                from,
                parent,
                name,
                dir,
            } => {
                //TODO: combine logic with NewItem
                let mut dialog = widget::dialog().title(if *dir {
                    fl!("rename-folder")
                } else {
                    fl!("rename-file")
                });

                let complete_maybe = if name.is_empty() {
                    None
                } else if name == "." || name == ".." {
                    dialog = dialog.tertiary_action(widget::text::body(fl!(
                        "name-invalid",
                        filename = name.as_str()
                    )));
                    None
                } else if name.contains('/') {
                    dialog = dialog.tertiary_action(widget::text::body(fl!("name-no-slashes")));
                    None
                } else {
                    let path = parent.join(name);
                    // Skip the collision check when renaming to the same path (a no-op rename).
                    let existing = (*from != path)
                        .then(|| self.existing_entry_in_dir(parent, name))
                        .flatten();
                    match existing {
                        Some(true) => {
                            dialog = dialog
                                .tertiary_action(widget::text::body(fl!("folder-already-exists")));
                            None
                        }
                        Some(false) => {
                            dialog = dialog
                                .tertiary_action(widget::text::body(fl!("file-already-exists")));
                            None
                        }
                        None => {
                            if name.starts_with('.') {
                                dialog =
                                    dialog.tertiary_action(widget::text::body(fl!("name-hidden")));
                            }
                            Some(Message::DialogComplete)
                        }
                    }
                };

                dialog
                    .primary_action(
                        widget::button::suggested(fl!("rename-confirm"))
                            .on_press_maybe(complete_maybe.clone()),
                    )
                    .secondary_action(
                        widget::button::standard(fl!("cancel")).on_press(Message::DialogCancel),
                    )
                    .control(
                        widget::column::with_children([
                            widget::text::body(if *dir {
                                fl!("folder-name")
                            } else {
                                fl!("file-name")
                            })
                            .into(),
                            widget::text_input("", name.as_str())
                                .id(self.dialog_text_input.clone())
                                .double_click_select_delimiter('.')
                                .on_input(move |name| {
                                    Message::DialogUpdate(DialogPage::RenameItem {
                                        from: from.clone(),
                                        parent: parent.clone(),
                                        name,
                                        dir: *dir,
                                    })
                                })
                                .on_submit_maybe(complete_maybe.map(|maybe| move |_| maybe.clone()))
                                .into(),
                        ])
                        .spacing(space_xxs),
                    )
            }
            DialogPage::Replace {
                from,
                to,
                multiple,
                apply_to_all,
                conflict_count,
                tx,
            } => {
                let military_time = self.config.tab.military_time;
                let dialog = widget::dialog()
                    .title(fl!("replace-title", filename = to.name.as_str()))
                    .body(fl!("replace-warning-operation"))
                    .control(
                        to.replace_view(fl!("original-file"), military_time)
                            .map(|x| Message::TabMessage(None, x)),
                    )
                    .control(
                        from.replace_view(fl!("replace-with"), military_time)
                            .map(|x| Message::TabMessage(None, x)),
                    )
                    .primary_action(
                        widget::button::suggested(fl!("replace"))
                            .on_press(Message::ReplaceResult(ReplaceResult::Replace(
                                *apply_to_all,
                            )))
                            .id(REPLACE_BUTTON_ID.clone()),
                    );
                if *multiple {
                    dialog
                        .control(
                            widget::checkbox(*apply_to_all)
                                .label(format!("{} ({})", fl!("apply-to-all"), *conflict_count))
                                .on_toggle(|apply_to_all| {
                                    Message::DialogUpdate(DialogPage::Replace {
                                        from: from.clone(),
                                        to: to.clone(),
                                        multiple: *multiple,
                                        apply_to_all,
                                        conflict_count: *conflict_count,
                                        tx: tx.clone(),
                                    })
                                }),
                        )
                        .secondary_action(
                            widget::button::standard(fl!("skip")).on_press(Message::ReplaceResult(
                                ReplaceResult::Skip(*apply_to_all),
                            )),
                        )
                        .tertiary_action(
                            widget::button::text(fl!("cancel"))
                                .on_press(Message::ReplaceResult(ReplaceResult::Cancel)),
                        )
                } else {
                    dialog
                        .secondary_action(
                            widget::button::standard(fl!("cancel"))
                                .on_press(Message::ReplaceResult(ReplaceResult::Cancel)),
                        )
                        .tertiary_action(
                            widget::button::text(fl!("keep-both"))
                                .on_press(Message::ReplaceResult(ReplaceResult::KeepBoth)),
                        )
                }
            }
            DialogPage::SetExecutableAndLaunch { path } => {
                let name = match path.file_name() {
                    Some(file_name) => file_name.to_str(),
                    None => path.as_os_str().to_str(),
                };
                widget::dialog()
                    .title(fl!("set-executable-and-launch"))
                    .primary_action(
                        widget::button::text(fl!("set-and-launch"))
                            .class(theme::Button::Suggested)
                            .on_press(Message::DialogComplete)
                            .id(SET_EXECUTABLE_AND_LAUNCH_CONFIRM_BUTTON_ID.clone()),
                    )
                    .secondary_action(
                        widget::button::text(fl!("cancel"))
                            .class(theme::Button::Standard)
                            .on_press(Message::DialogCancel),
                    )
                    .control(widget::text::text(fl!(
                        "set-executable-and-launch-description",
                        name = name
                    )))
            }
            DialogPage::FavoritePathError { path, .. } => widget::dialog()
                .title(fl!("favorite-path-error"))
                .body(fl!(
                    "favorite-path-error-description",
                    path = path.as_os_str().to_str()
                ))
                .icon(icon::from_name("dialog-error").size(64))
                .primary_action(
                    widget::button::destructive(fl!("remove"))
                        .on_press(Message::DialogComplete)
                        .id(FAVORITE_PATH_ERROR_REMOVE_BUTTON_ID.clone()),
                )
                .secondary_action(
                    widget::button::standard(fl!("keep")).on_press(Message::DialogCancel),
                ),
        };
        Some(dialog.into())
    }

    fn footer(&self) -> Option<Element<'_, Message>> {
        if self.progress_operations.is_empty() {
            return None;
        }

        let cosmic_theme::Spacing {
            space_xs, space_s, ..
        } = theme::spacing();

        let mut title = String::new();
        let mut total_progress = 0.0;
        let mut count = 0;
        let mut all_paused = true;
        for (op, controller) in self.pending_operations.values() {
            if !controller.is_paused() {
                all_paused = false;
            }
            if op.show_progress_notification() {
                let progress = controller.progress();
                if title.is_empty() {
                    title = op.pending_text(progress, controller.state());
                }
                total_progress += progress;
                count += 1;
            }
        }
        let running = count;
        // Adjust the progress bar so it does not jump around when operations finish
        for id in &self.progress_operations {
            if self.complete_operations.contains_key(id) {
                total_progress += 1.0;
                count += 1;
            }
        }
        let finished = count - running;
        total_progress /= count as f32;
        if running >= 1 && (running > 1 || finished > 0) {
            if finished > 0 {
                title = fl!(
                    "operations-running-finished",
                    running = running,
                    finished = finished,
                    percent = ((total_progress * 100.0) as i32)
                );
            } else {
                title = fl!(
                    "operations-running",
                    running = running,
                    percent = ((total_progress * 100.0) as i32)
                );
            }
        }

        //TODO: get height from theme?
        let progress_bar_height = Length::Fixed(4.0);
        let progress_bar = widget::determinate_linear(total_progress)
            .width(Length::Fill)
            .girth(progress_bar_height);

        let container = widget::layer_container(widget::column::with_children([
            widget::row::with_children([
                progress_bar.into(),
                if all_paused {
                    widget::tooltip(
                        widget::button::icon(icon::from_name("media-playback-start-symbolic"))
                            .on_press(Message::PendingPauseAll(false))
                            .padding(8),
                        widget::text::body(fl!("resume")),
                        widget::tooltip::Position::Top,
                    )
                    .into()
                } else {
                    widget::tooltip(
                        widget::button::icon(icon::from_name("media-playback-pause-symbolic"))
                            .on_press(Message::PendingPauseAll(true))
                            .padding(8),
                        widget::text::body(fl!("pause")),
                        widget::tooltip::Position::Top,
                    )
                    .into()
                },
                widget::tooltip(
                    widget::button::icon(icon::from_name("window-close-symbolic"))
                        .on_press(Message::PendingCancelAll)
                        .padding(8),
                    widget::text::body(fl!("cancel")),
                    widget::tooltip::Position::Top,
                )
                .into(),
            ])
            .align_y(Alignment::Center)
            .into(),
            widget::text::body(title).into(),
            widget::space::vertical().height(space_s).into(),
            widget::row::with_children([
                widget::button::link(fl!("details"))
                    .on_press(Message::ToggleContextPage(ContextPage::EditHistory))
                    .padding(0)
                    .trailing_icon(true)
                    .into(),
                widget::space::horizontal().into(),
                widget::button::standard(fl!("dismiss"))
                    .on_press(Message::PendingDismiss)
                    .into(),
            ])
            .align_y(Alignment::Center)
            .into(),
        ]))
        .padding([8, space_xs])
        .layer(cosmic_theme::Layer::Primary);

        Some(container.into())
    }

    fn header_start(&self) -> Vec<Element<'_, Self::Message>> {
        vec![menu::menu_bar(
            &self.core,
            self.tab_model.active_data::<Tab>(),
            &self.config,
            &self.modifiers,
            &self.key_binds,
            self.clipboard_has_content(),
        )]
    }

    fn header_end(&self) -> Vec<Element<'_, Self::Message>> {
        let mut elements = Vec::with_capacity(2);

        if let Some(term) = self.search_get() {
            if self.core.is_condensed() {
                elements.push(
                    //TODO: selected state is not appearing different
                    widget::button::icon(icon::from_name("system-search-symbolic"))
                        .on_press(Message::SearchClear)
                        .padding(8)
                        .selected(true)
                        .into(),
                );
            } else {
                elements.push(
                    widget::text_input::search_input("", term)
                        .width(Length::Fixed(240.0))
                        .id(self.search_id.clone())
                        .on_clear(Message::SearchClear)
                        .on_input(Message::SearchInput)
                        .into(),
                );
            }
        } else {
            elements.push(
                widget::button::icon(icon::from_name("system-search-symbolic"))
                    .on_press(Message::SearchActivate)
                    .padding(8)
                    .into(),
            );
        }

        elements
    }

    /// Creates a view after each update.
    fn view(&self) -> Element<'_, Self::Message> {
        let cosmic_theme::Spacing {
            space_xxs, space_s, ..
        } = theme::spacing();

        let mut tab_column = widget::column::with_capacity(4);

        if self.core.is_condensed()
            && let Some(term) = self.search_get()
        {
            tab_column = tab_column.push(
                widget::container(
                    widget::text_input::search_input("", term)
                        .width(Length::Fill)
                        .id(self.search_id.clone())
                        .on_clear(Message::SearchClear)
                        .on_input(Message::SearchInput),
                )
                .padding(space_xxs),
            );
        }

        if self.tab_model.len() > 1 {
            tab_column = tab_column.push(
                widget::container(
                    widget::tab_bar::horizontal(&self.tab_model)
                        .button_height(32)
                        .button_spacing(space_xxs)
                        .enable_tab_drag(String::from("x-cosmic-files/tab-dnd"))
                        .on_reorder(Message::ReorderTab)
                        .tab_drag_threshold(25.)
                        .on_activate(Message::TabActivate)
                        .on_close(|entity| Message::TabClose(Some(entity)))
                        .on_dnd_enter(Message::DndEnterTab)
                        .on_dnd_leave(|_| Message::DndExitTab)
                        .on_dnd_drop(|entity, data, action| {
                            Message::DndDropTab(entity, data, action)
                        })
                        .drag_id(self.tab_drag_id),
                )
                .width(Length::Fill)
                .padding([0, space_s]),
            );
        }

        let entity = self.tab_model.active();
        if let Some(tab) = self.tab_model.data::<Tab>(entity) {
            let tab_view = tab
                .view(
                    &self.key_binds,
                    &self.modifiers,
                    self.clipboard_has_content(),
                    &self.config.context_actions,
                )
                .map(move |message| Message::TabMessage(Some(entity), message));
            tab_column = tab_column.push(tab_view);
        } else {
            //TODO
        }

        // The toaster is added on top of an empty element to ensure that it does not override context menus
        tab_column = tab_column.push(widget::toaster(&self.toasts, widget::space::horizontal()));

        let content: Element<_> = tab_column.into();

        // Uncomment to debug layout:
        //content.explain(cosmic::iced::Color::WHITE)
        content
    }

    fn view_window(&self, id: WindowId) -> Element<'_, Self::Message> {
        let content = match self.windows.get(&id) {
            Some(window) => match &window.kind {
                WindowKind::Desktop(entity) => {
                    let mut tab_column = widget::column::with_capacity(3);

                    let tab_view = match self.tab_model.data::<Tab>(*entity) {
                        Some(tab) => tab
                            .view(
                                &self.key_binds,
                                &window.modifiers,
                                self.clipboard_has_content(),
                                &self.config.context_actions,
                            )
                            .map(move |message| Message::TabMessage(Some(*entity), message)),
                        None => widget::space::vertical().into(),
                    };

                    tab_column = tab_column.push(tab_view);

                    // The toaster is added on top of an empty element to ensure that it does not override context menus
                    tab_column =
                        tab_column.push(widget::toaster(&self.toasts, widget::space::horizontal()));
                    return if let Some(margin) = self.margin.get(&id) {
                        if margin.0 >= 0. || margin.2 >= 0. {
                            tab_column = widget::column::with_children([
                                space::vertical().height(margin.0).into(),
                                tab_column.into(),
                                space::vertical().height(margin.2).into(),
                            ]);
                        }
                        if margin.1 >= 0. || margin.3 >= 0. {
                            Element::from(widget::row::with_children([
                                space::horizontal().width(margin.1).into(),
                                tab_column.into(),
                                space::horizontal().width(margin.3).into(),
                            ]))
                        } else {
                            tab_column.into()
                        }
                    } else {
                        tab_column.into()
                    };
                }
                WindowKind::DesktopViewOptions => self.desktop_view_options(),
                WindowKind::Dialogs(id) => match self.dialog() {
                    Some(element) => return widget::autosize::autosize(element, id.clone()).into(),
                    None => widget::space::horizontal().into(),
                },
                WindowKind::Preview(entity_opt, kind) => self
                    .preview(entity_opt, kind, false)
                    .map(|x| Message::TabMessage(*entity_opt, x)),
                WindowKind::FileDialog(..) => match &self.file_dialog_opt {
                    Some(dialog) => return dialog.view(id),
                    None => widget::text("Unknown window ID").into(),
                },
            },
            None => {
                //TODO: distinct views per monitor in desktop mode
                return self.view_main().map(|message| match message {
                    cosmic::Action::App(app) => app,
                    cosmic::Action::Cosmic(cosmic) => Message::Cosmic(cosmic),
                    cosmic::Action::Surface(action) => Message::Surface(action),
                    cosmic::Action::None => Message::None,
                });
            }
        };

        widget::container(widget::id_container(
            widget::scrollable(content),
            widget::Id::new("main container for files"),
        ))
        .width(Length::Fill)
        .height(Length::Fill)
        .class(theme::Container::WindowBackground)
        .into()
    }

    fn system_theme_update(
        &mut self,
        _keys: &[&'static str],
        _new_theme: &cosmic::cosmic_theme::Theme,
    ) -> Task<Self::Message> {
        self.update(Message::SystemThemeModeChange)
    }

    fn subscription(&self) -> Subscription<Self::Message> {
        struct WatcherSubscription;
        struct TrashWatcherSubscription;
        struct TimeSubscription;
        #[cfg(all(
            not(feature = "desktop-applet"),
            not(target_os = "ios"),
            not(target_os = "android")
        ))]
        struct RecentsWatcherSubscription;

        let mut subscriptions = vec![
            //TODO: filter more events by window id
            event::listen_with(|event, status, window_id| match event {
                Event::Mouse(mouse::Event::ButtonPressed(button)) => match status {
                    event::Status::Ignored => Some(Message::Mouse(window_id, button)),
                    event::Status::Captured => None,
                },
                Event::Keyboard(KeyEvent::KeyPressed {
                    key,
                    physical_key,
                    modifiers,
                    text,
                    ..
                }) => match status {
                    event::Status::Ignored => {
                        Some(Message::Key(window_id, modifiers, key, physical_key, text))
                    }
                    event::Status::Captured => None,
                },
                Event::Keyboard(KeyEvent::ModifiersChanged(modifiers)) => {
                    Some(Message::ModifiersChanged(window_id, modifiers))
                }
                #[cfg(all(feature = "wayland", feature = "desktop-applet"))]
                Event::Window(WindowEvent::Focused) => Some(Message::Focused(window_id)),
                #[cfg(not(all(feature = "wayland", feature = "desktop-applet")))]
                Event::Window(WindowEvent::Focused) => Some(Message::CheckClipboard),
                Event::Window(WindowEvent::CloseRequested) => Some(Message::WindowClose),
                Event::Window(WindowEvent::Opened { position: _, size }) => {
                    Some(Message::Size(window_id, size))
                }
                Event::Window(WindowEvent::Resized(s)) => Some(Message::Size(window_id, s)),
                #[cfg(all(feature = "wayland", feature = "desktop-applet"))]
                Event::PlatformSpecific(event::PlatformSpecific::Wayland(wayland_event)) => {
                    match wayland_event {
                        WaylandEvent::Output(output_event, output) => {
                            Some(Message::OutputEvent(output_event, output))
                        }
                        #[cfg(feature = "desktop")]
                        WaylandEvent::OverlapNotify(event, ..) => {
                            Some(Message::Overlap(window_id, event))
                        }
                        _ => None,
                    }
                }
                _ => None,
            }),
            Config::subscription().map(|update| {
                if !update.errors.is_empty() {
                    log::info!(
                        "errors loading config {:?}: {:?}",
                        update.keys,
                        update.errors
                    );
                }
                Message::Config(update.config)
            }),
            cosmic_config::config_subscription::<_, TimeConfig>(
                TypeId::of::<TimeSubscription>(),
                TIME_CONFIG_ID.into(),
                1,
            )
            .map(|update| {
                if !update.errors.is_empty() {
                    log::info!(
                        "errors loading time config {:?}: {:?}",
                        update.keys,
                        update.errors
                    );
                }
                Message::TimeConfigChange(update.config)
            }),
            Subscription::run_with(TypeId::of::<WatcherSubscription>(), |_| {
                stream::channel(
                    100,
                    |mut output: futures::channel::mpsc::Sender<Message>| async move {
                        let watcher_res = {
                            let mut output = output.clone();
                            new_debouncer(
                                time::Duration::from_millis(250),
                                Some(time::Duration::from_millis(250)),
                                move |events_res: notify_debouncer_full::DebounceEventResult| {
                                    match events_res {
                                        Ok(mut events) => {
                                            log::debug!("{events:?}");

                                            events.retain(|event| {
                                            match &event.kind {
                                                notify::EventKind::Access(_) => {
                                                    // Data not mutated
                                                    false
                                                }
                                                notify::EventKind::Modify(
                                                    notify::event::ModifyKind::Metadata(e),
                                                ) if (*e != notify::event::MetadataKind::Any
                                                    && *e
                                                        != notify::event::MetadataKind::WriteTime) =>
                                                {
                                                    // Data not mutated nor modify time changed
                                                    false
                                                }
                                                _ => true
                                            }
                                        });

                                            if !events.is_empty() {
                                                match futures::executor::block_on(async {
                                                    output.send(Message::NotifyEvents(events)).await
                                                }) {
                                                    Ok(()) => {}
                                                    Err(err) => {
                                                        log::warn!(
                                                            "failed to send notify events: {err:?}"
                                                        );
                                                    }
                                                }
                                            }
                                        }
                                        Err(err) => {
                                            log::warn!("failed to watch files: {err:?}");
                                        }
                                    }
                                },
                            )
                        };

                        match watcher_res {
                            Ok(watcher) => {
                                match output
                                    .send(Message::NotifyWatcher(WatcherWrapper {
                                        watcher_opt: Some(watcher),
                                    }))
                                    .await
                                {
                                    Ok(()) => {}
                                    Err(err) => {
                                        log::warn!("failed to send notify watcher: {err:?}");
                                    }
                                }
                            }
                            Err(err) => {
                                log::warn!("failed to create file watcher: {err:?}");
                            }
                        }

                        std::future::pending().await
                    },
                )
            }),
            #[cfg(feature = "desktop")]
            Subscription::run(|| {
                stream::channel(
                    1,
                    |mut output: futures::channel::mpsc::Sender<Message>| async move {
                        mime_app::watch(move || {
                            futures::executor::block_on(async {
                                _ = output.send(Message::ReloadMimeAppCache).await;
                            })
                        })
                        .await;

                        std::future::pending().await
                    },
                )
            }),
            Subscription::run_with(TypeId::of::<TrashWatcherSubscription>(), |_| {
                stream::channel(
                    1,
                    |mut output: futures::channel::mpsc::Sender<Message>| async move {
                        let watcher_res = new_debouncer(
                            time::Duration::from_millis(250),
                            Some(time::Duration::from_millis(250)),
                            move |event_res: notify_debouncer_full::DebounceEventResult| {
                                match event_res {
                                    Ok(events) => {
                                        // Rescan on any event. We don't need to evaluate each event
                                        // because as long as the trash changed in any way we need to
                                        // rescan.
                                        let should_rescan =
                                            events.iter().any(|event| !event.kind.is_access());

                                        if should_rescan
                                            && let Err(e) = futures::executor::block_on(async {
                                                output.send(Message::RescanTrash).await
                                            })
                                        {
                                            log::warn!(
                                                "trash needs to be rescanned but sending message failed: {e:?}"
                                            );
                                        }
                                    }
                                    Err(e) => {
                                        log::warn!("failed to watch trash bin for changes: {e:?}");
                                    }
                                }
                            },
                        );

                        // Enumerating the trash bins reads the filesystem; run it on the blocking
                        // pool so this one-shot subscription setup never stalls the executor.
                        let trash_folders = tokio::task::spawn_blocking(Trash::folders)
                            .await
                            .unwrap_or_else(|err| {
                                log::warn!("failed to enumerate trash folders: {err}");
                                Ok(std::collections::HashSet::new())
                            });
                        match (watcher_res, trash_folders) {
                            (Ok(mut watcher), Ok(trash_bins)) => {
                                // Watch the "bins" themselves as well as the files folder where
                                // trashed items are placed. This allows us to avoid recursively
                                // watching the trash which is slow but also properly get events.
                                let trash_paths = trash_bins
                                    .into_iter()
                                    .flat_map(|path| [path.join("files"), path]);
                                for path in trash_paths {
                                    if let Err(e) =
                                        watcher.watch(&path, notify::RecursiveMode::NonRecursive)
                                    {
                                        log::warn!(
                                            "failed to add trash bin `{}` to watcher: {e:?}",
                                            path.display()
                                        );
                                    }
                                }

                                // Don't drop the watcher
                                std::future::pending().await
                            }
                            (Err(e), _) => {
                                log::warn!("failed to create new watcher for trash bin: {e:?}");
                            }
                            (_, Err(e)) => {
                                log::warn!("could not find any valid trash bins to watch: {e:?}");
                            }
                        }

                        std::future::pending().await
                    },
                )
            }),
            #[cfg(all(
                not(feature = "desktop-applet"),
                not(target_os = "ios"),
                not(target_os = "android")
            ))]
            Subscription::run_with(TypeId::of::<RecentsWatcherSubscription>(), |_| {
                stream::channel(
                    1,
                    |mut output: futures::channel::mpsc::Sender<Message>| async move {
                        let Some(recents_path) = recently_used_xbel::dir() else {
                            log::warn!(
                                "failed to watch recents changes: .recently_used.xbel does not exist"
                            );
                            return std::future::pending().await;
                        };

                        let watcher_res = new_debouncer(
                            time::Duration::from_millis(250),
                            Some(time::Duration::from_millis(250)),
                            move |event_res: notify_debouncer_full::DebounceEventResult| {
                                match event_res {
                                    Ok(events) => {
                                        // Programs differ in how they modify the recents file so the
                                        // rescan is triggered on any event but access.
                                        if events.iter().any(|event| {
                                            let kind = event.kind;
                                            kind.is_create()
                                                || kind.is_modify()
                                                || kind.is_remove()
                                                || kind.is_other()
                                        }) && let Err(e) = futures::executor::block_on(async {
                                            output.send(Message::RescanRecents).await
                                        }) {
                                            log::warn!(
                                                "open recents tabs need to be updated but sending message failed: {e:?}"
                                            );
                                        }
                                    }
                                    Err(e) => {
                                        log::warn!(
                                            "failed to watch recents file for changes: {e:?}"
                                        )
                                    }
                                }
                            },
                        );

                        match watcher_res {
                            Ok(mut watcher) => {
                                if let Err(e) = watcher
                                    .watch(&recents_path, notify::RecursiveMode::NonRecursive)
                                {
                                    log::warn!(
                                        "failed to add recents file `{}` to watcher: {}",
                                        recents_path.display(),
                                        e
                                    );
                                }

                                // Don't drop the watcher.
                                std::future::pending::<()>().await;
                            }
                            Err(e) => {
                                log::warn!("failed to create new watcher for recents file: {e:?}")
                            }
                        }

                        std::future::pending().await
                    },
                )
            }),
        ];

        if let Some(scroll_speed) = self.auto_scroll_speed {
            subscriptions.push(
                iced::time::every(time::Duration::from_millis(10))
                    .with(scroll_speed)
                    .map(|(scroll_speed, _)| Message::ScrollTab(scroll_speed)),
            );
        }

        subscriptions.extend(MOUNTERS.iter().map(|(key, mounter)| {
            mounter
                .subscription()
                .with(*key)
                .map(|(key, mounter_message)| match mounter_message {
                    MounterMessage::Items(items) => Message::MounterItems(key, items),
                    MounterMessage::MountResult(item, res) => Message::MountResult(key, item, res),
                    MounterMessage::NetworkAuth(uri, auth, auth_tx) => {
                        Message::NetworkAuth(key, uri, auth, auth_tx)
                    }
                    MounterMessage::NetworkResult(uri, res) => {
                        Message::NetworkResult(key, uri, res)
                    }
                })
        }));

        if !self.pending_operations.is_empty() {
            //TODO: inhibit suspend/shutdown?

            if self.core.main_window_id().is_some() {
                // Force refresh the UI every 100ms while an operation is active.
                if self
                    .pending_operations
                    .values()
                    .any(|(_, controller)| !controller.is_paused())
                {
                    subscriptions.push(
                        cosmic::iced::time::every(Duration::from_millis(100))
                            .map(|_| Message::None),
                    );
                }
            } else {
                // Handle notification when window is closed and operations are in progress
                #[cfg(feature = "notify")]
                {
                    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
                    struct NotificationSubscription;
                    subscriptions.push(Subscription::run_with(
                        TypeId::of::<NotificationSubscription>(),
                        |_| {
                            stream::channel(
                                1,
                                move |mut msg_tx: futures::channel::mpsc::Sender<_>| async move {
                                    tokio::task::spawn_blocking(move || {
                                        match notify_rust::Notification::new()
                                            .summary(&fl!("notification-in-progress"))
                                            .timeout(notify_rust::Timeout::Never)
                                            .show()
                                        {
                                            Ok(notification) => {
                                                let _ = futures::executor::block_on(async {
                                                    msg_tx
                                                        .send(Message::Notification(Arc::new(
                                                            Mutex::new(notification),
                                                        )))
                                                        .await
                                                });
                                            }
                                            Err(err) => {
                                                log::warn!("failed to create notification: {err}");
                                            }
                                        }
                                    })
                                    .await
                                    .unwrap_or_else(|err| {
                                        log::warn!("failed to run the notification task: {err}");
                                    });

                                    std::future::pending().await
                                },
                            )
                        },
                    ));
                }
            }
        }

        let mut selected_previews = Vec::new();
        match self.mode {
            Mode::App => {
                if self.core.window.show_context
                    && let ContextPage::Preview(entity_opt, PreviewKind::Selected) =
                        self.context_page
                {
                    selected_previews
                        .push(Some(entity_opt.unwrap_or_else(|| self.tab_model.active())));
                }
            }
            Mode::Desktop => {
                for window_kind in self.windows.values().map(|window| &window.kind) {
                    if let WindowKind::Preview(entity_opt, _) = window_kind {
                        selected_previews
                            .push(Some(entity_opt.unwrap_or_else(|| self.tab_model.active())));
                    }
                }
            }
        }

        subscriptions.extend(self.tab_model.iter().filter_map(|entity| {
            let tab = self.tab_model.data::<Tab>(entity)?;
            Some(
                tab.subscription(
                    selected_previews
                        .iter()
                        .any(|preview| preview.as_ref() == Some(entity).as_ref()),
                )
                .with(entity)
                .map(|(entity, tab_msg)| Message::TabMessage(Some(entity), tab_msg)),
            )
        }));

        if let Some(dialog) = &self.file_dialog_opt {
            subscriptions.push(dialog.subscription());
        };

        Subscription::batch(subscriptions)
    }
}

// Utilities to build a temporary file hierarchy for tests.
//
// Ideally, tests would use the cap-std crate which limits path traversal.
#[cfg(test)]
pub(crate) mod test_utils {
    use std::cmp::Ordering;
    use std::fs::File;
    use std::io::{self, Write};
    use std::iter;
    use std::path::Path;

    use log::{debug, trace};
    use tempfile::{TempDir, tempdir};

    use crate::config::{IconSizes, TabConfig, ThumbCfg};
    use crate::tab::Item;

    use super::*;

    // Default number of files, directories, and nested directories for test file system
    pub const NUM_FILES: usize = 2;
    pub const NUM_HIDDEN: usize = 1;
    pub const NUM_DIRS: usize = 2;
    pub const NUM_NESTED: usize = 1;
    pub const NAME_LEN: usize = 5;

    /// Add `n` temporary files in `dir`
    ///
    /// Each file is assigned a numeric name from [0, n) with a prefix.
    pub fn file_flat_hier<D: AsRef<Path>>(dir: D, n: usize, prefix: &str) -> io::Result<Vec<File>> {
        let dir = dir.as_ref();
        (0..n)
            .map(|i| -> io::Result<File> {
                let name = format!("{prefix}{i}");
                let path = dir.join(&name);

                let mut file = File::create(path)?;
                file.write_all(name.as_bytes())?;

                Ok(file)
            })
            .collect()
    }

    // Random alphanumeric String of length `len`
    fn rand_string(len: usize) -> String {
        let mut rng = fastrand::Rng::new();
        iter::repeat_with(|| rng.alphanumeric()).take(len).collect()
    }

    /// Create a small, temporary file hierarchy.
    ///
    /// # Arguments
    ///
    /// * `files` - Number of files to create in temp directories
    /// * `hidden` - Number of hidden files to create
    /// * `dirs` - Number of directories to create
    /// * `nested` - Number of nested directories to create in new dirs
    /// * `name_len` - Length of randomized directory names
    pub fn simple_fs(
        files: usize,
        hidden: usize,
        dirs: usize,
        nested: usize,
        name_len: usize,
    ) -> io::Result<TempDir> {
        // Files created inside of a TempDir are deleted with the directory
        // TempDir won't leak resources as long as the destructor runs
        let root = tempdir()?;
        debug!("Root temp directory: {}", root.as_ref().display());
        trace!(
            "Creating {files} files and {hidden} hidden files in {dirs} temp dirs with {nested} nested temp dirs"
        );

        // All paths for directories and nested directories
        let paths = iter::repeat_with(|| {
            let root = root.as_ref();
            let current = rand_string(name_len);

            iter::once(root.join(&current)).chain(
                iter::repeat_with(move || {
                    let mut path = root.join(&current);
                    path.push(rand_string(name_len));
                    path
                })
                .take(nested),
            )
        })
        .take(dirs)
        .flatten();

        // Create directories from `paths` and add a few files
        for path in paths {
            fs::create_dir_all(&path)?;

            // Normal files
            file_flat_hier(&path, files, "")?;
            // Hidden files
            file_flat_hier(&path, hidden, ".")?;

            for entry in path.read_dir()? {
                let entry = entry?;
                if entry.file_type()?.is_file() {
                    trace!("Created file: {}", entry.path().display());
                }
            }
        }

        Ok(root)
    }

    /// Empty file hierarchy
    pub fn empty_fs() -> io::Result<TempDir> {
        tempdir()
    }

    /// Sort files.
    ///
    /// Directories are placed before files.
    /// Files are lexically sorted.
    /// This is more or less copied right from the [Tab] code
    pub fn sort_files(a: &Path, b: &Path) -> Ordering {
        match (a.is_dir(), b.is_dir()) {
            (true, false) => Ordering::Less,
            (false, true) => Ordering::Greater,
            _ => LANGUAGE_SORTER.compare(
                a.file_name()
                    .expect("temp entries should have names")
                    .to_str()
                    .expect("temp entries should be valid UTF-8"),
                b.file_name()
                    .expect("temp entries should have names")
                    .to_str()
                    .expect("temp entries should be valid UTF-8"),
            ),
        }
    }

    /// Read directory entries from `path` and sort.
    pub fn read_dir_sorted(path: &Path) -> io::Result<Vec<PathBuf>> {
        let mut entries: Vec<_> = path
            .read_dir()?
            .map(|maybe_entry| maybe_entry.map(|entry| entry.path()))
            .collect::<io::Result<_>>()?;
        entries.sort_by(|a, b| sort_files(a, b));

        Ok(entries)
    }

    /// Filter `path` for directories
    pub fn filter_dirs(path: &Path) -> io::Result<impl Iterator<Item = PathBuf> + use<>> {
        Ok(path.read_dir()?.filter_map(|entry| {
            entry.ok().and_then(|entry| {
                let path = entry.path();
                path.is_dir().then_some(path)
            })
        }))
    }

    // Filter `path` for files
    pub fn filter_files(path: &Path) -> io::Result<impl Iterator<Item = PathBuf> + use<>> {
        Ok(path.read_dir()?.filter_map(|entry| {
            entry.ok().and_then(|entry| {
                let path = entry.path();
                path.is_file().then_some(path)
            })
        }))
    }

    /// Boiler plate for Tab tests
    pub fn tab_click_new(
        files: usize,
        hidden: usize,
        dirs: usize,
        nested: usize,
        name_len: usize,
    ) -> io::Result<(TempDir, Tab)> {
        let fs = simple_fs(files, hidden, dirs, nested, name_len)?;
        let path = fs.path();

        // New tab with items
        let location = Location::Path(path.to_owned());
        let (parent_item_opt, items) = location.scan(IconSizes::default());
        let mut tab = Tab::new(
            location,
            TabConfig::default(),
            ThumbCfg::default(),
            None,
            widget::Id::unique(),
            None,
        );
        tab.parent_item_opt = parent_item_opt;
        tab.set_items(items);

        // Ensure correct number of directories as a sanity check
        let items = tab.items_opt().expect("tab should be populated with Items");
        assert_eq!(NUM_DIRS, items.len());

        Ok((fs, tab))
    }

    /// Equality for [Path] and [Item].
    pub fn eq_path_item(path: &Path, item: &Item) -> bool {
        let name = path
            .file_name()
            .expect("temp entries should have names")
            .to_str()
            .expect("temp entries should be valid UTF-8");
        let is_dir = path.is_dir();

        // NOTE: I don't want to change `tab::hidden_attribute` to `pub(crate)` for
        // tests without asking
        #[cfg(not(target_os = "windows"))]
        let is_hidden = name.starts_with('.');

        #[cfg(target_os = "windows")]
        let is_hidden = {
            use std::os::windows::fs::MetadataExt;
            const FILE_ATTRIBUTE_HIDDEN: u32 = 2;
            let metadata = path.metadata().expect("fetching file metadata");
            metadata.file_attributes() & FILE_ATTRIBUTE_HIDDEN == FILE_ATTRIBUTE_HIDDEN
        };

        name == item.name
            && is_dir == item.metadata.is_dir()
            && path == item.path_opt().expect("item should have path")
            && is_hidden == item.hidden
    }

    /// Asserts `tab`'s location changed to `path`
    pub fn assert_eq_tab_path(tab: &Tab, path: &Path) {
        // Paths should be the same
        let Some(tab_path) = tab.location.path_opt() else {
            panic!("Expected tab's location to be a path");
        };

        assert_eq!(
            path,
            tab_path,
            "Tab's path is {} instead of being updated to {}",
            tab_path.display(),
            path.display()
        );
    }

    /// Assert that tab's items are equal to a path's entries.
    pub fn assert_eq_tab_path_contents(tab: &Tab, path: &Path) {
        let Some(tab_path) = tab.location.path_opt() else {
            panic!("Expected tab's location to be a path");
        };

        // Tab items are sorted so paths from read_dir must be too
        let entries = read_dir_sorted(path).expect("should be able to read paths from temp dir");

        // Check lengths.
        // `items_opt` is optional and the directory at `path` may have zero entries
        // Therefore, this doesn't panic if `items_opt` is None
        let items_len = tab.items_opt().map(Vec::len).unwrap_or_default();
        assert_eq!(entries.len(), items_len);

        assert!(
            entries
                .into_iter()
                .zip(tab.items_opt().map_or([].as_slice(), Vec::as_slice))
                .all(|(a, b)| eq_path_item(&a, b)),
            "Path ({}) and Tab path ({}) don't have equal contents",
            path.display(),
            tab_path.display()
        );
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::path::{Path, PathBuf};
    use std::time::Instant;

    use notify_debouncer_full::DebouncedEvent;
    use notify_debouncer_full::notify::EventKind;
    use notify_debouncer_full::notify::event::{
        AccessKind, CreateKind, DataChange, MetadataKind, ModifyKind, RemoveKind, RenameMode,
    };

    use super::{
        ListingChange, MAX_OPERATION_HISTORY, Mime, OpenTargets, Operation, cap_operation_history,
        classify_events, failed_operation_text, failed_operations_text,
        retry_extract_with_password,
    };

    fn event(kind: EventKind, path: &str) -> DebouncedEvent {
        DebouncedEvent::new(
            notify_debouncer_full::notify::Event::new(kind).add_path(PathBuf::from(path)),
            Instant::now(),
        )
    }

    fn classify(kind: EventKind, path: &str) -> ListingChange {
        classify_events(&[event(kind, path)], Path::new("/dir"))
    }

    #[test]
    fn reading_a_listed_file_does_not_touch_the_listing() {
        assert!(matches!(
            classify(EventKind::Access(AccessKind::Read), "/dir/file.log"),
            ListingChange::None
        ));
    }

    #[test]
    fn writing_a_listed_file_refreshes_only_that_entry() {
        let ListingChange::Entries(paths) = classify(
            EventKind::Modify(ModifyKind::Data(DataChange::Content)),
            "/dir/file.log",
        ) else {
            panic!("a write to a listed file should refresh that entry");
        };
        assert_eq!(vec![PathBuf::from("/dir/file.log")], paths);

        assert!(matches!(
            classify(
                EventKind::Modify(ModifyKind::Metadata(MetadataKind::Any)),
                "/dir/file.log"
            ),
            ListingChange::Entries(_)
        ));
    }

    #[test]
    fn changing_hidden_names_requires_a_listing_rescan() {
        assert!(matches!(
            classify(
                EventKind::Modify(ModifyKind::Data(DataChange::Content)),
                "/dir/.hidden"
            ),
            ListingChange::Structural
        ));
        assert!(matches!(
            classify(
                EventKind::Modify(ModifyKind::Metadata(MetadataKind::Any)),
                "/dir/.hidden"
            ),
            ListingChange::Structural
        ));
        assert!(matches!(
            classify(EventKind::Access(AccessKind::Read), "/dir/.hidden"),
            ListingChange::None
        ));
    }

    #[test]
    fn writing_below_a_listed_directory_changes_nothing_on_screen() {
        assert!(matches!(
            classify(
                EventKind::Modify(ModifyKind::Data(DataChange::Content)),
                "/dir/sub/file.log"
            ),
            ListingChange::None
        ));
    }

    #[test]
    fn adding_removing_or_renaming_an_entry_needs_a_rescan() {
        for kind in [
            EventKind::Create(CreateKind::File),
            EventKind::Remove(RemoveKind::File),
            EventKind::Modify(ModifyKind::Name(RenameMode::Both)),
            EventKind::Any,
        ] {
            assert!(
                matches!(classify(kind, "/dir/file.log"), ListingChange::Structural),
                "{kind:?} should rescan"
            );
        }
    }

    #[test]
    fn a_tab_path_keeps_its_trailing_separator() {
        // Location::normalize() appends one unconditionally, so the comparison against an
        // event path's parent has to tolerate it.
        let dir = PathBuf::from("/dir/");
        let change = classify_events(
            &[event(
                EventKind::Modify(ModifyKind::Data(DataChange::Content)),
                "/dir/file.log",
            )],
            &dir,
        );
        assert!(
            matches!(change, ListingChange::Entries(_)),
            "a write to a listed file was not recognised against a normalized tab path"
        );
    }

    #[test]
    fn events_outside_the_listed_directory_are_ignored() {
        assert!(matches!(
            classify(EventKind::Create(CreateKind::File), "/elsewhere/file.log"),
            ListingChange::None
        ));
    }

    #[test]
    fn cap_operation_history_keeps_the_newest_and_bounds_growth() {
        let extra = 50;
        let mut history: BTreeMap<u64, ()> = (0..(MAX_OPERATION_HISTORY as u64 + extra))
            .map(|id| (id, ()))
            .collect();

        cap_operation_history(&mut history);

        assert_eq!(history.len(), MAX_OPERATION_HISTORY);
        assert!(!history.contains_key(&0));
        assert!(history.contains_key(&(MAX_OPERATION_HISTORY as u64 + extra - 1)));
    }

    #[test]
    fn retrying_an_evicted_operation_skips_instead_of_panicking() {
        let failed: BTreeMap<u64, (Operation, (), ())> = BTreeMap::new();

        assert!(retry_extract_with_password(&failed, 7, "pw".to_string()).is_none());
    }

    #[test]
    fn retrying_a_non_extract_operation_is_refused() {
        let mut failed = BTreeMap::new();
        failed.insert(
            7,
            (
                Operation::PermanentlyDelete {
                    paths: Box::from([]),
                },
                (),
                (),
            ),
        );

        assert!(retry_extract_with_password(&failed, 7, "pw".to_string()).is_none());
    }

    #[test]
    fn retrying_an_extract_carries_the_password() {
        let mut failed = BTreeMap::new();
        failed.insert(
            7,
            (
                Operation::Extract {
                    paths: Box::from([PathBuf::from("archive.zip")]),
                    to: PathBuf::from("out"),
                    password: None,
                },
                (),
                (),
            ),
        );

        let retry = retry_extract_with_password(&failed, 7, "pw".to_string());

        assert!(matches!(
            retry,
            Some(Operation::Extract { password: Some(password), .. }) if password == "pw"
        ));
    }

    fn archive_paths(names: &[&str]) -> (Vec<PathBuf>, Vec<Mime>) {
        let zip: Mime = "application/zip".parse().expect("a valid mime");
        (
            names.iter().map(PathBuf::from).collect(),
            vec![zip; names.len()],
        )
    }

    #[test]
    fn archives_open_in_the_order_they_were_given() {
        let (paths, mimes) = archive_paths(&["/b.zip", "/a.zip", "/c.zip"]);

        match OpenTargets::classify(paths.clone(), mimes) {
            OpenTargets::Archives(ordered) => assert_eq!(ordered, paths),
            OpenTargets::ByMime(_) => panic!("archives should not be grouped by mime"),
        }
    }

    #[test]
    fn one_non_archive_groups_every_path_by_mime() {
        let (mut paths, mut mimes) = archive_paths(&["/a.zip"]);
        paths.push(PathBuf::from("/notes.txt"));
        mimes.push("text/plain".parse().expect("a valid mime"));

        match OpenTargets::classify(paths, mimes) {
            OpenTargets::ByMime(groups) => assert_eq!(groups.len(), 2),
            OpenTargets::Archives(_) => panic!("a text file is not an archive"),
        }
    }

    fn delete_failure(id: u64, err: &str) -> BTreeMap<u64, (Operation, (), String)> {
        let mut failed = BTreeMap::new();
        failed.insert(
            id,
            (
                Operation::Delete {
                    paths: vec![PathBuf::from("doc.txt")],
                },
                (),
                err.to_string(),
            ),
        );
        failed
    }

    #[test]
    fn a_failed_operation_carries_both_the_operation_and_its_error() {
        let failed = delete_failure(7, "Permission denied");

        let text = failed_operation_text(&failed, 7).expect("text for a failure still in history");

        assert!(text.contains("Delete"));
        assert!(text.contains("doc.txt"));
        assert!(text.ends_with("Permission denied"));
    }

    #[test]
    fn a_failure_no_longer_in_the_history_has_no_text() {
        let failed: BTreeMap<u64, (Operation, (), String)> = BTreeMap::new();

        assert!(failed_operation_text(&failed, 7).is_none());
    }

    #[test]
    fn several_failures_read_as_one_blank_line_separated_block() {
        let mut failed = delete_failure(7, "Permission denied");
        failed.extend(delete_failure(8, "Read-only file system"));

        let text = failed_operations_text(&failed, &[7, 8]);

        assert_eq!(
            text,
            [
                failed_operation_text(&failed, 7).expect("the first failure"),
                failed_operation_text(&failed, 8).expect("the second failure"),
            ]
            .join("\n\n")
        );
    }

    #[test]
    fn failures_no_longer_in_the_history_are_skipped_rather_than_left_blank() {
        let failed = delete_failure(7, "Permission denied");

        let text = failed_operations_text(&failed, &[6, 7, 8]);

        assert_eq!(
            text,
            failed_operation_text(&failed, 7).expect("the only failure still in history")
        );
    }
    #[test]
    fn pasted_files_preserve_existing_content() -> std::io::Result<()> {
        let dir = tempfile::TempDir::new()?;
        let first = super::write_unique_file(dir.path(), "pasted.txt", b"first")?;
        let second = super::write_unique_file(dir.path(), "pasted.txt", b"second")?;
        assert_ne!(first, second);
        assert_eq!(std::fs::read(first)?, b"first");
        assert_eq!(std::fs::read(second)?, b"second");
        Ok(())
    }

    #[test]
    fn concurrent_pastes_keep_both_payloads() -> std::io::Result<()> {
        let dir = tempfile::TempDir::new()?;
        let barrier = std::sync::Barrier::new(2);
        let (first, second) = std::thread::scope(|scope| {
            let first = scope.spawn(|| {
                barrier.wait();
                super::write_unique_file(dir.path(), "pasted.txt", b"first")
            });
            let second = scope.spawn(|| {
                barrier.wait();
                super::write_unique_file(dir.path(), "pasted.txt", b"second")
            });
            (first.join().unwrap(), second.join().unwrap())
        });
        let (first, second) = (first?, second?);
        assert_ne!(first, second);
        assert_eq!(std::fs::read(first)?, b"first");
        assert_eq!(std::fs::read(second)?, b"second");
        Ok(())
    }

    fn test_app(dir: &tempfile::TempDir) -> super::App {
        use cosmic::Application;
        let mut core = cosmic::app::Core::default();
        core.set_main_window_id(Some(cosmic::iced::window::Id::unique()));
        let location = crate::tab::Location::Path(dir.path().to_path_buf());
        let (mut app, _) = super::App::init(
            core,
            super::Flags {
                config_handler: None,
                config: crate::config::Config::default(),
                state_handler: None,
                state: crate::config::State::default(),
                mode: super::Mode::App,
                locations: vec![location.clone()],
                uris: Vec::new(),
            },
        );
        let _ = app.open_tab(location, true, None);
        app
    }

    async fn task_result(task: super::Task<super::Message>) -> super::Message {
        use cosmic::iced::futures::StreamExt;
        let mut stream = cosmic::iced::runtime::task::into_stream(task).unwrap();
        match stream.next().await.unwrap() {
            cosmic::iced::runtime::Action::Output(cosmic::Action::App(message)) => message,
            _ => panic!("the task must return an application message"),
        }
    }

    #[tokio::test]
    async fn failed_launch_resolution_opens_a_usable_tab_in_a_tabless_app() -> std::io::Result<()> {
        use cosmic::Application;
        use cosmic::iced::futures::StreamExt;

        let dir = tempfile::TempDir::new()?;
        let mut core = cosmic::app::Core::default();
        core.set_main_window_id(Some(cosmic::iced::window::Id::unique()));
        let (mut app, _) = super::App::init(
            core,
            super::Flags {
                config_handler: None,
                config: crate::config::Config::default(),
                state_handler: None,
                state: crate::config::State::default(),
                mode: super::Mode::App,
                locations: vec![crate::tab::Location::Path(dir.path().to_path_buf())],
                uris: Vec::new(),
            },
        );
        assert!(app.tab_model.entity_at(0).is_none());

        let fallback = app.update(super::Message::OpenLaunchLocations(Vec::new()));
        let entity = app
            .tab_model
            .entity_at(0)
            .expect("a fallback tab must open");
        assert_eq!(app.tab_model.active(), entity);
        let tab = app.tab_model.data::<crate::tab::Tab>(entity).unwrap();
        assert_eq!(tab.location.path_opt(), Some(&std::env::current_dir()?));
        let mut fallback = cosmic::iced::runtime::task::into_stream(fallback).unwrap();
        let mut listed = false;
        while let Some(action) = fallback.next().await {
            if let cosmic::iced::runtime::Action::Output(cosmic::Action::App(
                message @ super::Message::TabRescan(..),
            )) = action
            {
                let _ = app.update(message);
                listed = true;
            }
        }
        assert!(listed, "the fallback tab must complete its initial listing");
        assert!(
            app.tab_model
                .data::<crate::tab::Tab>(entity)
                .unwrap()
                .items_opt()
                .is_some()
        );
        Ok(())
    }

    #[test]
    fn clean_exit_waits_for_queued_persistence() -> std::io::Result<()> {
        use cosmic::Application;
        const CHILD_PROFILE: &str = "COSMIC_TEST_EXIT_PROFILE";
        if let Some(profile) = std::env::var_os(CHILD_PROFILE) {
            let profile = std::path::PathBuf::from(profile);
            return tokio::runtime::Runtime::new().unwrap().block_on(async {
                let dir = tempfile::TempDir::new_in(&profile)?;
                let path = dir.path().join("saved-before-exit.txt");
                std::fs::write(&path, b"content")?;
                let mut app = test_app(&dir);
                crate::recents::record_recently_used(vec![path], "test");
                app.core.set_main_window_id(None);
                let _ = task_result(app.update(super::Message::MaybeExit)).await;
                let history =
                    std::fs::read_to_string(profile.join(".local/share/recently-used.xbel"))?;
                assert!(history.contains("saved-before-exit.txt"));
                std::fs::write(profile.join("drained"), b"complete")
            });
        }
        let profile = tempfile::TempDir::new()?;
        let status = std::process::Command::new(std::env::current_exe()?)
            .args([
                "--exact",
                "app::tests::clean_exit_waits_for_queued_persistence",
                "--nocapture",
            ])
            .env(CHILD_PROFILE, profile.path())
            .env("HOME", profile.path())
            .env("XDG_DATA_HOME", profile.path().join(".local/share"))
            .env("XDG_CONFIG_HOME", profile.path().join(".config"))
            .status()?;
        assert!(status.success());
        assert!(profile.path().join("drained").is_file());
        Ok(())
    }

    #[tokio::test]
    async fn a_failed_watch_does_not_resubmit_without_a_new_request() -> std::io::Result<()> {
        use cosmic::Application;
        let dir = tempfile::TempDir::new()?;
        let path = dir.path().join("missing");
        let mut app = test_app(&dir);
        let entity = app.tab_model.active();
        app.tab_model
            .data_mut::<crate::tab::Tab>(entity)
            .unwrap()
            .location = crate::tab::Location::Path(path);
        let watcher = notify_debouncer_full::new_debouncer(
            std::time::Duration::from_millis(100),
            None,
            |_events| {},
        )
        .unwrap();
        app.watcher_opt = Some((watcher, Default::default()));
        app.watcher_update_pending = false;
        let result = task_result(app.update_watcher()).await;
        let followup = app.update(result);
        assert_eq!(followup.units(), 0);
        assert!(app.watcher_opt.as_ref().unwrap().1.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn sidebar_metadata_from_an_older_menu_cannot_replace_current_facts()
    -> std::io::Result<()> {
        use cosmic::Application;
        let dir = tempfile::TempDir::new()?;
        let path = dir.path().join("favorite");
        std::fs::write(&path, b"file")?;
        let mut app = test_app(&dir);
        let entity = app
            .nav_model
            .insert()
            .text("Favorite")
            .data(crate::tab::Location::Path(path.clone()))
            .id();
        let old = task_result(app.update(super::Message::NavBarContext(entity))).await;
        std::fs::remove_file(&path)?;
        std::fs::create_dir(&path)?;
        let current = task_result(app.update(super::Message::NavBarContext(entity))).await;
        let _ = app.update(current);
        let _ = app.update(old);
        assert!(app.nav_context_info.as_ref().unwrap().1.is_dir);
        assert!(!app.nav_context_info.as_ref().unwrap().1.is_file);
        Ok(())
    }

    #[tokio::test]
    async fn an_unmounted_path_invalidates_pending_navigation() -> std::io::Result<()> {
        use cosmic::Application;
        let dir = tempfile::TempDir::new()?;
        let mut app = test_app(&dir);
        let entity = app.tab_model.active();
        let request = app.navigation.begin();
        let _ = app.leave_unmounted_paths(&[dir.path().to_path_buf()]);
        let _ = app.update(super::Message::NavigationResolved {
            entity,
            request,
            target: Some((crate::tab::Location::Path(dir.path().to_path_buf()), None)),
        });
        assert_eq!(
            app.tab_model
                .data::<crate::tab::Tab>(entity)
                .unwrap()
                .location
                .path_opt(),
            Some(&crate::home_dir())
        );
        Ok(())
    }

    #[tokio::test]
    async fn sidebar_drop_navigation_invalidates_pending_location_resolution() -> std::io::Result<()>
    {
        use cosmic::Application;
        let dir = tempfile::TempDir::new()?;
        let target = crate::tab::Location::Path(dir.path().join("target"));
        std::fs::create_dir(target.path_opt().unwrap())?;
        let mut app = test_app(&dir);
        let entity = app.tab_model.active();
        let request = app.navigation.begin();
        app.nav_dnd_hover = Some((
            target.clone(),
            std::time::Instant::now() - super::HOVER_DURATION,
        ));
        let _ = app.update(super::Message::DndHoverLocTimeout(target.clone()));
        let _ = app.update(super::Message::NavigationResolved {
            entity,
            request,
            target: Some((crate::tab::Location::Path(dir.path().to_path_buf()), None)),
        });
        assert_eq!(
            app.tab_model
                .data::<crate::tab::Tab>(entity)
                .unwrap()
                .location,
            target
        );
        Ok(())
    }

    #[tokio::test]
    async fn review_an_old_scan_cannot_replace_a_newer_listing() -> std::io::Result<()> {
        use cosmic::Application;
        let dir = tempfile::TempDir::new()?;
        let mut app = test_app(&dir);
        let entity = app.tab_model.active();
        let location = crate::tab::Location::Path(dir.path().to_path_buf());
        let old_result = task_result(app.rescan_tab(entity, location.clone(), None)).await;
        std::fs::write(dir.path().join("new.txt"), b"new")?;
        let new_result = task_result(app.rescan_tab(entity, location, None)).await;
        let _ = app.update(new_result);
        let _ = app.update(old_result);
        let tab = app.tab_model.data::<crate::tab::Tab>(entity).unwrap();
        assert_eq!(tab.items_opt().unwrap().len(), 1);
        assert_eq!(tab.items_opt().unwrap()[0].name, "new.txt");
        Ok(())
    }

    #[tokio::test]
    async fn a_direct_scan_and_watcher_scan_keep_their_own_completions() -> std::io::Result<()> {
        use cosmic::Application;
        let dir = tempfile::TempDir::new()?;
        let mut app = test_app(&dir);
        let entity = app.tab_model.active();
        let location = crate::tab::Location::Path(dir.path().to_path_buf());
        let initial = task_result(app.rescan_tab(entity, location.clone(), None)).await;
        let _ = app.update(initial);
        let old = task_result(app.rescan_tab_if_idle(entity, location.clone()).unwrap()).await;
        std::fs::write(dir.path().join("new.txt"), b"new")?;
        let current = task_result(app.rescan_tab(entity, location.clone(), None)).await;
        assert!(app.rescan_tab_if_idle(entity, location.clone()).is_none());
        let _ = app.update(current);
        let _ = app.update(old);
        assert!(app.rescan_tab_if_idle(entity, location).is_none());
        assert_eq!(
            app.tab_model
                .data::<crate::tab::Tab>(entity)
                .unwrap()
                .items_opt()
                .unwrap()[0]
                .name,
            "new.txt"
        );
        Ok(())
    }

    #[tokio::test]
    async fn a_failed_scan_releases_only_its_own_request() -> std::io::Result<()> {
        use cosmic::Application;
        let dir = tempfile::TempDir::new()?;
        let mut app = test_app(&dir);
        let entity = app.tab_model.active();
        let scans = &mut app
            .tab_model
            .data_mut::<crate::tab::Tab>(entity)
            .unwrap()
            .scans;
        let old = scans.begin();
        let current = scans.begin();
        let _ = app.update(super::Message::TabRescanFailed(entity, old));
        assert!(
            app.tab_model
                .data_mut::<crate::tab::Tab>(entity)
                .unwrap()
                .scans
                .defer_if_running()
        );
        let _ = app.update(super::Message::TabRescanFailed(entity, current));
        assert_eq!(
            app.tab_model
                .data_mut::<crate::tab::Tab>(entity)
                .unwrap()
                .scans
                .finish(current),
            None
        );
        assert!(
            app.tab_model
                .data_mut::<crate::tab::Tab>(entity)
                .unwrap()
                .scans
                .defer_if_running()
        );
        Ok(())
    }

    #[tokio::test]
    async fn review_a_pending_sidebar_stat_does_not_offer_directory_actions() -> std::io::Result<()>
    {
        let dir = tempfile::TempDir::new()?;
        let mut app = test_app(&dir);
        let path = dir.path().join("notes.txt");
        std::fs::write(&path, b"notes")?;
        let entity = app
            .nav_model
            .insert()
            .text("Notes")
            .data(crate::tab::Location::Path(path))
            .id();
        app.config.context_actions = [
            crate::context_action::ContextActionSelection::Files,
            crate::context_action::ContextActionSelection::Folders,
            crate::context_action::ContextActionSelection::Any,
        ]
        .into_iter()
        .map(|selection| crate::context_action::ContextActionPreset {
            name: "Action".into(),
            selection,
            ..Default::default()
        })
        .collect();
        app.nav_context_info = None;
        let items = app.nav_context_items(entity);
        assert!(!items.iter().any(|item| matches!(
            item,
            cosmic::widget::menu::Item::Button(
                _,
                _,
                super::NavMenuAction::OpenInNewTab(_)
                    | super::NavMenuAction::OpenInNewWindow(_)
                    | super::NavMenuAction::RunContextAction(_, 0 | 1)
            )
        )));
        assert!(items.iter().any(|item| matches!(
            item,
            cosmic::widget::menu::Item::Button(_, _, super::NavMenuAction::Preview(_))
        )));
        assert!(items.iter().any(|item| matches!(
            item,
            cosmic::widget::menu::Item::Button(_, _, super::NavMenuAction::RunContextAction(_, 2))
        )));
        Ok(())
    }

    #[tokio::test]
    async fn sidebar_actions_follow_loaded_file_and_folder_metadata() -> std::io::Result<()> {
        let dir = tempfile::TempDir::new()?;
        let mut app = test_app(&dir);
        let entity = app
            .nav_model
            .insert()
            .text("Favorite")
            .data(crate::tab::Location::Path(dir.path().to_path_buf()))
            .id();
        app.nav_context_info = Some((
            entity,
            super::NavContextInfo {
                is_file: true,
                ..Default::default()
            },
        ));
        assert!(app.nav_context_items(entity).iter().any(|item| matches!(
            item,
            cosmic::widget::menu::Item::Button(_, _, super::NavMenuAction::OpenWith(_))
        )));
        assert!(!app.nav_context_items(entity).iter().any(|item| matches!(
            item,
            cosmic::widget::menu::Item::Button(_, _, super::NavMenuAction::OpenInNewTab(_))
        )));
        app.nav_context_info = Some((
            entity,
            super::NavContextInfo {
                is_dir: true,
                ..Default::default()
            },
        ));
        assert!(app.nav_context_items(entity).iter().any(|item| matches!(
            item,
            cosmic::widget::menu::Item::Button(_, _, super::NavMenuAction::OpenInNewTab(_))
        )));
        Ok(())
    }

    #[tokio::test]
    async fn virtual_sidebar_locations_do_not_wait_for_path_metadata() -> std::io::Result<()> {
        let dir = tempfile::TempDir::new()?;
        let mut app = test_app(&dir);
        let entity = app
            .nav_model
            .insert()
            .text("Recent")
            .data(crate::tab::Location::Recents)
            .id();
        app.nav_context_info = None;
        assert!(app.nav_context_items(entity).iter().any(|item| matches!(
            item,
            cosmic::widget::menu::Item::Button(_, _, super::NavMenuAction::OpenInNewTab(_))
        )));
        Ok(())
    }

    #[tokio::test]
    async fn a_current_preview_result_opens_its_item() -> std::io::Result<()> {
        use cosmic::Application;
        let dir = tempfile::TempDir::new()?;
        let mut app = test_app(&dir);
        let request = app.preview_request.begin();
        let item =
            crate::tab::item_from_path(dir.path(), crate::config::IconSizes::default()).unwrap();
        let _ = app.update(super::Message::PreviewReady(request, None, Box::new(item)));
        assert!(matches!(
            app.context_page,
            super::ContextPage::Preview(_, super::PreviewKind::Custom(_))
        ));
        assert!(app.core.window.show_context);
        Ok(())
    }

    #[tokio::test]
    async fn a_preview_result_cannot_reopen_dismissed_details() -> std::io::Result<()> {
        use cosmic::Application;
        let dir = tempfile::TempDir::new()?;
        let mut app = test_app(&dir);
        let request = app.preview_request.begin();
        let _ = app.update(super::Message::SetShowDetails(false));
        let item =
            crate::tab::item_from_path(dir.path(), crate::config::IconSizes::default()).unwrap();
        let _ = app.update(super::Message::PreviewReady(request, None, Box::new(item)));
        assert!(!app.core.window.show_context);
        assert!(!matches!(
            app.context_page,
            super::ContextPage::Preview(_, super::PreviewKind::Custom(_))
        ));
        Ok(())
    }

    #[tokio::test]
    async fn a_preview_result_cannot_replace_newer_context() -> std::io::Result<()> {
        use cosmic::Application;
        let dir = tempfile::TempDir::new()?;
        let mut app = test_app(&dir);
        let request = app.preview_request.begin();
        let _ = app.update(super::Message::ToggleContextPage(
            super::ContextPage::Settings,
        ));
        let item =
            crate::tab::item_from_path(dir.path(), crate::config::IconSizes::default()).unwrap();
        let _ = app.update(super::Message::PreviewReady(request, None, Box::new(item)));
        assert!(matches!(app.context_page, super::ContextPage::Settings));
        Ok(())
    }

    #[tokio::test]
    async fn opening_a_new_tab_invalidates_pending_navigation() -> std::io::Result<()> {
        use cosmic::Application;
        let dir = tempfile::TempDir::new()?;
        let mut app = test_app(&dir);
        let request = app.navigation.begin();
        let previous_tab = app.tab_model.active();
        let _ = app.update(super::Message::TabNew);
        assert_ne!(app.tab_model.active(), previous_tab);
        assert!(!app.navigation.is_current(&request));
        Ok(())
    }

    #[tokio::test]
    async fn an_obsolete_mount_cannot_redirect_the_active_tab() -> std::io::Result<()> {
        use cosmic::Application;
        let dir = tempfile::TempDir::new()?;
        let mut app = test_app(&dir);
        let entity = app
            .nav_model
            .insert()
            .text("Trash")
            .data(crate::tab::Location::Trash)
            .id();
        let tab_entity = app.tab_model.active();
        let request = app.navigation.begin();
        let _ = app.navigation.begin();
        let _ = app.update(super::Message::NavigationMounted {
            entity,
            tab_entity,
            request,
        });
        assert_eq!(
            app.tab_model
                .data::<crate::tab::Tab>(tab_entity)
                .unwrap()
                .location
                .path_opt(),
            Some(&dir.path().to_path_buf())
        );
        Ok(())
    }

    #[tokio::test]
    async fn a_current_mount_continues_the_requested_navigation() -> std::io::Result<()> {
        use cosmic::Application;
        let dir = tempfile::TempDir::new()?;
        let mut app = test_app(&dir);
        let entity = app
            .nav_model
            .insert()
            .text("Trash")
            .data(crate::tab::Location::Trash)
            .id();
        let tab_entity = app.tab_model.active();
        let request = app.navigation.begin();
        let _ = app.update(super::Message::NavigationMounted {
            entity,
            tab_entity,
            request,
        });
        assert_eq!(
            app.tab_model
                .data::<crate::tab::Tab>(tab_entity)
                .unwrap()
                .location,
            crate::tab::Location::Trash
        );
        Ok(())
    }

    #[tokio::test]
    async fn an_older_mime_reload_cannot_consume_the_current_request() -> std::io::Result<()> {
        use cosmic::Application;
        let dir = tempfile::TempDir::new()?;
        let mut app = test_app(&dir);
        let stale = app.mime_cache_request.begin();
        let current = app.mime_cache_request.begin();
        let _ = app.update(super::Message::MimeAppCacheLoaded(
            stale,
            crate::mime_app::MimeAppCache::empty(),
        ));
        assert!(app.mime_cache_request.is_current(&current));
        let _ = app.update(super::Message::MimeAppCacheLoaded(
            current.clone(),
            crate::mime_app::MimeAppCache::empty(),
        ));
        assert!(!app.mime_cache_request.is_current(&current));
        Ok(())
    }
    #[tokio::test]
    async fn searching_invalidates_a_pending_preview() -> std::io::Result<()> {
        use cosmic::Application;
        let dir = tempfile::TempDir::new()?;
        let mut app = test_app(&dir);
        let request = app.preview_request.begin();
        let _ = app.update(super::Message::SearchInput("report".into()));
        let item =
            crate::tab::item_from_path(dir.path(), crate::config::IconSizes::default()).unwrap();
        let _ = app.update(super::Message::PreviewReady(request, None, Box::new(item)));
        assert!(!matches!(
            app.context_page,
            super::ContextPage::Preview(_, super::PreviewKind::Custom(_))
        ));
        Ok(())
    }

    // A folder opened from the Trash is a plain path location, so its children are already in
    // the Trash: deleting them must ask to remove them for good rather than trash them again.
    #[tokio::test]
    async fn deleting_a_trashed_folders_children_asks_to_delete_them_permanently()
    -> std::io::Result<()> {
        use cosmic::Application;

        let dir = tempfile::TempDir::new()?;
        let child = dir.path().join("trashed.txt");
        let local = dir.path().join("local.txt");
        std::fs::write(&child, b"")?;
        std::fs::write(&local, b"")?;
        let trashed = crate::tab::item_from_trash_entry(
            trash::TrashItem {
                id: child.clone().into_os_string(),
                name: "trashed.txt".into(),
                original_parent: dir.path().join("documents"),
                time_deleted: 0,
            },
            trash::TrashItemMetadata {
                size: trash::TrashItemSize::Bytes(0),
            },
            crate::config::IconSizes::default(),
        );
        let kept = crate::tab::item_from_path(&local, crate::config::IconSizes::default())
            .expect("a written file should resolve to an item");

        let mut app = test_app(&dir);
        let entity = app.tab_model.active();
        let tab = app.tab_model.data_mut::<crate::tab::Tab>(entity).unwrap();
        tab.set_items(vec![trashed, kept]);
        tab.items_opt
            .as_mut()
            .unwrap()
            .iter_mut()
            .for_each(|item| item.selected = true);

        match task_result(app.update(super::Message::Delete(None))).await {
            super::Message::DeleteClassified {
                trash_paths,
                dialog_paths,
            } => {
                assert_eq!(trash_paths, vec![local]);
                assert_eq!(dialog_paths, vec![child]);
            }
            _ => panic!("deleting must classify the selection"),
        }
        Ok(())
    }

    // The menus offer "Move to Trash" for a local symlink whose target is gone, so deleting it
    // must trash the link rather than ask to delete it permanently.
    #[tokio::test]
    async fn deleting_a_dangling_symlink_trashes_it() -> std::io::Result<()> {
        use cosmic::Application;

        let dir = tempfile::TempDir::new()?;
        let link = dir.path().join("dangling");
        std::os::unix::fs::symlink(dir.path().join("missing"), &link)?;
        // Search lists a dangling link with the link's own metadata.
        let item = crate::tab::item_from_entry(
            link.clone(),
            "dangling".into(),
            std::fs::symlink_metadata(&link)?,
            crate::config::IconSizes::default(),
        );
        assert!(item.metadata.can_trash());

        let mut app = test_app(&dir);
        let entity = app.tab_model.active();
        let tab = app.tab_model.data_mut::<crate::tab::Tab>(entity).unwrap();
        tab.set_items(vec![item]);
        tab.items_opt.as_mut().unwrap()[0].selected = true;

        match task_result(app.update(super::Message::Delete(None))).await {
            super::Message::DeleteClassified {
                trash_paths,
                dialog_paths,
            } => {
                assert_eq!(trash_paths, vec![link]);
                assert!(dialog_paths.is_empty());
            }
            _ => panic!("deleting must classify the selection"),
        }
        Ok(())
    }
}
