use std::io;
use std::path::Path;

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
