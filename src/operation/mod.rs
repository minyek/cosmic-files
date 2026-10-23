use crate::app::{ArchiveType, DialogPage, Message, REPLACE_BUTTON_ID};
use crate::config::IconSizes;
use crate::spawn_detached::spawn_detached;
use crate::{archive, fl, tab};
use cosmic::iced::futures::channel::mpsc::Sender;
use cosmic::iced::futures::{self, SinkExt, StreamExt, stream};
use std::borrow::Cow;
use std::fmt::Formatter;
use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::{Mutex as TokioMutex, mpsc};
use walkdir::WalkDir;
use zip::AesMode::Aes256;

pub use self::controller::{Controller, ControllerState};
pub mod controller;

#[cfg(all(test, unix))]
mod test_faults;

pub use notifiers::*;
mod notifiers;

pub use self::reader::OpReader;
pub mod reader;

use self::recursive::{Context, Method};
pub mod recursive;

/// Builds the item shown for one side of a replace prompt, on compio's blocking pool because
/// `item_from_path` stats the path and would otherwise stall the executor on a slow mount.
async fn build_replace_item(path: PathBuf) -> Option<Box<tab::Item>> {
    match compio::runtime::spawn_blocking(move || tab::item_from_path(path, IconSizes::default()))
        .await
    {
        Ok(Ok(item)) => Some(Box::new(item)),
        Ok(Err(err)) => {
            log::warn!("{err}");
            None
        }
        Err(_) => {
            log::error!("compio runtime spawn failed while building replace item");
            None
        }
    }
}

fn already_exists_error(to: &Path) -> io::Error {
    let message = if to.is_dir() {
        fl!("folder-already-exists")
    } else {
        fl!("file-already-exists")
    };
    io::Error::new(io::ErrorKind::AlreadyExists, message)
}

async fn rename_noreplace(from: PathBuf, to: PathBuf) -> io::Result<()> {
    if from == to {
        // Confirming the dialog with the name unchanged renames a path onto itself.
        return Ok(());
    }

    #[cfg(target_os = "linux")]
    {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt;

        let from_c = CString::new(from.as_os_str().as_bytes())?;
        let to_c = CString::new(to.as_os_str().as_bytes())?;
        let result = compio::runtime::spawn_blocking(move || {
            let rc = unsafe {
                libc::renameat2(
                    libc::AT_FDCWD,
                    from_c.as_ptr(),
                    libc::AT_FDCWD,
                    to_c.as_ptr(),
                    libc::RENAME_NOREPLACE,
                )
            };
            if rc == 0 {
                Ok(())
            } else {
                Err(io::Error::last_os_error())
            }
        })
        .await
        .map_err(|_| io::Error::other("compio runtime spawn failed while renaming"))?;

        result.map_err(|err| match err.raw_os_error() {
            Some(libc::EEXIST | libc::ENOTEMPTY) => already_exists_error(&to),
            _ => err,
        })
    }
    #[cfg(not(target_os = "linux"))]
    {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "renaming without replacing an existing file is not supported on this platform",
        ))
    }
}

async fn handle_replace(
    msg_tx: Arc<TokioMutex<Sender<Message>>>,
    file_from: PathBuf,
    file_to: PathBuf,
    multiple: bool,
    conflict_count: usize,
) -> ReplaceResult {
    let Some(item_from) = build_replace_item(file_from).await else {
        return ReplaceResult::Cancel;
    };
    let Some(item_to) = build_replace_item(file_to).await else {
        return ReplaceResult::Cancel;
    };

    let (tx, mut rx) = mpsc::channel(1);
    let _ = msg_tx
        .lock()
        .await
        .send(Message::DialogPush(
            DialogPage::Replace {
                from: item_from,
                to: item_to,
                multiple,
                apply_to_all: false,
                conflict_count,
                tx,
            },
            Some(REPLACE_BUTTON_ID.clone()),
        ))
        .await;
    rx.recv().await.unwrap_or(ReplaceResult::Cancel)
}

fn get_directory_name(file_name: &str) -> &str {
    // TODO: Chain with COMPOUND_EXTENSIONS once more formats are supported
    for ext in crate::archive::SUPPORTED_EXTENSIONS {
        if let Some(stripped) = file_name.strip_suffix(ext) {
            return stripped;
        }
    }
    file_name
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ReplaceResult {
    Replace(bool),
    KeepBoth,
    Skip(bool),
    Cancel,
}

async fn copy_or_move(
    paths: Vec<PathBuf>,
    to: PathBuf,
    method: Method,
    msg_tx: &Arc<TokioMutex<Sender<Message>>>,
    controller: Controller,
) -> Result<OperationSelection, OperationError> {
    let msg_tx = msg_tx.clone();
    let controller_c = controller.clone();

    compio::runtime::spawn(async move {
        let controller = controller_c;
        log::info!(
            "{} {:?} to {}",
            match method {
                Method::Copy => "Copy",
                Method::Move { .. } => "Move",
            },
            paths,
            to.display()
        );

        // Handle duplicate file names by renaming paths
        let from_to_pairs_iter = paths
            .into_iter()
            .zip(std::iter::repeat(to.as_path()))
            .filter_map(|(from, to)| {
                if matches!(from.parent(), Some(parent) if parent == to)
                    && matches!(method, Method::Copy)
                {
                    // `from`'s parent is equal to `to` which means we're copying to the same
                    // directory (duplicating files)
                    let to = copy_unique_path(&from, to);
                    Some((from, to))
                } else if let Some(name) = from.file_name() {
                    let to = to.join(name);
                    Some((from, to))
                } else {
                    //TODO: how to handle from missing file name?
                    None
                }
            });

        // Attempt quick and simple renames
        //TODO: allow rename to be used for directories in recursive context?

        let from_to_pairs: Vec<(PathBuf, PathBuf)> = if matches!(method, Method::Move { .. }) {
            from_to_pairs_iter
                .map(|(from, to)| async move {
                    //TODO: show replace dialog here?
                    if to.exists() {
                        return Some((from, to));
                    }

                    match compio::fs::rename(&from, &to).await {
                        Ok(()) => {
                            log::info!("renamed {} to {}", from.display(), to.display());
                            None
                        }
                        Err(err) => {
                            log::info!(
                                "failed to rename {} to {}, fallback to recursive move: {}",
                                from.display(),
                                to.display(),
                                err
                            );
                            Some((from, to))
                        }
                    }
                })
                .collect::<cosmic::iced::futures::stream::FuturesOrdered<_>>()
                .fold(Vec::new(), |mut pairs, pair| async move {
                    if let Some(pair) = pair {
                        pairs.push(pair);
                    }
                    pairs
                })
                .await
        } else {
            from_to_pairs_iter.collect()
        };

        let mut context = Context::new(controller.clone());

        {
            let controller = controller.clone();
            context = context.on_progress(move |_op, progress| {
                let item_progress = match progress.total_bytes {
                    Some(total_bytes) => {
                        if total_bytes == 0 {
                            1.0
                        } else {
                            progress.current_bytes as f32 / total_bytes as f32
                        }
                    }
                    None => 0.0,
                };
                let total_progress =
                    (item_progress + progress.current_ops as f32) / progress.total_ops as f32;
                controller.set_progress(total_progress);
            });
        }

        {
            let msg_tx = msg_tx.clone();
            context = context.on_replace(move |op, conflict_count| {
                let msg_tx = msg_tx.clone();
                Box::pin(handle_replace(
                    msg_tx,
                    op.from.clone(),
                    op.to.clone(),
                    true,
                    conflict_count,
                ))
            });
        }

        context
            .recursive_copy_or_move(from_to_pairs, method)
            .await?;

        Result::<OperationSelection, OperationError>::Ok(context.op_sel)
    })
    .await
    .map_err(wrap_compio_spawn_error)?
}

pub async fn sync_to_disk(
    written_files: Vec<PathBuf>,
    target_dirs: std::collections::HashSet<PathBuf>,
) {
    // Sync files to disk
    stream::iter(written_files.into_iter().map(|path| async move {
        if let Ok(file) = compio::fs::OpenOptions::new().write(true).open(&path).await {
            let _ = file.sync_all().await;
        }
    }))
    .buffer_unordered(32)
    .collect::<()>()
    .await;

    // Sync directories to disk
    stream::iter(target_dirs.into_iter().map(|path| async move {
        if let Ok(dir) = compio::fs::OpenOptions::new().read(true).open(&path).await {
            let _ = dir.sync_all().await;
        }
    }))
    .buffer_unordered(16)
    .collect::<()>()
    .await;
}

pub fn copy_unique_path(from: &Path, to: &Path) -> PathBuf {
    // List of compound extensions to check
    const COMPOUND_EXTENSIONS: &[&str] = &[
        ".tar.gz",
        ".tar.bz2",
        ".tar.xz",
        ".tar.zst",
        ".tar.lz",
        ".tar.lzma",
        ".tar.sz",
        ".tar.lzo",
        ".tar.br",
        ".tar.Z",
        ".tar.pz",
    ];

    let mut to = to.to_owned();
    if let Some(file_name) = from.file_name().and_then(|name| name.to_str()) {
        let (stem, ext) = if from.is_dir() {
            (file_name.to_string(), None)
        } else {
            let file_name = file_name.to_string();
            COMPOUND_EXTENSIONS
                .iter()
                .copied()
                .find(|&ext| file_name.ends_with(ext))
                .map(|ext| {
                    (
                        file_name.strip_suffix(ext).unwrap().to_string(),
                        Some(ext[1..].to_string()),
                    )
                })
                .unwrap_or_else(|| {
                    from.file_stem()
                        .and_then(|s| s.to_str())
                        .map_or((file_name, None), |stem| {
                            (
                                stem.to_string(),
                                from.extension()
                                    .and_then(|e| e.to_str())
                                    .map(str::to_string),
                            )
                        })
                })
        };

        for n in 0.. {
            let new_name = if n == 0 {
                file_name.to_string()
            } else {
                match ext {
                    Some(ref ext) => format!("{} ({} {}).{}", stem, fl!("copy_noun"), n, ext),
                    None => format!("{} ({} {})", stem, fl!("copy_noun"), n),
                }
            };

            to.push(&new_name);

            if !matches!(to.try_exists(), Ok(true)) {
                break;
            }
            // Continue if a copy with index exists
            to.pop();
        }
    }
    to
}

fn file_name(path: &Path) -> Cow<'_, str> {
    path.file_name()
        .map_or_else(|| fl!("unknown-folder").into(), |x| x.to_string_lossy())
}

