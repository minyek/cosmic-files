// Copyright 2023 System76 <info@system76.com>
// SPDX-License-Identifier: GPL-3.0-only

use cosmic::app::cosmic::Cosmic;
use cosmic::app::{Core, Task, context_drawer};
use cosmic::cosmic_config::ConfigSet;
use cosmic::iced::core::SmolStr;
use cosmic::iced::core::widget::operation;
use cosmic::iced::futures::{self, SinkExt};
use cosmic::iced::keyboard::key::{Named, Physical};
use cosmic::iced::keyboard::{Event as KeyEvent, Key, Modifiers};
use cosmic::iced::platform_specific::shell::{self as iced_winit, SurfaceIdWrapper};
use cosmic::iced::widget::scrollable;
use cosmic::iced::widget::scrollable::AbsoluteOffset;
use cosmic::iced::{
    self, Alignment, Event, Length, Size, Subscription, event, mouse, stream, window,
};
use cosmic::widget::menu::key_bind::Modifier;
use cosmic::widget::menu::{Action as MenuAction, KeyBind};
use cosmic::widget::{self, Operation, segmented_button};
use cosmic::{Application, ApplicationExt, Element, cosmic_config, cosmic_theme, executor, theme};
use mime_guess::{Mime, mime};
use notify_debouncer_full::notify::{self, RecommendedWatcher};
use notify_debouncer_full::{DebouncedEvent, Debouncer, RecommendedCache, new_debouncer};
use rustc_hash::{FxHashMap, FxHashSet};
use std::any::TypeId;
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::time::{self, Instant};
use std::{env, fmt, fs};

use crate::app::{
    Action, ContextPage, Message as AppMessage, PreviewItem, PreviewKind, REPLACE_BUTTON_ID,
};
use crate::config::{
    Config, DialogConfig, State, TIME_CONFIG_ID, ThumbCfg, TimeConfig, TypeToSearch,
};
use crate::key_bind::key_binds;
use crate::localize::LANGUAGE_SORTER;
use crate::mounter::{MOUNTERS, MounterItem, MounterItems, MounterKey, MounterMessage};
use crate::tab::{self, HeadingOptions, ItemMetadata, Location, SearchLocation, Tab};
use crate::zoom::{zoom_in_view, zoom_out_view, zoom_to_default};
use crate::{fl, home_dir, menu, mime_icon};

#[derive(Clone, Debug)]
pub struct DialogMessage(cosmic::Action<Message>);

#[derive(Clone, Debug)]
pub enum DialogResult {
    Cancel,
    Open(Vec<PathBuf>),
}

#[derive(Clone, Debug)]
pub enum DialogKind {
    OpenFile,
    OpenFolder,
    OpenMultipleFiles,
    OpenMultipleFolders,
    SaveFile { filename: String },
}

impl DialogKind {
    pub fn title(&self) -> String {
        match self {
            Self::OpenFile => fl!("open-file"),
            Self::OpenFolder => fl!("open-folder"),
            Self::OpenMultipleFiles => fl!("open-multiple-files"),
            Self::OpenMultipleFolders => fl!("open-multiple-folders"),
            Self::SaveFile { .. } => fl!("save-file"),
        }
    }

    pub fn accept_label(&self) -> String {
        match self {
            Self::SaveFile { .. } => fl!("save"),
            _ => fl!("open"),
        }
    }

    pub const fn is_dir(&self) -> bool {
        matches!(self, Self::OpenFolder | Self::OpenMultipleFolders)
    }

    pub const fn multiple(&self) -> bool {
        matches!(self, Self::OpenMultipleFiles | Self::OpenMultipleFolders)
    }

    pub const fn save(&self) -> bool {
        matches!(self, Self::SaveFile { .. })
    }
}

#[derive(Clone, Debug)]
pub struct DialogChoiceOption {
    pub id: String,
    pub label: String,
}

impl AsRef<str> for DialogChoiceOption {
    fn as_ref(&self) -> &str {
        &self.label
    }
}

#[derive(Clone, Debug)]
pub enum DialogChoice {
    CheckBox {
        id: String,
        label: String,
        value: bool,
    },
    ComboBox {
        id: String,
        label: String,
        options: Vec<DialogChoiceOption>,
        selected: Option<usize>,
    },
}

#[derive(Clone, Debug)]
pub enum DialogFilterPattern {
    Glob(String),
    Mime(String),
}

#[derive(Clone, Debug)]
pub struct DialogFilter {
    pub label: String,
    pub patterns: Vec<DialogFilterPattern>,
}

impl AsRef<str> for DialogFilter {
    fn as_ref(&self) -> &str {
        &self.label
    }
}

#[derive(Clone, Debug)]
pub struct DialogLabelSpan {
    pub text: String,
    pub underline: bool,
}

#[derive(Clone, Debug)]
pub struct DialogLabel {
    pub spans: Vec<DialogLabelSpan>,
    pub key_bind_opt: Option<KeyBind>,
}

impl<T: AsRef<str>> From<T> for DialogLabel {
    fn from(text: T) -> Self {
        let mut spans = Vec::<DialogLabelSpan>::new();
        let mut key_bind_opt = None;
        let mut next_underline = false;
        for c in text.as_ref().chars() {
            let underline = next_underline;
            next_underline = false;

            if c == '_' && !underline {
                next_underline = true;
                continue;
            }

            if underline && key_bind_opt.is_none() {
                key_bind_opt = Some(KeyBind {
                    modifiers: vec![Modifier::Alt],
                    key: Key::Character(c.to_lowercase().to_string().into()),
                });
            }

            if let Some(span) = spans.last_mut()
                && underline == span.underline
            {
                span.text.push(c);
                continue;
            }

            spans.push(DialogLabelSpan {
                text: String::from(c),
                underline,
            });
        }

        Self {
            spans,
            key_bind_opt,
        }
    }
}

impl<'a, M: Clone + 'static> From<&'a DialogLabel> for Element<'a, M> {
    fn from(label: &'a DialogLabel) -> Self {
        let mut iced_spans: Vec<cosmic::iced::core::text::Span<'_, ()>> =
            Vec::with_capacity(label.spans.len());
        for span in &label.spans {
            iced_spans.push(cosmic::iced::widget::span(&span.text).underline(span.underline));
        }
        cosmic::iced::widget::rich_text(iced_spans).into()
    }
}

pub struct DialogSettings {
    app_id: String,
    kind: DialogKind,
    path_opt: Option<PathBuf>,
}

impl DialogSettings {
    pub fn new() -> Self {
        Default::default()
    }

    pub fn app_id(mut self, app_id: String) -> Self {
        self.app_id = app_id;
        self
    }

    pub fn kind(mut self, kind: DialogKind) -> Self {
        self.kind = kind;
        self
    }

    pub fn path(mut self, path: PathBuf) -> Self {
        self.path_opt = Some(path);
        self
    }
}

impl Default for DialogSettings {
    fn default() -> Self {
        Self {
            app_id: App::APP_ID.to_string(),
            kind: DialogKind::OpenFile,
            path_opt: None,
        }
    }
}

pub struct Dialog<M> {
    cosmic: Cosmic<App>,
    mapper: fn(DialogMessage) -> M,
    on_result: Box<dyn Fn(DialogResult) -> M>,
}

impl<M: Send + 'static> Dialog<M> {
    pub fn new(
        dialog_settings: DialogSettings,
        mapper: fn(DialogMessage) -> M,
        on_result: impl Fn(DialogResult) -> M + 'static,
    ) -> (Self, Task<M>) {
        //TODO: only do this once somehow?
        crate::localize::localize();

        let (config_handler, config) = Config::load();
        let (state_handler, state) = State::load();

        let mut settings = window::Settings {
            decorations: false,
            exit_on_close_request: false,
            min_size: Some(Size::new(360.0, 180.0)),
            resizable: true,
            size: Size::new(1024.0, 640.0),
            transparent: true,
            ..Default::default()
        };

        #[cfg(target_os = "linux")]
        {
            settings.platform_specific.application_id = dialog_settings.app_id;
        }

        let (window_id, window_command) = window::open(settings);

        let mut core = Core::default();
        core.set_main_window_id(Some(window_id));
        let flags = Flags {
            kind: dialog_settings.kind,
            // Tab::new normalizes this path lexically; canonicalizing it here would stat-walk
            // every component and freeze dialog launch on a slow mount.
            path_opt: dialog_settings.path_opt,
            window_id,
            config_handler,
            config,
            state_handler,
            state,
        };

        let (cosmic, cosmic_command) = Cosmic::<App>::init((core, flags));
        (
            Self {
                cosmic,
                mapper,
                on_result: Box::new(on_result),
            },
            Task::batch([
                window_command.map(|_id| cosmic::action::none()),
                cosmic_command
                    .map(DialogMessage)
                    .map(move |message| cosmic::action::app(mapper(message))),
            ]),
        )
    }

    pub fn set_title(&mut self, title: impl Into<String>) -> Task<M> {
        let mapper = self.mapper;
        self.cosmic.app.title = title.into();
        self.cosmic
            .app
            .update_title()
            .map(DialogMessage)
            .map(move |message| cosmic::action::app(mapper(message)))
    }

    pub fn set_accept_label(&mut self, accept_label: impl AsRef<str>) {
        self.cosmic.app.accept_label = DialogLabel::from(accept_label);
    }

    pub fn choices(&self) -> &[DialogChoice] {
        &self.cosmic.app.choices
    }

    pub fn set_choices(&mut self, choices: impl Into<Vec<DialogChoice>>) {
        self.cosmic.app.choices = choices.into();
    }

    pub fn filters(&self) -> (&[DialogFilter], Option<usize>) {
        (&self.cosmic.app.filters, self.cosmic.app.filter_selected)
    }

    pub fn set_filters(
        &mut self,
        filters: impl Into<Vec<DialogFilter>>,
        filter_selected: Option<usize>,
    ) -> Task<M> {
        let mapper = self.mapper;
        self.cosmic.app.filters = filters.into();
        self.cosmic.app.filter_selected = filter_selected;
        self.cosmic
            .app
            .rescan_tab(None)
            .map(DialogMessage)
            .map(move |message| cosmic::action::app(mapper(message)))
    }

    pub fn subscription(&self) -> Subscription<M> {
        self.cosmic
            .subscription()
            .map(DialogMessage)
            .with(self.mapper)
            .map(|(mapper, message)| mapper(message))
    }

    pub fn update(&mut self, message: DialogMessage) -> Task<M> {
        let mapper = self.mapper;
        let command = self
            .cosmic
            .update(message.0)
            .map(DialogMessage)
            .map(move |message| cosmic::action::app(mapper(message)));
        if self.cosmic.app.completed
            && let Some(result) = self.cosmic.app.result_opt.take()
        {
            #[cfg(feature = "wayland")]
            if !self.cosmic.surface_views.is_empty() {
                log::debug!("waiting for surfaces to close...");
                let mut tasks = Vec::new();
                for id in self.cosmic.surface_views.iter() {
                    match id.1.1 {
                        SurfaceIdWrapper::Window(id) => {
                            tasks.push(window::close::<M>(id).discard());
                        }
                        SurfaceIdWrapper::LayerSurface(id) => {
                            tasks.push(iced_winit::wayland::commands::layer_surface::destroy_layer_surface::<M>(id).discard());
                        }
                        SurfaceIdWrapper::Popup(id) => {
                            tasks.push(
                                iced_winit::wayland::commands::popup::destroy_popup::<M>(id)
                                    .discard(),
                            );
                        }
                        SurfaceIdWrapper::Subsurface(id) => {
                            tasks.push(
                                iced_winit::wayland::commands::subsurface::destroy_subsurface::<M>(
                                    id,
                                )
                                .discard(),
                            );
                        }
                        _ => {}
                    }
                }
                let on_result_message = (self.on_result)(result);

                tasks.push(Task::future(async move {
                    cosmic::action::app(on_result_message)
                }));
                tasks.push(command);
                return Task::batch(tasks);
            }
            let on_result_message = (self.on_result)(result);

            Task::batch([
                command,
                Task::future(async move { cosmic::action::app(on_result_message) }),
            ])
        } else {
            command
        }
    }

    pub fn view(&self, window_id: window::Id) -> Element<'_, M> {
        self.cosmic
            .view(window_id)
            .map(DialogMessage)
            .map(self.mapper)
    }

    pub const fn window_id(&self) -> window::Id {
        self.cosmic.app.flags.window_id
    }

    #[cfg(feature = "wayland")]
    pub fn contains_surface(&self, id: &window::Id) -> bool {
        self.cosmic.surface_views.contains_key(id)
    }
}

