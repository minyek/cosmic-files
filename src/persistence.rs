pub(crate) async fn flush() {
    let recents = crate::recents::barrier();
    let defaults = crate::mime_app::barrier();
    let (recents, defaults) = cosmic::iced::futures::join!(
        tokio::task::spawn_blocking(move || recents.recv()),
        defaults,
    );
    match recents {
        Ok(Ok(Ok(()))) => {}
        result => log::warn!("failed to drain recents updates: {result:?}"),
    }
    match defaults {
        Ok(Ok(())) => {}
        result => log::warn!("failed to drain default handler updates: {result:?}"),
    }
}