fn parent_name(path: &Path) -> Cow<'_, str> {
    let Some(parent) = path.parent() else {
        return fl!("unknown-folder").into();
    };

    file_name(parent)
}

fn paths_parent_name(paths: &[PathBuf]) -> Cow<'_, str> {
    let Some(first_path) = paths.first() else {
        return fl!("unknown-folder").into();
    };

    let Some(parent) = first_path.parent() else {
        return fl!("unknown-folder").into();
    };

    for path in paths {
        //TODO: is it possible to have different parents, and what should be returned?
        if path.parent() != Some(parent) {
            return fl!("unknown-folder").into();
        }
    }

    file_name(parent)
}

#[derive(Clone, Debug, Default)]
pub struct OperationSelection {
    // Paths to ignore if they are already selected
    pub ignored: Vec<PathBuf>,
    // Paths to select
    pub selected: Vec<PathBuf>,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum Operation {
    /// Compress files
    Compress {
        paths: Vec<PathBuf>,
        to: PathBuf,
        archive_type: ArchiveType,
        password: Option<String>,
    },
    /// Copy items
    Copy {
        paths: Vec<PathBuf>,
        to: PathBuf,
    },
    /// Move items to the trash
    Delete {
        paths: Vec<PathBuf>,
    },
    /// Delete a path from the trash
    DeleteTrash {
        items: Vec<trash::TrashItem>,
    },
    /// Empty the trash
    EmptyTrash,
    /// Uncompress files
    Extract {
        paths: Box<[PathBuf]>,
        to: PathBuf,
        password: Option<String>,
    },
    /// Move items
    Move {
        paths: Vec<PathBuf>,
        to: PathBuf,
        cross_device_copy: bool,
    },
    NewFile {
        path: PathBuf,
    },
    NewFolder {
        path: PathBuf,
    },
    /// Permanently delete items, skipping the trash
    PermanentlyDelete {
        paths: Box<[PathBuf]>,
    },
    RemoveFromRecents {
        paths: Box<[PathBuf]>,
    },
    Rename {
        from: PathBuf,
        to: PathBuf,
    },
    /// Restore a path from the trash
    Restore {
        items: Vec<trash::TrashItem>,
    },
    /// Set executable and launch
    SetExecutableAndLaunch {
        path: PathBuf,
    },
    /// Set permissions
    SetPermissions {
        path: PathBuf,
        mode: u32,
    },
}

#[derive(Clone, Debug)]
pub enum OperationErrorType {
    Generic(String),
    PasswordRequired,
}
#[derive(Clone, Debug)]
pub struct OperationError {
    pub kind: OperationErrorType,
}

impl OperationError {
    pub fn from_state(state: ControllerState, controller: &Controller) -> Self {
        let message = if state == ControllerState::Failed {
            controller.set_state(ControllerState::Failed);
            fl!("failed")
        } else {
            controller.cancel();
            fl!("cancelled")
        };

        Self {
            kind: OperationErrorType::Generic(message),
        }
    }

    pub fn from_err<T: ToString>(err: T, controller: &Controller) -> Self {
        controller.set_state(ControllerState::Failed);

        Self {
            kind: OperationErrorType::Generic(err.to_string()),
        }
    }

    pub fn from_kind(kind: OperationErrorType, controller: &Controller) -> Self {
        controller.set_state(ControllerState::Failed);
        Self { kind }
    }

    pub fn from_msg(m: impl Into<String>) -> Self {
        Self {
            kind: OperationErrorType::Generic(m.into()),
        }
    }
}

impl std::error::Error for OperationError {}

impl std::fmt::Display for OperationError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match &self.kind {
            OperationErrorType::Generic(s) => s.fmt(f),
            OperationErrorType::PasswordRequired => f.write_str("Password required"),
        }
    }
}

impl Operation {
    pub fn pending_text(&self, ratio: f32, state: ControllerState) -> String {
        let percent = (ratio * 100.0) as i32;
        let progress = || match state {
            ControllerState::Running => fl!("progress", percent = percent),
            ControllerState::Paused => fl!("progress-paused", percent = percent),
            ControllerState::Cancelled => fl!("progress-cancelled", percent = percent),
            ControllerState::Failed => fl!("progress-failed", percent = percent),
        };
        match self {
            Self::Compress { paths, to, .. } => fl!(
                "compressing",
                items = paths.len(),
                from = paths_parent_name(paths),
                to = file_name(to),
                progress = progress()
            ),
            Self::Copy { paths, to } => fl!(
                "copying",
                items = paths.len(),
                from = paths_parent_name(paths),
                to = file_name(to),
                progress = progress()
            ),
            Self::Delete { paths } => fl!(
                "moving",
                items = paths.len(),
                from = paths_parent_name(paths),
                to = fl!("trash"),
                progress = progress()
            ),
            Self::DeleteTrash { items } => {
                fl!("deleting", items = items.len(), progress = progress())
            }
            Self::EmptyTrash => fl!("emptying-trash", progress = progress()),
            Self::Extract {
                paths,
                to,
                password: _,
            } => fl!(
                "extracting",
                items = paths.len(),
                from = paths_parent_name(paths),
                to = file_name(to),
                progress = progress()
            ),
            Self::Move { paths, to, .. } => fl!(
                "moving",
                items = paths.len(),
                from = paths_parent_name(paths),
                to = file_name(to),
                progress = progress()
            ),
            Self::NewFile { path } => fl!(
                "creating",
                name = file_name(path),
                parent = parent_name(path)
            ),
            Self::NewFolder { path } => fl!(
                "creating",
                name = file_name(path),
                parent = parent_name(path)
            ),
            Self::PermanentlyDelete { paths } => fl!("permanently-deleting", items = paths.len()),
            Self::Rename { from, to } => {
                fl!("renaming", from = file_name(from), to = file_name(to))
            }
            Self::RemoveFromRecents { paths } => fl!("removing-from-recents", items = paths.len()),
            Self::Restore { items } => fl!("restoring", items = items.len(), progress = progress()),
            Self::SetExecutableAndLaunch { path } => {
                fl!("setting-executable-and-launching", name = file_name(path))
            }
            Self::SetPermissions { path, mode } => {
                fl!(
                    "setting-permissions",
                    name = file_name(path),
                    mode = format!("{:#03o}", mode)
                )
            }
        }
    }

