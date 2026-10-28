// SPDX-License-Identifier: GPL-3.0-only

use cosmic::app::Core;
use cosmic::iced::keyboard::Modifiers;
use cosmic::widget::menu::action::MenuAction;
use cosmic::widget::menu::key_bind::KeyBind;
use cosmic::widget::menu::{self, ItemHeight, ItemWidth, MenuBar};
use cosmic::widget::{self, responsive_menu_bar};
use cosmic::{Element, theme};
use mime_guess::Mime;
use std::collections::HashMap;
use std::path::Path;
use std::sync::LazyLock;

use crate::app::{Action, Message};
use crate::config::{Config, ContextActionPreset};
use crate::fl;
use crate::tab::{
    self, HeadingOptions, ItemMetadata, Location, LocationMenuAction, SearchLocation, Tab,
};
use crate::trash::{Trash, TrashExt};

static MENU_ID: LazyLock<cosmic::widget::Id> =
    LazyLock::new(|| cosmic::widget::Id::new("responsive-menu"));

const fn menu_button_optional(
    label: String,
    action: Action,
    enabled: bool,
) -> menu::Item<Action, String> {
    if enabled {
        menu::Item::Button(label, None, action)
    } else {
        menu::Item::ButtonDisabled(label, None, action)
    }
}

/// A menu action dispatched to the tab the menu was opened on.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TabAction(pub Action);

impl MenuAction for TabAction {
    type Message = tab::Message;

    fn message(&self) -> tab::Message {
        tab::Message::ContextAction(self.0)
    }
}

pub(crate) struct MenuSnapshot<Message> {
    open: bool,
    items: std::cell::RefCell<Option<Vec<menu::Tree<Message>>>>,
}

impl<Message: Clone + 'static> MenuSnapshot<Message> {
    pub(crate) fn new() -> Self {
        Self {
            open: false,
            items: std::cell::RefCell::new(None),
        }
    }

    pub(crate) fn set_open(&mut self, open: bool) {
        if open && !self.open {
            self.items.get_mut().take();
        }
        self.open = open;
    }

    pub(crate) fn view(
        &self,
        build: impl FnOnce() -> Vec<menu::Tree<Message>>,
    ) -> Vec<menu::Tree<Message>> {
        if crate::is_wayland() {
            return build();
        }
        let mut items = self.items.borrow_mut();
        if !self.open || items.is_none() {
            *items = Some(build());
        }
        items.as_ref().unwrap().clone()
    }
}

pub(crate) fn lone_desktop_entry(items: &[tab::Item]) -> Option<&Path> {
    let mut selected = items.iter().filter(|item| item.selected);
    let only = selected.next()?;
    if selected.next().is_some()
        || !matches!(&only.metadata, tab::ItemMetadata::Path { metadata, .. } if metadata.is_file())
    {
        return None;
    }
    match &only.location_opt {
        Some(Location::Path(path))
            if path.extension().and_then(|extension| extension.to_str()) == Some("desktop") =>
        {
            Some(path)
        }
        _ => None,
    }
}

#[derive(Clone, Debug)]
pub struct DesktopAction {
    pub name: String,
    pub exec: String,
}

#[derive(Clone, Debug)]
pub struct DesktopEntryActions {
    pub id: u64,
    pub actions: Vec<DesktopAction>,
}

impl DesktopEntryActions {
    pub fn new(actions: Vec<DesktopAction>) -> Self {
        Self {
            id: tab::next_work_id(),
            actions,
        }
    }

    #[cfg(feature = "desktop")]
    pub fn from_entry(entry: cosmic::desktop::DesktopEntryData) -> Self {
        Self::new(
            entry
                .desktop_actions
                .into_iter()
                .map(|action| DesktopAction {
                    name: action.name,
                    exec: action.exec,
                })
                .collect(),
        )
    }
}

pub(crate) fn desktop_entry_actions(tab: &Tab) -> Option<&DesktopEntryActions> {
    let items = tab.items_opt()?;
    let selected_path = lone_desktop_entry(items)?;
    let (path, work_id, actions) = tab.context_menu_desktop_entry.as_ref()?;
    (path == selected_path
        && items
            .iter()
            .any(|item| item.selected && item.work_id == *work_id))
    .then_some(actions)
}

pub fn context_menu(
    tab: &Tab,
    key_binds: &HashMap<KeyBind, Action>,
    modifiers: &Modifiers,
    clipboard_paste_available: bool,
    context_actions: &[ContextActionPreset],
) -> Vec<menu::Tree<tab::Message>> {
    let children = context_menu_items(tab, modifiers, clipboard_paste_available, context_actions);
    let key_binds: HashMap<KeyBind, TabAction> = key_binds
        .iter()
        .map(|(key_bind, action)| (key_bind.clone(), TabAction(*action)))
        .collect();
    menu::items(&key_binds, children)
}