#[derive(Clone, Debug)]
enum DialogPage {
    NewFolder {
        parent: PathBuf,
        name: String,
        /// A create that already failed, shown until the name is edited.
        error: Option<String>,
    },
    Replace {
        request: widget::Id,
        filename: String,
        path: PathBuf,
    },
}

#[derive(Clone, Debug)]
struct Flags {
    kind: DialogKind,
    path_opt: Option<PathBuf>,
    window_id: window::Id,
    #[allow(dead_code)]
    config_handler: Option<cosmic_config::Config>,
    config: Config,
    state_handler: Option<cosmic_config::Config>,
    state: State,
}

/// Messages that are used specifically by our [`App`].
#[derive(Clone, Debug)]
enum Message {
    None,
    Cancel,
    Choice(usize, usize),
    Config(Config),
    DialogCancel,
    DialogComplete,
    DialogUpdate(DialogPage),
    Escape,
    Filename(String),
    Filter(usize),
    FolderCreated {
        request: widget::Id,
        location: Location,
        path: PathBuf,
        result: Result<(), String>,
    },
    Key(Modifiers, Key, Physical, Option<SmolStr>),
    ModifiersChanged(Modifiers),
    MounterItems(MounterKey, MounterItems),
    Mouse(window::Id, mouse::Button),
    NavigationResolved {
        request: widget::Id,
        target: Option<(Location, Option<PathBuf>)>,
    },
    NewFolder,
    NotifyEvents(Vec<DebouncedEvent>),
    NotifyWatcher(WatcherWrapper),
    WatcherUpdated(WatcherWrapper, FxHashSet<PathBuf>, FxHashSet<PathBuf>),
    Open,
    Preview,
    PersistenceFlushed,
    Save(bool),
    SaveChecked {
        request: widget::Id,
        replace: bool,
        path: PathBuf,
        existing: Option<bool>,
    },
    ScrollTab(i16),
    SearchActivate,
    SearchClear,
    SearchInput(String),
    Surface(cosmic::surface::Action<Message>),
    #[allow(clippy::enum_variant_names)]
    TabMessage(tab::Message),
    TabRescan(
        u64,
        Location,
        Option<Box<tab::Item>>,
        Vec<tab::Item>,
        Option<Vec<PathBuf>>,
    ),
    TabRescanFailed(u64),
    TabView(tab::View),
    TimeConfigChange(TimeConfig),
    ToggleFoldersFirst,
    ToggleShowHidden,
    ZoomDefault,
    ZoomIn,
    ZoomOut,
}

impl From<AppMessage> for Message {
    fn from(app_message: AppMessage) -> Self {
        match app_message {
            AppMessage::None => Self::None,
            AppMessage::Preview(_entity_opt) => Self::Preview,
            AppMessage::SearchActivate => Self::SearchActivate,
            AppMessage::ScrollTab(scroll_speed) => Self::ScrollTab(scroll_speed),
            AppMessage::TabMessage(_entity_opt, tab_message) => Self::TabMessage(tab_message),
            AppMessage::TabView(_entity_opt, view) => Self::TabView(view),
            AppMessage::ToggleFoldersFirst => Self::ToggleFoldersFirst,
            AppMessage::ToggleShowHidden => Self::ToggleShowHidden,
            AppMessage::ZoomDefault(_entity_opt) => Self::ZoomDefault,
            AppMessage::ZoomIn(_entity_opt) => Self::ZoomIn,
            AppMessage::ZoomOut(_entity_opt) => Self::ZoomOut,
            AppMessage::NewItem(_entity_opt, true) => Self::NewFolder,
            AppMessage::Surface(action) => Self::Surface(action.map(Self::from)),
            unsupported => {
                log::warn!("{unsupported:?} not supported in dialog mode");
                Self::None
            }
        }
    }
}

pub struct MounterData(MounterKey, MounterItem);

struct WatcherWrapper {
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

/// The [`App`] stores application-specific state.
struct App {
    core: Core,
    flags: Flags,
    title: String,
    accept_label: DialogLabel,
    choices: Vec<DialogChoice>,
    context_page: ContextPage,
    dialog_pages: VecDeque<DialogPage>,
    dialog_text_input: widget::Id,
    filters: Vec<DialogFilter>,
    filter_selected: Option<usize>,
    filename_id: widget::Id,
    modifiers: Modifiers,
    mounter_items: FxHashMap<MounterKey, MounterItems>,
    nav_model: segmented_button::SingleSelectModel,
    navigation: crate::navigation::PendingRequest,
    save_request: crate::navigation::PendingRequest,
    result_opt: Option<DialogResult>,
    completed: bool,
    search_id: widget::Id,
    tab: Tab,
    key_binds: HashMap<KeyBind, Action>,
    watcher_opt: Option<(
        Debouncer<RecommendedWatcher, RecommendedCache>,
        FxHashSet<PathBuf>,
    )>,
    watcher_update_pending: bool,
    auto_scroll_speed: Option<i16>,
    type_select_prefix: String,
    type_select_last_key: Option<Instant>,
}

impl App {
    fn close(&self) -> Task<Message> {
        Task::future(async {
            crate::persistence::flush().await;
            cosmic::action::app(Message::PersistenceFlushed)
        })
    }

    fn invalidate_save(&mut self) {
        self.save_request.invalidate();
        self.dialog_pages
            .retain(|page| !matches!(page, DialogPage::Replace { .. }));
    }

    fn check_save(&mut self, path: PathBuf, replace: bool) -> Task<Message> {
        self.invalidate_save();
        let request = self.save_request.begin();
        Task::future(async move {
            let stat_path = path.clone();
            match tokio::task::spawn_blocking(move || {
                stat_path.metadata().ok().map(|metadata| metadata.is_dir())
            })
            .await
            {
                Ok(existing) => cosmic::action::app(Message::SaveChecked {
                    request,
                    replace,
                    path,
                    existing,
                }),
                Err(err) => {
                    log::warn!("failed to check {}: {}", path.display(), err);
                    cosmic::action::none()
                }
            }
        })
    }

    /// Whether `name` exists in `parent` and whether it is a directory, read from the picker's
    /// scanned listing: `view` calls this every frame, where a `stat` would block on a slow
    /// mount. The listing is filtered to the picker's file types and may not be showing
    /// `parent` at all, so `None` means "not found here", not "does not exist" -- anything that
    /// overwrites or creates has to establish that for itself.
    fn existing_entry_in_dir(&self, parent: &Path, name: &str) -> Option<bool> {
        self.tab
            .lists_directory(parent)
            .then(|| self.tab.find_entry_is_dir(name))
            .flatten()
    }

    fn button_view(&self) -> Element<'_, Message> {
        let cosmic_theme::Spacing {
            space_xxxs,
            space_xxs,
            space_xs,
            space_s,
            space_l,
            ..
        } = theme::spacing();
        let is_condensed = self.core().is_condensed();

        let mut col = widget::column::with_capacity(2).spacing(space_xxs);
        if let DialogKind::SaveFile { filename } = &self.flags.kind {
            col = col.push(
                widget::text_input("", filename)
                    .id(self.filename_id.clone())
                    .double_click_select_delimiter('.')
                    .on_input(Message::Filename)
                    .on_submit(|_| Message::Save(false)),
            );
        }

        let mut row = widget::row::with_capacity(
            usize::from(!self.filters.is_empty())
                + self.choices.len() * 2
                + if is_condensed { 0 } else { 3 },
        )
        .align_y(Alignment::Center)
        .spacing(space_xxs);
        if !self.filters.is_empty() {
            row = row.push(widget::dropdown(
                &self.filters,
                self.filter_selected,
                Message::Filter,
            ));
        }
        for (choice_i, choice) in self.choices.iter().enumerate() {
            match choice {
                DialogChoice::CheckBox { label, value, .. } => {
                    row = row.push(
                        widget::checkbox(*value)
                            .label(label)
                            .on_toggle(move |checked| {
                                Message::Choice(choice_i, usize::from(checked))
                            }),
                    );
                }
                DialogChoice::ComboBox {
                    label,
                    options,
                    selected,
                    ..
                } => {
                    row = row.push(widget::text::heading(label));
                    row = row.push(widget::dropdown(options, *selected, move |option_i| {
                        Message::Choice(choice_i, option_i)
                    }));
                }
            }
        }

        if is_condensed {
            col = col.push(row);
            row = widget::row::with_capacity(3)
                .align_y(Alignment::Center)
                .spacing(space_xxs);
        }
        row = row.push(widget::space::horizontal());
        row = row.push(widget::button::standard(fl!("cancel")).on_press(Message::Cancel));

