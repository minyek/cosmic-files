## Summary

Keep large-directory browsing responsive while preserving file-operation and asynchronous-result correctness. This branch includes the earlier GUI-thread I/O and file-operation fixes; the PR covers the complete diff against upstream.

- Cache sorted order and build list/grid widgets only for visible rows, preserving focus with stable item identities.
- Load local directory child counts on demand, except when sorting by size requires every count. Preserve valid item work across rescans and invalidate it after observed writes.
- Coalesce structural watcher events and refresh modified entries in one background pass. Move filesystem work off the GUI thread; obsolete results cannot replace newer navigation, scans, previews, or picker decisions.
- Build bounded text previews only when needed. Keep original image dimensions separate from cached-thumbnail dimensions.
- Preserve existing files during rename, archive creation, clipboard writes, and picker overwrite confirmation. Surface failures and make their messages copyable.
- Serialize recents mutations, initialize fresh histories safely, publish only current trash state, and keep launcher/sidebar menus correct while metadata loads.

The lockfile updates libcosmic to `34caca138738af92f4b8fc60db52a82812161345` for its native context-menu refresh fix. Rapid-click Wayland testing reproduced a stale-widget panic before that update. X11 menus retain a stable action snapshot until reopened because their overlay layout has a separate lifetime.

## Review guide

Start with `src/scan.rs`, `src/navigation.rs`, and `src/watcher.rs` for request ownership and event classification, then their app/picker consumers. `src/tab.rs` contains sorted-order caching, viewport culling, and item-work invalidation. Recents, trash, MIME, and image-cache collaborators own their shared state. File-preservation changes are in `src/operation/mod.rs` and the picker.

Existing commits retain the development sequence; final repairs and acceptance harnesses are grouped separately. This PR includes the complete dependency stack and does not assume that other PRs have merged.

## Validation

**Final isolated acceptance is pending.** The functional matrix and release results below cover source `2ab787e`; the GUI, storage, and remote checks must be refreshed against that snapshot before posting.

- CI-equivalent minimal/default/all-features matrix: **227 minimal, 241 default, and 241 all-features tests passed**, no failures or ignored tests.
- Canonical `just build-release --locked --offline -j12`: application, desktop applet, and executable thumbnailer built successfully.
- Private X11 and nested Wayland: launcher menus, `320×180` image details, 20,000-file list/grid traversal, watcher refreshes, and rapid press/release sequences.
- Real file picker: traverse 20,000 files, observe watcher updates, and return the final sorted selection.
- Real XBEL record/clear/remove and trash delete/list/restore under an isolated profile. Failed writes leave no partial history; normal initialization recovers.
- Live loopback FTP/GVFS: initial, nested, create, and remove scans agree across both URI entry points and the mounted FUSE path.
- Formatting, shell syntax, Python compilation, and diff checks passed.

See `docs/reports/2026-09-22-branch-acceptance.md` and its artifacts for commands and evidence. Exclusive performance measurements remain pending, so this PR makes no fresh numerical speedup claim.

## Trade-offs and limits

A same-size rewrite within filesystem timestamp granularity that is not observed by the watcher can retain cached results; a test documents that fingerprint limitation. Targeted metadata refresh does not reclassify MIME types. Local directories enumerated through GVFS still count children during scanning; remote directory counts remain unknown. A pending X11 launcher menu receives completed actions on its next opening.

Software-rendered GUI checks establish behavior, not physical-GPU latency. Loopback FTP does not cover every protocol or WAN condition. GUI acceptance used the development build; release binaries were separately compiled.

## AI assistance and contributor checklist

Developed with AI assistance from Claude Code and Codex. The PR author must personally review the changes and complete the understanding and DCO attestations before posting.

- [ ] I have disclosed use of any AI generated code in my commit messages.
- [ ] I understand these changes in full and will be able to respond to review comments.
- [ ] My change is accurately described in the commit message.
- [ ] My contribution is tested and working as described.
- [ ] I have read the [Developer Certificate of Origin](https://developercertificate.org/) and certify my contribution under its conditions.
