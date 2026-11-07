## Summary

Seven independent pre-existing bugs and improvements, each root-caused and fixed
separately, one commit apiece, ordered so related ones read together: two trash
fixes, the dialog affordance, then the scanning fixes. Five follow-up commits
close the branch out — a rustfmt pass, the tests those two scanning changes and
the dialog affordance turned out to need, and comment corrections from two
review rounds. None depend on the `gui-thread` I/O work; all apply, build and
test cleanly on plain `master`.

**Delete reports success but nothing is deleted.** `Operation::Delete` checked
only the outer `spawn_blocking` join error and discarded `trash::delete`'s own
`Result`. When the trash operation itself failed — target on a different
filesystem from `~/.local/share/Trash`, or a read-only/network/NTFS mount — the
error was swallowed and the UI reported success while the file was still there.
The inner `Result` is now checked too, mirroring the already-correct
`PermanentlyDelete` arm, so the failure surfaces with its filesystem reason.

**A trashed image's size is read from a path that no longer holds it.**
`item_from_trash_entry` took image dimensions from the entry's *original* path —
where the file sat before deletion — so the read always missed and no trashed
image ever reported a size. Once something new takes that name, the read
succeeds against an unrelated file and reports its dimensions instead. The MIME
lookup on the line above already avoids the original path for exactly this
reason; the dimensions now come from the same trash location the item already
uses for its `location_opt`.

**Error text in the failed-operations dialog is not selectable.** The dialog
rendered the error through the dialog body, so a failed copy/move/delete could
not be copied out to report it. Adds a "Copy to clipboard" action to both the
single- and multi-failure dialogs, writing the same string the body renders.
Both dialogs build that string through free functions over the failed-operations
map, so what is shown and what is copied stay one text under test.

**Watcher rescans pile up unbounded.** Every debounced watcher batch spawned a
new full-directory scan with no check for one already running. A sustained burst
of events queued scans faster than they completed, and since each holds its own
listing until done, the backlog became unbounded resident memory — observed at
~189 GB, with the process unkillable except by `SIGKILL` — while the I/O
contention made each scan slower still. A per-tab in-flight guard now coalesces a
burst into at most one follow-up scan. The trash and recents reloads, driven by
their own debounced watchers, go through the same guard — a trash listing walks
every mount, so its scans pile up the same way under sustained trash churn.

**Files without an extension are typed as empty on remote mounts and in the
trash.** Guessing a MIME type from a file name alone gives `xdg-mime` no
contents to inspect, and it matches *empty* data against the magic database
before giving up — `application/x-zerosize` wins at full confidence. So every
file whose name carries no glob was reported as an empty file, and drew the icon
for one, however much it held. This reached every entry on a network or gvfs
mount and every entry in the trash, all of which type entries by name. The
name's answer is now taken only where a glob actually claims it, falling back to
`mime_guess` otherwise, so an unrecognised name reads as unknown rather than as
empty; a file that really is empty still types as empty.

**Double-clicking a folder on a slow disk visibly freezes the tab.** `scan_path`
read each entry's metadata, sniffed its MIME type and re-read subdirectories to
count children strictly one entry at a time, so nothing rendered until the whole
listing was built. Entries are now collected first and processed concurrently
with rayon; ordering and behaviour are otherwise identical.

**Every file in a directory is opened and read just to type it.** Sniffing a
MIME type from a *path* makes `xdg-mime` open the file and read as many bytes as
the longest rule in the magic database — 18730 on a current `shared-mime-info`.
A 481-file directory on a rotating disk spent about five seconds in those reads
before anything appeared, one seek per entry, and paid it again on every visit
that outlived the page cache. Where exactly one glob claims the name,
`xdg-mime` returns that type without ever consulting the contents, so the read
cannot change the answer: the name is now consulted first in that case, and the
read is kept for the names that need it. Measured over the same 481-file
directory, cold, the per-entry typing drops from ~5s to ~17ms; a directory of
extensionless files still sniffs every one.

**Reviewing this branch corrected three claims in the MIME work's comments**:
empty files are typed from their empty contents rather than from their metadata,
one remote caller does pass metadata, and the special types are not returned as
uncertain -- `application/x-zerosize` arrives at full confidence, which is the
trap the change exists to close. A test named for eliding a read only observes
that an unambiguous name outranks the contents, so it is named for that.

## Testing

`cargo test` passes in the default, `--no-default-features` and `--all-features`
configurations, matching CI (53 tests, up from 39 on master), including new
regression tests for the swallowed trash error, the rescan coalescing guard, a
trashed image's dimensions being read from the file that still exists, and an
extensionless file no longer typing as empty. The MIME change is pinned from
both sides: that an unambiguous name still decides the type when the contents
say otherwise, and that a directory named like a file is still a directory, so
the read cannot be skipped where it is load-bearing.

A third case pins the inverse: locally, a name no glob claims must still fall
through to the contents, so a PNG named `payload` is `image/png` and the
shortcut cannot be made unconditional. The existing `scan_path` tests pass
unchanged against the parallelised implementation, and a new tab-level test pins
that the scanned listing matches the directory's contents exactly — it exercises
the previously-unused `assert_eq_tab_path_contents` helper, which also clears
one existing clippy warning. Reading `.hidden` had to move ahead of the entries
because they are now typed in parallel, and nothing covered `.hidden` at all, so
a test pins that a name it lists is marked hidden. `cargo clippy` adds no new
warnings and the release build succeeds.

The first two were reproduced by hand: deleting onto a filesystem without a usable
trash now reports the failure, and the dialog's error text can be copied.

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