    pub fn completed_text(&self) -> String {
        match self {
            Self::Compress { paths, to, .. } => fl!(
                "compressed",
                items = paths.len(),
                from = paths_parent_name(paths),
                to = file_name(to)
            ),
            Self::Copy { paths, to } => fl!(
                "copied",
                items = paths.len(),
                from = paths_parent_name(paths),
                to = file_name(to)
            ),
            Self::Delete { paths } => fl!(
                "moved",
                items = paths.len(),
                from = paths_parent_name(paths),
                to = fl!("trash")
            ),
            Self::DeleteTrash { items } => fl!("deleted", items = items.len()),
            Self::EmptyTrash => fl!("emptied-trash"),
            Self::Extract {
                paths,
                to,
                password: _,
            } => fl!(
                "extracted",
                items = paths.len(),
                from = paths_parent_name(paths),
                to = file_name(to)
            ),
            Self::Move { paths, to, .. } => fl!(
                "moved",
                items = paths.len(),
                from = paths_parent_name(paths),
                to = file_name(to)
            ),
            Self::NewFile { path } => fl!(
                "created",
                name = file_name(path),
                parent = parent_name(path)
            ),
            Self::NewFolder { path } => fl!(
                "created",
                name = file_name(path),
                parent = parent_name(path)
            ),
            Self::PermanentlyDelete { paths } => fl!("permanently-deleted", items = paths.len()),
            Self::RemoveFromRecents { paths } => fl!("removed-from-recents", items = paths.len()),
            Self::Rename { from, to } => fl!("renamed", from = file_name(from), to = file_name(to)),
            Self::Restore { items } => fl!("restored", items = items.len()),
            Self::SetExecutableAndLaunch { path } => {
                fl!("set-executable-and-launched", name = file_name(path))
            }
            Self::SetPermissions { path, mode } => {
                fl!(
                    "set-permissions",
                    name = file_name(path),
                    mode = format!("{:#03o}", mode)
                )
            }
        }
    }

    pub const fn show_progress_notification(&self) -> bool {
        // Long running operations show a progress notification
        match self {
            Self::Compress { .. }
            | Self::Copy { .. }
            | Self::Delete { .. }
            | Self::DeleteTrash { .. }
            | Self::EmptyTrash
            | Self::Extract { .. }
            | Self::Move { .. }
            | Self::PermanentlyDelete { .. }
            | Self::Restore { .. } => true,
            Self::NewFile { .. }
            | Self::NewFolder { .. }
            | Self::RemoveFromRecents { .. }
            | Self::Rename { .. }
            | Self::SetExecutableAndLaunch { .. }
            | Self::SetPermissions { .. } => false,
        }
    }

    pub fn toast(&self) -> Option<String> {
        match self {
            Self::Compress { .. } => Some(self.completed_text()),
            Self::Delete { .. } => Some(self.completed_text()),
            Self::Extract { .. } => Some(self.completed_text()),
            //TODO: more toasts
            _ => None,
        }
    }

