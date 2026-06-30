# Keeping filesystem I/O off the GUI thread — follow-up fixes

This branch (`fix-gui-thread-blocking-remaining`) extends the original fix
(commit `058457a`) that moved startup and post-delete filesystem work off the
GUI thread. An audit of the remaining code found ~15 more places where the
`cosmic`/`iced` GUI thread (the `view`/`update`/`init` handlers and everything
they call synchronously) performed blocking filesystem calls. On systems with
slow or network-backed mounts (NTFS via ntfs-3g, CIFS/NFS automounts, cold gvfs
mounts) each of these freezes the UI until the mount responds.

The Elm/iced architecture requires `view`, the body of `update`, and `init` to
stay non-blocking; all I/O belongs in `Task`s (`spawn_blocking`) or must read a
value cached off-thread. The fixes below follow three patterns:

- **Read a cached value** instead of re-stating (the item/tab already holds the
  metadata from its off-thread scan).
- **Derive from in-memory data** (e.g. an existence check from the already-scanned
  directory listing) instead of hitting the filesystem.
- **Move the work into a `Task`** (`spawn_blocking`) and finish in a follow-up
  message, mirroring the existing `delete` → `delete_classified` split.

The fixes are grouped by how often the blocking call fired.

---

## Tier 1 — render path (fired every frame)

These ran inside `view`/`dialog`/`nav_bar` builders, so they re-hit the
filesystem on **every repaint** (mouse move, keystroke, thumbnail tick) while the
relevant UI was shown — the worst case.

### R1 — Properties pane re-read the image header every frame
- **Was:** `Item::preview_view` called `image::image_dimensions(path)` (opens the
  file and reads its header) on every render while the preview/properties pane was
  open, for *any* selected file.
- **Now:** reads the cached `Item::image_dimensions`, computed off-thread at scan
  time. The redundant per-frame file read is gone.
- **Impact:** no behavior change; dimensions still shown for images. Pure win.

### R2 — Properties/multi-select pane stat-ed gvfs (network) items every frame
- **Was:** `Item::file_metadata()` did a synchronous `fs::metadata` for gvfs items;
  the single- and multi-item preview panes call it every frame.
- **Now:** renamed to `Item::cached_metadata()`, which only returns the metadata
  already cached for local paths and never does I/O. For gvfs items the pane
  renders size and modified time from the cached directory listing instead.