fn context_menu_items(
    tab: &Tab,
    modifiers: &Modifiers,
    clipboard_paste_available: bool,
    context_actions: &[ContextActionPreset],
) -> Vec<menu::Item<TabAction, String>> {
    let menu_item =
        |label: String, action: Action| menu::Item::Button(label, None, TabAction(action));
    let menu_item_disabled =
        |label: String, action: Action| menu::Item::ButtonDisabled(label, None, TabAction(action));
    let delete_item = || {
        let (label, action) = delete_menu_entry(false, modifiers);
        menu_item(label, action)
    };

    // Allow paste when clipboard has data and we're in a location that supports it
    let can_paste = clipboard_paste_available && tab.location.supports_paste();

    let (sort_name, sort_direction, _) = tab.sort_options();
    let sort_item = |label: String, variant| {
        let entry = menu::Entry::new(label, TabAction(Action::ToggleSort(variant)));
        menu::Item::Entry(if sort_name == variant {
            let icon_name = if sort_direction {
                "view-sort-ascending-symbolic"
            } else {
                "view-sort-descending-symbolic"
            };
            entry.icon(widget::icon::from_name(icon_name).size(14).handle())
        } else {
            entry.reserve_icon()
        })
    };

    let mut selected_dir = 0;
    let mut selected = 0;
    let mut selected_trash_only = false;
    let mut selected_types: Vec<Mime> = vec![];
    let mut selected_mount_point = 0;
    let mut any_trash_item = false;
    if let Some(items) = tab.items_opt() {
        for item in items {
            if item.selected {
                selected += 1;
                if item.metadata.is_dir() {
                    selected_mount_point += i32::from(item.is_mount_point);
                    selected_dir += 1;
                }
                match &item.location_opt {
                    Some(Location::Trash) | Some(Location::Search(SearchLocation::Trash, ..)) => {
                        selected_trash_only = true
                    }
                    _ => (),
                }
                if matches!(&item.metadata, ItemMetadata::Trash { .. }) {
                    any_trash_item = true;
                }
                selected_types.push(item.mime.clone());
            }
        }
    }
    selected_types.sort_unstable();
    selected_types.dedup();
    selected_trash_only = selected_trash_only && selected == 1;
    let context_action_items = |selected: usize, selected_dir: usize| {
        context_actions
            .iter()
            .enumerate()
            .filter(|(_, action)| action.matches_selection(selected, selected_dir))
            .map(|(i, action)| menu_item(action.name.clone(), Action::RunContextAction(i)))
            .collect::<Vec<_>>()
    };
    let desktop_actions = desktop_entry_actions(tab);

    let mut children: Vec<menu::Item<TabAction, String>> = Vec::new();
    match (&tab.mode, &tab.location) {
        (
            tab::Mode::App | tab::Mode::Desktop,
            Location::Desktop(..)
            | Location::Path(..)
            | Location::Search(SearchLocation::Path(..), ..)
            | Location::Search(SearchLocation::Recents, ..)
            | Location::Recents
            | Location::Network(_, _, Some(_)),
        ) => {
            if selected_trash_only {
                children.push(menu_item(fl!("open"), Action::Open));
                if !Trash::is_empty_cached() {
                    children.push(menu_item(fl!("empty-trash"), Action::EmptyTrash));
                }
            } else if let Some(snapshot) = desktop_actions {
                children.push(menu_item(fl!("open"), Action::Open));
                #[cfg(feature = "desktop")]
                {
                    children.extend(snapshot.actions.iter().enumerate().map(|(i, action)| {
                        menu_item(action.name.clone(), Action::ExecEntryAction(snapshot.id, i))
                    }));
                }
                #[cfg(not(feature = "desktop"))]
                let _ = snapshot;
                children.push(menu::Item::Divider);
                children.push(menu_item(fl!("rename"), Action::Rename));
                children.push(menu_item(fl!("cut"), Action::Cut));
                if modifiers.shift() && !modifiers.control() {
                    children.push(menu_item(fl!("copy-path"), Action::CopyPath));
                } else {
                    children.push(menu_item(fl!("copy"), Action::Copy));
                }
                children.push(delete_item());
                let action_items = context_action_items(selected, selected_dir);
                if !action_items.is_empty() {
                    children.push(menu::Item::Divider);
                    children.extend(action_items);
                }
            } else if selected > 0 {
                if selected_dir == 1 && selected == 1 || selected_dir == 0 {
                    children.push(menu_item(fl!("open"), Action::Open));
                }
                if selected == 1 {
                    children.push(menu_item(fl!("menu-open-with"), Action::OpenWith));
                    if selected_dir == 1 {
                        children.push(menu_item(fl!("open-in-terminal"), Action::OpenTerminal));
                    }
                }
                if tab.location.is_recents() || matches!(tab.location, Location::Search(..)) {
                    children.push(menu_item(
                        fl!("open-item-location"),
                        Action::OpenItemLocation,
                    ));
                }
                // All selected items are directories
                if selected == selected_dir && matches!(tab.mode, tab::Mode::App) {
                    children.push(menu_item(fl!("open-in-new-tab"), Action::OpenInNewTab));
                    children.push(menu_item(
                        fl!("open-in-new-window"),
                        Action::OpenInNewWindow,
                    ));
                }
                let action_items = context_action_items(selected, selected_dir);
                if !action_items.is_empty() {
                    children.push(menu::Item::Divider);
                    children.extend(action_items);
                }
                children.push(menu::Item::Divider);
                if selected_mount_point == 0 {
                    children.push(menu_item(fl!("rename"), Action::Rename));
                    children.push(menu_item(fl!("cut"), Action::Cut));
                }
                if modifiers.shift() && !modifiers.control() {
                    children.push(menu_item(fl!("copy-path"), Action::CopyPath));
                } else {
                    children.push(menu_item(fl!("copy"), Action::Copy));
                }
                if selected_mount_point == 0 {
                    children.push(menu_item(fl!("move-to"), Action::MoveTo));
                }
                children.push(menu_item(fl!("copy-to"), Action::CopyTo));

                children.push(menu::Item::Divider);
                let supported_archive_types = crate::archive::SUPPORTED_ARCHIVE_TYPES;
                selected_types.retain(|t| supported_archive_types.iter().copied().all(|m| *t != m));
                if selected_types.is_empty() {
                    children.push(menu_item(fl!("extract-here"), Action::ExtractHere));
                    children.push(menu_item(fl!("extract-to"), Action::ExtractTo));
                }
                children.push(menu_item(fl!("compress"), Action::Compress));
                children.push(menu::Item::Divider);

                //TODO: Print?
                children.push(menu_item(fl!("show-details"), Action::Preview));
                if any_trash_item {
                    children.push(menu::Item::Divider);
                    children.push(menu_item(
                        fl!("restore-from-trash"),
                        Action::RestoreFromTrash,
                    ));
                    children.push(menu::Item::Divider);
                    children.push(menu_item(
                        fl!("delete-permanently"),
                        Action::PermanentlyDelete,
                    ));
                } else {
                    if matches!(tab.mode, tab::Mode::App) {
                        children.push(menu::Item::Divider);
                        children.push(menu_item(fl!("add-to-sidebar"), Action::AddToSidebar));
                    }
                    children.push(menu::Item::Divider);
                    if tab.location.is_recents() {
                        children.push(menu_item(
                            fl!("remove-from-recents"),
                            Action::RemoveFromRecents,
                        ));
                        children.push(menu::Item::Divider);
                    }
                    if selected_mount_point == 0 {
                        children.push(delete_item());
                    } else if selected == 1 {
                        children.push(menu_item(fl!("eject"), Action::Eject));
                    }
                }
            } else {
                //TODO: need better designs for menu with no selection
                //TODO: have things like properties but they apply to the folder?
                if tab.location != Location::Recents {
                    children.push(menu_item(fl!("new-folder"), Action::NewFolder));
                    children.push(menu_item(fl!("new-file"), Action::NewFile));
                    children.push(menu_item(fl!("open-in-terminal"), Action::OpenTerminal));
                    children.push(menu::Item::Divider);
                }

                if tab.mode.multiple() {
                    children.push(menu_item(fl!("select-all"), Action::SelectAll));
                }
                if can_paste {
                    children.push(menu_item(fl!("paste"), Action::Paste));
                } else {
                    children.push(menu_item_disabled(fl!("paste"), Action::Paste));
                }

                //TODO: only show if cosmic-settings is found?
                if matches!(tab.mode, tab::Mode::Desktop) {
                    children.push(menu::Item::Divider);
                    children.push(menu_item(
                        fl!("change-wallpaper"),
                        Action::CosmicSettingsWallpaper,
                    ));
                    children.push(menu_item(
                        fl!("desktop-appearance"),
                        Action::CosmicSettingsDesktop,
                    ));
                    children.push(menu_item(
                        fl!("display-settings"),
                        Action::CosmicSettingsDisplays,
                    ));
                }

                children.push(menu::Item::Divider);
                // TODO: Nested menu
                children.push(sort_item(fl!("sort-by-name"), HeadingOptions::Name));
                children.push(sort_item(fl!("sort-by-modified"), HeadingOptions::Modified));
                children.push(sort_item(fl!("sort-by-size"), HeadingOptions::Size));
                if matches!(tab.location, Location::Desktop(..)) {
                    children.push(menu::Item::Divider);
                    children.push(menu_item(
                        fl!("desktop-view-options"),
                        Action::DesktopViewOptions,
                    ));
                }
            }
        }
        (
            tab::Mode::Dialog(dialog_kind),
            Location::Desktop(..)
            | Location::Path(..)
            | Location::Search(SearchLocation::Path(..), ..)
            | Location::Search(SearchLocation::Recents, ..)
            | Location::Recents
            | Location::Network(_, _, Some(_)),
        ) => {
            if selected > 0 {
                if selected_dir == 1 && selected == 1 || selected_dir == 0 {
                    children.push(menu_item(fl!("open"), Action::Open));
                }
                if matches!(tab.location, Location::Search(..)) || tab.location.is_recents() {
                    children.push(menu_item(
                        fl!("open-item-location"),
                        Action::OpenItemLocation,
                    ));
                }
                children.push(menu::Item::Divider);
                children.push(menu_item(fl!("show-details"), Action::Preview));
            } else {
                if dialog_kind.save() {
                    children.push(menu_item(fl!("new-folder"), Action::NewFolder));
                }
                if tab.mode.multiple() {
                    children.push(menu_item(fl!("select-all"), Action::SelectAll));
                }
                if !children.is_empty() {
                    children.push(menu::Item::Divider);
                }
                children.push(sort_item(fl!("sort-by-name"), HeadingOptions::Name));
                children.push(sort_item(fl!("sort-by-modified"), HeadingOptions::Modified));
                children.push(sort_item(fl!("sort-by-size"), HeadingOptions::Size));
            }
        }
        (_, Location::Network(..)) => {
            if selected > 0 {
                if selected_dir == 1 && selected == 1 || selected_dir == 0 {
                    children.push(menu_item(fl!("open"), Action::Open));
                }
            } else {
                if tab.mode.multiple() {
                    children.push(menu_item(fl!("select-all"), Action::SelectAll));
                }
                if !children.is_empty() {
                    children.push(menu::Item::Divider);
                }
                children.push(sort_item(fl!("sort-by-name"), HeadingOptions::Name));
                children.push(sort_item(fl!("sort-by-modified"), HeadingOptions::Modified));
                children.push(sort_item(fl!("sort-by-size"), HeadingOptions::Size));
            }
        }
        (_, Location::Trash | Location::Search(SearchLocation::Trash, ..)) => {
            if tab.mode.multiple() {
                children.push(menu_item(fl!("select-all"), Action::SelectAll));
            }
            if !children.is_empty() {
                children.push(menu::Item::Divider);
            }
            if selected > 0 {
                children.push(menu_item(fl!("show-details"), Action::Preview));
                children.push(menu::Item::Divider);
                children.push(menu_item(
                    fl!("restore-from-trash"),
                    Action::RestoreFromTrash,
                ));
                children.push(menu::Item::Divider);
                let (label, action) = delete_menu_entry(true, modifiers);
                children.push(menu_item(label, action));
            } else {
                // TODO: Nested menu
                children.push(sort_item(fl!("sort-by-name"), HeadingOptions::Name));
                children.push(sort_item(fl!("sort-by-trashed"), HeadingOptions::TrashedOn));
                children.push(sort_item(fl!("sort-by-size"), HeadingOptions::Size));
            }
        }
    }

    children
}

