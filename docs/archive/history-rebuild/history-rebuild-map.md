# History rebuild: commit map

The series built by the [history rebuild](history-rebuild-spec.md) ([plan](history-rebuild-plan.md)):
upstream `e03001c`, then 50 code commits (one per issue, or per group of issues that could not be
separated), then four fork-only commits and the commit that archives this. Commits are named by
position and issue; the files in `results/` name them by their SHAs before the `Depends on:` lines
were added, which `results/rewrite-map.txt` maps to the series SHAs. Inputs are in `map/`: the
hunk-level inventory (`assign/*.tsv`: diff file, hunk, line range, issues, kind, note) against the
diffs in `map/diffs/`, the order (`order-16.txt`), earlier versions of lines a later issue rewrites
(`variants.tsv`, `keep-variants.tsv`), dependencies (`semantic-deps.tsv`) and the issue texts the
messages were built from. Checker results are in `results/`, and the scripts in `tooling/`.

## Commits

| # | Issues | Subject | Depends on |
|---|---|---|---|
| 1 | #1 | Look up each owner and group once per frame in the multi-selection pane | nothing beyond upstream |
| 2 | #29 | Build search results in the search worker | nothing beyond upstream |
| 3 | #35 | Run at most one rescan per folder, with one queued follow-up | nothing beyond upstream |
| 4 | #3, #4, #5, #15 | Rescan only what changed and keep computed item state | #1; #35 |
| 5 | #6 | Cache the sort order and rebuild it only when its inputs change | #29; #3, #4, #5, #15 |
| 6 | #39 | Clear the gallery's per-image state together | #3, #4, #5, #15 |
| 7 | #2 | Show a badge on symlinked folders in list and grid views | #3, #4, #5, #15 |
| 8 | #48 | Read the dimensions of a trashed image from the trashed copy | #6; #2 |
| 9 | #7 | Build list and grid rows only for the visible range | #48 |
| 10 | #8 | Build text previews only where one is displayed | #7 |
| 11 | #9 | Show a large folder's entries while it is still being scanned | #6 |
| 12 | #11 | Draw icons without waiting on a slow file read | nothing beyond upstream |
| 13 | #10, #49 | Type files from their names first, and never type unknown names as empty | #11 |
| 14 | #51 | Offer the launcher menu only for a single .desktop file | nothing beyond upstream |
| 15 | #19, #53 | Parse a .desktop file once per context menu and run the action it showed | #3, #4, #5, #15; #51 |
| 16 | #14 | Classify deletes off the UI thread | nothing beyond upstream |
| 17 | #54, #55 | Make every Delete permanently menu entry delete permanently | #2; #19, #53; #14 |
| 18 | #28 | Resolve launch locations off the UI thread | #35 |
| 19 | #43 | Stage compressed archives and keep an existing archive | nothing beyond upstream |
| 20 | #26 | Reuse scanned metadata instead of statting items again | #28 |
| 21 | #13, #25 | Resolve navigation and typed locations off the UI thread | #8; #9; #54, #55; #26 |
| 22 | #20 | Sniff the types of opened files off the UI thread | #3, #4, #5, #15; #28 |
| 23 | #22 | Check network favourites off the UI thread | #13, #25; #20 |
| 24 | #32, #37, #46, #33 | Record recents, defaults and picker results off the UI thread, in order | #43; #22 |
| 25 | #27 | Update the file watcher off the UI thread | #32, #37, #46, #33 |
| 26 | #36 | Watch a folder again after its watch failed | #27 |
| 27 | #31 | Parse a .desktop file off the UI thread when launching it | nothing beyond upstream |
| 28 | #18 | Stop the New, Rename and Compress dialogs statting on every keystroke | #32, #37, #46, #33 |
| 29 | #47 | Show an error when New folder fails in the file picker | #27; #18 |
| 30 | #12 | Check the Trash state off the UI thread | #13, #25 |
| 31 | #16 | Read cached metadata in the properties pane | #13, #25 |
| 32 | #17 | Stop the sidebar context menu statting its entry on every frame | #36; #12 |
| 33 | #21 | Write pasted clipboard data off the UI thread without a name race | #20; #12 |
| 34 | #23 | Build and save the Open-with application cache off the UI thread | #11; #17; #21 |
| 35 | #52 | Keep a .desktop segment inside a default terminal id | #23 |
| 36 | #24 | Cache the default terminal instead of asking xdg-mime each time | #52 |
| 37 | #30 | Build the item for sidebar Open with and Preview off the UI thread | #23 |
| 38 | #38 | Cap the operation history | #20 |
| 39 | #40 | Test that a failed move to Trash is reported | #43 |
| 40 | #57 | Fix pausing, resuming and cancelling file operations | #40 |
| 41 | #44 | Report a failure while finishing an archive | #43 |
| 42 | #34 | Build the overwrite dialog's items off the async runtime | nothing beyond upstream |
| 43 | #42 | Refuse to rename over a file that appeared under the new name | #40; #44; #34 |
| 44 | #56 | Update compio so an idle runtime is not counted as IO wait | #57; #42 |
| 45 | #41 | Make the Failed operations text selectable and copyable | #21; #38 |
| 46 | #45 | Include file names that are not valid UTF-8 in archives | #42 |
| 47 | #50 | Type files in GVFS URI listings as files | #2; #10, #49 |
| 48 | #59 | Name the SVG preview size limit | #8 |
| 49 | #60 | Log rather than panic when the notification task fails to run | nothing beyond upstream |
| 50 | #61 | Remove the unused Tab::items_opt_mut | nothing beyond upstream |
| 51 | — | Add the fork's acceptance tooling | (fork-only) |
| 52 | — | Add the fork's RPM packaging | (fork-only) |
| 53 | — | Add the fork's acceptance evidence | (fork-only) |
| 54 | — | Add the fork's documentation | (fork-only) |