- **Impact:** local-file properties are unchanged. For **network files**, the
  properties pane no longer shows *created*/*accessed* time or *permissions*
  (those require a network stat); size and modified time are still shown. This is
  a deliberate trade-off — the alternative was a per-frame network freeze.

### R3 — Sidebar context menu stat-ed the right-clicked entry every frame
- **Was:** `nav_context_menu` stat-ed the right-clicked sidebar path (`is_file`,
  `is_dir`) and read `recently-used.xbel`; `nav_bar()` rebuilds this menu every
  repaint, so once a sidebar context menu was open it re-stat-ed per frame.
- **Now:** those facts are computed off-thread when the menu opens
  (`Message::NavBarContext` → `NavContextInfoLoaded`) and cached in
  `NavContextInfo`; the menu renders from the cache.
- **Impact:** for a frame or two after opening, the menu renders from defaults
  (e.g. the dir-style entries) before the cached facts arrive. Imperceptible on
  fast mounts; on slow mounts it replaces a per-frame freeze.

### R4 — Create/rename/compress dialogs stat-ed the target every keystroke
- **Was:** the New file/folder, Rename, and Compress dialogs (and the file
  picker's New Folder) computed `parent.join(name)` and called
  `path.exists()`/`is_dir()` inside the dialog builder — re-stating on **every
  keystroke** while typing a name.
- **Now:** existence is derived from the already-scanned tab listing for the target
  directory (`App::existing_entry_in_dir` / `Dialog::existing_entry_in_dir`, both
  delegating to `Tab::find_entry_is_dir`); no I/O.
- **Impact:** the "already exists" hint is unchanged for the normal case (the
  dialog operates on a currently-open tab's directory). If no open tab is showing
  the target directory, the advisory hint is skipped; the create/rename operation
  itself remains the authoritative guard against collisions.

### R5 — Context menu parsed the selected `.desktop` file every frame
- **Was:** `menu::context_menu` called `load_desktop_file` (reads + parses the
  `.desktop` file) on every frame the menu was open over a lone `.desktop`
  selection.
- **Now:** the entry is parsed once off-thread when the menu opens
  (`Command::ParseDesktopEntry` → `Message::DesktopEntryParsed`) and the action
  names are cached on the tab; the menu renders from the cache. The menu *layout*
  is still chosen from the file extension (no I/O), so there is no layout flicker —
  only the per-app action items appear a frame or two later.
- **Impact:** none beyond the brief delay before custom desktop actions appear.

---

## Tier 2 — inline in `update` (fired once per user action)

Same bug class, lower frequency: these blocked on a discrete action rather than
every frame.

### U1 — Opening files sniffed each file's MIME type inline
- **Was:** `open_file` ran `mime_for_path` (which does `fs::metadata` + `File::open`
  + reads magic bytes) for **each** selected path before launching. This is the
  biggest slow-mount exposure in this tier because it touches arbitrary user files.
- **Now:** MIME classification runs off-thread (`spawn_blocking`); launching
  happens in `Message::OpenFileClassified` (`launch_paths`), mirroring the
  `delete`/`delete_classified` split.

### U2 — Pasting image/video/text wrote the payload inline
- **Was:** `copy_unique_path` (a stat loop) + `fs::write` of the (possibly
  multi-MB) payload ran inline in the paste handlers.
- **Now:** both run off-thread in `spawn_blocking`.

### U3 — Clicking a sidebar favorite stat-ed it inline
- **Was:** `on_nav_select` called `try_exists()` (and a gvfs mount-needed check)
  inline; a dead/slow network favorite blocked the click.
- **Now:** existence (and dir-vs-file) is computed off-thread and the branching —
  mount / "favorite not found" dialog / open file / navigate — happens in
  `Message::NavSelectChecked`. A favorited *file* now routes to `OpenFile` from
  there, so `Tab::update`'s `Message::Location` handler no longer needs to stat
  `is_dir` (every sender passes a directory).

### U4 — The mime-app cache loaded and reloaded on the GUI thread
- **Was:** `MimeAppCache::new()`/`reload()` scan every desktop entry; this ran
  inline at `init` and again whenever `mimeapps.list` changed (via its watcher).
- **Now:** built in `spawn_blocking` (`App::refresh_mime_app_cache`), swapped in via
  `Message::MimeAppCacheLoaded`. `init` starts from `MimeAppCache::empty()`.
- **Impact:** for a brief window at startup the "open with" lists and cached
  default-app fast path are empty; they populate within a frame or two (file
  opening still works via the fallback path meanwhile).

### U5 — Setting the default "open with" wrote `mimeapps.list` inline
- **Was:** `set_default` read + wrote `mimeapps.list` and reloaded the whole cache
  inline (already flagged with a "this will block" TODO).
- **Now:** the write runs off-thread (`MimeAppCache::write_default`), then the
  cache reloads off-thread.

### U6 — Resolving the default terminal spawned `xdg-mime` inline
- **Was:** `MimeAppCache::terminal()` shelled out to `xdg-mime` and blocked on its
  output every time the terminal was opened.
- **Now:** the default terminal is resolved once during the off-thread `reload()`
  and cached in `default_terminal_id`; `terminal()` reads the cache.

### U7 — Submitting the location bar stat-ed and canonicalized inline
- **Was:** `EditLocationSubmit` called `path.exists()` and `resolve()` (which
  canonicalizes) on the GUI thread when Enter was pressed.
- **Now:** both run off-thread via `Command::EditLocationSubmit`; the resolved
  location is navigated to via a follow-up message.

### U8 — Re-stating already-scanned items
- **Was:** Open-in-new-tab/window, Rename, the preview Open button, and the
  Message::Open/Location handlers re-stat-ed `is_dir` on paths that were already
  scanned. Drag-drop onto a folder ran `fs::read_dir(target)` to filter
  already-present files.
- **Now:** all decide from cached metadata (`Tab::selected_paths_with_dir`,
  `item.metadata.is_dir()`). The drop filter compares parent paths instead of
  reading the target directory — equivalent for real paths, with no I/O.

### U9 — Updating the file watcher stat-ed paths inline
- **Was:** `update_watcher` called `watch()`/`unwatch()` (which call
  `inotify_add_watch`, resolving the path) inline; this blocks on a cold mount.
- **Now:** runs off-thread; the debouncer is restored via `Message::WatcherUpdated`.
  A `watcher_update_pending` flag reconciles the watched-path set if a tab changed
  while the update was in flight, so no watch update is lost.

---

## Tier 3 — one-shot at startup / rare action

### L1 — `init` stat-ed launch path arguments
- **Was:** `init` called `path.is_file()` on each launch argument (to open a file
  argument's parent with the file selected), blocking startup on a slow mount.
- **Now:** launch locations are resolved off-thread (`Message::OpenLaunchLocations`);
  the default-tab fallback is gated on whether any launch locations were passed so
  startup behavior is otherwise unchanged.

### L2 — Shift-all permissions stat-ed gvfs items
- **Was:** the shift-all-permissions path read each selected item's mode via the
  blocking `file_metadata()` (a network stat for gvfs items).
- **Now:** reads the mode from cached metadata only; gvfs items are skipped (a
  chmod over the network would not apply anyway) rather than stat-ed.

---

## Verification

- `cargo test`, `cargo test --no-default-features`, and `cargo test --all-features`
  all pass (43 tests).
- `cargo build --release` succeeds with no warnings in the default/all-features
  configurations.
- A unit test (`cached_metadata_reflects_scan_without_restat`) pins the contract
  that `Item::cached_metadata()` returns scan-time metadata and never re-stats
  (it keeps returning the cached value after the backing files are removed).

## Summary of intentional behavior changes

| Area | Change | Why it's acceptable |
|------|--------|---------------------|
| gvfs properties pane | No created/accessed time or permissions for network files (size + modified still shown) | Those need a per-frame network stat; size + modified come from the cached listing |
| gvfs shift-all permissions | gvfs items skipped | chmod over the network does not apply anyway |
| mime-app "open with" lists | Empty for a brief moment at startup | Populated off-thread within a frame or two; opening still works via fallback |
| Sidebar/desktop context menus | Custom actions appear a frame or two after opening | Avoids a per-frame parse/stat; layout is stable |
| Create/rename "already exists" hint | Derived from the open tab's listing; skipped if no tab shows the target dir | The create/rename operation remains the authoritative collision guard |
