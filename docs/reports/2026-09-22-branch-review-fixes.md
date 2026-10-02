# Branch review repairs

The six accepted findings are repaired in the large-directory-scroll worktree. No changes have been pushed or deployed.

| Finding | Repair and regression coverage |
| --- | --- |
| Stale app and picker scans | Each request owns its completion and pending watcher work. Reverse completion and failed current/obsolete requests exercise both consumers. |
| Recents mutation order | One FIFO worker handles record, clear, and remove. Isolated sinks verify submission order and continuation after an error. |
| Stale trash state | Only the latest requested scan can publish its result. Local cache tests reject obsolete and duplicate completions. |
| Missing image details | Accepted thumbnail work supplies original dimensions; obsolete work cannot update a replacement item. A real PNG exercises decoding. Cached thumbnails retain unknown original dimensions as unknown. |
| Incorrect launcher menus | Only a selected regular desktop file with a successful current parse gets launcher actions. Tests cover directories, special files, pending/failed parses, valid empty actions, replaced items, and reversed completions for different selections. |
| Unknown sidebar metadata | File/folder actions wait for metadata. Preview and custom actions with selection `Any` remain available; virtual locations retain directory navigation. |

## Decisions

Request identities allow a new location to finish without waiting for an old slow scan; serializing all scans would retain that delay. Recents instead require serialization because mutations rewrite one shared history store. Ordinary menu actions remain useful while parsing is pending; treating a filename suffix as proof of a launcher would admit invalid files. Cached thumbnail bucket sizes are unsuitable as original dimensions, so unknown dimensions remain absent even when a cached preview exists.

The independent review identified four additional boundary cases in the initial repairs: fabricated cached dimensions, obsolete launcher results clearing another menu's cache, special-file candidates, and hidden `Any` sidebar actions. All four were accepted and repaired. Each corresponding assertion failed before its repair and passed afterward. Placement and changed-comment checks found no additional required changes.

## Verification

- Four regressions replayed against the pre-repair source failed as expected: stale app scan, stale picker scan, missing local-image dimensions, and desktop-suffixed directory classification.
- The first repaired snapshot passed 36 focused tests and all three CI configurations, with 200 tests passing in each configuration.
- The four independent-review regression cases failed before the follow-up repairs and then passed together.
- Final minimal/default/all-features matrix: 203 tests passed in each configuration, with zero failures or ignored tests; the composite gate exited successfully. Commands were `cargo test --locked --offline --no-default-features`, `cargo test --locked --offline`, and `cargo test --locked --offline --all-features`.
- `git diff --check` passes. The minimal-feature compiler warnings match the pre-repair source; the first default/all-features runs emitted no compiler warnings.

## Impact and limits

The repairs affect listings, image details, launcher/sidebar menus, recents ordering, and trash icon publication in this branch. The tests use temporary filesystem fixtures, injected recents sinks, and local trash cache instances; no real recents history or trash contents were changed. The cached-thumbnail fixture removes only its own generated cache entry. No deployed binaries, shipped artifacts, training data, or existing data caches require regeneration. No unrelated jobs were stopped.

The final matrix supersedes the earlier functional results after the follow-up repairs. Subsequent real storage, GUI, remote-mount, and performance validation is recorded in the [acceptance report](2026-09-22-branch-acceptance.md), including additional defects discovered during that work. Its newer results supersede this report's original testing limits.
