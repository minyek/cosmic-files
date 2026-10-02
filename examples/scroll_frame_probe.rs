#[cfg(feature = "wgpu")]
fn main() {
    eprintln!("run this probe with --no-default-features");
    std::process::exit(2);
}

#[cfg(not(feature = "wgpu"))]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    use cosmic::iced::advanced::widget::operation::scrollable::AbsoluteOffset;
    use cosmic::iced::advanced::{
        Layout, Renderer as _, Shell, clipboard, layout, renderer, widget::Tree,
    };
    use cosmic::iced::{Event, Font, Pixels, Point, Rectangle, Size, keyboard::Modifiers, mouse};
    use cosmic_files::{
        config::IconSizes,
        tab::{Location, Tab, View, scan_path},
    };
    use std::{collections::HashMap, path::PathBuf, time::Instant};

    let path = PathBuf::from(
        std::env::args()
            .nth(1)
            .ok_or("usage: scroll_frame_probe DIRECTORY")?,
    );
    let scanned = Instant::now();
    let items = scan_path(&path, IconSizes::default());
    println!(
        "scan entries={} elapsed_ms={:.3}",
        items.len(),
        scanned.elapsed().as_secs_f64() * 1000.0
    );
    let id = cosmic::widget::Id::unique();
    let mut tab = Tab::new(
        Location::Path(path),
        Default::default(),
        Default::default(),
        None,
        id.clone(),
        None,
    );
    tab.set_items(items);
    let mut renderer = cosmic::Renderer::new(Font::default(), Pixels(14.0));
    let theme = cosmic::theme::active();
    let keys = HashMap::new();
    let modifiers = Modifiers::empty();
    let size = Size::new(1000.0, 700.0);
    let viewport = Rectangle::new(Point::ORIGIN, size);
    let cursor = mouse::Cursor::Available(Point::new(500.0, 350.0));
    for view in [View::List, View::Grid] {
        tab.config.view = view;
        let mut tree = Tree::empty();
        let mut samples = Vec::new();
        let mut greatest_offset = 0.0_f32;
        for frame in 0..660 {
            let max_offset = (tab.content_height_opt.get().unwrap_or(700.0) - 700.0).max(0.0);
            let offset = max_offset * ((frame % 220) as f32 / 219.0);
            greatest_offset = greatest_offset.max(offset);
            tab.scroll_opt = Some(AbsoluteOffset { x: 0.0, y: offset });
            let start = Instant::now();
            renderer.reset(viewport);
            let mut element = tab.view(&keys, &modifiers, false, &[]);
            tree.diff(element.as_widget_mut());
            let node = element.as_widget_mut().layout(
                &mut tree,
                &renderer,
                &layout::Limits::new(size, size),
            );
            let mut scroll = cosmic::iced::advanced::widget::operation::scrollable::scroll_to(
                id.clone(),
                AbsoluteOffset {
                    x: None,
                    y: Some(offset),
                },
            );
            element
                .as_widget_mut()
                .operate(&mut tree, Layout::new(&node), &renderer, &mut scroll);
            let mut messages = Vec::new();
            let mut shell = Shell::new(&mut messages);
            for event in [
                Event::Mouse(mouse::Event::CursorMoved {
                    position: Point::new(500.0, 350.0),
                }),
                Event::Mouse(mouse::Event::WheelScrolled {
                    delta: mouse::ScrollDelta::Pixels { x: 0.0, y: -20.0 },
                }),
            ] {
                element.as_widget_mut().update(
                    &mut tree,
                    &event,
                    Layout::new(&node),
                    cursor,
                    &renderer,
                    &mut clipboard::Null,
                    &mut shell,
                    &viewport,
                );
            }
            element.as_widget().draw(
                &tree,
                &mut renderer,
                &theme,
                &renderer::Style::default(),
                Layout::new(&node),
                cursor,
                &viewport,
            );
            assert!(!std::hint::black_box(renderer.layers()).is_empty());
            drop(element);
            let elapsed = start.elapsed().as_secs_f64() * 1_000_000.0;
            if frame >= 60 {
                samples.push(elapsed);
            }
            for message in messages {
                let _ = tab.update(message, modifiers);
            }
        }
        samples.sort_by(f64::total_cmp);
        let content_height = tab
            .content_height_opt
            .get()
            .expect("listing layout must set content height");
        assert!(
            content_height > size.height,
            "fixture must require scrolling"
        );
        assert!(
            greatest_offset >= (content_height - size.height) * 0.99,
            "probe must reach the end of the listing"
        );
        println!("content_height={content_height:.1} greatest_offset={greatest_offset:.1}");
        println!(
            "view={view:?} frames={} median_us={:.3} p95_us={:.3} p99_us={:.3} max_us={:.3}",
            samples.len(),
            samples[samples.len() / 2],
            samples[samples.len() * 95 / 100],
            samples[samples.len() * 99 / 100],
            samples[samples.len() - 1]
        );
    }
    Ok(())
}
