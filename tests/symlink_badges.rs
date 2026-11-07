#![cfg(unix)]

use std::fs;
use std::io;
use std::os::unix::fs::symlink;

use cosmic::iced::advanced::layout::{Layout, Limits};
use cosmic::iced::advanced::renderer::{Headless, Style};
use cosmic::iced::advanced::widget::Tree;
use cosmic::iced::{Color, Font, Pixels, Rectangle, Size, mouse};
use cosmic::widget;
use cosmic_files::config::{IconSizes, TabConfig, ThumbCfg};
use cosmic_files::item::{Item, item_from_path, item_from_search_item};
use cosmic_files::location::{Location, SearchLocation};
use cosmic_files::scan::{scan_path, scan_search};
use cosmic_files::tab::{Tab, View};
use image::RgbaImage;
use tempfile::TempDir;

const ICON_COLOR: [u8; 4] = [102, 136, 170, 255];

fn with_test_icon(mut item: Item) -> Item {
    let icon = widget::icon::from_svg_bytes(
        br##"<svg xmlns="http://www.w3.org/2000/svg" width="32" height="32"><path fill="#6688aa" d="M0 0h32v32H0z"/></svg>"##.as_slice(),
    );
    item.icon_handle_grid = icon.clone();
    item.icon_handle_list = icon.clone();
    item.icon_handle_list_condensed = icon;
    item
}

fn render_item(mut item: Item, config: TabConfig, width: u32) -> RgbaImage {
    item.display_name = "Projects".to_owned();

    let mut tab = Tab::new(
        Location::Path(std::path::PathBuf::from("/")),
        config,
        ThumbCfg::default(),
        None,
        widget::Id::unique(),
        None,
    );
    let height = u32::from(
        config
            .icon_sizes
            .grid()
            .max(config.icon_sizes.list_condensed()),
    ) + 96;
    let size = Size::new(width as f32, height as f32);
    tab.size_opt.set(Some(size));
    tab.set_items(vec![item]);
    let (_, mut element, _) = match config.view {
        View::Grid => tab.grid_view(),
        View::List => tab.list_view(),
    };
    let mut renderer = cosmic::iced::futures::executor::block_on(
        <cosmic::Renderer as Headless>::new(Font::default(), Pixels(14.0), Some("tiny-skia")),
    )
    .expect("software renderer must be available");
    let mut tree = Tree::new(element.as_widget());
    let layout =
        element
            .as_widget_mut()
            .layout(&mut tree, &renderer, &Limits::new(Size::ZERO, size));
    element.as_widget().draw(
        &tree,
        &mut renderer,
        &cosmic::theme::system_dark(),
        &Style::default(),
        Layout::new(&layout),
        mouse::Cursor::Unavailable,
        &Rectangle::with_size(size),
    );
    RgbaImage::from_raw(
        width,
        height,
        renderer.screenshot(Size::new(width, height), 1.0, Color::BLACK),
    )
    .expect("renderer must return an RGBA image")
}

fn folder_and_symlink() -> io::Result<(TempDir, Item, Item)> {
    let directory = TempDir::new()?;
    fs::create_dir(directory.path().join("folder"))?;
    symlink("folder", directory.path().join("link"))?;
    let items = scan_path(&directory.path().to_owned(), IconSizes::default());
    let folder = items
        .iter()
        .find(|item| item.name == "folder")
        .expect("directory must be listed")
        .clone();
    let link = items
        .into_iter()
        .find(|item| item.name == "link")
        .expect("directory symlink must be listed");
    assert!(link.metadata.is_dir(), "link must retain folder behaviour");
    Ok((directory, folder, link))
}

