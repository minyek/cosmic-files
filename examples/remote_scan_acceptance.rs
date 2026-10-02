use cosmic_files::config::IconSizes;
use cosmic_files::tab::{Item, Location, scan_network, scan_path};
use std::collections::BTreeMap;

#[derive(Debug, Eq, PartialEq)]
struct EntryShape {
    is_dir: bool,
    size: Option<u64>,
    mime: String,
    location: String,
    path: Option<String>,
}

fn shape(items: Vec<Item>) -> BTreeMap<String, EntryShape> {
    items
        .into_iter()
        .map(|item| {
            let location = item
                .location_opt
                .as_ref()
                .map(ToString::to_string)
                .unwrap_or_default();
            let path = item.path_opt().map(|path| path.display().to_string());
            (
                item.name,
                EntryShape {
                    is_dir: item.metadata.is_dir(),
                    size: item.metadata.file_size(),
                    mime: item.mime.to_string(),
                    location,
                    path,
                },
            )
        })
        .collect()
}

fn expected(label: &str) -> BTreeMap<String, (bool, String)> {
    let entries: &[(&str, bool, &str)] = match label {
        "initial" | "after-remove" => &[
            ("empty-folder", true, "inode/directory"),
            ("nested", true, "inode/directory"),
            ("root-file.txt", false, "text/plain"),
        ],
        "nested" => &[("nested-file.txt", false, "text/plain")],
        "after-create" => &[
            ("created-after-mount.txt", false, "text/plain"),
            ("created-folder", true, "inode/directory"),
            ("empty-folder", true, "inode/directory"),
            ("nested", true, "inode/directory"),
            ("root-file.txt", false, "text/plain"),
        ],
        other => panic!("unknown probe label: {other}"),
    };
    entries
        .iter()
        .map(|(name, is_dir, mime)| ((*name).to_owned(), (*is_dir, (*mime).to_owned())))
        .collect()
}

fn main() {
    let mut args = std::env::args().skip(1);
    let label = args.next().expect("usage: remote-scan-probe LABEL URI");
    let uri = args.next().expect("usage: remote-scan-probe LABEL URI");
    assert!(args.next().is_none(), "usage: remote-scan-probe LABEL URI");

    let direct = shape(scan_network(&uri, IconSizes::default()));
    let (_, location_items) =
        Location::Network(uri.clone(), label.clone(), None).scan(IconSizes::default());
    let through_location = shape(location_items);
    assert_eq!(
        direct, through_location,
        "scan_network and Location::scan differ"
    );

    let kinds_and_mimes = direct
        .iter()
        .map(|(name, entry)| (name.clone(), (entry.is_dir, entry.mime.clone())))
        .collect::<BTreeMap<_, _>>();
    assert_eq!(
        kinds_and_mimes,
        expected(&label),
        "unexpected remote entries"
    );

    if let Some(path) = direct.values().find_map(|entry| entry.path.as_ref()) {
        let root = std::path::Path::new(path).parent().unwrap().to_path_buf();
        let mapped = shape(scan_path(&root, IconSizes::default()));
        let mapped_kinds = mapped
            .iter()
            .map(|(name, entry)| (name.clone(), entry.is_dir))
            .collect::<BTreeMap<_, _>>();
        let kinds = direct
            .iter()
            .map(|(name, entry)| (name.clone(), entry.is_dir))
            .collect::<BTreeMap<_, _>>();
        assert_eq!(
            mapped_kinds, kinds,
            "GVFS FUSE path scan differs from URI scan"
        );
        let mapped_files = mapped
            .into_iter()
            .filter(|(_, entry)| !entry.is_dir)
            .map(|(name, entry)| (name, (entry.size, entry.mime)))
            .collect::<BTreeMap<_, _>>();
        let direct_files = direct
            .iter()
            .filter(|(_, entry)| !entry.is_dir)
            .map(|(name, entry)| (name.clone(), (entry.size, entry.mime.clone())))
            .collect::<BTreeMap<_, _>>();
        assert_eq!(
            mapped_files, direct_files,
            "GVFS FUSE file metadata differs from URI scan"
        );
        println!("{label}: FUSE path scan PASS");
    } else {
        assert!(
            std::env::var_os("COSMIC_REQUIRE_FUSE").is_none(),
            "required GVFS FUSE path unavailable"
        );
    }

    println!("{label}: PASS");
    for (name, entry) in direct {
        println!(
            "{name}\ttype={}\tsize={:?}\tmime={}\tlocation={}\tpath={:?}",
            if entry.is_dir { "directory" } else { "file" },
            entry.size,
            entry.mime,
            entry.location,
            entry.path,
        );
    }
}
