use std::{collections::HashMap, fs, io};

use cosmic::iced::advanced::layout::{Layout, Limits};
use cosmic::iced::advanced::renderer::Headless;
use cosmic::iced::advanced::widget::{Operation, Tree, operation};
use cosmic::iced::advanced::{Shell, clipboard};
use cosmic::iced::widget::scrollable::AbsoluteOffset;
use cosmic::iced::{
    Event, Font, Pixels, Point, Rectangle, Size, Vector, keyboard::Modifiers, mouse,
};
use cosmic::widget::menu::key_bind::KeyBind;
use cosmic::{Element, widget};
use cosmic_files::app::Action;
use cosmic_files::config::{IconSizes, TabConfig, ThumbCfg};
use cosmic_files::tab::{Location, Message, Tab, View, scan_path};
use tempfile::TempDir;

const VIEW_SIZE: Size = Size::new(800.0, 300.0);
const SCROLL_Y: f32 = 500.0;

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
        _: &mut dyn operation::Focusable,
    ) {
        if let Some(id) = id {
            self.0.push((id.clone(), bounds));
        }
    }
}

#[derive(Default)]
struct ScrollToViewport(Option<Rectangle>);

impl Operation for ScrollToViewport {
    fn traverse(&mut self, visit: &mut dyn FnMut(&mut dyn Operation)) {
        visit(self);
    }

    fn scrollable(
        &mut self,
        _: Option<&widget::Id>,
        bounds: Rectangle,
        _: Rectangle,
        _: Vector,
        state: &mut dyn operation::Scrollable,
    ) {
        state.scroll_to(
            AbsoluteOffset {
                x: 0.0,
                y: SCROLL_Y,
            }
            .into(),
        );
        assert!(
            self.0.replace(bounds).is_none(),
            "tab must have one scrollable"
        );
    }
}

fn tab_view<'a>(
    tab: &'a Tab,
    key_binds: &'a HashMap<KeyBind, Action>,
    modifiers: &'a Modifiers,
) -> Element<'a, Message> {
    tab.view_responsive(key_binds, modifiers, VIEW_SIZE, false, &[])
}

fn drag_selects_visible_items(view: View, expected: [&str; 2]) -> io::Result<()> {
    let directory = TempDir::new()?;
    for index in 0..100 {
        fs::write(directory.path().join(format!("file{index:05}.txt")), b"x")?;
    }
    let mut tab = Tab::new(
        Location::Path(directory.path().to_owned()),
        TabConfig {
            view,
            ..TabConfig::default()
        },
        ThumbCfg::default(),
        None,
        widget::Id::unique(),
        None,
    );
    tab.set_items(scan_path(
        &directory.path().to_owned(),
        IconSizes::default(),
    ));
    tab.size_opt.set(Some(VIEW_SIZE));
    tab.scroll_opt = Some(AbsoluteOffset {
        x: 0.0,
        y: SCROLL_Y,
    });

    let renderer = cosmic::iced::futures::executor::block_on(<cosmic::Renderer as Headless>::new(
        Font::default(),
        Pixels(14.0),
        Some("tiny-skia"),
    ))
    .expect("software renderer must be available");
    let key_binds = HashMap::new();
    let modifiers = Modifiers::empty();
    let (mut tree, start, end) = {
        let mut element = tab_view(&tab, &key_binds, &modifiers);
        let mut tree = Tree::new(element.as_widget());
        let node = element.as_widget_mut().layout(
            &mut tree,
            &renderer,
            &Limits::new(Size::ZERO, VIEW_SIZE),
        );
        let mut scroll = ScrollToViewport::default();
        element
            .as_widget_mut()
            .operate(&mut tree, Layout::new(&node), &renderer, &mut scroll);
        let viewport = scroll.0.expect("tab must have a scrollable");
        let items = tab.items_opt().expect("directory must be listed");
        let first_item = items
            .iter()
            .find(|item| item.name == "file00000.txt")
            .unwrap();
        assert!(
            tree.find(&first_item.button_id).is_none(),
            "earlier rows must be culled"
        );

        let mut bounds = ButtonBounds::default();
        element
            .as_widget_mut()
            .operate(&mut tree, Layout::new(&node), &renderer, &mut bounds);
        let target_bounds = expected.map(|name| {
            let item = items
                .iter()
                .find(|item| item.name == name)
                .expect("target must be listed");
            bounds
                .0
                .iter()
                .find(|(id, _)| *id == item.button_id)
                .map(|(_, bounds)| *bounds)
                .expect("target button must be in the widget tree")
        });
        let start = Point::new(viewport.x + 1.0, target_bounds[0].center_y() - SCROLL_Y);
        let end = Point::new(
            target_bounds[1].center_x(),
            target_bounds[1].center_y() - SCROLL_Y + 4.0,
        );
        assert!(
            viewport.contains(start) && viewport.contains(end),
            "drag must stay in the viewport"
        );
        (tree, start, end)
    };

    for (event, position) in [
        (mouse::Event::CursorMoved { position: start }, start),
        (mouse::Event::ButtonPressed(mouse::Button::Left), start),
        (mouse::Event::CursorMoved { position: end }, end),
        (mouse::Event::ButtonReleased(mouse::Button::Left), end),
    ] {
        let messages = {
            let mut element = tab_view(&tab, &key_binds, &modifiers);
            tree.diff(element.as_widget_mut());
            let node = element.as_widget_mut().layout(
                &mut tree,
                &renderer,
                &Limits::new(Size::ZERO, VIEW_SIZE),
            );
            let mut messages = Vec::new();
            element.as_widget_mut().update(
                &mut tree,
                &Event::Mouse(event),
                Layout::new(&node),
                mouse::Cursor::Available(position),
                &renderer,
                &mut clipboard::Null,
                &mut Shell::new(&mut messages),
                &Rectangle::with_size(VIEW_SIZE),
            );
            messages
        };
        for message in messages {
            tab.update(message, modifiers);
        }
    }

    let selected: Vec<_> = tab
        .items_opt()
        .unwrap()
        .iter()
        .filter(|item| item.selected)
        .map(|item| item.name.as_str())
        .collect();
    assert_eq!(
        selected, expected,
        "drag must select exactly the visible targets in {view:?}"
    );
    Ok(())
}

#[test]
fn grid_drag_selection_survives_culled_rows() -> io::Result<()> {
    drag_selects_visible_items(View::Grid, ["file00024.txt", "file00025.txt"])
}

#[test]
fn list_drag_selection_survives_culled_rows() -> io::Result<()> {
    drag_selects_visible_items(View::List, ["file00011.txt", "file00012.txt"])
}
