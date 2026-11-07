use std::io;
use std::path::Path;

#[cfg(feature = "desktop")]
pub(crate) fn resolve_destination(path: &Path) -> io::Result<std::path::PathBuf> {
    let mut destination = path.to_path_buf();
    loop {
        match std::fs::canonicalize(&destination) {
            Ok(path) => return Ok(path),
            Err(err) if err.kind() == io::ErrorKind::NotFound => {}
            Err(err) => return Err(err),
        }
        match std::fs::read_link(&destination) {
            Ok(target) if target.is_absolute() => destination = target,
            Ok(target) => {
                destination = destination
                    .parent()
                    .unwrap_or_else(|| Path::new("."))
                    .join(target);
            }
            Err(err)
                if matches!(
                    err.kind(),
                    io::ErrorKind::NotFound | io::ErrorKind::InvalidInput
                ) =>
            {
                return Ok(destination);
            }
            Err(err) => return Err(err),
        }
    }
}

pub(crate) fn stage(destination: &Path) -> io::Result<tempfile::NamedTempFile> {
    let parent = destination
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let mut builder = tempfile::Builder::new();
    builder.prefix(".cosmic-files-");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        builder.permissions(std::fs::Permissions::from_mode(0o666));
    }
    builder.tempfile_in(parent)
}