pub fn dialog_menu(
    tab: &Tab,
    key_binds: &HashMap<KeyBind, Action>,
    show_details: bool,
) -> Element<'static, Message> {
    let (sort_name, sort_direction, _) = tab.sort_options();
    let sort_item = |label, sort, dir| {
        menu::Item::CheckBox(
            label,
            None,
            sort_name == sort && sort_direction == dir,
            Action::SetSort(sort, dir),
        )
    };
    let in_trash = tab.location.is_trash();

    let mut selected_gallery = 0;
    if let Some(items) = tab.items_opt() {
        for item in items {
            if item.selected && item.can_gallery() {
                selected_gallery += 1;
            }
        }
    }

    MenuBar::new(vec![
        menu::Tree::with_children(
            Element::from(
                widget::button::icon(widget::icon::from_name(match tab.config.view {
                    tab::View::Grid => "view-grid-symbolic",
                    tab::View::List => "view-list-symbolic",
                }))
                // This prevents the button from being shown as insensitive
                .on_press(Message::None)
                .padding(8),
            ),
            menu::items(
                key_binds,
                vec![
                    menu::Item::CheckBox(
                        fl!("grid-view"),
                        None,
                        matches!(tab.config.view, tab::View::Grid),
                        Action::TabViewGrid,
                    ),
                    menu::Item::CheckBox(
                        fl!("list-view"),
                        None,
                        matches!(tab.config.view, tab::View::List),
                        Action::TabViewList,
                    ),
                ],
            ),
        ),
        menu::Tree::with_children(
            Element::from(
                widget::button::icon(widget::icon::from_name(if sort_direction {
                    "view-sort-ascending-symbolic"
                } else {
                    "view-sort-descending-symbolic"
                }))
                // This prevents the button from being shown as insensitive
                .on_press(Message::None)
                .padding(8),
            ),
            menu::items(
                key_binds,
                vec![
                    sort_item(fl!("sort-a-z"), tab::HeadingOptions::Name, true),
                    sort_item(fl!("sort-z-a"), tab::HeadingOptions::Name, false),
                    sort_item(
                        fl!("sort-newest-first"),
                        if in_trash {
                            tab::HeadingOptions::TrashedOn
                        } else {
                            tab::HeadingOptions::Modified
                        },
                        false,
                    ),
                    sort_item(
                        fl!("sort-oldest-first"),
                        if in_trash {
                            tab::HeadingOptions::TrashedOn
                        } else {
                            tab::HeadingOptions::Modified
                        },
                        true,
                    ),
                    sort_item(
                        fl!("sort-smallest-to-largest"),
                        tab::HeadingOptions::Size,
                        true,
                    ),
                    sort_item(
                        fl!("sort-largest-to-smallest"),
                        tab::HeadingOptions::Size,
                        false,
                    ),
                    //TODO: sort by type
                ],
            ),
        ),
        menu::Tree::with_children(
            Element::from(
                widget::button::icon(widget::icon::from_name("view-more-symbolic"))
                    // This prevents the button from being shown as insensitive
                    .on_press(Message::None)
                    .padding(8),
            ),
            menu::items(
                key_binds,
                vec![
                    menu::Item::Button(fl!("zoom-in"), None, Action::ZoomIn),
                    menu::Item::Button(fl!("default-size"), None, Action::ZoomDefault),
                    menu::Item::Button(fl!("zoom-out"), None, Action::ZoomOut),
                    menu::Item::Divider,
                    menu::Item::CheckBox(
                        fl!("show-hidden-files"),
                        None,
                        tab.config.show_hidden,
                        Action::ToggleShowHidden,
                    ),
                    menu::Item::CheckBox(
                        fl!("list-directories-first"),
                        None,
                        tab.config.folders_first,
                        Action::ToggleFoldersFirst,
                    ),
                    menu::Item::CheckBox(fl!("show-details"), None, show_details, Action::Preview),
                    menu::Item::Divider,
                    menu_button_optional(
                        fl!("gallery-preview"),
                        Action::Gallery,
                        selected_gallery > 0,
                    ),
                ],
            ),
        ),
    ])
    .item_height(ItemHeight::Dynamic(40))
    .item_width(ItemWidth::Uniform(360))
    .spacing(theme::spacing().space_xxxs.into())
    .into()
}

