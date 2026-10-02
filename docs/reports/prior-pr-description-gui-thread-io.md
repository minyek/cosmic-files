## Summary

Fixes multi-second UI freezes caused by synchronous filesystem calls on the GUI
thread. On a machine with NTFS (`ntfs-3g`) drives and a CIFS automount, startup
froze for over a minute, deleting a file froze the window for ~12 s, and clicking
a sidebar favorite froze it for 1–2 s.

The Elm/iced architecture requires `init`/`update`/`view` to stay non-blocking —
all I/O belongs in a `Task`. This PR fixes the calls that bypassed that, found
first by profiling the freezes and then by auditing the rest of the codebase.

**Trash empty/full check — the startup culprit.** `Trash::is_empty`/`list` walk
the trash directory of *every* mounted filesystem. This ran while building the
nav bar, both context menus, and the post-operation trash rescan. On a cold
network mount each call blocks for seconds, and the nav bar is rebuilt several
times during startup. The state is now cached and refreshed off-thread at
startup, on the trash watcher, and after operations.

**Navigation.** `Location::normalize()` ran `fs::canonicalize` — a `realpath()`
stat-walk of every path component, resolving every symlink — plus `is_dir()`, on
the GUI thread on *every* navigation, again on result-apply, and on every
tab-title and sort-key computation. `normalize()` is now filesystem-free (lexical
`.`/`..` resolution); the file-vs-directory decision happens off-thread.

**Per-frame calls.** The properties and preview panes re-read image headers and
stat-ed gvfs items on every repaint; context menus stat-ed the right-clicked
entry and parsed the selected `.desktop` file on every repaint; create, rename
and compress dialogs stat-ed the target on every keystroke.

**Per-action calls.** Nav favorites were `is_dir`-stat-ed to pick an icon (they
now use the folder icon directly); `folder_name`, which runs for every tab title
and breadcrumb, stat-ed the path; `delete()` stat-ed each path to choose trash vs
permanent delete; `NotifyEvents` stat-ed changed paths inline. Also MIME sniffing
when opening files, paste payloads, location-bar submit, the mime-app cache,
default-terminal lookup, watcher updates, search-result and breadcrumb item
builds, `recently-used.xbel` writes, and several file-picker dialog calls.

Location-bar submit is the one place this meets a recent master change. `3c9a78a`
parses a typed URI in that same handler to accept a network location from
anywhere. The parse is pure and stays on the GUI thread; only the `exists()` and
`resolve()` calls it feeds are deferred, so `Command::EditLocationSubmit` carries
a flag saying the text was a URI and the off-thread half navigates to it as
entered when nothing on disk resolves.

**Two unbounded caches (separate concern, same audit).**
`LargeImageManager::clear_cache()` freed decoded image handles but left the
per-path display-size, error and generation maps to grow one entry per viewed
image for the session; they are now cleared together. The operation-history maps
grew one entry per finished operation and were never pruned; they are now capped,
and a retry whose entry has been evicted skips gracefully instead of panicking.

**Trade-offs.** Symlinks are no longer resolved on navigation, so the breadcrumb
keeps the path you navigated to and a symlinked favorite highlights correctly;
listings are unchanged, since the kernel follows the link when reading the
directory. A file bookmark in the sidebar shows the folder icon. gvfs mounts show
their path-component name in titles and breadcrumbs rather than the GIO
"friendly" name, and the properties pane no longer shows created/accessed time or
permissions for gvfs files, since resolving those stats the mount. A non-existent
path argument opens an empty tab at that path rather than being silently skipped.
A data/metadata write in an open directory now goes through the same off-thread
rescan as any other change instead of an inline GUI-thread re-stat; the
large-directory follow-up PR narrows that to a targeted per-entry refresh.
A network favorite that points at a file now opens the file, where it previously
logged a warning and did nothing. Because `normalize()` no longer canonicalizes,
a per-folder sort setting saved under a symlink-resolved path is keyed afresh
under the lexical path the user navigates; the old entry stops matching.

**Defects found reviewing this branch, fixed here.** Moving the work off the GUI
thread left sharp edges of its own:

- *The save dialog could overwrite a file with no prompt.* Raising the replace
  prompt from the tab's listing misses a name that exists on disk but does not
  match the picker's active filter, because the listing is filtered before it
  reaches the tab. The target is now stat'ed off-thread, with the path carried in
  the message so a filename edited during the stat cannot be answered with a
  stale result. The listing lookup stays where it is cheap and advisory: the
  per-frame view.
- *Recently-used entries were lost.* Recording a path in `recently-used.xbel` is
  a read-modify-write of the whole file and each path got its own detached
  thread; the detached-open fallback started one per path in a loop, and the
  file picker's Open had its own unserialized copy. One mutex, in one module,
  now serialises every writer.
- *Rename could silently destroy a file.* The dialog's collision hint reads
  only open-tab listings, which cannot see every directory — renaming a search
  result, whose real directory no tab lists, passed unchecked into a bare
  `rename(2)`, which replaces an existing target. The operation itself now
  refuses an occupied name: atomically via `RENAME_NOREPLACE` where the kernel
  and filesystem support it, behind a stat elsewhere. Confirming the dialog
  with the name unchanged stays a no-op.
- *A panicking watch update killed file watching for the session.* The watcher
  moves into the off-thread reconcile; a panic dropped it during unwind and
  nothing could ever restore it. The reconcile now catches the panic, hands the
  watcher back, and re-registers the watch set.
- *The picker swallowed a failed folder creation.* The create runs off-thread
  and its failure was only logged; with the collision hint blind to names the
  active file-type filter hides, the button stays enabled and nothing visible
  happened. The result now returns as a message and the dialog reopens showing
  the error.
- *Archives opened in hash order.* Classifying an all-archive selection rebuilt
  the path list from a hash map. The choice is now a named function that keeps
  the caller's order.
- *Search tabs answered "name already taken".* The check accepted any tab whose
  location matched the directory, including a search tab, which lists matches
  from below it. The file picker had its own copy of the same check and needed
  the same guard, so creating a folder while a search was active could be
  refused for a name only a subdirectory held; both now share
  `Tab::lists_directory`.
- *Startup could open with no tabs* if resolving the launch locations failed.
- *The single-file context menu appeared for multi-selections* in builds without
  the `desktop` feature, where the re-check on the selection count was compiled
  out but its consumer was not.
- *`org.gnome.desktop.Terminal` resolved to `org.gnomeTerminal`* — the default
  terminal lookup stripped `.desktop` anywhere in the id rather than only at the
  end.

## Testing

Reproduced and confirmed fixed on the affected machine: startup is instant,
deletes no longer freeze, and navigating to a slow mount no longer blocks the
window.

`cargo test` passes in the default, `--no-default-features` and `--all-features`
configurations (76 tests, up from 39 on master), including new pins for the
lexical `normalize()` contract, `LocationSelect` navigation, that `folder_name`
does not stat, and both cache bounds. The rename guard is pinned from all four
sides — occupied file, occupied folder, unchanged name, free name — and the
occupied-file pin was checked to fail against the bare rename. The retry an
evicted operation resolves to is pinned from three: evicted, wrong kind, and an
extract carrying the new password. `cargo clippy` adds no new warnings and the
release build succeeds.

The dialog had no test module at all, which is why the overwrite guard was
unpinned; it has one now. `Core::default()` and `Flags` are both constructible,
so the dialog builds in a test like any other type, and the guard test was
checked against the pre-fix code and fails on it.

Where a blocking call was replaced by a cheaper reader, the test asserts the
blocking call is *gone*, not merely that the answer is right: `trash.rs` had no
tests either, and its new module asserts the sidebar icons never call the walk
that visits every mount. `selected_paths_with_dir` is tested with the entries
deleted first, so anything that re-stats them fails. `normalize()` expands a
leading tilde before collapsing `..`, without which `~/..` normalizes to the
empty path.


## AI assistance

Developed with AI assistance (Claude Code), disclosed in the commit messages per
the contribution guidelines. All changes have been reviewed and are understood by
the author.

## Checklist

- [x] I have disclosed use of any AI generated code in my commit messages.
- [x] I understand these changes in full and will be able to respond to review comments.
- [x] My change is accurately described in the commit message.
- [x] My contribution is tested and working as described.
- [x] I have read the [Developer Certificate of Origin](https://developercertificate.org/) and certify my contribution under its conditions.
