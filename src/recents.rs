use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, mpsc};

enum RecentsMutation {
    Record {
        paths: Vec<PathBuf>,
        app_id: &'static str,
    },
    Clear,
    Remove(Vec<PathBuf>),
}

struct QueuedMutation {
    mutation: Option<RecentsMutation>,
    completion: mpsc::Sender<Result<(), recently_used_xbel::Error>>,
}

struct RecentsWriter {
    sender: mpsc::Sender<QueuedMutation>,
}

impl RecentsWriter {
    fn new(
        mut apply: impl FnMut(RecentsMutation) -> Result<(), recently_used_xbel::Error> + Send + 'static,
    ) -> Self {
        let (sender, receiver) = mpsc::channel::<QueuedMutation>();
        std::thread::spawn(move || {
            for update in receiver {
                let result = update.mutation.map_or(Ok(()), &mut apply);
                let _ = update.completion.send(result);
            }
        });
        Self { sender }
    }

    fn submit(
        &self,
        mutation: RecentsMutation,
    ) -> mpsc::Receiver<Result<(), recently_used_xbel::Error>> {
        self.enqueue(Some(mutation))
    }

    fn barrier(&self) -> mpsc::Receiver<Result<(), recently_used_xbel::Error>> {
        self.enqueue(None)
    }

    fn enqueue(
        &self,
        mutation: Option<RecentsMutation>,
    ) -> mpsc::Receiver<Result<(), recently_used_xbel::Error>> {
        let (completion, receiver) = mpsc::channel();
        if let Err(err) = self.sender.send(QueuedMutation {
            mutation,
            completion,
        }) {
            log::warn!("failed to queue recents update");
            let _ = err
                .0
                .completion
                .send(Err(recently_used_xbel::Error::Update));
        }
        receiver
    }
}

fn apply_recents_mutation(mutation: RecentsMutation) -> Result<(), recently_used_xbel::Error> {
    let path = recently_used_xbel::dir().ok_or(recently_used_xbel::Error::DoesNotExist)?;
    initialize_history(&path)
        .inspect_err(|err| log::warn!("failed to initialize recents history: {}", err))
        .map_err(recently_used_xbel::Error::Read)?;
    match mutation {
        RecentsMutation::Record { paths, app_id } => {
            let mut result = Ok(());
            for path in paths {
                if let Err(err) = recently_used_xbel::update_recently_used(
                    &path,
                    app_id.to_string(),
                    "cosmic-files".to_string(),
                    None,
                ) {
                    log::warn!("failed to add {} to recently used: {}", path.display(), err);
                    result = Err(err);
                }
            }
            result
        }
        RecentsMutation::Clear => recently_used_xbel::clear_recently_used().inspect_err(|err| {
            log::warn!("failed to clear recents history: {}", err);
        }),
        RecentsMutation::Remove(paths) => {
            let paths = paths.iter().map(PathBuf::as_path).collect::<Vec<_>>();
            recently_used_xbel::remove_recently_used(&paths)
        }
    }
}

fn initialize_history(path: &Path) -> std::io::Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => return Ok(()),
        Err(err) if err.kind() != std::io::ErrorKind::NotFound => return Err(err),
        Err(_) => {}
    }
    let parent = path
        .parent()
        .ok_or_else(|| std::io::Error::other("recents path has no parent"))?;
    std::fs::create_dir_all(parent)?;
    let mut file = tempfile::NamedTempFile::new_in(parent)?;
    file.write_all(br#"<?xml version="1.0" encoding="UTF-8"?><xbel version="1.0" xmlns:bookmark="http://www.freedesktop.org/standards/desktop-bookmarks" xmlns:mime="http://www.freedesktop.org/standards/shared-mime-info"/>"#)?;
    match file.persist_noclobber(path) {
        Ok(_) => Ok(()),
        Err(err) if err.error.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
        Err(err) => Err(err.error),
    }
}

static RECENTS_WRITER: LazyLock<RecentsWriter> =
    LazyLock::new(|| RecentsWriter::new(apply_recents_mutation));

pub fn record_recently_used(paths: Vec<PathBuf>, app_id: &'static str) {
    if !paths.is_empty() {
        RECENTS_WRITER.submit(RecentsMutation::Record { paths, app_id });
    }
}

pub(crate) fn barrier() -> mpsc::Receiver<Result<(), recently_used_xbel::Error>> {
    RECENTS_WRITER.barrier()
}