    /// Perform the operation
    pub async fn perform(
        self,
        msg_tx: &Arc<TokioMutex<Sender<Message>>>,
        controller: Controller,
    ) -> Result<OperationSelection, OperationError> {
        let controller_clone = controller.clone();

        //TODO: IF ERROR, RETURN AN Operation THAT CAN UNDO THE CURRENT STATE
        let paths: Result<OperationSelection, OperationError> = match self {
            Self::Compress {
                paths,
                to,
                archive_type,
                password,
            } => {
                let controller_c = controller.clone();
                compio::runtime::spawn_blocking(
                    move || -> Result<OperationSelection, OperationError> {
                        let controller = controller_c;
                        let Some(relative_root) = to.parent() else {
                            return Err(OperationError::from_err(
                                format!("path {} has no parent directory", to.display()),
                                &controller,
                            ));
                        };

                        let op_sel = OperationSelection {
                            ignored: paths.clone(),
                            selected: vec![to.clone()],
                        };

                        let mut paths = paths;
                        for path in &paths.clone() {
                            if path.is_dir() {
                                let new_paths_it = WalkDir::new(path).into_iter();
                                for entry in new_paths_it.skip(1) {
                                    let entry = entry
                                        .map_err(|e| OperationError::from_err(e, &controller))?;
                                    paths.push(entry.into_path());
                                }
                            }
                        }

                        let mut staged = crate::atomic_file::stage(&to)
                            .map_err(|e| OperationError::from_err(e, &controller))?;
                        match archive_type {
                            ArchiveType::Tgz => {
                                let writer = io::BufWriter::new(staged.as_file_mut());
                                let encoder = flate2::write::GzEncoder::new(
                                    writer, flate2::Compression::default(),
                                );
                                let mut archive = tar::Builder::new(encoder);

                                let total_paths = paths.len();
                                for (i, path) in paths.iter().enumerate() {
                                    futures::executor::block_on(async {
                                        controller
                                            .check()
                                            .await
                                            .map_err(|e| OperationError::from_state(e, &controller))
                                    })?;

                                    controller.set_progress((i as f32) / total_paths as f32);

                                    let relative_path = path
                                        .strip_prefix(relative_root)
                                        .map_err(|e| OperationError::from_err(e, &controller))?;
                                    archive
                                        .append_path_with_name(path, relative_path)
                                        .map_err(|e| OperationError::from_err(e, &controller))?;
                                }

                                archive
                                    .into_inner()
                                    .and_then(flate2::write::GzEncoder::finish)
                                    .and_then(|mut writer| writer.flush())
                                    .map_err(|e| OperationError::from_err(e, &controller))?;
                            }
                            ArchiveType::Zip => {
                                let writer = io::BufWriter::new(staged.as_file_mut());
                                let mut archive = zip::ZipWriter::new(writer);

                                let total_paths = paths.len();
                                let mut buffer = vec![0; 4 * 1024 * 1024];
                                for (i, path) in paths.iter().enumerate() {
                                    futures::executor::block_on(async {
                                        controller
                                            .check()
                                            .await
                                            .map_err(|s| OperationError::from_state(s, &controller))
                                    })?;

                                    controller.set_progress((i as f32) / total_paths as f32);

                                    let mut zip_options = zip::write::SimpleFileOptions::default();
                                    if password.is_some() {
                                        zip_options = zip_options.with_aes_encryption(
                                            Aes256,
                                            password.as_deref().unwrap(),
                                        );
                                    }
                                    let relative_path = path
                                        .strip_prefix(relative_root)
                                        .map_err(|e| OperationError::from_err(e, &controller))?
                                        .to_str()
                                        .ok_or_else(|| OperationError::from_err(
                                            format!("path {} cannot be represented in a ZIP archive", path.display()),
                                            &controller,
                                        ))?;
                                        let mut file = fs::File::open(path).map_err(|e| {
                                            OperationError::from_err(e, &controller)
                                        })?;
                                        let metadata = file.metadata().map_err(|e| {
                                            OperationError::from_err(e, &controller)
                                        })?;

                                        if let Ok(modified) = metadata.modified()
                                            && let Some(last_modified) =
                                                archive::system_time_to_zip_date_time(modified)
                                        {
                                            zip_options =
                                                zip_options.last_modified_time(last_modified);
                                        }

                                        #[cfg(unix)]
                                        {
                                            use std::os::unix::fs::MetadataExt;
                                            let mode = metadata.mode();
                                            zip_options = zip_options.unix_permissions(mode);
                                        }

                                        if path.is_file() {
                                            let total = metadata.len();
                                            if total >= 4 * 1024 * 1024 * 1024 {
                                                // The large file option must be enabled for files above 4 GiB
                                                zip_options = zip_options.large_file(true);
                                            }
                                            archive
                                                .start_file(relative_path, zip_options)
                                                .map_err(|e| {
                                                    OperationError::from_err(e, &controller)
                                                })?;
                                            let mut current = 0;
                                            loop {
                                                futures::executor::block_on(async {
                                                    controller.check().await.map_err(|s| {
                                                        OperationError::from_state(s, &controller)
                                                    })
                                                })?;

                                                let count =
                                                    file.read(&mut buffer).map_err(|e| {
                                                        OperationError::from_err(e, &controller)
                                                    })?;
                                                if count == 0 {
                                                    break;
                                                }
                                                archive.write_all(&buffer[..count]).map_err(
                                                    |e| OperationError::from_err(e, &controller),
                                                )?;
                                                current += count;

                                                let file_progress = current as f32 / total as f32;
                                                let total_progress =
                                                    (i as f32 + file_progress) / total_paths as f32;
                                                controller.set_progress(total_progress);
                                            }
                                        } else {
                                            archive
                                                .add_directory(relative_path, zip_options)
                                                .map_err(|e| {
                                                    OperationError::from_err(e, &controller)
                                                })?;
                                        }
                                }

                                archive
                                    .finish()
                                    .map_err(|e| OperationError::from_err(e, &controller))?
                                    .flush()
                                    .map_err(|e| OperationError::from_err(e, &controller))?;
                            }
                        }

                        staged.as_file().sync_all()
                            .map_err(|e| OperationError::from_err(e, &controller))?;
                        futures::executor::block_on(controller.check())
                            .map_err(|e| OperationError::from_state(e, &controller))?;
                        staged.persist_noclobber(&to)
                            .map_err(|e| OperationError::from_err(e.error, &controller))?;

                        Ok(op_sel)
                    },
                )
                .await
                .map_err(wrap_compio_spawn_error)?
            }
            Self::Copy { paths, to } => {
                copy_or_move(paths, to, Method::Copy, msg_tx, controller).await
            }
            Self::Delete { paths } => {
                let total = paths.len();
                for (i, path) in paths.into_iter().enumerate() {
                    futures::executor::block_on(async {
                        controller
                            .check()
                            .await
                            .map_err(|s| OperationError::from_state(s, &controller))
                    })?;

                    controller.set_progress((i as f32) / (total as f32));

                    compio::runtime::spawn_blocking(|| trash::delete(path))
                        .await
                        .map_err(wrap_compio_spawn_error)?
                        .map_err(|e| OperationError::from_err(e, &controller))?;
                    //TODO: items_opt allows for easy restore
                }
                Ok(OperationSelection::default())
            }
            Self::DeleteTrash { items } => {
                #[cfg(any(
                    target_os = "windows",
                    all(
                        unix,
                        not(target_os = "macos"),
                        not(target_os = "ios"),
                        not(target_os = "android")
                    )
                ))]
                {
                    let controller_clone = controller.clone();
                    compio::runtime::spawn_blocking(move || -> Result<(), OperationError> {
                        let controller = controller_clone;
                        let count = items.len();
                        for (i, item) in items.into_iter().enumerate() {
                            futures::executor::block_on(async {
                                controller
                                    .check()
                                    .await
                                    .map_err(|s| OperationError::from_state(s, &controller))
                            })?;

                            controller.set_progress(i as f32 / count as f32);

                            trash::os_limited::purge_all([item])
                                .map_err(|e| OperationError::from_err(e, &controller))?;
                        }
                        Ok(())
                    })
                    .await
                    .map_err(wrap_compio_spawn_error)?
                    .map_err(|e| OperationError::from_err(e, &controller))?;
                }
                Ok(OperationSelection::default())
            }
            Self::EmptyTrash => {
                #[cfg(any(
                    target_os = "windows",
                    all(
                        unix,
                        not(target_os = "macos"),
                        not(target_os = "ios"),
                        not(target_os = "android")
                    )
                ))]
                {
                    let controller_clone = controller.clone();
                    compio::runtime::spawn_blocking(move || -> Result<(), OperationError> {
                        let controller = controller_clone;
                        let items = trash::os_limited::list()
                            .map_err(|e| OperationError::from_err(e, &controller))?;
                        let count = items.len();
                        let mut errors: Vec<trash::Error> = Vec::new();

                        for (i, item) in items.into_iter().enumerate() {
                            futures::executor::block_on(async {
                                controller
                                    .check()
                                    .await
                                    .map_err(|s| OperationError::from_state(s, &controller))
                            })?;

                            if let Err(e) = trash::os_limited::purge_all([item]) {
                                errors.push(e);
                            }

                            controller.set_progress(i as f32 / count as f32);
                        }

                        // Report errors at the end
                        if !errors.is_empty() {
                            log::warn!("Failed to purge {} items:", errors.len());
                            for e in &errors {
                                log::warn!("  - {e}");
                            }

                            // Return an error to signal partial failure
                            return Err(OperationError::from_err(
                                format!(
                                    "Failed to delete {} of {} items. Check log for details.",
                                    errors.len(),
                                    count
                                ),
                                &controller,
                            ));
                        }

                        Ok(())
                    })
                    .await
                    .map_err(wrap_compio_spawn_error)?
                    .map_err(|e| OperationError::from_err(e, &controller))?;
                }
                Ok(OperationSelection::default())
            }
            Self::Extract {
                paths,
                to,
                password,
            } => {
                let controller_clone = controller.clone();
                compio::runtime::spawn(async move {
                    let extracted = compio::runtime::spawn_blocking(move || {
                        let controller = controller_clone;
                        let total_paths = paths.len();
                        let mut op_sel = OperationSelection::default();
                        let mut written_files = Vec::new();
                        let mut target_dirs = std::collections::HashSet::new();
                        for (i, path) in paths.iter().enumerate() {
                            futures::executor::block_on(async {
                                controller
                                    .check()
                                    .await
                                    .map_err(|s| OperationError::from_state(s, &controller))
                            })?;

                            controller.set_progress((i as f32) / total_paths as f32);

                            if let Some(file_name) = path.file_name().and_then(|f| f.to_str()) {
                                let dir_name = get_directory_name(file_name);
                                let mut new_dir = to.join(dir_name);

                                if new_dir.exists()
                                    && let Some(new_dir_parent) = new_dir.parent()
                                {
                                    new_dir = copy_unique_path(&new_dir, new_dir_parent);
                                }

                                op_sel.ignored.push(path.clone());
                                op_sel.selected.push(new_dir.clone());

                                let (files, dirs) = crate::archive::extract(
                                    path,
                                    &new_dir,
                                    &password,
                                    &controller,
                                )?;
                                written_files.extend(files);
                                target_dirs.extend(dirs);
                            }
                        }

                        Ok::<_, OperationError>((op_sel, written_files, target_dirs))
                    })
                    .await
                    .map_err(wrap_compio_spawn_error)??;

                    let (op_sel, written_files, target_dirs) = extracted;
                    if !written_files.is_empty() || !target_dirs.is_empty() {
                        sync_to_disk(written_files, target_dirs).await;
                    }

                    Ok::<_, OperationError>(op_sel)
                })
                .await
                .map_err(wrap_compio_spawn_error)?
            }
            Self::Move {
                paths,
                to,
                cross_device_copy,
            } => {
                copy_or_move(
                    paths,
                    to,
                    Method::Move { cross_device_copy },
                    msg_tx,
                    controller,
                )
                .await
            }
            Self::NewFolder { path } => {
                let controller_clone = controller.clone();
                compio::runtime::spawn(async move {
                    let controller = controller_clone;
                    controller
                        .check()
                        .await
                        .map_err(|s| OperationError::from_state(s, &controller))?;
                    compio::fs::create_dir(&path)
                        .await
                        .map_err(|e| OperationError::from_err(e, &controller))?;
                    Result::<_, OperationError>::Ok(OperationSelection {
                        ignored: Vec::new(),
                        selected: vec![path],
                    })
                })
            }
            .await
            .map_err(wrap_compio_spawn_error)?,
            Self::NewFile { path } => {
                let controller_clone = controller.clone();
                compio::runtime::spawn(async move {
                    let controller = controller_clone;
                    controller
                        .check()
                        .await
                        .map_err(|s| OperationError::from_state(s, &controller))?;
                    compio::fs::File::create(&path)
                        .await
                        .map_err(|e| OperationError::from_err(e, &controller))?;
                    Result::<_, OperationError>::Ok(OperationSelection {
                        ignored: Vec::new(),
                        selected: vec![path],
                    })
                })
            }
            .await
            .map_err(wrap_compio_spawn_error)?,
            Self::PermanentlyDelete { paths } => {
                let total = paths.len();
                for (idx, path) in paths.into_iter().enumerate() {
                    controller
                        .check()
                        .await
                        .map_err(|s| OperationError::from_state(s, &controller))?;

                    controller.set_progress((idx as f32) / (total as f32));

                    tokio::task::spawn_blocking(|| {
                        if path.is_symlink() || path.is_file() {
                            fs::remove_file(path)
                        } else if path.is_dir() {
                            fs::remove_dir_all(path)
                        } else {
                            Err(std::io::Error::new(
                                std::io::ErrorKind::InvalidData,
                                "File to delete is not symlink, file or directory",
                            ))
                        }
                    })
                    .await
                    .map_err(|e| OperationError::from_err(e, &controller))?
                    .map_err(|e| OperationError::from_err(e, &controller))?;
                }

                Ok(OperationSelection::default())
            }
            Self::RemoveFromRecents { paths } => {
                tokio::task::spawn_blocking(move || {
                    let path_refs = paths.iter().map(PathBuf::as_path).collect::<Box<[_]>>();
                    crate::recents::remove_recently_used(&path_refs)
                })
                .await
                .map_err(|e| OperationError::from_err(e, &controller))?
                .map_err(|e| OperationError::from_err(e, &controller))?;

                Ok(OperationSelection::default())
            }
            Self::Rename { from, to } => {
                let controller_clone = controller.clone();

                compio::runtime::spawn(async move {
                    let controller = controller_clone;
                    controller
                        .check()
                        .await
                        .map_err(|s| OperationError::from_state(s, &controller))?;
                    rename_noreplace(from.clone(), to.clone())
                        .await
                        .map_err(|e| OperationError::from_err(e, &controller))?;
                    Result::<_, OperationError>::Ok(OperationSelection {
                        ignored: vec![from],
                        selected: vec![to],
                    })
                })
            }
            .await
            .map_err(wrap_compio_spawn_error)?,
            #[cfg(target_os = "macos")]
            Self::Restore { .. } => {
                // TODO: add support for macos
                return Err(OperationError::from_msg(
                    "Restoring from trash is not supported on macos",
                ));
            }
            #[cfg(not(target_os = "macos"))]
            Self::Restore { items } => {
                let total = items.len();
                let mut paths = Vec::with_capacity(total);
                for (i, item) in items.into_iter().enumerate() {
                    controller
                        .check()
                        .await
                        .map_err(|s| OperationError::from_state(s, &controller))?;

                    controller.set_progress((i as f32) / (total as f32));

                    paths.push(item.original_path());

                    // Items with .trashinfo id use standard restore; sub-items use manual move
                    if item.id.to_str().is_some_and(|s| s.ends_with(".trashinfo")) {
                        compio::runtime::spawn_blocking(|| trash::os_limited::restore_all([item]))
                            .await
                            .map_err(wrap_compio_spawn_error)?
                            .map_err(|e| OperationError::from_err(e, &controller))?;
                    } else {
                        let from = PathBuf::from(&item.id);
                        let to = item.original_path();
                        if let Some(parent) = to.parent() {
                            std::fs::create_dir_all(parent)
                                .map_err(|e| OperationError::from_err(e, &controller))?;
                        }
                        std::fs::rename(&from, &to)
                            .map_err(|e| OperationError::from_err(e, &controller))?;
                    }
                }
                Ok(OperationSelection {
                    ignored: Vec::new(),
                    selected: paths,
                })
            }
            Self::SetExecutableAndLaunch { path } => {
                controller
                    .check()
                    .await
                    .map_err(|s| OperationError::from_state(s, &controller))?;

                let controller_clone = controller.clone();
                compio::runtime::spawn_blocking(move || -> Result<(), OperationError> {
                    let controller = controller_clone;
                    //TODO: what to do on non-Unix systems?
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::PermissionsExt;

                        let mut perms = fs::metadata(&path)
                            .map_err(|e| OperationError::from_err(e, &controller))?
                            .permissions();
                        let current_mode = perms.mode();
                        let new_mode = current_mode | 0o111;
                        perms.set_mode(new_mode);
                        fs::set_permissions(&path, perms)
                            .map_err(|e| OperationError::from_err(e, &controller))?;
                    }

                    let mut command = std::process::Command::new(path);
                    spawn_detached(&mut command)
                        .map_err(|e| OperationError::from_err(e, &controller))?;

                    Ok(())
                })
                .await
                .map_err(wrap_compio_spawn_error)?
                .map_err(|e| OperationError::from_err(e, &controller))?;
                Ok(OperationSelection::default())
            }
            Self::SetPermissions { path, mode } => {
                controller
                    .check()
                    .await
                    .map_err(|s| OperationError::from_state(s, &controller))?;

                let controller_clone = controller.clone();
                let path_clone = path.clone();
                compio::runtime::spawn_blocking(move || -> Result<(), OperationError> {
                    let controller = controller_clone;
                    let path = path_clone;
                    //TODO: what to do on non-Unix systems?
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::PermissionsExt;
                        let perms = fs::Permissions::from_mode(mode);
                        fs::set_permissions(&path, perms)
                            .map_err(|e| OperationError::from_err(e, &controller))?;
                    }

                    Ok(())
                })
                .await
                .map_err(wrap_compio_spawn_error)?
                .map_err(|e| OperationError::from_err(e, &controller))?;
                Ok(OperationSelection {
                    ignored: Vec::new(),
                    selected: vec![path],
                })
            }
        };

        controller_clone.set_progress(1.0);

        paths
    }
}