/// The label and action of a menu's delete entry. Inside the Trash, `Delete` removes
/// entries together with their trash records.
fn delete_menu_entry(in_trash: bool, modifiers: &Modifiers) -> (String, Action) {
    if in_trash {
        (fl!("delete-permanently"), Action::Delete)
    } else if modifiers.shift() && !modifiers.control() {
        (fl!("delete-permanently"), Action::PermanentlyDelete)
    } else {
        (fl!("move-to-trash"), Action::Delete)
    }
}

pub fn menu_bar<'a>(
    core: &Core,
    tab_opt: Option<&Tab>,
    config: &Config,
    modifiers: &Modifiers,
    key_binds: &HashMap<KeyBind, Action>,
    clipboard_paste_available: bool,
) -> Element<'a, Message> {
    let sort_options = tab_opt.map(Tab::sort_options);
    let sort_item = |label, sort, dir| {
        menu::Item::CheckBox(
            label,
            None,
            sort_options.is_some_and(|(sort_name, sort_direction, _)| {
                sort_name == sort && sort_direction == dir
            }),
            Action::SetSort(sort, dir),
        )
    };
    let in_trash = tab_opt.is_some_and(|tab| tab.location.is_trash());

    let mut selected_dir = 0;
    let mut selected = 0;
    let mut selected_gallery = 0;
    if let Some(items) = tab_opt.and_then(|tab| tab.items_opt()) {
        for item in items {
            if item.selected {
                selected += 1;
                if item.metadata.is_dir() {
                    selected_dir += 1;
                }
                if item.can_gallery() {
                    selected_gallery += 1;
                }
            }
        }
    }

    // Allow paste when clipboard has data and we're in a location that supports it
    let can_paste =
        clipboard_paste_available && tab_opt.is_some_and(|tab| tab.location.supports_paste());

    let (delete_label, delete_action) = delete_menu_entry(in_trash, modifiers);

    responsive_menu_bar()
        .item_height(ItemHeight::Dynamic(40))
        .item_width(ItemWidth::Uniform(360))
        .spacing(theme::spacing().space_xxxs.into())
        .into_element(
            core,
            key_binds,
            MENU_ID.clone(),
            Message::Surface,
            vec![
                (
                    fl!("file"),
                    vec![
                        menu::Item::Button(fl!("new-tab"), None, Action::TabNew),
                        menu::Item::Button(fl!("new-window"), None, Action::WindowNew),
                        menu::Item::Button(fl!("new-folder"), None, Action::NewFolder),
                        menu::Item::Button(fl!("new-file"), None, Action::NewFile),
                        menu_button_optional(
                            fl!("open"),
                            Action::Open,
                            (selected > 0 && selected_dir == 0)
                                || (selected_dir == 1 && selected == 1),
                        ),
                        menu_button_optional(
                            fl!("menu-open-with"),
                            Action::OpenWith,
                            selected == 1,
                        ),
                        menu::Item::Divider,
                        menu_button_optional(fl!("rename"), Action::Rename, selected > 0),
                        menu::Item::Divider,
                        menu::Item::Button(fl!("reload-folder"), None, Action::Reload),
                        menu::Item::Divider,
                        menu_button_optional(
                            fl!("add-to-sidebar"),
                            Action::AddToSidebar,
                            selected > 0,
                        ),
                        menu::Item::Divider,
                        menu_button_optional(
                            fl!("restore-from-trash"),
                            Action::RestoreFromTrash,
                            selected > 0 && in_trash,
                        ),
                        menu_button_optional(delete_label, delete_action, selected > 0),
                        menu::Item::Divider,
                        menu::Item::Button(fl!("close-tab"), None, Action::TabClose),
                        menu::Item::Button(fl!("quit"), None, Action::WindowClose),
                    ],
                ),
                (
                    (fl!("edit")),
                    vec![
                        menu_button_optional(fl!("cut"), Action::Cut, selected > 0),
                        menu_button_optional(fl!("copy"), Action::Copy, selected > 0),
                        menu_button_optional(fl!("move-to"), Action::MoveTo, selected > 0),
                        menu_button_optional(fl!("copy-to"), Action::CopyTo, selected > 0),
                        menu_button_optional(fl!("paste"), Action::Paste, can_paste),
                        menu::Item::Button(fl!("select-all"), None, Action::SelectAll),
                        menu::Item::Divider,
                        menu::Item::Button(fl!("history"), None, Action::EditHistory),
                    ],
                ),
                (
                    (fl!("view")),
                    vec![
                        menu::Item::Button(fl!("zoom-in"), None, Action::ZoomIn),
                        menu::Item::Button(fl!("default-size"), None, Action::ZoomDefault),
                        menu::Item::Button(fl!("zoom-out"), None, Action::ZoomOut),
                        menu::Item::Divider,
                        menu::Item::CheckBox(
                            fl!("grid-view"),
                            None,
                            tab_opt.is_some_and(|tab| matches!(tab.config.view, tab::View::Grid)),
                            Action::TabViewGrid,
                        ),
                        menu::Item::CheckBox(
                            fl!("list-view"),
                            None,
                            tab_opt.is_some_and(|tab| matches!(tab.config.view, tab::View::List)),
                            Action::TabViewList,
                        ),
                        menu::Item::Divider,
                        menu::Item::CheckBox(
                            fl!("show-hidden-files"),
                            None,
                            tab_opt.is_some_and(|tab| tab.config.show_hidden),
                            Action::ToggleShowHidden,
                        ),
                        menu::Item::CheckBox(
                            fl!("list-directories-first"),
                            None,
                            tab_opt.is_some_and(|tab| tab.config.folders_first),
                            Action::ToggleFoldersFirst,
                        ),
                        menu::Item::CheckBox(
                            fl!("show-details"),
                            None,
                            config.show_details,
                            Action::Preview,
                        ),
                        menu::Item::Divider,
                        menu_button_optional(
                            fl!("gallery-preview"),
                            Action::Gallery,
                            selected_gallery > 0,
                        ),
                        menu::Item::Divider,
                        menu::Item::Button(fl!("menu-settings"), None, Action::Settings),
                        menu::Item::Divider,
                        menu::Item::Button(fl!("menu-about"), None, Action::About),
                    ],
                ),
                (
                    (fl!("sort")),
                    vec![
                        sort_item(fl!("sort-a-z"), tab::HeadingOptions::Name, true),
                        sort_item(fl!("sort-z-a"), tab::HeadingOptions::Name, false),
                        sort_item(
                            fl!("sort-newest-first"),
                            if in_trash {
                                tab::HeadingOptions::TrashedOn
                            } else {
                                tab::HeadingOptions::Modified
                            },
                            false,
                        ),
                        sort_item(
                            fl!("sort-oldest-first"),
                            if in_trash {
                                tab::HeadingOptions::TrashedOn
                            } else {
                                tab::HeadingOptions::Modified
                            },
                            true,
                        ),
                        sort_item(
                            fl!("sort-smallest-to-largest"),
                            tab::HeadingOptions::Size,
                            true,
                        ),
                        sort_item(
                            fl!("sort-largest-to-smallest"),
                            tab::HeadingOptions::Size,
                            false,
                        ),
                        //TODO: sort by type
                    ],
                ),
            ],
        )
}