        let mut has_selected = false;
        if let Some(items) = self.tab.items_opt() {
            for item in items {
                if item.selected {
                    has_selected = true;
                    break;
                }
            }
        }
        row = row.push(
            //TODO: easier way to create buttons with rich text
            widget::button::custom(
                widget::row::with_children([Element::from(&self.accept_label)])
                    .padding([0, space_s])
                    .width(Length::Shrink)
                    .height(space_l)
                    .spacing(space_xxxs)
                    .align_y(Alignment::Center)
            )
            .padding(0)
            .on_press_maybe(if self.flags.kind.save() {
                if let DialogKind::SaveFile { filename } = &self.flags.kind {
                    (!filename.is_empty()).then_some(Message::Save(false))
                } else {
                    None
                }
            } else if has_selected || self.flags.kind.is_dir() {
                Some(Message::Open)
            } else {
                None
            })
            .class(widget::button::ButtonClass::Suggested)
            /*TODO: a11y feature: .label(&self.accept_label.text)*/
        );

        col = col.push(row);

        widget::layer_container(col)
            .layer(cosmic_theme::Layer::Primary)
            .padding([8, space_xs])
            .into()
    }

    fn preview<'a>(&'a self, kind: &'a PreviewKind) -> Element<'a, tab::Message> {
        let military_time = self.tab.config.military_time;
        let mut children = Vec::with_capacity(1);
        match kind {
            PreviewKind::Custom(PreviewItem(item)) => {
                children.push(item.preview_view(None, military_time));
            }
            PreviewKind::Location(location) => {
                if let Some(items) = self.tab.items_opt() {
                    for item in items {
                        if item.location_opt.as_ref() == Some(location) {
                            children.push(item.preview_view(None, military_time));
                            // Only show one property view to avoid issues like hangs when generating
                            // preview images on thousands of files
                            break;
                        }
                    }
                }
            }
            PreviewKind::Selected => {
                if let Some(items) = self.tab.items_opt() {
                    let preview_opt = {
                        let mut selected = items.iter().filter(|item| item.selected);

                        match (selected.next(), selected.next()) {
                            // At least two selected items
                            (Some(_), Some(_)) => Some(self.tab.multi_preview_view(None)),
                            // Exactly one selected item
                            (Some(item), None) => Some(item.preview_view(None, military_time)),
                            // No selected items
                            _ => None,
                        }
                    };

                    if let Some(preview) = preview_opt {
                        children.push(preview);
                    }

                    if children.is_empty()
                        && let Some(item) = &self.tab.parent_item_opt
                    {
                        children.push(item.preview_view(None, military_time));
                    }
                }
            }
        }
        widget::column::with_children(children).into()
    }