pub fn clear_recently_used() {
    RECENTS_WRITER.submit(RecentsMutation::Clear);
}

pub(crate) fn remove_recently_used(paths: &[&Path]) -> Result<(), recently_used_xbel::Error> {
    RECENTS_WRITER
        .submit(RecentsMutation::Remove(
            paths.iter().map(|path| path.to_path_buf()).collect(),
        ))
        .recv()
        .unwrap_or(Err(recently_used_xbel::Error::Update))
}

#[cfg(test)]
mod tests {
    use super::{RecentsMutation, RecentsWriter};
    use std::path::PathBuf;
    use std::sync::mpsc;

    #[test]
    fn initializing_history_preserves_existing_contents() -> std::io::Result<()> {
        let dir = tempfile::TempDir::new()?;
        let path = dir.path().join("recently-used.xbel");
        let contents = b"existing history, even if malformed";
        std::fs::write(&path, contents)?;
        super::initialize_history(&path)?;
        assert_eq!(std::fs::read(path)?, contents);
        Ok(())
    }

    #[test]
    fn initializing_history_propagates_storage_errors() -> std::io::Result<()> {
        let dir = tempfile::TempDir::new()?;
        let parent = dir.path().join("not-a-directory");
        std::fs::write(&parent, b"file")?;
        assert!(super::initialize_history(&parent.join("recently-used.xbel")).is_err());
        Ok(())
    }

    #[test]
    fn a_barrier_waits_for_preceding_mutations_without_applying_another() {
        let (started, running) = mpsc::channel();
        let (release, blocked) = mpsc::channel();
        let writer = RecentsWriter::new(move |_| {
            started.send(()).unwrap();
            blocked.recv().unwrap();
            Ok(())
        });
        drop(writer.submit(RecentsMutation::Clear));
        running.recv().unwrap();
        let completion = writer.barrier();
        assert!(matches!(
            completion.try_recv(),
            Err(mpsc::TryRecvError::Empty)
        ));
        release.send(()).unwrap();
        completion.recv().unwrap().unwrap();
        assert!(matches!(running.try_recv(), Err(mpsc::TryRecvError::Empty)));
    }

    #[test]
    fn recents_mutations_follow_submission_order() {
        let (snapshots, observed) = mpsc::channel();
        let mut paths = Vec::new();
        let writer = RecentsWriter::new(move |mutation| {
            match mutation {
                RecentsMutation::Record { paths: added, .. } => paths.extend(added),
                RecentsMutation::Clear => paths.clear(),
                RecentsMutation::Remove(removed) => paths.retain(|path| !removed.contains(path)),
            }
            snapshots.send(paths.clone()).unwrap();
            Ok(())
        });
        drop(writer.submit(RecentsMutation::Record {
            paths: vec![PathBuf::from("old")],
            app_id: "test",
        }));
        drop(writer.submit(RecentsMutation::Clear));
        drop(writer.submit(RecentsMutation::Record {
            paths: vec![PathBuf::from("new")],
            app_id: "test",
        }));
        writer
            .submit(RecentsMutation::Remove(vec![PathBuf::from("new")]))
            .recv()
            .unwrap()
            .unwrap();
        assert_eq!(observed.recv().unwrap(), vec![PathBuf::from("old")]);
        assert_eq!(observed.recv().unwrap(), Vec::<PathBuf>::new());
        assert_eq!(observed.recv().unwrap(), vec![PathBuf::from("new")]);
        assert_eq!(observed.recv().unwrap(), Vec::<PathBuf>::new());
    }

    #[test]
    fn a_failed_recents_mutation_does_not_stop_later_updates() {
        let (sender, observed) = mpsc::channel();
        let writer = RecentsWriter::new(move |mutation| match mutation {
            RecentsMutation::Clear => Err(recently_used_xbel::Error::Update),
            RecentsMutation::Record { paths, .. } => {
                sender.send(paths).unwrap();
                Ok(())
            }
            RecentsMutation::Remove(_) => unreachable!(),
        });
        let failed = writer.submit(RecentsMutation::Clear);
        let next = writer.submit(RecentsMutation::Record {
            paths: vec![PathBuf::from("kept")],
            app_id: "test",
        });
        assert!(matches!(
            failed.recv().unwrap(),
            Err(recently_used_xbel::Error::Update)
        ));
        next.recv().unwrap().unwrap();
        assert_eq!(observed.recv().unwrap(), vec![PathBuf::from("kept")]);
    }
}