#[track_caller]
fn wrap_compio_spawn_error(err: Box<dyn std::any::Any + Send>) -> OperationError {
    log::error!(
        "compio runtime spawn failed: {}",
        std::backtrace::Backtrace::capture()
    );

    // Preserve error if it's already an OperationError
    if let Ok(err) = err.downcast() {
        *err
    } else {
        OperationError::from_msg("compio runtime spawn failed")
    }
}

#[cfg(test)]
mod tests {
    use std::fs::{self, File};
    use std::io;
    use std::path::PathBuf;

    use cosmic::iced::futures::channel::mpsc;
    use cosmic::iced::futures::{StreamExt, future};
    use log::debug;
    use test_log::test;
    use tokio::sync;

    use super::{Controller, Operation, OperationError, OperationSelection, ReplaceResult};
    use crate::app::test_utils::{
        NAME_LEN, NUM_DIRS, NUM_FILES, NUM_HIDDEN, NUM_NESTED, empty_fs, filter_dirs, filter_files,
        simple_fs,
    };
    use crate::app::{DialogPage, Message};
    use crate::fl;

    /// Simple wrapper around `[Operation::Copy]`
    pub async fn operation_copy(
        paths: Vec<PathBuf>,
        to: PathBuf,
    ) -> Result<OperationSelection, OperationError> {
        let id = fastrand::u64(0..u64::MAX);
        let (tx, mut rx) = mpsc::channel(1);
        let paths_clone = paths.clone();
        let to_clone = to.clone();

        // Wrap this into its own future so that it may be polled concurerntly with the message handler.
        let handle_copy = async move {
            Operation::Copy {
                paths: paths_clone,
                to: to_clone,
            }
            .perform(&sync::Mutex::new(tx).into(), Controller::default())
            .await
        };

        // Concurrently handling messages will prevent the mpsc channel from blocking when full.
        let handle_messages = async move {
            while let Some(msg) = rx.next().await {
                match msg {
                    Message::DialogPush(DialogPage::Replace { tx, .. }, _id_to_focus) => {
                        debug!("[{id}] Replace request");
                        tx.send(ReplaceResult::Cancel)
                            .await
                            .expect("Sending a response to a replace request should succeed");
                    }
                    _ => unreachable!(
                        "Only [ `Message::PendingProgress`, `Message::DialogPush(DialogPage::Replace)` ] are sent from operation"
                    ),
                }
            }
        };

        future::join(handle_messages, handle_copy).await.1
    }