#[test]
fn symlink_folder_has_a_bottom_left_badge_in_each_view() -> io::Result<()> {
    let (_directory, folder, link) = folder_and_symlink()?;
    for scale in [50, 100, 500] {
        for (view, width) in [(View::Grid, 800), (View::List, 800), (View::List, 480)] {
            let config = TabConfig {
                view,
                icon_sizes: IconSizes {
                    grid: scale.try_into().unwrap(),
                    list: scale.try_into().unwrap(),
                },
                ..TabConfig::default()
            };
            let plain = render_item(with_test_icon(folder.clone()), config, width);
            let badged = render_item(with_test_icon(link.clone()), config, width);
            if let Some(directory) = std::env::var_os("SYMLINK_BADGE_SCREENSHOTS") {
                let directory = std::path::PathBuf::from(directory);
                render_item(folder.clone(), config, width)
                    .save(directory.join(format!("{view:?}-{width}-{scale}-plain.png")))
                    .unwrap();
                render_item(link.clone(), config, width)
                    .save(directory.join(format!("{view:?}-{width}-{scale}-badged.png")))
                    .unwrap();
            }
            let icon_pixels: Vec<_> = plain
                .enumerate_pixels()
                .filter(|(_, _, pixel)| pixel.0 == ICON_COLOR)
                .map(|(x, y, _)| (x, y))
                .collect();
            let left = icon_pixels.iter().map(|(x, _)| *x).min().unwrap();
            let right = icon_pixels.iter().map(|(x, _)| *x).max().unwrap();
            let top = icon_pixels.iter().map(|(_, y)| *y).min().unwrap();
            let bottom = icon_pixels.iter().map(|(_, y)| *y).max().unwrap();
            let changed: Vec<_> = plain
                .enumerate_pixels()
                .filter(|(x, y, pixel)| *pixel != badged.get_pixel(*x, *y))
                .map(|(x, y, _)| (x, y))
                .collect();

            assert!(
                !changed.is_empty(),
                "symlink folder must have a visible badge in {view:?} at width {width}, zoom {scale}%"
            );
            assert!(
                changed.iter().all(|&(x, y)| {
                    x >= left && x <= (left + right) / 2 && y > (top + bottom) / 2 && y <= bottom
                }),
                "badge must only change the icon's bottom-left corner in {view:?} at width {width}, zoom {scale}%; \
             icon bounds: ({left}, {top}) to ({right}, {bottom}); changed bounds: {:?}",
                (
                    changed.iter().map(|(x, _)| x).min(),
                    changed.iter().map(|(_, y)| y).min(),
                    changed.iter().map(|(x, _)| x).max(),
                    changed.iter().map(|(_, y)| y).max(),
                )
            );
        }
    }
    Ok(())
}

#[test]
fn symlink_folder_detection_checks_the_entry_not_its_ancestors() -> io::Result<()> {
    let (directory, _, _) = folder_and_symlink()?;
    fs::create_dir(directory.path().join("folder/child"))?;
    fs::write(directory.path().join("file"), "contents")?;
    symlink("file", directory.path().join("file-link"))?;
    symlink("link", directory.path().join("chain"))?;
    symlink(
        directory.path().join("folder"),
        directory.path().join("absolute"),
    )?;

    for (name, expected) in [
        ("folder", false),
        ("link", true),
        ("chain", true),
        ("absolute", true),
        ("link/child", false),
        ("file-link", false),
    ] {
        let item = item_from_path(directory.path().join(name), IconSizes::default()).unwrap();
        assert_eq!(item.is_symlink_dir, expected, "incorrect badge for {name}");
    }
    Ok(())
}

#[test]
fn symlink_folder_badge_uses_scanned_metadata() -> io::Result<()> {
    let (directory, _, link) = folder_and_symlink()?;
    let config = TabConfig {
        view: View::List,
        ..TabConfig::default()
    };
    let before = render_item(with_test_icon(link.clone()), config, 800);
    fs::remove_file(directory.path().join("link"))?;
    let after = render_item(with_test_icon(link), config, 800);
    assert_eq!(
        before, after,
        "painting must not read symlink state from disk"
    );
    Ok(())
}

#[test]
fn search_preserves_symlink_folder_badges() -> io::Result<()> {
    let (directory, _, _) = folder_and_symlink()?;
    let results = std::sync::Mutex::new(Vec::new());
    scan_search(
        &SearchLocation::Path(directory.path().to_owned()),
        "link",
        false,
        |item| {
            results.lock().unwrap().push(item);
            true
        },
    );
    let mut results = results.into_inner().unwrap();
    assert_eq!(results.len(), 1);
    let item = item_from_search_item(results.remove(0), IconSizes::default());
    assert!(
        item.metadata.is_dir(),
        "search result must retain folder behaviour"
    );
    assert!(
        item.is_symlink_dir,
        "search result must retain its link badge"
    );
    assert_eq!(item.path_opt(), Some(&directory.path().join("link")));
    Ok(())
}

#[cfg(feature = "gvfs")]
#[test]
fn gvfs_symlink_folder_badge_uses_link_metadata() -> Result<(), Box<dyn std::error::Error>> {
    use gio::prelude::FileExt;

    let (directory, _, _) = folder_and_symlink()?;
    let path = directory.path().join("link");
    let info = gio::File::for_path(&path).query_info(
        "standard::*,time::modified",
        gio::FileQueryInfoFlags::NONE,
        gio::Cancellable::NONE,
    )?;
    let item = cosmic_files::item::item_from_gvfs_info(path, info, IconSizes::default());
    assert!(item.metadata.is_dir());
    assert!(
        item.is_symlink_dir,
        "GIO directory links must retain their badge"
    );
    Ok(())
}