pub fn location_context_menu(ancestor_index: usize) -> Vec<menu::Tree<tab::Message>> {
    //TODO: only add some of these when in App mode
    menu::items(
        &HashMap::new(),
        vec![
            menu::Item::Button(
                fl!("open-in-new-tab"),
                None,
                LocationMenuAction::OpenInNewTab(ancestor_index),
            ),
            menu::Item::Button(
                fl!("open-in-new-window"),
                None,
                LocationMenuAction::OpenInNewWindow(ancestor_index),
            ),
            menu::Item::Divider,
            menu::Item::Button(
                fl!("show-details"),
                None,
                LocationMenuAction::Preview(ancestor_index),
            ),
            menu::Item::Divider,
            menu::Item::Button(
                fl!("add-to-sidebar"),
                None,
                LocationMenuAction::AddToSidebar(ancestor_index),
            ),
        ],
    )
}

#[cfg(test)]
mod tests {
    use std::io;

    use tempfile::TempDir;

    use cosmic::iced::keyboard::Modifiers;
    use cosmic::widget::menu;

    use super::{TabAction, lone_desktop_entry};
    use crate::app::Action;
    use crate::config::IconSizes;
    use crate::fl;
    use crate::tab::{Item, item_from_path};

    fn selected_items(dir: &TempDir, names: &[&str]) -> io::Result<Vec<Item>> {
        let mut items = Vec::new();
        for name in names {
            let path = dir.path().join(name);
            std::fs::write(&path, b"")?;
            let mut item = item_from_path(&path, IconSizes::default())
                .expect("a written file should resolve to an item");
            item.selected = true;
            items.push(item);
        }
        Ok(items)
    }