    async fn compress_existing_target(archive_type: crate::app::ArchiveType) -> io::Result<()> {
        let dir = empty_fs()?;
        let source = dir.path().join("source.txt");
        let target = dir
            .path()
            .join(format!("archive{}", archive_type.extension()));
        fs::write(&source, b"source")?;
        fs::write(&target, b"existing archive")?;
        let (tx, mut rx) = mpsc::channel(1);
        let operation = Operation::Compress {
            paths: vec![source.clone()],
            to: target.clone(),
            archive_type,
            password: None,
        };
        let run = async move {
            operation
                .perform(&sync::Mutex::new(tx).into(), Controller::default())
                .await
        };
        let drain = async move { while rx.next().await.is_some() {} };
        let result = future::join(run, drain).await.0;
        assert!(result.is_err());
        assert_eq!(fs::read(&target)?, b"existing archive");
        assert_eq!(fs::read(&source)?, b"source");
        Ok(())
    }

    #[test(compio::test)]
    async fn compress_preserves_an_existing_zip() -> io::Result<()> {
        compress_existing_target(crate::app::ArchiveType::Zip).await
    }

    #[test(compio::test)]
    async fn compress_preserves_an_existing_tgz() -> io::Result<()> {
        compress_existing_target(crate::app::ArchiveType::Tgz).await
    }

    async fn compress_missing_source(archive_type: crate::app::ArchiveType) -> io::Result<()> {
        let dir = empty_fs()?;
        let source = dir.path().join("source.txt");
        fs::write(&source, b"source")?;
        let target = dir
            .path()
            .join(format!("archive{}", archive_type.extension()));
        let (tx, mut rx) = mpsc::channel(1);
        let operation = Operation::Compress {
            paths: vec![source, dir.path().join("missing.txt")],
            to: target.clone(),
            archive_type,
            password: None,
        };
        let run = async move {
            operation
                .perform(&sync::Mutex::new(tx).into(), Controller::default())
                .await
        };
        let drain = async move { while rx.next().await.is_some() {} };
        assert!(future::join(run, drain).await.0.is_err());
        assert!(
            !target.exists(),
            "failed compression published a partial archive"
        );
        assert_eq!(fs::read_dir(dir.path())?.count(), 1);
        Ok(())
    }

    #[test(compio::test)]
    async fn failed_zip_leaves_no_destination() -> io::Result<()> {
        compress_missing_source(crate::app::ArchiveType::Zip).await
    }

    #[test(compio::test)]
    async fn failed_tgz_leaves_no_destination() -> io::Result<()> {
        compress_missing_source(crate::app::ArchiveType::Tgz).await
    }

    #[cfg(unix)]
    async fn compress_finalization_failure(
        archive_type: crate::app::ArchiveType,
        test_name: &str,
    ) -> io::Result<()> {
        if let Some(dir) = super::test_faults::child_directory() {
            let (tx, mut rx) = mpsc::channel(1);
            let operation = Operation::Compress {
                paths: Vec::new(),
                to: dir.join(format!("archive{}", archive_type.extension())),
                archive_type,
                password: None,
            };
            let run = async move {
                operation
                    .perform(&sync::Mutex::new(tx).into(), Controller::default())
                    .await
            };
            let drain = async move { while rx.next().await.is_some() {} };
            let err = future::join(run, drain)
                .await
                .0
                .expect_err("archive finalization should report the write failure");
            assert!(
                err.to_string()
                    .contains(&io::Error::from_raw_os_error(libc::EFBIG).to_string()),
                "unexpected compression error: {err}"
            );
            return Ok(());
        }

        let dir = empty_fs()?;
        super::test_faults::run_child(
            test_name,
            dir.path(),
            super::test_faults::Fault::FileSizeLimit,
        )?;
        assert_eq!(
            fs::read_dir(dir.path())?.count(),
            0,
            "failed finalization left a destination or staging file"
        );
        Ok(())
    }

    #[cfg(unix)]
    #[test(compio::test)]
    async fn zip_finalization_failure_leaves_no_destination_or_staging_file() -> io::Result<()> {
        compress_finalization_failure(
            crate::app::ArchiveType::Zip,
            "zip_finalization_failure_leaves_no_destination_or_staging_file",
        )
        .await
    }

    #[cfg(unix)]
    #[test(compio::test)]
    async fn tgz_finalization_failure_leaves_no_destination_or_staging_file() -> io::Result<()> {
        compress_finalization_failure(
            crate::app::ArchiveType::Tgz,
            "tgz_finalization_failure_leaves_no_destination_or_staging_file",
        )
        .await
    }

    async fn compress_cancelled(archive_type: crate::app::ArchiveType) -> io::Result<()> {
        let dir = empty_fs()?;
        let source = dir.path().join("source.txt");
        fs::write(&source, b"source")?;
        let target = dir
            .path()
            .join(format!("archive{}", archive_type.extension()));
        let controller = Controller::default();
        controller.cancel();
        let (tx, mut rx) = mpsc::channel(1);
        let operation = Operation::Compress {
            paths: vec![source],
            to: target.clone(),
            archive_type,
            password: None,
        };
        let run = async move {
            operation
                .perform(&sync::Mutex::new(tx).into(), controller)
                .await
        };
        let drain = async move { while rx.next().await.is_some() {} };
        assert!(future::join(run, drain).await.0.is_err());
        assert!(
            !target.exists(),
            "cancelled compression published an archive"
        );
        assert_eq!(fs::read_dir(dir.path())?.count(), 1);
        Ok(())
    }

    #[test(compio::test)]
    async fn cancelled_zip_leaves_no_destination() -> io::Result<()> {
        compress_cancelled(crate::app::ArchiveType::Zip).await
    }

    #[test(compio::test)]
    async fn cancelled_tgz_leaves_no_destination() -> io::Result<()> {
        compress_cancelled(crate::app::ArchiveType::Tgz).await
    }

