use std::path::PathBuf;

use notify_debouncer_full::notify::{self, Watcher};
use notify_debouncer_full::{Debouncer, FileIdCache};
use rustc_hash::FxHashSet;

pub(crate) fn reconcile<W: Watcher, C: FileIdCache>(
    mut watcher: Debouncer<W, C>,
    old_paths: FxHashSet<PathBuf>,
    new_paths: FxHashSet<PathBuf>,
) -> (Debouncer<W, C>, FxHashSet<PathBuf>) {
    let mut registered = old_paths.clone();
    let reconciled = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        for path in &old_paths {
            if !new_paths.contains(path) {
                match watcher.unwatch(path) {
                    Ok(()) => {
                        registered.remove(path);
                        log::debug!("unwatching {}", path.display());
                    }
                    Err(err) if matches!(err.kind, notify::ErrorKind::WatchNotFound) => {
                        registered.remove(path);
                    }
                    Err(err) => {
                        log::debug!("failed to unwatch {}: {}", path.display(), err)
                    }
                }
            }
        }

        for path in &new_paths {
            if !old_paths.contains(path) {
                match watcher.watch(path, notify::RecursiveMode::NonRecursive) {
                    Ok(()) => {
                        registered.insert(path.clone());
                        log::debug!("watching {}", path.display());
                    }
                    Err(err) => {
                        log::debug!("failed to watch {}: {}", path.display(), err)
                    }
                }
            }
        }
    }));

    match reconciled {
        Ok(()) => (watcher, registered),
        Err(_) => {
            log::error!("watch update panicked; rebuilding the watch set");
            (watcher, FxHashSet::default())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::notify;

    struct PanicOnceWatcher {
        has_panicked: bool,
    }

    impl notify::Watcher for PanicOnceWatcher {
        fn new<F: notify::EventHandler>(_: F, _: notify::Config) -> notify::Result<Self> {
            Ok(Self {
                has_panicked: false,
            })
        }

        fn watch(&mut self, _: &std::path::Path, _: notify::RecursiveMode) -> notify::Result<()> {
            if !std::mem::replace(&mut self.has_panicked, true) {
                panic!("injected watch failure");
            }
            Ok(())
        }

        fn unwatch(&mut self, _: &std::path::Path) -> notify::Result<()> {
            Ok(())
        }

        fn kind() -> notify::WatcherKind {
            notify::WatcherKind::NullWatcher
        }
    }

    #[test]
    fn panicking_watch_preserves_the_watcher_for_a_later_request() -> std::io::Result<()> {
        let directory = tempfile::tempdir()?;
        let old_path = directory.path().join("old");
        let new_path = directory.path().join("new");
        std::fs::create_dir(&old_path)?;
        std::fs::create_dir(&new_path)?;
        let watcher = notify_debouncer_full::new_debouncer_opt::<
            _,
            PanicOnceWatcher,
            notify_debouncer_full::NoCache,
        >(
            std::time::Duration::from_millis(100),
            None,
            |_events| {},
            notify_debouncer_full::NoCache::new(),
            notify::Config::default(),
        )
        .unwrap();
        let requested = [old_path.clone(), new_path.clone()].into_iter().collect();
        let (watcher, registered) =
            super::reconcile(watcher, [old_path.clone()].into_iter().collect(), requested);
        assert!(registered.is_empty());
        let requested = [old_path, new_path]
            .into_iter()
            .collect::<super::FxHashSet<_>>();
        let (_watcher, registered) = super::reconcile(watcher, registered, requested.clone());
        assert_eq!(registered, requested);
        Ok(())
    }

    #[test]
    fn failed_watch_can_be_retried_after_the_directory_appears() -> std::io::Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("late");
        let watcher = notify_debouncer_full::new_debouncer(
            std::time::Duration::from_millis(100),
            None,
            |_events| {},
        )
        .unwrap();
        let requested = [path.clone()].into_iter().collect();
        let (watcher, registered) = super::reconcile(watcher, Default::default(), requested);
        assert!(registered.is_empty());
        std::fs::create_dir(&path)?;
        let requested = [path.clone()].into_iter().collect();
        let (_watcher, registered) = super::reconcile(watcher, registered, requested);
        assert!(registered.contains(&path));
        Ok(())
    }
}
