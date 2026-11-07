# Large-directory scrolling branch review

Review baseline: `e0b5cdb55561b1e0f25bf566bf25756d3076eb1b` against upstream merge base `089ad2b417edded26385029166e5b49c13432bf8`. The scope includes the inherited GUI-thread I/O and file-operation improvements: 51 commits, 14 files, 4,135 insertions and 1,146 deletions. The preexisting untracked PR description is reference material, not fresh verification evidence.

## Findings and decisions

All confirmed defects below are assigned **FIX** and implemented. Final verification is recorded below.

| Severity | Finding | Decision and trade-off |
| --- | --- | --- |
| Blocking | A delayed save check can prompt for one filename and authorize another. | Bind confirmation to its exact target and reject stale results. This adds request bookkeeping but preserves the user's actual overwrite decision. |
| Blocking | The picker ignores the new address-bar submit command. | Share the off-thread resolver between the app and picker. A shared resolver prevents their navigation rules drifting. |
| Blocking | Archive creation truncates an existing destination missed by the UI listing. | Use atomic exclusive creation for both archive formats. A final filesystem check is authoritative; a listing check remains only a UI hint. |
| Blocking | Concurrent clipboard pastes can select the same free filename and overwrite each other. | Allocate filenames with exclusive creation and retry collisions. This adds a retry on contention and preserves both payloads. |
| Important | Delayed navigation can override a later selection or target another active tab. | Carry request identity and originating tab through completion. Comparing paths alone misses repeated requests to the same location. |
| Important | Metadata refresh retains old derived content and then legitimizes it during rescan. | Invalidate derived results on every observed targeted write, including equal-size/equal-mtime writes. Metadata-only events may recompute an affected item; ordinary rescans retain the documented fingerprint carryover optimization. |
| Important | Item subscriptions and completions identify work only by path. | Include an item work token in subscription identity and reject obsolete completions. This also restarts cancelled directory work after replacement. |
| Important | Recursive directory sizes survive changes in descendants. | Recompute recursive size after rescan. A subtree fingerprint requires the traversal that this optimization seeks to avoid. |
| Important | Lexically eliminating `..` changes navigation through symlinks. | Preserve filesystem path semantics and resolve off-thread. Pure lexical simplification cannot determine a symlink's parent. |
| Important | Image-cache clearing can accept late work or reuse an old generation. | Allocate monotonic decode generations and require an exact current-generation match. Keep per-path bookkeeping bounded. |
| Important | Unknown original image dimensions trigger a GUI-thread header read. | Retain the thumbnail when dimensions are unknown; only request full-image decoding with known dimensions. This matches the gallery's existing fallback. |
| Important | MIME content guessing holds the icon-cache mutex during file reads. | Separate the immutable MIME database from mutable icon caching. This permits parallel reads and avoids blocking GUI cache consumers on a slow file. |
| Important | Rename's stat-then-rename fallback can overwrite a concurrently created target. | Preserve atomic no-replace semantics; report unsupported operations where no safe primitive exists. Compatibility cannot justify silently replacing user data. |
| Important | Removing recents bypasses the new shared serialization. | Route every in-process XBEL mutation through the same collaborator. This protects this process; coordination with unrelated applications is a separate limitation. |
| Important | Picker watcher changes still perform blocking filesystem work inline. | Move watcher updates off-thread using the app's ownership pattern. |
| Important | Remote listing items still use positional button IDs. | Use the same named identity as local items so culled network rows retain keyboard focus after reconciliation. |
| Important | Mount, preview and folder-create completions outlive the user action that requested them. | Carry request identity through every asynchronous stage, including mount continuations and search changes. Preserve completed filesystem work but discard obsolete UI navigation. |

| Important | Reordered handler-cache reloads can replace a newer default-handler choice. | Accept only the current reload request and invalidate earlier requests before a handler write. This protects completion ordering without serializing filesystem work. |
| Important | Writes to `.hidden` are treated as ordinary entry metadata changes. | Rescan visibility after these writes; targeted replacement cannot update the other entries named in that file. |
| Important | Failed image decoding leaves matching work pending indefinitely. | Clear only the failed generation so current work can retry without an old failure cancelling a newer decode. |

| Blocking | Concurrent default-handler writes read and rewrite the same preferences file. | Queue writes in submission order. A mutex alone avoids overlapping writes but does not preserve submission order; a single worker preserves both different MIME preferences and the latest choice for the same MIME. |

## Strengths and placement review

Preserve cached listing order, bounded visible-row construction, named widget identity and background filesystem workers. The added real-widget layout test checks list/grid content height and visible item bounds across window widths and scroll positions, without timing thresholds.

Navigation resolution and watcher reconciliation live in shared collaborators. Request tokens belong to the UI lifecycle they guard; item work identity and decode generations belong to their respective item/cache lifetimes. The immutable MIME database is separate from mutable icon storage; preference persistence has a dedicated FIFO writer. New responsibilities and changed comments/diagnostics received a targeted placement and wording check.

## Coverage audit