    async fn compressed_fixture(
        archive_type: crate::app::ArchiveType,
    ) -> io::Result<(tempfile::TempDir, PathBuf)> {
        let dir = empty_fs()?;
        let source = dir.path().join("source.txt");
        fs::write(&source, b"source payload")?;
        let target = dir
            .path()
            .join(format!("archive{}", archive_type.extension()));
        let (tx, mut rx) = mpsc::channel(1);
        let operation = Operation::Compress {
            paths: vec![source],
            to: target.clone(),
            archive_type,
            password: None,
        };
        let run = async move {
            operation
                .perform(&sync::Mutex::new(tx).into(), Controller::default())
                .await
        };
        let drain = async move { while rx.next().await.is_some() {} };
        assert!(future::join(run, drain).await.0.is_ok());
        Ok((dir, target))
    }

    #[test(compio::test)]
    async fn published_zip_is_readable() -> io::Result<()> {
        use std::io::Read;

        let (_dir, target) = compressed_fixture(crate::app::ArchiveType::Zip).await?;
        let mut archive = zip::ZipArchive::new(File::open(target)?)?;
        assert_eq!(archive.len(), 1);
        let mut payload = Vec::new();
        archive.by_name("source.txt")?.read_to_end(&mut payload)?;
        assert_eq!(payload, b"source payload");
        Ok(())
    }

    #[test(compio::test)]
    async fn published_tgz_is_readable() -> io::Result<()> {
        use std::io::Read;

        let (_dir, target) = compressed_fixture(crate::app::ArchiveType::Tgz).await?;
        let mut decoded = Vec::new();
        flate2::read::GzDecoder::new(File::open(target)?).read_to_end(&mut decoded)?;
        let mut archive = tar::Archive::new(io::Cursor::new(decoded));
        let mut entries = archive.entries()?;
        let mut entry = entries.next().unwrap()?;
        assert_eq!(entry.path()?.as_ref(), std::path::Path::new("source.txt"));
        let mut payload = Vec::new();
        entry.read_to_end(&mut payload)?;
        assert_eq!(payload, b"source payload");
        assert!(entries.next().is_none());
        Ok(())
    }

    #[cfg(unix)]
    async fn compress_non_utf8_source(
        archive_type: crate::app::ArchiveType,
    ) -> io::Result<(
        tempfile::TempDir,
        PathBuf,
        Result<OperationSelection, OperationError>,
    )> {
        use std::os::unix::ffi::OsStrExt;

        let dir = empty_fs()?;
        let source = dir.path().join(std::ffi::OsStr::from_bytes(b"source-\xff"));
        fs::write(&source, b"source payload")?;
        let target = dir
            .path()
            .join(format!("archive{}", archive_type.extension()));
        let (tx, mut rx) = mpsc::channel(1);
        let operation = Operation::Compress {
            paths: vec![source],
            to: target.clone(),
            archive_type,
            password: None,
        };
        let run = async move {
            operation
                .perform(&sync::Mutex::new(tx).into(), Controller::default())
                .await
        };
        let drain = async move { while rx.next().await.is_some() {} };
        let result = future::join(run, drain).await.0;
        Ok((dir, target, result))
    }

