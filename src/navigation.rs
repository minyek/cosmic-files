use std::path::PathBuf;

use cosmic::widget;

use crate::tab::{self, Location};

#[derive(Default)]
pub(crate) struct PendingRequest {
    request: Option<widget::Id>,
}

impl PendingRequest {
    pub(crate) fn begin(&mut self) -> widget::Id {
        let request = widget::Id::unique();
        self.request = Some(request.clone());
        request
    }

    pub(crate) fn invalidate(&mut self) {
        self.request = None;
    }

    pub(crate) fn is_current(&self, request: &widget::Id) -> bool {
        self.request.as_ref() == Some(request)
    }

    pub(crate) fn finish(&mut self, request: &widget::Id) -> bool {
        if !self.is_current(request) {
            return false;
        }
        self.invalidate();
        true
    }
}

pub(crate) fn resolve_edit_location(
    mut edit_location: tab::EditLocation,
    typed_uri: bool,
) -> Option<(Location, Option<PathBuf>)> {
    if edit_location.selected.is_none()
        && edit_location
            .completions
            .as_ref()
            .is_some_and(|items| !items.is_empty())
        && edit_location
            .location
            .path_opt()
            .is_some_and(|path| !path.exists())
    {
        edit_location.selected = Some(0);
    }
    let location = edit_location
        .resolve()
        .or_else(|| typed_uri.then_some(edit_location.location))?;
    let location = match &location {
        Location::Path(path) => {
            let path = path.components().as_path();
            location.with_path(path.canonicalize().unwrap_or_else(|_| path.to_path_buf()))
        }
        _ => location,
    };
    let redirect = location
        .path_opt()
        .map(|path| path.components().as_path().to_path_buf())
        .filter(|path| path.is_file())
        .and_then(|path| Some((path.parent()?.to_path_buf(), path)));
    Some(match redirect {
        Some((parent, file)) => (location.with_path(parent), Some(file)),
        None => (location, None),
    })
}

#[cfg(test)]
mod tests {
    use super::PendingRequest;

    #[test]
    fn the_latest_navigation_request_wins() {
        let mut pending = PendingRequest::default();
        let first = pending.begin();
        let second = pending.begin();
        assert!(!pending.finish(&first));
        assert!(pending.finish(&second));
        assert!(!pending.finish(&second));
    }

    #[test]
    fn navigation_cancelled_by_another_action_cannot_finish() {
        let mut pending = PendingRequest::default();
        let request = pending.begin();
        pending.invalidate();
        assert!(!pending.finish(&request));
    }

    #[cfg(unix)]
    #[test]
    fn a_parent_after_a_symlink_resolves_against_its_target() -> std::io::Result<()> {
        use crate::tab::Location;
        let dir = tempfile::tempdir()?;
        let target = dir.path().join("target/child");
        std::fs::create_dir_all(&target)?;
        std::os::unix::fs::symlink(&target, dir.path().join("link"))?;
        let typed = Location::Path(dir.path().join("link/..")).normalize();
        let (resolved, selected) = super::resolve_edit_location(typed.into(), false).unwrap();
        assert_eq!(resolved.path_opt(), Some(&dir.path().join("target")));
        assert_eq!(selected, None);
        Ok(())
    }
}