    fn rescan_tab(&mut self, selection_paths: Option<Vec<PathBuf>>) -> Task<Message> {
        let request = self.tab.scans.begin();
        let location = self.tab.location.clone();
        let icon_sizes = self.tab.config.icon_sizes;
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
                        request,
                        location,
                        parent_item_opt,
                        items,
                        selection_paths,
                    ))
                }
                Err(err) => {
                    log::warn!("failed to rescan: {err}");
                    cosmic::action::app(Message::TabRescanFailed(request))
                }
            }
        })
    }

    fn rescan_tab_if_idle(&mut self) -> Task<Message> {
        if self.tab.scans.defer_if_running() {
            Task::none()
        } else {
            self.rescan_tab(None)
        }
    }

    fn search_get(&self) -> Option<&str> {
        match &self.tab.location {
            Location::Search(_, term, ..) => Some(term),
            _ => None,
        }
    }

    fn search_set(&mut self, term_opt: Option<String>) -> Task<Message> {
        self.navigation.invalidate();
        self.invalidate_save();
        let location_opt = match term_opt {
            Some(term) => {
                let search_location = if let Some(path) = self.tab.location.path_opt() {
                    Some(SearchLocation::Path(path.clone()))
                } else if self.tab.location.is_recents() {
                    Some(SearchLocation::Recents)
                } else if self.tab.location.is_trash() {
                    Some(SearchLocation::Trash)
                } else {
                    None
                };

                search_location.map(|search_location| {
                    (
                        Location::Search(
                            search_location,
                            term,
                            self.tab.config.show_hidden,
                            Instant::now(),
                        ),
                        true,
                    )
                })
            }
            None => match &self.tab.location {
                Location::Search(search_location, ..) => match search_location {
                    SearchLocation::Path(path) => Some((Location::Path(path.clone()), false)),
                    SearchLocation::Recents => Some((Location::Recents, false)),
                    SearchLocation::Trash => Some((Location::Trash, false)),
                },
                _ => None,
            },
        };
        if let Some((location, focus_search)) = location_opt {
            self.tab.change_location(&location, None);
            return Task::batch([
                self.update_title(),
                self.update_watcher(),
                self.rescan_tab(None),
                if focus_search {
                    widget::text_input::focus(self.search_id.clone())
                } else {
                    Task::none()
                },
            ]);
        }
        Task::none()
    }

    fn update_config(&mut self) -> Task<Message> {
        self.core.window.show_context = self.flags.config.dialog.show_details;
        let config = self.flags.config.dialog_tab();
        self.tab.config.view = config.view;
        self.update_nav_model();
        self.update(Message::TabMessage(tab::Message::Config(config)))
    }

    fn with_dialog_config<F: Fn(&mut DialogConfig)>(&mut self, f: F) -> Task<Message> {
        let mut dialog = self.flags.config.dialog;
        f(&mut dialog);
        if dialog == self.flags.config.dialog {
            Task::none()
        } else {
            if let Some(config_handler) = &self.flags.config_handler {
                match self.flags.config.set_dialog(config_handler, dialog) {
                    Ok(_) => {}
                    Err(err) => {
                        log::warn!("failed to save config \"dialog\": {err}");
                    }
                }
            } else {
                self.flags.config.dialog = dialog;
                log::warn!("failed to save config \"dialog\": no config handler",);
            }
            self.update_config()
        }
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

        if self.flags.config.show_recents {
            nav_model = nav_model.insert(|b| {
                b.text(fl!("recents"))
                    .icon(widget::icon::from_name("document-open-recent-symbolic"))
                    .data(Location::Recents)
            });
        }

        for favorite in &self.flags.config.favorites {
            if let Some(path) = favorite.path_opt() {
                let Some(name) = favorite.display_name() else {
                    continue;
                };
                nav_model = nav_model.insert(move |b| {
                    b.text(name.clone())
                        .icon(
                            // Show the folder icon for every favorite; picking an icon per kind
                            // means an is_dir stat that blocks on a dead or slow network mount.
                            widget::icon::icon(tab::folder_icon_symbolic(&path, 16)).size(16),
                        )
                        .data(Location::Path(path.clone()))
                });
            }
        }

        // Collect all mounter items
        let mut nav_items = Vec::new();
        for (key, items) in &self.mounter_items {
            nav_items.extend(items.iter().map(|item| (*key, item)));
        }
        // Sort by name lexically
        nav_items.sort_unstable_by(|a, b| LANGUAGE_SORTER.compare(&a.1.name(), &b.1.name()));
        // Add items to nav model
        for (i, (key, item)) in nav_items.into_iter().enumerate() {
            nav_model = nav_model.insert(|mut b| {
                b = b.text(item.name()).data(MounterData(key, item.clone()));
                if let Some(path) = item.path() {
                    b = b.data(Location::Path(path));
                }
                if let Some(icon) = item.icon(true) {
                    b = b.icon(widget::icon::icon(icon).size(16));
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

        self.activate_nav_model_location(&self.tab.location.clone());
    }

    fn update_title(&mut self) -> Task<Message> {
        self.set_header_title(self.title.clone());
        self.set_window_title(self.title.clone(), self.flags.window_id)
    }

    fn watched_paths(&self) -> FxHashSet<PathBuf> {
        self.tab.location.path_opt().cloned().into_iter().collect()
    }

    fn leave_unmounted_paths(&mut self, unmounted: &[PathBuf]) -> Task<Message> {
        if self
            .tab
            .location
            .path_opt()
            .is_some_and(|current| unmounted.iter().any(|path| current.starts_with(path)))
        {
            self.navigation.invalidate();
            self.invalidate_save();
            self.tab.change_location(&Location::Path(home_dir()), None);
            return Task::batch([self.update_watcher(), self.rescan_tab(None)]);
        }
        Task::none()
    }

    fn update_watcher(&mut self) -> Task<Message> {
        let Some((watcher, old_paths)) = self.watcher_opt.take() else {
            self.watcher_update_pending = true;
            return Task::none();
        };
        let new_paths = self.watched_paths();
        Task::future(async move {
            let requested = new_paths.clone();
            match tokio::task::spawn_blocking(move || {
                crate::watcher::reconcile(watcher, old_paths, new_paths)
            })
            .await
            {
                Ok((watcher, paths)) => cosmic::action::app(Message::WatcherUpdated(
                    WatcherWrapper {
                        watcher_opt: Some(watcher),
                    },
                    paths,
                    requested,
                )),
                Err(err) => {
                    log::warn!("failed to update watcher: {err}");
                    cosmic::action::none()
                }
            }
        })
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
    const APP_ID: &'static str = "com.system76.CosmicFilesDialog";

    fn core(&self) -> &Core {
        &self.core
    }

    fn core_mut(&mut self) -> &mut Core {
        &mut self.core
    }

    /// Creates the application, and optionally emits command on initialize.
    fn init(mut core: Core, flags: Self::Flags) -> (Self, Task<Message>) {
        core.window.context_is_overlay = false;
        core.window.show_close = false;
        core.window.show_maximize = false;
        core.window.show_minimize = false;

        let title = flags.kind.title();
        let accept_label = flags.kind.accept_label();

        let location = Location::Path(match &flags.path_opt {
            Some(path) => path.clone(),
            None => match env::current_dir() {
                Ok(path) => path,
                Err(_) => home_dir(),
            },
        });

        let mut tab = Tab::new(
            location,
            flags.config.dialog_tab(),
            ThumbCfg::default(),
            None,
            widget::Id::unique(),
            None,
        );
        tab.mode = tab::Mode::Dialog(flags.kind.clone());
        (tab.sort_name, tab.sort_direction) = flags.state.dialog_sort_name;

        let key_binds = key_binds(&tab.mode);

        let mut app = Self {
            core,
            flags,
            title,
            accept_label: DialogLabel::from(accept_label),
            choices: Vec::new(),
            context_page: ContextPage::Preview(None, PreviewKind::Selected),
            dialog_pages: VecDeque::new(),
            dialog_text_input: widget::Id::new("Dialog Text Input"),
            filters: Vec::new(),
            filter_selected: None,
            filename_id: widget::Id::new("Dialog Filename"),
            modifiers: Modifiers::empty(),
            mounter_items: FxHashMap::default(),
            nav_model: segmented_button::ModelBuilder::default().build(),
            navigation: crate::navigation::PendingRequest::default(),
            save_request: crate::navigation::PendingRequest::default(),
            result_opt: None,
            completed: false,
            search_id: widget::Id::new("Dialog File Search"),
            tab,
            key_binds,
            watcher_opt: None,
            watcher_update_pending: false,
            auto_scroll_speed: None,
            type_select_prefix: String::new(),
            type_select_last_key: None,
        };

        let commands = Task::batch([
            app.update_config(),
            app.update_title(),
            app.update_watcher(),
            app.rescan_tab(None),
        ]);

        (app, commands)
    }

    fn context_drawer(&self) -> Option<context_drawer::ContextDrawer<'_, Message>> {
        if !self.core.window.show_context {
            return None;
        }

        match &self.context_page {
            ContextPage::Preview(_, kind) => {
                let actions = self
                    .tab
                    .items_opt()
                    .and_then(|items| {
                        items
                            .iter()
                            .find(|item| item.selected)
                            .map(|item| item.preview_actions().map(Message::TabMessage))
                    })
                    .unwrap_or_else(|| widget::space::horizontal().into());
                Some(
                    context_drawer::context_drawer(
                        self.preview(kind).map(Message::TabMessage),
                        Message::Preview,
                    )
                    .actions(actions),
                )
            }
            _ => None,
        }
    }

    fn dialog(&self) -> Option<Element<'_, Message>> {
        let cosmic_theme::Spacing { space_xxs, .. } = theme::spacing();

        //TODO: should gallery view just be a dialog?
        if self.tab.gallery {
            return Some(
                widget::column::with_children([
                    self.tab.gallery_view().map(Message::TabMessage),
                    // Draw button row as part of the overlay
                    widget::container(self.button_view())
                        .width(Length::Fill)
                        .padding(space_xxs)
                        .class(theme::Container::WindowBackground)
                        .into(),
                ])
                .into(),
            );
        }

        let dialog_page = self.dialog_pages.front()?;

        let dialog = match dialog_page {
            DialogPage::NewFolder {
                parent,
                name,
                error,
            } => {
                let mut dialog = widget::dialog().title(fl!("create-new-folder"));

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

                // A create that already failed outranks the advisory name hints.
                if let Some(error) = error {
                    dialog = dialog.tertiary_action(widget::text::body(error.clone()));
                }

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
                            widget::text::body(fl!("folder-name")).into(),
                            widget::text_input("", name.as_str())
                                .id(self.dialog_text_input.clone())
                                .on_input(move |name| {
                                    Message::DialogUpdate(DialogPage::NewFolder {
                                        parent: parent.clone(),
                                        name,
                                        error: None,
                                    })
                                })
                                .on_submit_maybe(complete_maybe.map(|maybe| move |_| maybe.clone()))
                                .into(),
                        ])
                        .spacing(space_xxs),
                    )
            }
            DialogPage::Replace { filename, .. } => widget::dialog()
                .title(fl!("replace-title", filename = filename.as_str()))
                .icon(widget::icon::from_name("dialog-question").size(64))
                .body(fl!("replace-warning"))
                .primary_action(
                    widget::button::suggested(fl!("replace"))
                        .on_press(Message::DialogComplete)
                        .id(REPLACE_BUTTON_ID.clone()),
                )
                .secondary_action(
                    widget::button::standard(fl!("cancel")).on_press(Message::DialogCancel),
                ),
        };

        Some(dialog.into())
    }

    fn footer(&self) -> Option<Element<'_, Message>> {
        Some(self.button_view())
    }

    fn header_end(&self) -> Vec<Element<'_, Message>> {
        let mut elements = Vec::with_capacity(3);

        if let Some(term) = self.search_get() {
            if self.core.is_condensed() {
                elements.push(
                    //TODO: selected state is not appearing different
                    widget::button::icon(widget::icon::from_name("system-search-symbolic"))
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
                widget::button::icon(widget::icon::from_name("system-search-symbolic"))
                    .on_press(Message::SearchActivate)
                    .padding(8)
                    .into(),
            );
        }

        if self.flags.kind.save() {
            elements.push(
                widget::button::icon(widget::icon::from_name("folder-new-symbolic"))
                    .on_press(Message::NewFolder)
                    .padding(8)
                    .into(),
            );
        }

        let show_details = match self.context_page {
            ContextPage::Preview(..) => self.core.window.show_context,
            _ => false,
        };
        elements
            .push(menu::dialog_menu(&self.tab, &self.key_binds, show_details).map(Message::from));

        elements
    }

    fn nav_bar(&self) -> Option<Element<'_, cosmic::Action<Self::Message>>> {
        if !self.core().nav_bar_active() {
            return None;
        }

        let nav_model = self.nav_model()?;

        let mut nav = cosmic::widget::nav_bar(nav_model, |entity| {
            cosmic::action::cosmic(cosmic::app::Action::NavBar(entity))
        })
        //TODO .on_close(|entity| cosmic::cosmic::action::app(Message::NavBarClose(entity)))
        .close_icon(
            widget::icon::from_name("media-eject-symbolic")
                .size(16)
                .icon(),
        )
        .into_container();

        if !self.core().is_condensed() {
            nav = nav.max_width(280);
        }

        Some(Element::from(
            nav.width(Length::Shrink).height(Length::Fill),
        ))
    }

    fn nav_model(&self) -> Option<&segmented_button::SingleSelectModel> {
        Some(&self.nav_model)
    }

    fn on_app_exit(&mut self) -> Option<Message> {
        Some(Message::Cancel)
    }

    fn on_nav_select(&mut self, entity: segmented_button::Entity) -> Task<Message> {
        self.nav_model.activate(entity);
        if let Some(location) = self.nav_model.data::<Location>(entity) {
            let message = Message::TabMessage(tab::Message::Location(location.clone()));
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

    fn on_escape(&mut self) -> Task<Message> {
        if self.tab.gallery {
            // Close gallery if open
            self.tab.gallery = false;
            return Task::none();
        }

        if self.tab.edit_location.is_some() {
            // Close location editing if enabled
            self.tab.edit_location = None;
            return Task::none();
        }

        if self.search_get().is_some() {
            // Close search if open
            return self.search_set(None);
        }

        let had_focused_button = self.tab.select_focus_id().is_some();
        if self.tab.select_none() {
            if had_focused_button {
                // Unfocus if there was a focused button
                return widget::button::focus(widget::Id::unique());
            }
            return Task::none();
        }

        // Close the dialog if the focused widget is the dialog's main text input instead of
        // unfocussing the widget.
        if let operation::Outcome::Some(focused) = operation::focusable::find_focused().finish()
            && self.dialog_text_input == focused
        {
            return self.update(Message::Cancel);
        }

        self.update(Message::Cancel)
    }

    /// Handle application events here.
    fn update(&mut self, message: Message) -> Task<Message> {
        if self.completed
            || (self.result_opt.is_some() && !matches!(message, Message::PersistenceFlushed))
        {
            return Task::none();
        }
        match message {
            Message::None => {}
            Message::PersistenceFlushed => {
                if self.result_opt.is_some() {
                    self.completed = true;
                    return window::close(self.flags.window_id);
                }
            }
            Message::Cancel => {
                self.navigation.invalidate();
                self.invalidate_save();
                self.result_opt = Some(DialogResult::Cancel);
                return self.close();
            }
            Message::Choice(choice_i, option_i) => {
                if let Some(choice) = self.choices.get_mut(choice_i) {
                    match choice {
                        DialogChoice::CheckBox { value, .. } => *value = option_i > 0,
                        DialogChoice::ComboBox {
                            options, selected, ..
                        } => {
                            if option_i < options.len() {
                                *selected = Some(option_i);
                            } else {
                                *selected = None;
                            }
                        }
                    }
                }
            }
            Message::Config(config) => {
                if config != self.flags.config {
                    log::info!("update config");
                    // Don't overwrite military time
                    let military_time = self.flags.config.tab.military_time;
                    self.flags.config = config;
                    self.flags.config.tab.military_time = military_time;
                    return self.update_config();
                }
            }
            Message::DialogCancel => {
                if matches!(
                    self.dialog_pages.pop_front(),
                    Some(DialogPage::Replace { .. })
                ) {
                    self.save_request.invalidate();
                }
            }
            Message::DialogComplete => {
                if let Some(dialog_page) = self.dialog_pages.pop_front() {
                    match dialog_page {
                        DialogPage::NewFolder { parent, name, .. } => {
                            self.invalidate_save();
                            let request = self.navigation.begin();
                            let location = self.tab.location.clone();
                            let path = parent.join(name);
                            // fs::create_dir blocks: on a slow mount it would freeze the dialog.
                            return Task::future(async move {
                                let result = match tokio::task::spawn_blocking({
                                    let path = path.clone();
                                    move || fs::create_dir(&path)
                                })
                                .await
                                {
                                    Ok(Ok(())) => Ok(()),
                                    Ok(Err(err)) => Err(err.to_string()),
                                    Err(err) => Err(err.to_string()),
                                };
                                cosmic::action::app(Message::FolderCreated {
                                    request,
                                    location,
                                    path,
                                    result,
                                })
                            });
                        }
                        DialogPage::Replace { request, path, .. } => {
                            if self.save_request.finish(&request) {
                                return self.check_save(path, true);
                            }
                        }
                    }
                }
            }
            Message::DialogUpdate(dialog_page) => {
                if !self.dialog_pages.is_empty() {
                    self.dialog_pages[0] = dialog_page;
                }
            }
            Message::Escape => return self.on_escape(),
            Message::Filename(new_filename) => {
                self.invalidate_save();
                // Select based on filename
                self.tab.select_name(&new_filename);

                if let DialogKind::SaveFile { filename } = &mut self.flags.kind {
                    *filename = new_filename;
                }
            }
            Message::Filter(filter_i) => {
                if filter_i < self.filters.len() {
                    self.filter_selected = Some(filter_i);
                } else {
                    self.filter_selected = None;
                }
                return self.rescan_tab(None);
            }
            Message::FolderCreated {
                request,
                location,
                path,
                result,
            } => {
                if !self.navigation.finish(&request)
                    || self.result_opt.is_some()
                    || self.tab.location != location
                {
                    if let Err(error) = result {
                        log::warn!("failed to create {}: {}", path.display(), error);
                    }
                    return Task::none();
                }
                match result {
                    Ok(()) => {
                        return self.update(Message::TabMessage(tab::Message::Location(
                            Location::Path(path),
                        )));
                    }
                    Err(error) => {
                        log::warn!("failed to create {}: {}", path.display(), error);
                        let name = path
                            .file_name()
                            .map(|name| name.to_string_lossy().into_owned())
                            .unwrap_or_default();
                        let parent = path.parent().map(Path::to_path_buf).unwrap_or_default();
                        self.dialog_pages.push_front(DialogPage::NewFolder {
                            parent,
                            name,
                            error: Some(error),
                        });
                        return widget::text_input::focus(self.dialog_text_input.clone());
                    }
                }
            }
            Message::Key(modifiers, key, physical_key, text) => {
                for (key_bind, action) in &self.key_binds {
                    if key_bind.matches(modifiers, &key, Some(&physical_key)) {
                        return self.update(Message::from(action.message()));
                    }
                }

                // Check key binds from accept label
                if let Some(key_bind) = &self.accept_label.key_bind_opt
                    && key_bind.matches(modifiers, &key, Some(&physical_key))
                {
                    return self.update(if self.flags.kind.save() {
                        Message::Save(false)
                    } else {
                        Message::Open
                    });
                }

                // Uncaptured keys with only shift modifiers go to the search or location box
                if !modifiers.logo()
                    && !modifiers.control()
                    && !modifiers.alt()
                    && matches!(key, Key::Character(_))
                    && let Some(text) = text
                {
                    match self.flags.config.type_to_search {
                        TypeToSearch::Recursive => {
                            let mut term = self.search_get().unwrap_or_default().to_string();
                            term.push_str(&text);
                            return self.search_set(Some(term));
                        }
                        TypeToSearch::EnterPath => {
                            let location = (self.tab.edit_location)
                                .as_ref()
                                .map_or_else(|| &self.tab.location, |x| &x.location);
                            // Try to add text to end of location
                            if let Some(path) = location.path_opt() {
                                let mut path_string = path.to_string_lossy().to_string();
                                path_string.push_str(&text);
                                self.tab.edit_location =
                                    Some(location.with_path(PathBuf::from(path_string)).into());
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

                            self.tab.select_by_prefix(&self.type_select_prefix);
                            if let Some(offset) = self.tab.select_focus_scroll() {
                                return scrollable::scroll_to(
                                    self.tab.scrollable_id.clone(),
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
            Message::ModifiersChanged(modifiers) => {
                self.modifiers = modifiers;
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

                let commands = vec![self.leave_unmounted_paths(&unmounted)];

                // Insert new items
                self.mounter_items.insert(mounter_key, mounter_items);

                // Update nav bar
                //TODO: this could change favorites IDs while they are in use
                self.update_nav_model();

                return Task::batch(commands);
            }
            Message::Mouse(window_id, _button) => {
                // Close context menu when clicking outside.
                if self.core.main_window_id() == Some(window_id) {
                    return Task::none();
                }
            }
            Message::NewFolder => {
                self.navigation.invalidate();
                self.invalidate_save();
                if let Some(path) = self.tab.location.path_opt() {
                    self.dialog_pages.push_back(DialogPage::NewFolder {
                        parent: path.clone(),
                        name: String::new(),
                        error: None,
                    });
                    return widget::text_input::focus(self.dialog_text_input.clone());
                }
            }
            Message::NotifyEvents(events) => {
                log::debug!("{events:?}");

                if let Some(path) = self.tab.location.path_opt() {
                    let contains_change = events.iter().any(|event| {
                        event
                            .paths
                            .iter()
                            .any(|event_path| event_path.starts_with(path))
                    });
                    if contains_change {
                        return self.rescan_tab_if_idle();
                    }
                }
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
            Message::WatcherUpdated(mut wrapper, paths, requested) => {
                if let Some(watcher) = wrapper.watcher_opt.take() {
                    let stale = requested != self.watched_paths();
                    self.watcher_opt = Some((watcher, paths));
                    if stale || self.watcher_update_pending {
                        self.watcher_update_pending = false;
                        return self.update_watcher();
                    }
                }
            }
            Message::Open => {
                let paths = self.tab.selected_paths_with_dir();

                // Ensure selection is allowed, deciding dir-vs-file from cached metadata.
                for (path, path_is_dir) in &paths {
                    if *path_is_dir != self.flags.kind.is_dir() {
                        if *path_is_dir && paths.len() == 1 {
                            // If the only selected item is a directory and we are selecting files, cd to it
                            let message = Message::TabMessage(tab::Message::Location(
                                Location::Path(path.clone()),
                            ));
                            return self.update(message);
                        }

                        // Otherwise, this is not a legal selection
                        return Task::none();
                    }
                }

                // If there are proper matching items, return them
                if !paths.is_empty() {
                    if self.flags.config.show_recents {
                        crate::recents::record_recently_used(
                            paths.iter().map(|(path, _)| path.clone()).collect(),
                            Self::APP_ID,
                        );
                    }
                    self.result_opt = Some(DialogResult::Open(
                        paths.into_iter().map(|(path, _)| path).collect(),
                    ));
                    return self.close();
                }

                // If we are in directory mode, return the current directory
                if self.flags.kind.is_dir()
                    && let Location::Path(tab_path) = &self.tab.location
                {
                    self.result_opt = Some(DialogResult::Open(vec![tab_path.clone()]));
                    return self.close();
                }
            }
            Message::Preview => {
                self.context_page = ContextPage::Preview(None, PreviewKind::Selected);
                return self.with_dialog_config(|config| {
                    config.show_details = !config.show_details;
                });
            }
            Message::Save(replace) => {
                if let DialogKind::SaveFile { filename } = &self.flags.kind
                    && !filename.is_empty()
                    && let Some(tab_path) = self.tab.location.path_opt()
                {
                    let path = tab_path.join(filename);
                    return self.check_save(path, replace);
                }
            }
            Message::SaveChecked {
                request,
                replace,
                path,
                existing,
            } => {
                let current_path = match &self.flags.kind {
                    DialogKind::SaveFile { filename } => self
                        .tab
                        .location
                        .path_opt()
                        .map(|parent| parent.join(filename)),
                    _ => None,
                };
                if !self.save_request.finish(&request)
                    || self.result_opt.is_some()
                    || current_path.as_ref() != Some(&path)
                {
                    return Task::none();
                }
                if existing == Some(true) {
                    return self.update(Message::TabMessage(tab::Message::Location(
                        Location::Path(path),
                    )));
                } else if !replace && existing.is_some() {
                    self.dialog_pages.push_back(DialogPage::Replace {
                        request: self.save_request.begin(),
                        filename: path
                            .file_name()
                            .map_or_else(String::new, |name| name.to_string_lossy().into_owned()),
                        path,
                    });
                    return widget::button::focus(REPLACE_BUTTON_ID.clone());
                }
                self.result_opt = Some(DialogResult::Open(vec![path]));
                return self.close();
            }

            Message::ScrollTab(scroll_speed) => {
                return self.update(Message::TabMessage(tab::Message::ScrollTab(
                    f32::from(scroll_speed) / 10.0,
                )));
            }
            Message::SearchActivate => {
                let mut tasks = vec![];

                if self.search_get().is_none() {
                    tasks.push(self.search_set(Some(String::new())));
                } else {
                    tasks.push(widget::text_input::focus(self.search_id.clone()));
                }

                return Task::batch(tasks);
            }
            Message::SearchClear => {
                return self.search_set(None);
            }
            Message::SearchInput(input) => {
                return self.search_set(Some(input));
            }
            Message::NavigationResolved { request, target } => {
                if !self.navigation.finish(&request) || self.result_opt.is_some() {
                    return Task::none();
                }
                if let Some((location, selected)) = target {
                    let message = match selected {
                        Some(path) => tab::Message::LocationSelect(location, vec![path]),
                        None => tab::Message::Location(location),
                    };
                    return self.update(Message::TabMessage(message));
                }
            }
            Message::TabMessage(tab_message) => {
                if matches!(
                    &tab_message,
                    tab::Message::EditLocation(_) | tab::Message::EditLocationEnable
                ) {
                    self.navigation.invalidate();
                    self.invalidate_save();
                }
                let click_i_opt = match tab_message {
                    tab::Message::Click(click_i_opt) => click_i_opt,
                    _ => None,
                };

                let tab_commands = self.tab.update(tab_message, self.modifiers);

                // Update filename box when anything is selected
                if let DialogKind::SaveFile { filename } = &mut self.flags.kind
                    && let Some(click_i) = click_i_opt
                    && let Some(items) = self.tab.items_opt()
                    && let Some(item) = items.get(click_i)
                    && item.selected
                    && !item.metadata.is_dir()
                {
                    if *filename != item.name {
                        self.save_request.invalidate();
                        self.dialog_pages
                            .retain(|page| !matches!(page, DialogPage::Replace { .. }));
                        filename.clone_from(&item.name);
                    }
                }

                let mut commands = Vec::new();
                for tab_command in tab_commands {
                    match tab_command {
                        tab::Command::Action(action) => {
                            commands.push(self.update(Message::from(action.message())));
                        }
                        tab::Command::ChangeLocation(_tab_title, _tab_path, selection_paths) => {
                            self.navigation.invalidate();
                            self.invalidate_save();
                            commands.push(Task::batch([
                                self.update_watcher(),
                                self.rescan_tab(selection_paths),
                            ]));
                        }
                        tab::Command::EditLocationSubmit(edit_location, typed_uri) => {
                            self.invalidate_save();
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
                                cosmic::action::app(Message::NavigationResolved { request, target })
                            }));
                        }
                        tab::Command::Surface(action) => {
                            let action = action.map(Message::TabMessage);
                            commands.push(self.update(Message::Surface(action)));
                        }
                        tab::Command::Iced(iced_command) => {
                            commands.push(iced_command.0.map(|tab_message| {
                                cosmic::action::app(Message::TabMessage(tab_message))
                            }));
                        }
                        tab::Command::OpenFile(_item_path) => {
                            if self.flags.kind.save() {
                                commands.push(self.update(Message::Save(false)));
                            } else {
                                commands.push(self.update(Message::Open));
                            }
                        }
                        tab::Command::Preview(kind) => {
                            self.context_page = ContextPage::Preview(None, kind);
                            commands.push(self.with_dialog_config(|config| {
                                config.show_details = true;
                            }));
                        }
                        tab::Command::WindowDrag => {
                            commands.push(window::drag(self.flags.window_id));
                        }
                        tab::Command::WindowToggleMaximize => {
                            commands.push(window::toggle_maximize(self.flags.window_id));
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
                        tab::Command::SetSort(_, heading_options, direction) => {
                            self.flags.state.dialog_sort_name = (heading_options, direction);
                            if let Some(state_handler) = self.flags.state_handler.as_ref()
                                && let Err(err) = state_handler.set::<&(HeadingOptions, bool)>(
                                    "dialog_sort_name",
                                    &self.flags.state.dialog_sort_name,
                                )
                            {
                                log::warn!("Failed to save dialog sort name: {err:?}");
                            }
                        }
                        unsupported => {
                            log::warn!("{unsupported:?} not supported in dialog mode");
                        }
                    }
                }
                return Task::batch(commands);
            }
            Message::TabRescanFailed(request) => {
                if self.tab.scans.finish(request) == Some(true) {
                    return self.rescan_tab_if_idle();
                }
            }
            Message::TabRescan(request, location, parent_item_opt, mut items, selection_paths) => {
                let Some(rescan_pending) = self.tab.scans.finish(request) else {
                    return Task::none();
                };

                if location == self.tab.location {
                    // Filter
                    if let Some(filter_i) = self.filter_selected
                        && let Some(filter) = self.filters.get(filter_i)
                    {
                        let mut parsed_globs = Vec::new();
                        let mut mimes = Vec::new();
                        for pattern in &filter.patterns {
                            match pattern {
                                DialogFilterPattern::Glob(value) => {
                                    match glob::Pattern::new(value) {
                                        Ok(glob) => parsed_globs.push(glob),
                                        Err(err) => {
                                            log::warn!("failed to parse glob {value:?}: {err}");
                                        }
                                    }
                                }
                                DialogFilterPattern::Mime(value) => match value.parse::<Mime>() {
                                    Ok(parsed) => mimes.push(parsed),
                                    Err(err) => {
                                        log::warn!("failed to parse mime {value:?}: {err}");
                                    }
                                },
                            }
                        }

                        items.retain(|item| {
                            // Directories are always shown
                            item.metadata.is_dir()
                                // Check for mime type match (first because it is faster)
                                    || mimes.iter().any(|filter_mime| {
                                        if filter_mime.subtype() == mime::STAR {
                                            filter_mime.type_() == item.mime.type_()
                                        } else {
                                            *filter_mime == item.mime
                                                || mime_icon::is_mime_subclass_of(&item.mime, filter_mime)
                                        }
                                    })
                                // Check for glob match (last because it is slower)
                                    || parsed_globs.iter().any(|glob| glob.matches(&item.name))
                        });
                    }

                    // Select based on filename
                    if let DialogKind::SaveFile { filename } = &self.flags.kind {
                        for item in &mut items {
                            item.selected = &item.name == filename;
                        }
                    }

                    self.tab.parent_item_opt = parent_item_opt;
                    self.tab.set_items(items);

                    if let Some(mut selection_paths) = selection_paths {
                        if !self.flags.kind.multiple() {
                            selection_paths.truncate(1);
                        }
                        self.tab.select_paths(selection_paths);
                    }

                    // Reset focus on location change
                    let focus_task = if self.search_get().is_some() {
                        widget::text_input::focus(self.search_id.clone())
                    } else if let DialogKind::SaveFile { filename } = &self.flags.kind {
                        Task::batch([
                            widget::text_input::focus(self.filename_id.clone()),
                            widget::text_input::select_until_last(
                                self.filename_id.clone(),
                                filename,
                                '.',
                            ),
                        ])
                    } else {
                        widget::text_input::focus(self.filename_id.clone())
                    };

                    return if rescan_pending {
                        Task::batch([focus_task, self.rescan_tab_if_idle()])
                    } else {
                        focus_task
                    };
                }

                if rescan_pending {
                    return self.rescan_tab_if_idle();
                }
            }
            Message::TabView(view) => {
                return self.with_dialog_config(|config| {
                    config.view = view;
                });
            }
            Message::TimeConfigChange(time_config) => {
                self.flags.config.tab.military_time = time_config.military_time;
                return self.update_config();
            }
            Message::ToggleFoldersFirst => {
                return self.with_dialog_config(|config| {
                    config.folders_first = !config.folders_first;
                });
            }
            Message::ToggleShowHidden => {
                return self.with_dialog_config(|config| {
                    config.show_hidden = !config.show_hidden;
                });
            }
            Message::ZoomDefault => {
                return self.with_dialog_config(|config| {
                    zoom_to_default(config.view, &mut config.icon_sizes);
                });
            }
            Message::ZoomIn => {
                return self.with_dialog_config(|config| {
                    zoom_in_view(config.view, &mut config.icon_sizes);
                });
            }
            Message::ZoomOut => {
                return self.with_dialog_config(|config| {
                    zoom_out_view(config.view, &mut config.icon_sizes);
                });
            }
            Message::Surface(action) => {
                return cosmic::task::message(cosmic::Action::Surface(action));
            }
        }

        Task::none()
    }

    /// Creates a view after each update.
    fn view(&self) -> Element<'_, Message> {
        let cosmic_theme::Spacing { space_xxs, .. } = theme::spacing();

        let mut col = widget::column::with_capacity(2);

        if self.core.is_condensed()
            && let Some(term) = self.search_get()
        {
            col = col.push(
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

        col = col.push(
            self.tab
                .view(&self.key_binds, &self.modifiers, false, &[])
                .map(Message::TabMessage),
        );

        col.into()
    }

    fn subscription(&self) -> Subscription<Message> {
        struct WatcherSubscription;
        struct TimeSubscription;
        let mut subscriptions = vec![
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
                        Some(Message::Key(modifiers, key, physical_key, text))
                    }
                    event::Status::Captured => {
                        if key == Key::Named(Named::Escape) {
                            Some(Message::Escape)
                        } else {
                            None
                        }
                    }
                },
                Event::Keyboard(KeyEvent::ModifiersChanged(modifiers)) => {
                    Some(Message::ModifiersChanged(modifiers))
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
                stream::channel(100, {
                    |mut output: futures::channel::mpsc::Sender<_>| async move {
                        let watcher_res = {
                            let mut output = output.clone();
                            new_debouncer(
                                time::Duration::from_millis(250),
                                Some(time::Duration::from_millis(250)),
                                move |events_res: notify_debouncer_full::DebounceEventResult| {
                                    match events_res {
                                        Ok(mut events) => {
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
                    }
                })
            }),
            self.tab
                .subscription(
                    self.core.window.show_context
                        && matches!(
                            self.context_page,
                            ContextPage::Preview(_, PreviewKind::Selected)
                        ),
                )
                .map(Message::TabMessage),
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
                .map(|(key, mounter_message)| {
                    if let MounterMessage::Items(items) = mounter_message {
                        Message::MounterItems(key, items)
                    } else {
                        log::warn!("{mounter_message:?} not supported in dialog mode");
                        Message::None
                    }
                })
        }));

        Subscription::batch(subscriptions)
    }
}

#[cfg(test)]
mod tests {
    use std::io;

    use tempfile::TempDir;

    use super::{
        App, Application, Config, Core, DialogKind, DialogPage, DialogResult, Flags, Message,
        window,
    };

    fn save_dialog(dir: &TempDir, filename: &str) -> App {
        let window_id = window::Id::unique();
        let mut core = Core::default();
        core.set_main_window_id(Some(window_id));
        let (app, _task) = App::init(
            core,
            Flags {
                kind: DialogKind::SaveFile {
                    filename: filename.to_string(),
                },
                path_opt: Some(dir.path().to_path_buf()),
                window_id,
                config_handler: None,
                config: Config::default(),
            },
        );
        app
    }

    async fn task_result(task: super::Task<Message>) -> Message {
        use cosmic::iced::futures::StreamExt;
        let mut stream = cosmic::iced::runtime::task::into_stream(task).unwrap();
        match stream.next().await.unwrap() {
            cosmic::iced::runtime::Action::Output(cosmic::Action::App(message)) => message,
            _ => panic!("the task must return an application message"),
        }
    }

    #[cfg(unix)]
    mod picker_completion {
        use std::ffi::CString;
        use std::io::Write;
        use std::os::unix::ffi::OsStrExt;

        use cosmic::iced::futures::{FutureExt, StreamExt};
        use cosmic::iced::runtime::{Action, task};

        use super::*;
        use crate::dialog::{Dialog, DialogMessage};

        #[derive(Clone, Debug)]
        enum ParentMessage {
            Dialog(DialogMessage),
            Result(DialogResult),
        }

        #[derive(Clone, Copy)]
        enum Completion {
            Save,
            Cancel,
            FrameworkClose,
        }

        fn framework_close() -> DialogMessage {
            DialogMessage(cosmic::Action::Cosmic(cosmic::app::Action::Close))
        }

        fn run_isolated(test_name: &str, completion: Completion) -> io::Result<()> {
            const CHILD_PROFILE: &str = "COSMIC_TEST_PICKER_COMPLETION_PROFILE";
            if let Some(profile) = std::env::var_os(CHILD_PROFILE) {
                return tokio::runtime::Runtime::new()
                    .unwrap()
                    .block_on(check_completion(std::path::Path::new(&profile), completion));
            }
            let profile = TempDir::new()?;
            let status = std::process::Command::new(std::env::current_exe()?)
                .args(["--exact", test_name, "--nocapture"])
                .env(CHILD_PROFILE, profile.path())
                .env("HOME", profile.path())
                .env("XDG_DATA_HOME", profile.path().join(".local/share"))
                .env("XDG_CONFIG_HOME", profile.path().join(".config"))
                .status()?;
            assert!(status.success(), "the isolated picker completion failed");
            assert!(profile.path().join("drained").is_file());
            Ok(())
        }

        async fn check_completion(
            profile: &std::path::Path,
            completion: Completion,
        ) -> io::Result<()> {
            let dir = TempDir::new_in(profile)?;
            let recorded = dir.path().join("queued-before-picker-close.txt");
            std::fs::write(&recorded, b"content")?;
            let app = save_dialog(&dir, "new.txt");
            let window_id = app.flags.window_id;
            let mut dialog = Dialog {
                cosmic: cosmic::app::cosmic::Cosmic::new(app),
                mapper: ParentMessage::Dialog,
                on_result: Box::new(ParentMessage::Result),
            };

            let history = profile.join(".local/share/recently-used.xbel");
            std::fs::create_dir_all(history.parent().unwrap())?;
            let fifo = CString::new(history.as_os_str().as_bytes()).unwrap();
            if unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) } != 0 {
                return Err(io::Error::last_os_error());
            }
            crate::recents::record_recently_used(vec![recorded], "test");
            let mut release = std::fs::OpenOptions::new().write(true).open(&history)?;
            let initial_history = br#"<?xml version="1.0" encoding="UTF-8"?><xbel version="1.0" xmlns:bookmark="http://www.freedesktop.org/standards/desktop-bookmarks" xmlns:mime="http://www.freedesktop.org/standards/shared-mime-info"/>"#;
            let mut replacement = tempfile::NamedTempFile::new_in(history.parent().unwrap())?;
            replacement.write_all(initial_history)?;
            replacement.persist(&history).map_err(|err| err.error)?;

            let closing = match completion {
                Completion::Save => {
                    let checking =
                        dialog.update(DialogMessage(cosmic::Action::App(Message::Save(false))));
                    let mut checking = task::into_stream(checking).unwrap();
                    let Some(Action::Output(cosmic::Action::App(ParentMessage::Dialog(checked)))) =
                        checking.next().await
                    else {
                        panic!("the save check must return a dialog message");
                    };
                    dialog.update(checked)
                }
                Completion::Cancel => {
                    dialog.update(DialogMessage(cosmic::Action::App(Message::Cancel)))
                }
                Completion::FrameworkClose => dialog.update(framework_close()),
            };
            let mut closing = task::into_stream(closing).unwrap();
            assert!(
                closing.next().now_or_never().is_none(),
                "the picker closed or published its result before the recents write finished"
            );
            assert_eq!(dialog.update(framework_close()).units(), 0);
            assert_eq!(
                dialog
                    .update(DialogMessage(cosmic::Action::App(Message::Cancel)))
                    .units(),
                0
            );

            release.write_all(initial_history)?;
            drop(release);
            let Some(Action::Output(cosmic::Action::App(ParentMessage::Dialog(flushed)))) =
                closing.next().await
            else {
                panic!("draining persistence must return a dialog message");
            };
            assert!(matches!(
                flushed.0,
                cosmic::Action::App(Message::PersistenceFlushed)
            ));
            assert!(closing.next().await.is_none());
            assert!(std::fs::read_to_string(&history)?.contains("queued-before-picker-close.txt"));

            let mut finished = task::into_stream(dialog.update(flushed.clone())).unwrap();
            let mut closes = 0;
            let mut results = 0;
            while let Some(action) = finished.next().await {
                match action {
                    Action::Window(cosmic::iced::runtime::window::Action::Close(id)) => {
                        assert_eq!(id, window_id);
                        closes += 1;
                    }
                    Action::Output(cosmic::Action::App(ParentMessage::Result(result))) => {
                        match completion {
                            Completion::Save => assert!(
                                matches!(result, DialogResult::Open(paths) if paths == [dir.path().join("new.txt")])
                            ),
                            Completion::Cancel | Completion::FrameworkClose => {
                                assert!(matches!(result, DialogResult::Cancel));
                            }
                        }
                        results += 1;
                    }
                    _ => panic!("picker completion returned an unexpected action"),
                }
            }
            assert_eq!(closes, 1);
            assert_eq!(results, 1);
            assert_eq!(dialog.update(flushed).units(), 0);
            assert_eq!(dialog.update(framework_close()).units(), 0);
            std::fs::write(profile.join("drained"), b"complete")
        }

        #[test]
        fn save_waits_for_queued_recents_before_publishing_its_result() -> io::Result<()> {
            run_isolated(
                "dialog::tests::picker_completion::save_waits_for_queued_recents_before_publishing_its_result",
                Completion::Save,
            )
        }

        #[test]
        fn cancel_waits_for_queued_recents_before_publishing_its_result() -> io::Result<()> {
            run_isolated(
                "dialog::tests::picker_completion::cancel_waits_for_queued_recents_before_publishing_its_result",
                Completion::Cancel,
            )
        }

        #[test]
        fn framework_close_waits_for_queued_recents_before_publishing_its_result() -> io::Result<()>
        {
            run_isolated(
                "dialog::tests::picker_completion::framework_close_waits_for_queued_recents_before_publishing_its_result",
                Completion::FrameworkClose,
            )
        }
    }

    #[tokio::test]
    async fn a_failed_watch_does_not_resubmit_without_a_new_request() -> std::io::Result<()> {
        use cosmic::Application;
        let dir = tempfile::TempDir::new()?;
        let path = dir.path().join("missing");
        let mut app = save_dialog(&dir, "new.txt");
        app.tab.location = crate::tab::Location::Path(path);
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
    async fn a_picker_result_waits_for_persistence_before_closing() -> io::Result<()> {
        let dir = TempDir::new()?;
        let mut app = save_dialog(&dir, "new.txt");
        let checked = task_result(app.update(Message::Save(false))).await;
        let completion = task_result(app.update(checked)).await;
        assert!(matches!(completion, Message::PersistenceFlushed));
        let _ = app.update(Message::Cancel);
        assert!(
            matches!(&app.result_opt, Some(DialogResult::Open(paths)) if paths == &[dir.path().join("new.txt")])
        );
        assert!(app.update(completion).units() > 0);
        Ok(())
    }

    #[tokio::test]
    async fn a_picker_leaves_unmounted_parent_paths() -> io::Result<()> {
        let dir = TempDir::new()?;
        let nested = dir.path().join("child");
        std::fs::create_dir(&nested)?;
        let mut app = save_dialog(&dir, "new.txt");
        app.tab.location = crate::tab::Location::Path(nested);
        let navigation = app.navigation.begin();
        let save = app.save_request.begin();
        let _ = app.leave_unmounted_paths(&[dir.path().to_path_buf()]);
        assert_eq!(app.tab.location.path_opt(), Some(&crate::home_dir()));
        assert!(!app.navigation.is_current(&navigation));
        assert!(!app.save_request.is_current(&save));
        Ok(())
    }

    #[tokio::test]
    async fn save_navigation_intent_invalidates_an_earlier_save_check() -> io::Result<()> {
        let dir = TempDir::new()?;
        let mut app = save_dialog(&dir, "new.txt");
        app.tab.edit_location = Some(crate::tab::Location::Path(dir.path().join("other")).into());
        let checked = task_result(app.update(Message::Save(false))).await;
        let _ = app.update(Message::TabMessage(crate::tab::Message::EditLocationSubmit));
        let _ = app.update(checked);
        assert!(app.result_opt.is_none());
        Ok(())
    }

    #[tokio::test]
    async fn an_edit_back_to_the_same_name_cannot_revive_a_confirmation() -> io::Result<()> {
        let dir = TempDir::new()?;
        std::fs::write(dir.path().join("report.pdf"), b"pdf")?;
        let mut app = save_dialog(&dir, "report.pdf");
        let checked = task_result(app.update(Message::Save(false))).await;
        let _ = app.update(checked);
        assert_eq!(app.dialog_pages.len(), 1);
        let _ = app.update(Message::Filename("other.pdf".into()));
        let _ = app.update(Message::Filename("report.pdf".into()));
        assert_eq!(app.update(Message::DialogComplete).units(), 0);
        assert!(app.result_opt.is_none());
        Ok(())
    }

    #[tokio::test]
    async fn repeated_save_checks_create_only_one_confirmation() -> io::Result<()> {
        let dir = TempDir::new()?;
        std::fs::write(dir.path().join("report.pdf"), b"pdf")?;
        let mut app = save_dialog(&dir, "report.pdf");
        let old = task_result(app.update(Message::Save(false))).await;
        let current = task_result(app.update(Message::Save(false))).await;
        let _ = app.update(current);
        let _ = app.update(old);
        assert_eq!(app.dialog_pages.len(), 1);
        Ok(())
    }

    #[tokio::test]
    async fn a_save_check_cannot_survive_an_edit_back_to_the_same_name() -> io::Result<()> {
        let dir = TempDir::new()?;
        let mut app = save_dialog(&dir, "report.pdf");
        let old = task_result(app.update(Message::Save(false))).await;
        let _ = app.update(Message::Filename("other.pdf".into()));
        let _ = app.update(Message::Filename("report.pdf".into()));
        let _ = app.update(old);
        assert!(app.result_opt.is_none());
        assert!(app.dialog_pages.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn review_an_old_picker_scan_cannot_replace_a_newer_listing() -> io::Result<()> {
        let dir = TempDir::new()?;
        let mut app = save_dialog(&dir, "new.txt");
        let old_result = task_result(app.rescan_tab(None)).await;
        std::fs::write(dir.path().join("new.txt"), b"new")?;
        let new_result = task_result(app.rescan_tab(None)).await;
        let _ = app.update(new_result);
        let _ = app.update(old_result);
        assert_eq!(app.tab.items_opt().unwrap().len(), 1);
        assert_eq!(app.tab.items_opt().unwrap()[0].name, "new.txt");
        Ok(())
    }

    #[tokio::test]
    async fn a_picker_direct_scan_cannot_release_a_newer_watcher_scan() -> io::Result<()> {
        let dir = TempDir::new()?;
        let mut app = save_dialog(&dir, "new.txt");
        let initial = task_result(app.rescan_tab(None)).await;
        let _ = app.update(initial);
        let old = task_result(app.rescan_tab_if_idle()).await;
        std::fs::write(dir.path().join("new.txt"), b"new")?;
        let current = task_result(app.rescan_tab(None)).await;
        assert_eq!(app.rescan_tab_if_idle().units(), 0);
        let _ = app.update(current);
        let _ = app.update(old);
        assert_eq!(app.rescan_tab_if_idle().units(), 0);
        assert_eq!(app.tab.items_opt().unwrap()[0].name, "new.txt");
        Ok(())
    }

    #[tokio::test]
    async fn a_failed_picker_scan_releases_only_its_own_request() -> io::Result<()> {
        let dir = TempDir::new()?;
        let mut app = save_dialog(&dir, "new.txt");
        let old = app.tab.scans.begin();
        let current = app.tab.scans.begin();
        let _ = app.update(Message::TabRescanFailed(old));
        assert!(app.tab.scans.defer_if_running());
        let _ = app.update(Message::TabRescanFailed(current));
        assert_eq!(app.tab.scans.finish(current), None);
        assert!(app.tab.scans.defer_if_running());
        Ok(())
    }

    /// The picker's listing is filtered to the selected file types, so a name it does not hold
    /// may still exist on disk. Deciding here would miss it; the decision waits for the stat.
    #[test]
    fn save_defers_the_decision_instead_of_reading_the_listing() -> io::Result<()> {
        let dir = TempDir::new()?;
        std::fs::write(dir.path().join("report.pdf"), b"pdf")?;
        let mut app = save_dialog(&dir, "report.pdf");

        let _task = app.update(Message::Save(false));

        assert!(
            app.dialog_pages.is_empty(),
            "Save prompted before anything had established whether the file exists"
        );
        assert!(
            app.result_opt.is_none(),
            "Save accepted before anything had established whether the file exists"
        );
        Ok(())
    }

    #[test]
    fn save_checked_prompts_before_replacing_a_file() -> io::Result<()> {
        let dir = TempDir::new()?;
        let mut app = save_dialog(&dir, "report.pdf");

        let request = app.save_request.begin();
        let _task = app.update(Message::SaveChecked {
            request,
            replace: false,
            path: dir.path().join("report.pdf"),
            existing: Some(false),
        });

        assert!(matches!(
            app.dialog_pages.front(),
            Some(DialogPage::Replace { filename, .. }) if filename == "report.pdf"
        ));
        assert!(app.result_opt.is_none(), "the file was accepted unprompted");
        Ok(())
    }

    #[test]
    fn save_checked_accepts_a_name_nothing_holds() -> io::Result<()> {
        let dir = TempDir::new()?;
        let mut app = save_dialog(&dir, "report.pdf");
        let path = dir.path().join("report.pdf");

        let request = app.save_request.begin();
        let _task = app.update(Message::SaveChecked {
            request,
            replace: false,
            path: path.clone(),
            existing: None,
        });

        assert!(app.dialog_pages.is_empty());
        assert!(matches!(
            app.result_opt,
            Some(DialogResult::Open(ref paths)) if paths == &[path]
        ));
        Ok(())
    }

    #[test]
    fn save_checked_accepts_a_file_once_replacing_is_confirmed() -> io::Result<()> {
        let dir = TempDir::new()?;
        let mut app = save_dialog(&dir, "report.pdf");
        let path = dir.path().join("report.pdf");

        let request = app.save_request.begin();
        let _task = app.update(Message::SaveChecked {
            request,
            replace: true,
            path: path.clone(),
            existing: Some(false),
        });

        assert!(
            app.dialog_pages.is_empty(),
            "confirming the replacement prompted for it again"
        );
        assert!(matches!(
            app.result_opt,
            Some(DialogResult::Open(ref paths)) if paths == &[path]
        ));
        Ok(())
    }

    #[test]
    fn save_checked_enters_a_directory_of_that_name() -> io::Result<()> {
        let dir = TempDir::new()?;
        let nested = dir.path().join("report.pdf");
        std::fs::create_dir(&nested)?;
        let mut app = save_dialog(&dir, "report.pdf");

        let request = app.save_request.begin();
        let _task = app.update(Message::SaveChecked {
            request,
            replace: false,
            path: nested,
            existing: Some(true),
        });

        assert!(app.dialog_pages.is_empty(), "entering a directory prompted");
        assert!(
            app.result_opt.is_none(),
            "entering a directory returned it as the saved file"
        );
        Ok(())
    }

    #[test]
    fn a_failed_folder_creation_reports_in_the_dialog() -> io::Result<()> {
        let dir = TempDir::new()?;
        let mut app = save_dialog(&dir, "report.pdf");

        let request = app.navigation.begin();
        let location = app.tab.location.clone();
        let _task = app.update(Message::FolderCreated {
            request,
            location,
            path: dir.path().join("drafts"),
            result: Err("permission denied".to_string()),
        });

        assert!(
            matches!(
                app.dialog_pages.front(),
                Some(DialogPage::NewFolder { name, error: Some(error), .. })
                    if name == "drafts" && error.as_str() == "permission denied"
            ),
            "the dialog should reopen carrying the failed name and the error"
        );
        Ok(())
    }

    #[test]
    fn a_created_folder_opens_in_the_picker() -> io::Result<()> {
        let dir = TempDir::new()?;
        let mut app = save_dialog(&dir, "report.pdf");
        let created = dir.path().join("drafts");
        std::fs::create_dir(&created)?;

        let request = app.navigation.begin();
        let location = app.tab.location.clone();
        let _task = app.update(Message::FolderCreated {
            request,
            location,
            path: created.clone(),
            result: Ok(()),
        });

        assert_eq!(app.tab.location.path_opt(), Some(&created));
        Ok(())
    }
    #[test]
    fn a_save_check_for_an_edited_filename_is_ignored() -> io::Result<()> {
        let dir = TempDir::new()?;
        let mut app = save_dialog(&dir, "first.txt");
        let request = app.save_request.begin();
        let _ = app.update(Message::Filename("second.txt".into()));
        let _ = app.update(Message::SaveChecked {
            request,
            replace: false,
            path: dir.path().join("first.txt"),
            existing: Some(false),
        });
        assert!(app.dialog_pages.is_empty());
        assert!(app.result_opt.is_none());
        Ok(())
    }

    #[test]
    fn a_save_check_after_cancellation_cannot_accept_a_file() -> io::Result<()> {
        let dir = TempDir::new()?;
        let mut app = save_dialog(&dir, "report.txt");
        let request = app.save_request.begin();
        let _ = app.update(Message::Cancel);
        let _ = app.update(Message::SaveChecked {
            request,
            replace: false,
            path: dir.path().join("report.txt"),
            existing: None,
        });
        assert!(matches!(app.result_opt, Some(DialogResult::Cancel)));
        Ok(())
    }

    #[test]
    fn a_save_check_for_another_directory_is_ignored() -> io::Result<()> {
        let dir = TempDir::new()?;
        let mut app = save_dialog(&dir, "report.txt");
        let request = app.save_request.begin();
        let _ = app.update(Message::SaveChecked {
            request,
            replace: false,
            path: dir.path().join("elsewhere/report.txt"),
            existing: None,
        });
        assert!(app.result_opt.is_none());
        Ok(())
    }

    #[tokio::test]
    async fn submitting_a_picker_location_opens_the_requested_directory() -> io::Result<()> {
        use crate::tab::{self, Location};
        use cosmic::iced::futures::StreamExt;

        let dir = TempDir::new()?;
        let target = dir.path().join("nested");
        std::fs::create_dir(&target)?;
        let mut app = save_dialog(&dir, "report.txt");
        app.tab.edit_location = Some(Location::Path(target.clone()).into());
        let task = app.update(Message::TabMessage(tab::Message::EditLocationSubmit));
        let mut stream = cosmic::iced::runtime::task::into_stream(task)
            .expect("submitting a location should dispatch its resolver");
        let action = stream
            .next()
            .await
            .expect("the resolver should return a result");
        let cosmic::iced::runtime::Action::Output(cosmic::Action::App(message)) = action else {
            panic!("expected the resolved navigation result");
        };
        let _ = app.update(message);
        assert_eq!(app.tab.location.path_opt(), Some(&target));
        Ok(())
    }

    #[test]
    fn an_older_picker_navigation_cannot_replace_a_newer_request() -> io::Result<()> {
        let dir = TempDir::new()?;
        let mut app = save_dialog(&dir, "report.txt");
        let original = app.tab.location.clone();
        let stale = app.navigation.begin();
        let _current = app.navigation.begin();
        let _ = app.update(Message::NavigationResolved {
            request: stale,
            target: Some((crate::tab::Location::Path(dir.path().join("old")), None)),
        });
        assert_eq!(app.tab.location, original);
        Ok(())
    }

    #[tokio::test]
    async fn confirming_a_replacement_cannot_switch_to_an_edited_name() -> io::Result<()> {
        let dir = TempDir::new()?;
        let mut app = save_dialog(&dir, "first.txt");
        let first = dir.path().join("first.txt");
        std::fs::write(&first, b"first")?;
        let request = app.save_request.begin();
        let _ = app.update(Message::SaveChecked {
            request,
            replace: false,
            path: first,
            existing: Some(false),
        });
        let _ = app.update(Message::Filename("second.txt".into()));
        let task = app.update(Message::DialogComplete);
        assert!(cosmic::iced::runtime::task::into_stream(task).is_none());
        assert!(app.result_opt.is_none());
        Ok(())
    }
    #[tokio::test]
    async fn picker_watch_updates_reconcile_navigation_during_the_worker() -> io::Result<()> {
        use crate::tab::Location;
        use cosmic::iced::futures::StreamExt;

        let dir = TempDir::new()?;
        let target = dir.path().join("nested");
        std::fs::create_dir(&target)?;
        let mut app = save_dialog(&dir, "report.txt");
        let watcher = notify_debouncer_full::new_debouncer(
            std::time::Duration::from_millis(100),
            None,
            |_events| {},
        )
        .expect("a watcher should be available for the temporary directory");
        app.watcher_opt = Some((watcher, Default::default()));
        let first = app.update_watcher();
        assert!(app.watcher_opt.is_none());
        app.tab.location = Location::Path(target.clone());
        let _ = app.update_watcher();
        let mut stream = cosmic::iced::runtime::task::into_stream(first).unwrap();
        let cosmic::iced::runtime::Action::Output(cosmic::Action::App(message)) =
            stream.next().await.unwrap()
        else {
            panic!("expected the watch update result");
        };
        let second = app.update(message);
        assert!(app.watcher_opt.is_none());
        let mut stream = cosmic::iced::runtime::task::into_stream(second).unwrap();
        let cosmic::iced::runtime::Action::Output(cosmic::Action::App(message)) =
            stream.next().await.unwrap()
        else {
            panic!("expected the reconciled watch update result");
        };
        let _ = app.update(message);
        let (_, watched) = app
            .watcher_opt
            .as_ref()
            .expect("the watcher should be restored");
        assert_eq!(watched, &std::iter::once(target).collect());
        Ok(())
    }

    #[test]
    fn a_created_folder_cannot_replace_newer_navigation() -> io::Result<()> {
        let dir = TempDir::new()?;
        let mut app = save_dialog(&dir, "report.txt");
        let request = app.navigation.begin();
        let location = app.tab.location.clone();
        let target = crate::tab::Location::Path(dir.path().join("elsewhere"));
        let _ = app.update(Message::TabMessage(crate::tab::Message::Location(
            target.clone(),
        )));
        let _ = app.update(Message::FolderCreated {
            request,
            location,
            path: dir.path().join("created"),
            result: Ok(()),
        });
        assert_eq!(app.tab.location, target);
        Ok(())
    }

    #[test]
    fn a_failed_folder_creation_cannot_reopen_a_cancelled_picker() -> io::Result<()> {
        let dir = TempDir::new()?;
        let mut app = save_dialog(&dir, "report.txt");
        let request = app.navigation.begin();
        let location = app.tab.location.clone();
        let _ = app.update(Message::Cancel);
        let _ = app.update(Message::FolderCreated {
            request,
            location,
            path: dir.path().join("created"),
            result: Err("permission denied".into()),
        });
        assert!(app.dialog_pages.is_empty());
        assert!(matches!(app.result_opt, Some(DialogResult::Cancel)));
        Ok(())
    }
}
