// SPDX-License-Identifier: GPL-3.0-only

use cosmic::widget::icon;
use mime_guess::Mime;
use rustc_hash::FxHashMap;
use std::fs;
use std::path::Path;
use std::sync::{LazyLock, Mutex};

pub const FALLBACK_MIME_ICON: &str = "text-x-generic";

#[derive(Debug, Eq, Hash, PartialEq)]
struct MimeIconKey {
    mime: Mime,
    size: u16,
}

#[derive(Default)]
pub struct MimeIconCache {
    cache: FxHashMap<MimeIconKey, Option<icon::Handle>>,
}

#[cfg(unix)]
pub(crate) static SHARED_MIME_INFO: LazyLock<xdg_mime::SharedMimeInfo> =
    LazyLock::new(xdg_mime::SharedMimeInfo::new);

impl MimeIconCache {
    #[cfg(not(unix))]
    pub fn get(&mut self, _key: MimeIconKey) -> Option<icon::Handle> {
        None
    }

    #[cfg(unix)]
    fn get(&mut self, key: MimeIconKey) -> Option<icon::Handle> {
        self.cache
            .entry(key)
            .or_insert_with_key(|key| {
                let mut icon_names = SHARED_MIME_INFO.lookup_icon_names(&key.mime);
                if icon_names.is_empty() {
                    return None;
                }
                let icon_name = icon_names.remove(0);
                let mut named = icon::from_name(icon_name).prefer_svg(true).size(key.size);
                if !icon_names.is_empty() {
                    let fallback_names =
                        icon_names.into_iter().map(std::borrow::Cow::from).collect();
                    named = named.fallback(Some(icon::IconFallback::Names(fallback_names)));
                }
                Some(named.handle())
            })
            .clone()
    }
}

pub static MIME_ICON_CACHE: LazyLock<Mutex<MimeIconCache>> =
    LazyLock::new(|| Mutex::new(MimeIconCache::default()));

#[cfg(not(unix))]
pub fn mime_for_path(
    path: impl AsRef<Path>,
    metadata_opt: Option<&fs::Metadata>,
    remote: bool,
) -> Mime {
    mime_guess::from_path(path).first_or_octet_stream()
}

/// A `Mime` describing the entry itself — a directory, a symlink, or an empty file — rather
/// than the format of any contents.
#[cfg(unix)]
fn is_special_mime(mime: &Mime) -> bool {
    *mime == "inode/directory" || *mime == "inode/symlink" || *mime == "application/x-zerosize"
}

/// A `Mime` standing in for an answer `xdg-mime` could not give, rather than one describing
/// the file. A name it holds no glob for is guessed against empty contents, which matches
/// `application/x-zerosize` whatever the file actually holds.
#[cfg(unix)]
fn is_placeholder_mime(mime: &Mime) -> bool {
    is_special_mime(mime) || *mime == "application/octet-stream"
}

/// Guesses the `Mime` from the file name alone, if exactly one glob claims that name.
///
/// `xdg-mime` answers an unambiguous name before it consults the file's contents, so this is
/// the type a content read would arrive at as well.
#[cfg(unix)]
fn mime_from_file_name(shared_mime_info: &xdg_mime::SharedMimeInfo, path: &Path) -> Option<Mime> {
    let file_name = path.file_name().and_then(std::ffi::OsStr::to_str)?;
    let mut guess_builder = shared_mime_info.guess_mime_type();
    guess_builder.zero_size(false);
    guess_builder.file_name(file_name);
    let guess = guess_builder.guess();
    let mime = guess.mime_type();
    (!guess.uncertain() && !is_placeholder_mime(mime)).then(|| mime.clone())
}

#[cfg(unix)]
pub fn mime_for_path(
    path: impl AsRef<Path>,
    metadata_opt: Option<&fs::Metadata>,
    remote: bool,
) -> Mime {
    let path = path.as_ref();

    // Metadata distinguishes directories and symlinks. Empty files use the full guessing
    // rules; remote callers without metadata have already ruled these cases out.
    let name_may_decide_type =
        metadata_opt.map_or(remote, |metadata| metadata.is_file() && metadata.len() > 0);
    if name_may_decide_type {
        if let Some(mime) = mime_from_file_name(&SHARED_MIME_INFO, path) {
            return mime;
        }
        if remote {
            // Narrowing this further means reading the contents, which costs a fetch per
            // entry over a remote listing.
            return mime_guess::from_path(path).first_or_octet_stream();
        }
    }

    // Try the shared mime info cache first
    let mut guess_builder = SHARED_MIME_INFO.guess_mime_type();
    guess_builder.zero_size(false);
    if remote {
        if let Some(file_name) = path.file_name().and_then(std::ffi::OsStr::to_str) {
            guess_builder.file_name(file_name);
        }
    } else {
        guess_builder.path(path);
    }
    if let Some(metadata) = metadata_opt {
        guess_builder.metadata(metadata.clone());
    }
    let guess = guess_builder.guess();
    let guessed_mime = guess.mime_type();

    // `xdg-mime-rs` sets the guess to uncertain if it returns special mime types.
    // The guess could also be uncertain on platforms without shared-mime-info.
    // Try mime_guess, but only if it is not one of the special mime types.
    if guess.uncertain() && (remote || !is_special_mime(guessed_mime)) {
        // If uncertain, try mime_guess. This could happen on platforms without shared-mime-info
        mime_guess::from_path(path).first_or_octet_stream()
    } else {
        guessed_mime.clone()
    }
}