    #[test]
    fn a_lone_desktop_entry_is_offered_its_own_menu() -> io::Result<()> {
        let dir = TempDir::new()?;
        let items = selected_items(&dir, &["editor.desktop"])?;

        assert_eq!(
            lone_desktop_entry(&items),
            Some(dir.path().join("editor.desktop").as_path())
        );
        Ok(())
    }

    #[test]
    fn a_desktop_entry_among_several_selected_items_is_not() -> io::Result<()> {
        let dir = TempDir::new()?;
        let items = selected_items(&dir, &["editor.desktop", "notes.txt"])?;

        assert_eq!(lone_desktop_entry(&items), None);
        Ok(())
    }

    #[test]
    fn a_lone_selected_file_that_is_not_a_desktop_entry_is_not() -> io::Result<()> {
        let dir = TempDir::new()?;
        let items = selected_items(&dir, &["notes.txt"])?;

        assert_eq!(lone_desktop_entry(&items), None);
        Ok(())
    }

    #[test]
    fn an_unselected_desktop_entry_is_not() -> io::Result<()> {
        let dir = TempDir::new()?;
        let mut items = selected_items(&dir, &["editor.desktop"])?;
        items[0].selected = false;

        assert_eq!(lone_desktop_entry(&items), None);
        Ok(())
    }

    #[test]
    fn review_a_directory_with_a_desktop_suffix_keeps_its_folder_menu() -> io::Result<()> {
        let dir = TempDir::new()?;
        let path = dir.path().join("backups.desktop");
        std::fs::create_dir(&path)?;
        let mut item = item_from_path(&path, IconSizes::default()).unwrap();
        item.selected = true;
        assert_eq!(lone_desktop_entry(&[item]), None);
        Ok(())
    }

    fn desktop_tab(dir: &TempDir) -> io::Result<crate::tab::Tab> {
        path_tab(dir, "editor.desktop")
    }

    fn path_tab(dir: &TempDir, name: &str) -> io::Result<crate::tab::Tab> {
        Ok(tab_showing(dir, selected_items(dir, &[name])?))
    }

    fn tab_showing(dir: &TempDir, items: Vec<Item>) -> crate::tab::Tab {
        tab_at(crate::tab::Location::Path(dir.path().to_path_buf()), items)
    }

    fn tab_at(location: crate::tab::Location, items: Vec<Item>) -> crate::tab::Tab {
        let mut tab = crate::tab::Tab::new(
            location,
            Default::default(),
            Default::default(),
            None,
            cosmic::widget::Id::unique(),
            None,
        );
        tab.set_items(items);
        tab.items_opt.as_mut().unwrap()[0].selected = true;
        tab
    }

    fn actions(names: &[&str]) -> super::DesktopEntryActions {
        super::DesktopEntryActions::new(
            names
                .iter()
                .map(|name| super::DesktopAction {
                    name: (*name).into(),
                    exec: format!("test-{name}"),
                })
                .collect(),
        )
    }

    fn action_names(tab: &crate::tab::Tab) -> Option<Vec<String>> {
        super::desktop_entry_actions(tab).map(|snapshot| {
            snapshot
                .actions
                .iter()
                .map(|action| action.name.clone())
                .collect()
        })
    }

    #[cfg(feature = "desktop")]
    #[test]
    fn a_desktop_action_keeps_the_command_shown_in_its_menu() -> io::Result<()> {
        let dir = TempDir::new()?;
        let mut tab = desktop_tab(&dir)?;
        let path = dir.path().join("editor.desktop");
        let work_id = tab.items_opt().unwrap()[0].work_id;
        let modifiers = cosmic::iced::keyboard::Modifiers::empty();
        std::fs::write(
            &path,
            "[Desktop Entry]\nType=Application\nName=Editor\nExec=editor\nActions=edit;\n\n[Desktop Action edit]\nName=Original\nExec=original-command\n",
        )?;
        let original = cosmic::desktop::load_desktop_file(&[], path.clone()).unwrap();
        tab.update(
            crate::tab::Message::DesktopEntryParsed(
                path.clone(),
                work_id,
                Some(super::DesktopEntryActions::from_entry(original)),
            ),
            modifiers,
        );
        std::fs::write(
            &path,
            "[Desktop Entry]\nType=Application\nName=Editor\nExec=editor\nActions=edit;\n\n[Desktop Action edit]\nName=Changed\nExec=changed-command\n",
        )?;
        let snapshot = super::desktop_entry_actions(&tab).unwrap().id;
        let commands = tab.update(crate::tab::Message::ExecEntryAction(snapshot, 0), modifiers);
        let command = commands.into_iter().find_map(|command| match command {
            crate::tab::Command::ExecEntryAction(command) => Some(command),
            _ => None,
        });
        assert_eq!(command.as_deref(), Some("original-command"));
        Ok(())
    }

