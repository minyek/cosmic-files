use cosmic_files::{config::IconSizes, tab::Location};
use std::{error::Error, fs, path::PathBuf};

#[path = "../src/recents.rs"]
mod recents;

fn main() -> Result<(), Box<dyn Error>> {
    tracing_subscriber::fmt().with_env_filter("warn").init();
    let profile = PathBuf::from(std::env::var("COSMIC_ACCEPTANCE_PROFILE")?);
    let history = recently_used_xbel::dir().ok_or("no recents path")?;
    assert!(
        history.starts_with(&profile),
        "recents must use the isolated profile"
    );
    let data = PathBuf::from(std::env::var("XDG_DATA_HOME")?);
    assert!(
        data.starts_with(&profile),
        "trash must use the isolated profile"
    );
    fs::create_dir_all(history.parent().unwrap())?;
    if std::env::args().any(|arg| arg == "--initialize-only") {
        recents::clear_recently_used();
        recents::barrier().recv()??;
        assert!(recently_used_xbel::parse_file()?.bookmarks.is_empty());
        println!("PASS fresh recents initialization");
        return Ok(());
    }
    let fixture = profile.join("storage-fixtures");
    fs::create_dir_all(&fixture)?;
    let old = fixture.join("old.txt");
    let kept = fixture.join("kept.txt");
    let removed = fixture.join("removed.txt");
    for path in [&old, &kept, &removed] {
        fs::write(path, path.file_name().unwrap().as_encoded_bytes())?;
    }
    recents::record_recently_used(vec![old.clone()], "com.system76.CosmicFiles");
    recents::clear_recently_used();
    recents::record_recently_used(
        vec![kept.clone(), removed.clone()],
        "com.system76.CosmicFiles",
    );
    recents::remove_recently_used(&[removed.as_path()])?;
    let (_, recent_items) = Location::Recents.scan(IconSizes::default());
    assert_eq!(recent_items.len(), 1);
    assert_eq!(recent_items[0].path_opt(), Some(&kept));
    assert_eq!(recently_used_xbel::parse_file()?.bookmarks.len(), 1);
    recents::clear_recently_used();
    recents::barrier().recv()??;
    assert!(recently_used_xbel::parse_file()?.bookmarks.is_empty());
    println!("PASS real XBEL record/clear/record/remove ordering and application recents scan");

    let payload = fixture.join("trash-roundtrip.txt");
    fs::write(&payload, b"preserve these bytes through trash and restore")?;
    trash::delete(&payload)?;
    assert!(!payload.exists());
    let entries = trash::os_limited::list()?;
    let entry = entries
        .into_iter()
        .find(|entry| entry.original_path() == payload)
        .ok_or("trashed fixture missing")?;
    assert!(data.join("Trash/files").join(&entry.name).exists());
    let (_, trash_items) = Location::Trash.scan(IconSizes::default());
    assert!(
        trash_items
            .iter()
            .any(|item| item.name == "trash-roundtrip.txt")
    );
    trash::os_limited::restore_all([entry])?;
    assert_eq!(
        fs::read(&payload)?,
        b"preserve these bytes through trash and restore"
    );
    assert!(
        !trash::os_limited::list()?
            .iter()
            .any(|entry| entry.original_path() == payload)
    );
    println!("PASS real trash deletion, application listing, and byte-preserving restore");
    Ok(())
}