The initial branch passed `cargo test --locked --offline --no-default-features`: **129 passed, zero failed or ignored**. The installed compiler is Rust 1.98.0; `rust-toolchain.toml` names 1.93.0, but this environment invokes the system compiler.

The static inventory found 86 newly named tests and no removed test names or assertion lines. Twelve new tests can be transplanted unchanged into upstream sources. Replaying those with the base suite produced **51 passes and four expected failures**. Of the twelve new tests, eight pass upstream and four pin changed behavior:

- Cache cleanup clears associated image metadata.
- Unknown remote files are not falsely typed as empty.
- Failed trash operations return an error.
- Rename refuses to replace an existing file.

Other new test modules depend on branch-only symbols; their compilation results and focused regression checks are recorded below when available. A passing helper test does not establish coverage of an asynchronous command's delivery or completion path.

Transplanting all new test modules onto upstream produced 40 compilation diagnostics for missing or changed branch APIs. Those modules have no valid pass/fail replay result. This is a coverage limitation, not a successful regression check.

Seven additional tests were run against the pre-review branch snapshot. All three stale-save tests and both archive-preservation tests failed as expected; empty-directory and dangling-symlink rename guards passed. Overall that replay produced **131 passes and five expected failures**. These failures reproduce cancellation being overwritten by a delayed save result, stale filename/location results being accepted, and both ZIP and TGZ destinations being truncated.

## Impact and follow-up

The invalidation defects affect visible previews, checksums, child counts, recursive sizes and gallery images. The navigation defects affect the destination shown or returned by the picker. The creation and confirmation defects can affect user files. Existing branch tests and the earlier PR narrative did not establish these missing safety properties.

This review changes repository sources and isolated test fixtures. It does not deploy a binary, modify user datasets, or regenerate prior performance measurements. Whether a deployed copy exercised these defects is unknown; no claim of a PROD all-clear is made. A release containing these fixes needs a rebuilt binary and manual GUI checks for scrolling, delayed navigation, save confirmation and gallery refresh. Previous timing figures must be remeasured before being used as evidence for the revised implementation.

No JIRA identifier or ticket-linked requirement document was found in this branch. No ticket transition or external publication is part of this review.

The equal-fingerprint targeted-refresh regression failed on the pre-fix implementation: an equal-length write with restored mtime retained the old thumbnail. The fix discards derived results for observed writes. The ordinary-rescan fingerprint trade-off test remains intact.

Two fixture assumptions were corrected after checking their actual contracts: pinned grid layout reports content height without the assumed extra bottom padding, and xdg-mime with `zero_size(false)` retains a known filename type for an empty `.txt` file. No production behavior was changed to satisfy those mistaken expectations. Seven new app tests were moved into a Tokio runtime because `App::init` requires `Handle::current`.

The FIFO preference-writer tests use an explicit temporary destination and cover different MIME keys, repeated updates to the same key, and continuation after an I/O failure. Submission happens synchronously before the UI schedules the completion task.

## Evidence limits

Recents mutations are statically verified to share the same in-process lock; the actual user XBEL store was not modified for a concurrency test. Startup CLI launch delivery, real GVFS mounts, cross-process preference/recents writes, and deployed GUI interaction remain integration checks. The MIME ordering test checks stale/current request consumption, not distinct handler-cache contents. These limits must not be represented as end-to-end test coverage.

Atomic no-replace rename now returns an error on unsupported platforms, kernels or filesystems. Linux no-replace behavior is tested; non-Linux execution is not. Failing safely trades compatibility for preserving existing destinations.

Memory lookup found historical GUI-freeze and filesystem context, but no exact prior scrolling review; none was treated as proof of current behavior. Journal notes capture the corrected test assumptions. No data/tensor cache, shipped binary, external verdict or running production job was changed. Existing performance conclusions require remeasurement if applied to this revised source.

## Verification status

All review fixes are implemented and verified. This repository has neither `.claude/run-checks.sh` nor `bin/pre-merge-check`; the equivalent checked-in CI matrix passed:

| Command | Passed | Failed / ignored |
| --- | ---: | ---: |
| `cargo test --locked --offline --no-default-features` | 176 | 0 / 0 |
| `cargo test --locked --offline` | 178 | 0 / 0 |
| `cargo test --locked --offline --all-features` | 178 | 0 / 0 |

The final matrix ran serially in one resource-managed scope with eight build jobs and a 24 GiB reservation; elapsed time was 4m04s. The test-count difference reflects three desktop-only writer tests and one TinySkia-only real-layout test. Binary and documentation test targets also passed.

Targeted `rustfmt --check` on all twelve changed/new Rust files and `git diff --check` passed. Stable rustfmt warns that the repository's `imports_granularity = Module` setting requires nightly. The default-feature build produced no compiler warnings. No-default warnings remain from feature-disabled code already present at the review baseline; the all-feature build reports eight preexisting unused assignments in desktop-applet margin calculation. These were inspected against the pre-review source and do not originate in these fixes.

No confirmed functional finding remains deferred. Manual/deployment and cross-process validation limits above remain explicit. The original PR draft and journal storage are excluded from the review commit. No push, merge, deployment, or external update was performed.