    #[cfg(feature = "desktop")]
    #[test]
    fn an_obsolete_menu_cannot_run_a_new_snapshots_action() -> io::Result<()> {
        let dir = TempDir::new()?;
        let mut tab = desktop_tab(&dir)?;
        let path = dir.path().join("editor.desktop");
        let work_id = tab.items_opt().unwrap()[0].work_id;
        let modifiers = cosmic::iced::keyboard::Modifiers::empty();
        let original = actions(&["Original"]);
        let old_id = original.id;
        tab.update(
            crate::tab::Message::DesktopEntryParsed(path.clone(), work_id, Some(original)),
            modifiers,
        );
        tab.update(
            crate::tab::Message::DesktopEntryParsed(path, work_id, Some(actions(&["Changed"]))),
            modifiers,
        );
        assert!(
            tab.update(crate::tab::Message::ExecEntryAction(old_id, 0), modifiers)
                .is_empty()
        );
        Ok(())
    }

    #[test]
    fn an_open_menu_keeps_its_layout_until_the_next_invocation() -> io::Result<()> {
        let dir = TempDir::new()?;
        let mut tab = desktop_tab(&dir)?;
        let mut snapshot = super::MenuSnapshot::new();
        let keys = std::collections::HashMap::new();
        let modifiers = cosmic::iced::keyboard::Modifiers::empty();
        let before = snapshot
            .view(|| super::context_menu(&tab, &keys, &modifiers, false, &[]))
            .len();
        snapshot.set_open(true);
        snapshot.view(|| super::context_menu(&tab, &keys, &modifiers, false, &[]));
        let work_id = tab.items_opt().unwrap()[0].work_id;
        tab.update(
            crate::tab::Message::DesktopEntryParsed(
                dir.path().join("editor.desktop"),
                work_id,
                Some(actions(&[])),
            ),
            modifiers,
        );
        let after = super::context_menu(&tab, &keys, &modifiers, false, &[]).len();
        assert_ne!(before, after);
        assert_eq!(
            snapshot
                .view(|| super::context_menu(&tab, &keys, &modifiers, false, &[]))
                .len(),
            before
        );
        snapshot.set_open(false);
        assert_eq!(
            snapshot
                .view(|| super::context_menu(&tab, &keys, &modifiers, false, &[]))
                .len(),
            after
        );
        Ok(())
    }

    #[test]
    fn pending_and_failed_desktop_parses_keep_the_ordinary_menu() -> io::Result<()> {
        let dir = TempDir::new()?;
        let mut tab = desktop_tab(&dir)?;
        assert!(super::desktop_entry_actions(&tab).is_none());
        let work_id = tab.items_opt().unwrap()[0].work_id;
        tab.update(
            crate::tab::Message::DesktopEntryParsed(
                dir.path().join("editor.desktop"),
                work_id,
                None,
            ),
            cosmic::iced::keyboard::Modifiers::empty(),
        );
        assert!(super::desktop_entry_actions(&tab).is_none());
        Ok(())
    }

    #[test]
    fn a_valid_desktop_entry_without_actions_still_gets_its_launcher_menu() -> io::Result<()> {
        let dir = TempDir::new()?;
        let mut tab = desktop_tab(&dir)?;
        let work_id = tab.items_opt().unwrap()[0].work_id;
        tab.update(
            crate::tab::Message::DesktopEntryParsed(
                dir.path().join("editor.desktop"),
                work_id,
                Some(actions(&[])),
            ),
            cosmic::iced::keyboard::Modifiers::empty(),
        );
        assert_eq!(action_names(&tab), Some(vec![]));
        Ok(())
    }

    #[test]
    #[cfg(unix)]
    fn a_desktop_suffixed_socket_is_not_a_launcher() -> io::Result<()> {
        let dir = TempDir::new()?;
        let path = dir.path().join("service.desktop");
        let _socket = std::os::unix::net::UnixListener::bind(&path)?;
        let mut item = item_from_path(&path, IconSizes::default()).unwrap();
        item.selected = true;
        assert_eq!(lone_desktop_entry(&[item]), None);
        Ok(())
    }

    #[test]
    fn an_obsolete_desktop_parse_cannot_erase_the_selected_launchers_actions() -> io::Result<()> {
        let dir = TempDir::new()?;
        let mut tab = desktop_tab(&dir)?;
        let old_path = dir.path().join("editor.desktop");
        let other = selected_items(&dir, &["viewer.desktop"])?;
        tab.items_opt.as_mut().unwrap()[0].selected = false;
        tab.items_opt.as_mut().unwrap().extend(other);
        let old_id = tab.items_opt().unwrap()[0].work_id;
        let current_id = tab.items_opt().unwrap()[1].work_id;
        let modifiers = cosmic::iced::keyboard::Modifiers::empty();
        tab.update(
            crate::tab::Message::DesktopEntryParsed(
                dir.path().join("viewer.desktop"),
                current_id,
                Some(actions(&["View"])),
            ),
            modifiers,
        );
        for result in [Some(actions(&["Edit"])), None] {
            tab.update(
                crate::tab::Message::DesktopEntryParsed(old_path.clone(), old_id, result),
                modifiers,
            );
            assert_eq!(action_names(&tab), Some(vec!["View".into()]));
        }
        Ok(())
    }