    #[cfg(unix)]
    #[test(compio::test)]
    async fn tgz_preserves_non_utf8_source_names() -> io::Result<()> {
        use std::io::Read;
        use std::os::unix::ffi::OsStrExt;

        let (_dir, target, result) = compress_non_utf8_source(crate::app::ArchiveType::Tgz).await?;
        assert!(result.is_ok());
        let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(File::open(target)?));
        let mut entries = archive.entries()?;
        let mut entry = entries.next().expect("source missing from archive")?;
        assert_eq!(entry.path()?.as_os_str().as_bytes(), b"source-\xff");
        let mut payload = Vec::new();
        entry.read_to_end(&mut payload)?;
        assert_eq!(payload, b"source payload");
        Ok(())
    }

    #[cfg(unix)]
    #[test(compio::test)]
    async fn zip_rejects_non_utf8_source_names_without_publishing() -> io::Result<()> {
        let (_dir, target, result) = compress_non_utf8_source(crate::app::ArchiveType::Zip).await?;
        assert!(
            result.is_err(),
            "ZIP silently omitted an unrepresentable name"
        );
        assert!(!target.exists());
        Ok(())
    }

    #[test(compio::test)]
    async fn rename_preserves_an_empty_destination_directory() -> io::Result<()> {
        let dir = empty_fs()?;
        let source = dir.path().join("source");
        let target = dir.path().join("target");
        fs::create_dir(&source)?;
        fs::create_dir(&target)?;
        assert!(
            operation_rename(source.clone(), target.clone())
                .await
                .is_err()
        );
        assert!(source.is_dir());
        assert!(target.is_dir());
        Ok(())
    }

    #[cfg(unix)]
    #[test(compio::test)]
    async fn rename_preserves_a_dangling_destination_symlink() -> io::Result<()> {
        let dir = empty_fs()?;
        let source = dir.path().join("source");
        let target = dir.path().join("target");
        let missing = dir.path().join("missing");
        fs::write(&source, b"source")?;
        std::os::unix::fs::symlink(&missing, &target)?;
        assert!(
            operation_rename(source.clone(), target.clone())
                .await
                .is_err()
        );
        assert_eq!(fs::read(&source)?, b"source");
        assert_eq!(fs::read_link(&target)?, missing);
        Ok(())
    }

    async fn operation_rename(
        from: PathBuf,
        to: PathBuf,
    ) -> Result<OperationSelection, OperationError> {
        let (tx, mut rx) = mpsc::channel(4);
        let handle_rename = async move {
            Operation::Rename { from, to }
                .perform(&sync::Mutex::new(tx).into(), Controller::default())
                .await
        };
        let handle_messages = async move { while rx.next().await.is_some() {} };
        future::join(handle_messages, handle_rename).await.1
    }

    #[test(compio::test)]
    async fn rename_refuses_to_replace_an_existing_file() -> io::Result<()> {
        let fs = empty_fs()?;
        let from = fs.path().join("draft.txt");
        let to = fs.path().join("report.txt");
        fs::write(&from, b"draft")?;
        fs::write(&to, b"report")?;

        operation_rename(from.clone(), to.clone())
            .await
            .expect_err("Renaming onto an existing file should fail");

        assert_eq!(fs::read(&from)?, b"draft", "Source should be untouched");
        assert_eq!(fs::read(&to)?, b"report", "Target should be untouched");
        Ok(())
    }

    #[test(compio::test)]
    async fn rename_refuses_to_replace_an_existing_folder() -> io::Result<()> {
        let fs = empty_fs()?;
        let from = fs.path().join("drafts");
        let to = fs.path().join("reports");
        fs::create_dir(&from)?;
        fs::create_dir(&to)?;
        fs::write(to.join("kept.txt"), b"kept")?;

        operation_rename(from.clone(), to.clone())
            .await
            .expect_err("Renaming onto an existing folder should fail");

        assert!(from.exists(), "Source should be untouched");
        assert_eq!(fs::read(to.join("kept.txt"))?, b"kept");
        Ok(())
    }

    #[test(compio::test)]
    async fn rename_onto_itself_succeeds_and_changes_nothing() -> io::Result<()> {
        let fs = empty_fs()?;
        let path = fs.path().join("draft.txt");
        fs::write(&path, b"draft")?;

        operation_rename(path.clone(), path.clone())
            .await
            .expect("Confirming an unchanged name should be a no-op");

        assert_eq!(fs::read(&path)?, b"draft");
        Ok(())
    }

    #[test(compio::test)]
    async fn rename_moves_when_the_target_is_free() -> io::Result<()> {
        let fs = empty_fs()?;
        let from = fs.path().join("draft.txt");
        let to = fs.path().join("report.txt");
        fs::write(&from, b"draft")?;

        operation_rename(from.clone(), to.clone())
            .await
            .expect("Renaming to a free name should succeed");

        assert!(!from.exists(), "Source should be gone");
        assert_eq!(fs::read(&to)?, b"draft");
        Ok(())
    }

    #[cfg(target_os = "linux")]
    #[test(compio::test)]
    async fn rename_fails_closed_when_atomic_rename_is_unsupported() -> io::Result<()> {
        if let Some(dir) = super::test_faults::child_directory() {
            super::test_faults::assert_rename_fault();
            operation_rename(dir.join("source"), dir.join("target"))
                .await
                .expect_err("unsupported atomic rename should fail closed");
            return Ok(());
        }

        for errno in [libc::ENOSYS, libc::EINVAL, libc::EOPNOTSUPP] {
            for target_exists in [false, true] {
                let dir = empty_fs()?;
                let source = dir.path().join("source");
                let target = dir.path().join("target");
                fs::write(&source, b"source")?;
                if target_exists {
                    fs::write(&target, b"target")?;
                }
                super::test_faults::run_child(
                    "rename_fails_closed_when_atomic_rename_is_unsupported",
                    dir.path(),
                    super::test_faults::Fault::RenameError(errno),
                )?;
                assert_eq!(fs::read(&source)?, b"source");
                if target_exists {
                    assert_eq!(fs::read(&target)?, b"target");
                } else {
                    assert!(
                        !target.exists(),
                        "unsupported rename published a destination"
                    );
                }
            }
        }
        Ok(())
    }

    async fn operation_delete(paths: Vec<PathBuf>) -> Result<OperationSelection, OperationError> {
        let (tx, mut rx) = mpsc::channel(1);

        let handle_delete = async move {
            Operation::Delete { paths }
                .perform(&sync::Mutex::new(tx).into(), Controller::default())
                .await
        };

        // Drain progress messages so the bounded channel never blocks the operation.
        let handle_messages = async move { while rx.next().await.is_some() {} };

        future::join(handle_messages, handle_delete).await.1
    }

    #[test(compio::test)]
    async fn delete_surfaces_error_when_trash_fails() -> io::Result<()> {
        // `trash::delete` fails on a path that does not exist, and that failure has to reach
        // the caller: reporting success would show the delete as done with nothing trashed.
        let fs = empty_fs()?;
        let missing = fs.path().join("does-not-exist");
        assert!(!missing.exists());

        let result = operation_delete(vec![missing]).await;

        assert!(
            result.is_err(),
            "Delete of a nonexistent path should surface an error, not report success"
        );

        Ok(())
    }

    #[test(compio::test)]
    async fn copy_file_to_same_location() -> io::Result<()> {
        let fs = simple_fs(NUM_FILES, 0, 1, 0, NAME_LEN)?;
        let path = fs.path();

        // Get the first file from the first directory
        let first_dir = filter_dirs(path)?
            .next()
            .expect("Should have at least one directory");
        let first_file = filter_files(&first_dir)?
            .next()
            .expect("Should have at least one file");

        // Duplicate that file
        let base_name = first_file
            .file_name()
            .and_then(|name| name.to_str())
            .expect("File name exists and is valid");
        debug!(
            "Duplicating {} in {}",
            first_file.display(),
            first_dir.display()
        );
        operation_copy(vec![first_file.clone()], first_dir.clone())
            .await
            .expect("Copy operation should have succeeded");

        assert!(first_file.exists(), "Original file should still exist");
        let expected = first_dir.join(format!("{base_name} ({} 1)", fl!("copy_noun")));
        assert!(expected.exists(), "File should have been duplicated");

        Ok(())
    }

    #[test(compio::test)]
    async fn copy_file_with_extension_to_same_loc() -> io::Result<()> {
        let fs = empty_fs()?;
        let path = fs.path();

        let base_name = "foo.txt";
        let base_path = path.join(base_name);
        File::create(&base_path)?;
        debug!("Duplicating {}", base_path.display());
        operation_copy(vec![base_path.clone()], path.to_owned())
            .await
            .expect("Copy operation should have succeeded");

        assert!(base_path.exists(), "Original file should still exist");
        let expected = path.join(format!("foo ({} 1).txt", fl!("copy_noun")));
        assert!(expected.exists(), "File should have been duplicated");

        Ok(())
    }

    #[test(compio::test)]
    async fn copy_dir_to_same_location() -> io::Result<()> {
        let fs = simple_fs(NUM_FILES, 0, NUM_DIRS, NUM_NESTED, NAME_LEN)?;
        let path = fs.path();

        // First directory path
        let first_dir = filter_dirs(path)?
            .next()
            .expect("Should have at least one directory");
        let base_name = first_dir
            .file_name()
            .and_then(|name| name.to_str())
            .expect("First directory exists and has a valid name");
        debug!("Duplicating directory {}", first_dir.display());
        operation_copy(vec![first_dir.clone()], path.to_owned())
            .await
            .expect("Copy operation should have succeeded");

        assert!(first_dir.exists(), "Original directory should still exist");
        let expected = path.join(format!("{base_name} ({} 1)", fl!("copy_noun")));
        assert!(expected.exists(), "Directory should have been duplicated");

        Ok(())
    }

    #[test(compio::test)]
    async fn copying_file_multiple_times_to_same_location() -> io::Result<()> {
        let fs = empty_fs()?;
        let path = fs.path();

        let base_name = "cosmic";
        let base_path = path.join(base_name);
        File::create(&base_path)?;

        for i in 1..5 {
            debug!("Duplicating {}", base_path.display());
            operation_copy(vec![base_path.clone()], path.to_owned())
                .await
                .expect("Copy operation should have succeeded");
            assert!(base_path.exists(), "Original file should still exist");
            assert!(
                path.join(format!("{base_name} ({} {i})", fl!("copy_noun")))
                    .exists(),
                "File should have been duplicated (copy #{i})"
            );
        }

        Ok(())
    }

    #[test(compio::test)]
    async fn copy_to_diff_dir_doesnt_dupe_files() -> io::Result<()> {
        let fs = simple_fs(NUM_FILES, NUM_HIDDEN, NUM_DIRS, NUM_NESTED, NAME_LEN)?;
        let path = fs.path();

        let (first_dir, second_dir) = {
            let mut dirs = filter_dirs(path)?;
            (
                dirs.next().expect("Should have at least two dirs"),
                dirs.next().expect("Should have at least two dirs"),
            )
        };
        let first_file = filter_files(&first_dir)?
            .next()
            .expect("Should have at least one file");
        // Both directories have a file with the same name.
        let base_name = first_file
            .file_name()
            .and_then(|name| name.to_str())
            .expect("File name exists and is valid");

        debug!(
            "Copying {} to {}",
            first_file.display(),
            second_dir.display()
        );
        operation_copy(vec![first_file.clone()], second_dir.clone())
            .await
            .expect(concat!(
                "Copy operation should have been cancelled ",
                "because we're copying to different directories ",
                "without replacement"
            ));
        assert!(
            first_dir.join(base_name).exists(),
            "First file should still exist"
        );
        assert!(
            second_dir.join(base_name).exists(),
            "Second file should still exist"
        );

        Ok(())
    }

    #[test(compio::test)]
    async fn copy_file_with_diff_name_to_diff_dir() -> io::Result<()> {
        let fs = empty_fs()?;
        let path = fs.path();

        let dir_path = path.join("cosmic");
        fs::create_dir(&dir_path)?;
        let file_path = path.join("ferris");
        File::create(&file_path)?;
        let expected = dir_path.join("ferris");

        debug!("Copying {} to {}", file_path.display(), expected.display());
        operation_copy(vec![file_path.clone()], dir_path.clone())
            .await
            .expect("Copy operation should have succeeded");

        assert!(file_path.exists(), "Original file should still exist");
        assert!(expected.exists(), "File should have been copied");

        Ok(())
    }
}