pub fn mime_icon(mime: Mime, size: u16) -> icon::Handle {
    let mut mime_icon_cache = MIME_ICON_CACHE.lock().unwrap();
    match mime_icon_cache.get(MimeIconKey { mime, size }) {
        Some(handle) => handle,
        None => icon::from_name(FALLBACK_MIME_ICON)
            .prefer_svg(true)
            .size(size)
            .handle(),
    }
}

#[cfg(not(unix))]
pub fn parent_mime_types(_mime: &Mime) -> Option<Vec<Mime>> {
    None
}

#[cfg(unix)]
pub fn parent_mime_types(mime: &Mime) -> Option<Vec<Mime>> {
    SHARED_MIME_INFO.get_parents_aliased(mime)
}

#[cfg(unix)]
pub fn is_mime_subclass_of(mime_type: &Mime, base: &Mime) -> bool {
    SHARED_MIME_INFO.mime_type_subclass(mime_type, base)
}

#[cfg(not(unix))]
pub fn is_mime_subclass_of(mime_type: &Mime, base: &Mime) -> bool {
    mime_type == base
}

#[cfg(all(test, unix))]
mod tests {
    use std::{fs, io};

    use tempfile::TempDir;

    use super::mime_for_path;

    #[test]
    fn mime_queries_can_run_while_the_icon_cache_is_in_use() {
        let _icons = super::MIME_ICON_CACHE.lock().unwrap();
        let mime = mime_for_path("notes.txt", None, true);
        assert_eq!(mime, "text/plain");
        assert!(super::is_mime_subclass_of(&mime, &mime));
        let _ = super::parent_mime_types(&mime);
    }

    #[test]
    fn remote_files_without_metadata_are_not_typed_as_empty() {
        assert_eq!(mime_for_path("notes.txt", None, true), "text/plain");
        assert_eq!(
            mime_for_path("unrecognized-payload", None, true),
            "application/octet-stream"
        );
    }

    #[test]
    fn an_empty_local_file_with_a_known_extension_keeps_its_named_type() -> io::Result<()> {
        let dir = TempDir::new()?;
        let path = dir.path().join("notes.txt");
        fs::write(&path, b"")?;
        assert_eq!(
            mime_for_path(&path, Some(&fs::metadata(&path)?), false),
            "text/plain"
        );
        Ok(())
    }

    #[test]
    fn symlink_metadata_takes_precedence_over_a_file_extension() -> io::Result<()> {
        let dir = TempDir::new()?;
        let target = dir.path().join("target");
        let link = dir.path().join("notes.txt");
        fs::write(&target, b"contents")?;
        std::os::unix::fs::symlink(target, &link)?;
        assert_eq!(
            mime_for_path(&link, Some(&fs::symlink_metadata(&link)?), false),
            "inode/symlink"
        );
        Ok(())
    }

    #[test]
    fn remote_file_without_a_known_extension_is_not_typed_as_empty() -> io::Result<()> {
        let dir = TempDir::new()?;
        let path = dir.path().join("payload");
        fs::write(&path, b"\x7fELF\x02\x01\x01")?;
        let metadata = fs::metadata(&path)?;

        assert_eq!(
            mime_for_path(&path, Some(&metadata), true),
            "application/octet-stream"
        );
        Ok(())
    }

    #[test]
    fn remote_file_with_a_known_extension_is_typed_from_its_name() -> io::Result<()> {
        let dir = TempDir::new()?;
        let path = dir.path().join("notes.txt");
        fs::write(&path, b"hello")?;
        let metadata = fs::metadata(&path)?;

        assert_eq!(mime_for_path(&path, Some(&metadata), true), "text/plain");
        Ok(())
    }

    #[test]
    fn empty_remote_file_is_typed_as_empty() -> io::Result<()> {
        let dir = TempDir::new()?;
        let path = dir.path().join("placeholder");
        fs::write(&path, b"")?;
        let metadata = fs::metadata(&path)?;

        assert_eq!(
            mime_for_path(&path, Some(&metadata), true),
            "application/x-zerosize"
        );
        Ok(())
    }

    #[test]
    fn an_unambiguous_name_outranks_the_file_contents() -> io::Result<()> {
        let dir = TempDir::new()?;
        let path = dir.path().join("notes.txt");
        // PNG magic: a content-derived answer would be image/png.
        fs::write(&path, b"\x89PNG\r\n\x1a\n")?;
        let metadata = fs::metadata(&path)?;

        assert_eq!(mime_for_path(&path, Some(&metadata), false), "text/plain");
        Ok(())
    }

    #[test]
    fn a_local_name_that_decides_nothing_still_reads_the_contents() -> io::Result<()> {
        let dir = TempDir::new()?;
        let path = dir.path().join("payload");
        fs::write(&path, b"\x89PNG\r\n\x1a\n")?;
        let metadata = fs::metadata(&path)?;

        // Skipping the read is only sound where the name settles the type. Locally, a name no
        // glob claims must still fall through to the contents.
        assert_eq!(mime_for_path(&path, Some(&metadata), false), "image/png");
        Ok(())
    }

    #[test]
    fn a_directory_named_like_a_file_is_typed_as_a_directory() -> io::Result<()> {
        let dir = TempDir::new()?;
        let path = dir.path().join("archive.txt");
        fs::create_dir(&path)?;
        let metadata = fs::metadata(&path)?;

        assert_eq!(
            mime_for_path(&path, Some(&metadata), false),
            "inode/directory"
        );
        Ok(())
    }
}