    #[test]
    fn a_desktop_parse_cannot_apply_to_a_replacement_item() -> io::Result<()> {
        let dir = TempDir::new()?;
        let mut tab = desktop_tab(&dir)?;
        let path = dir.path().join("editor.desktop");
        let old = tab.items_opt().unwrap()[0].work_id;
        tab.set_items(vec![item_from_path(&path, IconSizes::default()).unwrap()]);
        tab.items_opt.as_mut().unwrap()[0].selected = true;
        tab.update(
            crate::tab::Message::DesktopEntryParsed(path, old, Some(actions(&["Old action"]))),
            cosmic::iced::keyboard::Modifiers::empty(),
        );
        assert!(super::desktop_entry_actions(&tab).is_none());
        Ok(())
    }

    // In the Trash only `Delete`'s delete-from-trash operation removes an entry together with
    // its trash record; elsewhere `Delete` trashes local files, so only `PermanentlyDelete`
    // deletes them.
    fn deletes_permanently(in_trash: bool, action: Action) -> bool {
        action
            == if in_trash {
                Action::Delete
            } else {
                Action::PermanentlyDelete
            }
    }

    fn delete_entries(items: &[menu::Item<TabAction, String>]) -> Vec<(String, Action)> {
        items
            .iter()
            .filter_map(|item| match item {
                menu::Item::Button(
                    label,
                    _,
                    TabAction(action @ (Action::Delete | Action::PermanentlyDelete)),
                ) => Some((label.clone(), *action)),
                _ => None,
            })
            .collect()
    }

    fn assert_shift_offers_permanent_delete(tab: &crate::tab::Tab) {
        let entries =
            |modifiers| delete_entries(&super::context_menu_items(tab, &modifiers, false, &[]));
        assert_eq!(
            entries(Modifiers::SHIFT),
            vec![(fl!("delete-permanently"), Action::PermanentlyDelete)]
        );
        assert_eq!(
            entries(Modifiers::empty()),
            vec![(fl!("move-to-trash"), Action::Delete)]
        );
    }

    // Every combination of location and Shift/Ctrl: an entry labelled "Delete permanently"
    // must delete permanently, and outside the Trash only Shift without Ctrl does so.
    #[test]
    fn delete_menu_entry_label_matches_its_action() {
        for in_trash in [false, true] {
            for shift in [false, true] {
                for ctrl in [false, true] {
                    let mut modifiers = Modifiers::empty();
                    modifiers.set(Modifiers::SHIFT, shift);
                    modifiers.set(Modifiers::CTRL, ctrl);
                    let (label, action) = super::delete_menu_entry(in_trash, &modifiers);
                    let permanent = deletes_permanently(in_trash, action);
                    let case = format!("in_trash={in_trash} shift={shift} ctrl={ctrl}");
                    assert_eq!(label == fl!("delete-permanently"), permanent, "{case}");
                    assert_eq!(permanent, in_trash || (shift && !ctrl), "{case}");
                }
            }
        }
    }

    #[test]
    fn shift_offers_permanent_delete_for_a_lone_desktop_launcher() -> io::Result<()> {
        let dir = TempDir::new()?;
        let mut tab = desktop_tab(&dir)?;
        let work_id = tab.items_opt().unwrap()[0].work_id;
        tab.update(
            crate::tab::Message::DesktopEntryParsed(
                dir.path().join("editor.desktop"),
                work_id,
                Some(actions(&[])),
            ),
            Modifiers::empty(),
        );
        assert!(super::desktop_entry_actions(&tab).is_some());
        assert_shift_offers_permanent_delete(&tab);
        Ok(())
    }

    #[test]
    fn shift_offers_permanent_delete_for_an_ordinary_file() -> io::Result<()> {
        let dir = TempDir::new()?;
        assert_shift_offers_permanent_delete(&path_tab(&dir, "notes.txt")?);
        Ok(())
    }

    // A folder opened from the Trash is a plain path location, so trashing its children
    // again would only move them; its "Delete permanently" entry must remove them.
    #[test]
    fn a_trashed_folders_child_is_deleted_permanently() -> io::Result<()> {
        let dir = TempDir::new()?;
        let tab = tab_showing(&dir, vec![trashed_item(&dir)?]);
        assert_eq!(
            delete_entries(&super::context_menu_items(
                &tab,
                &Modifiers::empty(),
                false,
                &[]
            )),
            vec![(fl!("delete-permanently"), Action::PermanentlyDelete)]
        );
        Ok(())
    }

    fn trashed_item(dir: &TempDir) -> io::Result<Item> {
        let path = dir.path().join("notes.txt");
        std::fs::write(&path, b"")?;
        Ok(crate::tab::item_from_trash_entry(
            trash::TrashItem {
                id: path.into_os_string(),
                name: "notes.txt".into(),
                original_parent: dir.path().join("documents"),
                time_deleted: 0,
            },
            trash::TrashItemMetadata {
                size: trash::TrashItemSize::Bytes(0),
            },
            IconSizes::default(),
        ))
    }

    // In the Trash, every modifier state offers the delete-from-trash entry.
    #[test]
    fn the_trash_offers_its_delete_from_trash_entry() -> io::Result<()> {
        let dir = TempDir::new()?;
        let tab = tab_at(crate::tab::Location::Trash, vec![trashed_item(&dir)?]);
        for modifiers in [Modifiers::empty(), Modifiers::SHIFT] {
            assert_eq!(
                delete_entries(&super::context_menu_items(&tab, &modifiers, false, &[])),
                vec![(fl!("delete-permanently"), Action::Delete)],
                "{modifiers:?}"
            );
        }
        Ok(())
    }
}