Dependencies come first; otherwise the order is the one the build trials kept, not issue number
(for example #31 and #47 precede #12).

## Groups (D2)

Issues share a commit only where separating them would need code that exists in no commit, old
or new:

- #3, #4, #5, #15: #3's entry refresh accepts a result only for #5's current work id, #4's
  child-count test uses that work id, #15's stale-dimensions test needs it too, and #5 carries
  #4's child count and #15's dimensions across rescans; the old standalone #3 (`e857afcf`) is built
  on #6's sort cache.
- #10, #49 (M1): #10 alone fails clippy's collapsible-if lint; the nesting exists only for #49's
  remote branch, and no old commit has one without the other.
- #19, #53: `DesktopAction` and `DesktopEntryActions` were created together (old commit `1270092`).
- #54, #55 (M3): #55 rewrote #54's delete entry, its callers and its test; #54's old form
  (`79a17e1`) does not fit the final structure without new glue code.
- #13, #25: navigation requests and the typed-location resolver use each other.
- #32, #37, #46, #33 (M2, M4): `persistence::flush` was created draining both writers; #37's fix
  lives in #32's module; the picker's Open and Save handlers interleave #32's and #33's lines.

## Listed lines (AC 5)

36 added lines do not survive to the final tree (`results/non-surviving-lines.tsv`). Fifteen are
mechanical (a re-wrap, or a `use` or comma list that grew). Twenty-one are issue-driven: an
earlier issue's own form, taken from an old commit or from upstream, that a later issue rewrites.

| Path | Rewritten by | Why |
|---|---|---|
| `src/app.rs` | #3 | #35's form from 3d7f065; #3 classifies events and refreshes changed entries |
| `src/tab.rs` | #30 | #30 builds the item in a task, so tab.rs stops importing item_from_path |
| `src/menu.rs` | #19 | #51's form from 6ea68d5; #19 replaces it with one parse per context menu |
| `src/app.rs` | #55 | #14's form from 66dac3f; #55 seeds the dialog list with the caller's paths |
| `src/app.rs` | #55 | #14's form from 66dac3f keeps upstream's TODO; #55 removes it (d70f661) |
| `src/app.rs` | #55 | #14's form (upstream's metadata check); #55 classifies with symlink_metadata |
| `src/app.rs` | #55 | #14's form (upstream's FsKind check, re-indented); #55 decides by item::can_trash |
| `src/app.rs` | #55 | #14's form from 66dac3f; #55 seeds the dialog list with the caller's paths |
| `src/app.rs` | #13 | #13 calls crate::item::folder_icon_symbolic path-qualified for favourites |
| `src/app.rs` | #30 | #30 builds the item in a task, so app.rs stops importing item_from_path |
| `src/app.rs` | #36 | #27's form from f80f57df (app.rs also a4a0c806); #36 adds the requested set |
| `src/app.rs` | #36 | #27's form from f80f57df (app.rs also a4a0c806); #36 adds the requested set |
| `src/app.rs` | #36 | #27's form from f80f57df (app.rs also a4a0c806); #36 compares against the requested set |
| `src/dialog.rs` | #36 | #27's form from f80f57df (app.rs also a4a0c806); #36 adds the requested set |
| `src/dialog.rs` | #36 | #27's form from f80f57df (app.rs also a4a0c806); #36 adds the requested set |
| `src/dialog.rs` | #36 | #27's form from f80f57df (app.rs also a4a0c806); #36 compares against the requested set |
| `src/watcher.rs` | #36 | #27's form from f80f57df (app.rs also a4a0c806); #36 returns the registered set |
| `src/app.rs` | #17 | #17 restructures the sidebar menu, moving and re-indenting the empty-Trash entry |
| `src/operation/mod.rs` | #56 | #34's compio 0.18 form from 66dac3f; compio 0.19 reports the JoinError, which #56 logs |
| `src/operation/mod.rs` | #56 | #34's compio 0.18 form from 66dac3f; compio 0.19 reports the JoinError, which #56 logs |
| `src/operation/mod.rs` | #56 | #42's compio 0.18 form from 66dac3f; #56 formats the JoinError |

## Subsumed issues

None: every issue with code has lines of its own in its commit.
