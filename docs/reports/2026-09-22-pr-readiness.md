# PR readiness review

Status: source review and repairs are complete at `2ab787e`. The full functional matrix and all three release builds passed. Final isolated acceptance and performance evidence remain pending; the branch is not yet declared ready.

The review covers the original 54 commits after `fe71aca`, the committed review repairs, upstream integration, and new acceptance harnesses. Three independent read-only reviewers examined rendering/menu state, app/picker request ownership, and storage/MIME behavior. Root reviewed history, build/CI configuration, contribution requirements, the PR draft, and integration with current upstream `b326e48`.

## Findings and disposition

| Finding | Decision and impact |
| --- | --- |
| URI scans assign directory MIME to regular files; remote probe omits MIME | FIX. Affects remote open-with/default-handler behavior and invalidates any claim of complete URI/FUSE equivalence. Existing names/types checks remain valid. Rebuild and rerun live remote acceptance with MIME assertions. |
| Archive failure/cancellation leaves a partial destination; encoder finalization errors can be lost | FIX. Publish a completed temporary archive without replacement. Add failure/cancellation and readable-success regressions. No user archive is regenerated or deleted. |
| Default-handler writes truncate the live preferences file before a fallible write | FIX. Atomic replacement must preserve old contents on failure and retain permissions/symlink semantics. No real user preferences are touched by tests. |
| Recents worker has no clean-exit drain | FIX. Ensure queued persistence finishes before clean application exit/picker completion; preserve off-thread behavior. Investigate equivalent default-handler shutdown behavior too. |
| Same-path save checks lack per-submission identity | FIX. Reverse completion and A→B→A changes must not duplicate/accept obsolete confirmations. |
| Direct location changes can leave a previous resolution current | FIX. Cover unmount fallback and hover navigation, including app and picker consumers. |
| Same-entity sidebar loads can finish out of order | FIX. Bind facts to the latest request, not only the entity. |
| Failed watches are returned as successfully registered | FIX. Track actual registrations and keep requested-set identity separate so failure does not cause a tight retry loop. |
| Capped search mutations can reuse a cached permutation | NOT REPRODUCED for the reported case. Search insertion and stable display sorting both use descending modification time, without filename tie collation. A 200-result test with equal-time pairs and a newer insertion passes before any search-source repair. Keep the boundary regression; retract the claimed tie-order defect and item-omission claim. |
| Desktop menu activation reloads mutable file contents after displaying labels | FIX. Bind activation to parsed invocation data or reject stale identity. |
| Drag-selection behavior already fixed in newer upstream | Integrate upstream's fix. Isolated merge checks exit 0, and inspection of the merged tree confirms both persistent drag callbacks and removal of `watch_drag`; a merge conflict is not established. |
| Atomic rename unavailable on some platforms/backends | Preserve fail-closed safety while checking concrete backend impact. A check-then-rename fallback is rejected because it can overwrite another file. The reviewer's blanket portability severity is not yet accepted. |
| PR draft has obsolete counts, benchmark claims, and checked personal attestations | FIX. Draft now describes the full branch and current evidence; pending benchmarks are explicit, and personal attestations are unchecked. |
| One cleanup commit lacks AI disclosure | FIX with a tree-preserving message rewrite and backup ref. Two historical commits lack sign-off; the contributor must personally satisfy the upstream DCO checklist. |
| Changed comments and a report's trailing blank line | FIX. Keep technical constraints concise and remove narration; full-diff whitespace check now passes. |

## Verification boundaries

The earlier matrix passed 206 tests per feature configuration, and both release binaries built. Those outcomes predate these new repairs and cannot establish their correctness. Source-affecting repairs require fresh targeted tests, the final functional matrix, affected real acceptance, and a rebuilt release bundle.

No deployed binary, real user history/trash/preferences, external data cache, shipped artifact, or unrelated job has been modified by this review. The files and verdicts requiring updates are this branch's tests, reports, acceptance artifacts, source manifest, PR draft, and release output. No push or PR publication is authorized or performed.

## Confirmed repair evidence

- Remote MIME and size parity now pass against the private FTP/GVFS URI and FUSE paths. The live mount also supports no-replace rename and preserves an existing destination; retain fail-closed behavior for untested unsupported backends.
- Atomic archive publication and preference replacement pass failure, cancellation, readable-success, symlink, permissions, and non-UTF8-name regressions. Historical archives containing non-UTF8 names could have silently omitted them; no user archive was inspected, regenerated, or deleted. TGZ preserves those names; ZIP fails without publishing because its writer requires UTF-8 names.
- Two save-check tests reproduced duplicate confirmations and A→B→A stale acceptance. The picker tests pass after binding checks and confirmations to individual requests.
- Same-entity sidebar reorder and hover-navigation regressions reproduced, then pass after identity/invalidation repairs. Seven watcher tests pass, including failed-registration retry and consumer tests that prevent immediate retry loops.
- Desktop-file mutation reproduced a displayed action executing the replacement file's command. The repair carries parsed command data and a snapshot token; obsolete snapshots cannot select actions from a replacement snapshot. This removes the activation-time file read as well.
- A private-home child-process test reproduced immediate exit before queued persistence was drained. Queue barriers now let app and picker closure await preceding recents/default-handler writes without blocking the GUI thread. The final functional matrix and production builds verify this repair; final real-profile acceptance remains pending.

The search review correction narrows a review conclusion only; it does not invalidate earlier search output or require regeneration. The confirmed lifecycle repairs affect this branch's app/picker behavior and invalidate a ready-for-review verdict for earlier binaries. Final source, acceptance, release, and report regeneration remain required. No deployed app, real user store, external data cache, or unrelated job was modified.

## Source integration

The repaired source is committed in `1270092`, with the toolkit update in `f933bfd`, and current upstream `b326e48` merged conflict-free in `3778fee`. The first full gate against `3778fee` exposed a production dependency error. Commit `2ab787e` repairs it, and the complete gate then passed as recorded below. The release recipe includes the upstream thumbnailer alongside the app and applet. Real acceptance and final artifact refresh follow that gate.

The final focused groups passed 161 tests: app65, picker26, menu13, MIME27, operations23, recents5, watcher1, and capped search1. The new unmount test also exposed the picker's exact-path-only fallback; it now leaves child directories within an unmounted path and invalidates pending save/navigation requests. This affects picker navigation only; no external runs, user data, or deployed binaries were changed. Re-run the picker acceptance on the rebuilt binary.

The missing disclosure was repaired with a tree-preserving rewrite: `b551eff` became `19a33611`, and its child `87f3baf` became `2df67726`. The original tip remains at `backup/pr-readiness-before-disclosure`. New commits disclose Codex assistance; personal DCO and understanding attestations remain the author's responsibility. Nothing was pushed.

## Final-gate attempt 1: failed release build

The gate on source `3778fee` completed every child; [the retained gate excerpt](artefacts/scroll-acceptance/pr-readiness-gate-1.txt) records the cited lines. Functional tests passed: minimal227 (`minimal.log:419`), default241 (`default.log:254`), all-features241 (`all.log:256`), with no failures or ignored tests. GUI/remote/storage/picker and frame-probe builds succeeded. Formatting and full-diff whitespace checks passed.

The release recipe failed in its first application build: `release.log:3-10` reports E0433 at `src/persistence.rs:4`, because `tokio::join!` needs the macros feature, enabled only by the dev dependency. Test builds therefore masked a production dependency-boundary error. Use the existing production-available `cosmic::iced::futures::join!` instead of broadening Tokio's production feature set, then rerun the final gate. No new release bundle was produced; earlier release binaries remain older-source artifacts. This defect affects production compilation, not the passing test outcomes, user stores, deployed binaries, external runs, or data caches.

Minimal builds also report feature-disabled unused-code warnings. The new atomic destination resolver is desktop-only and will be gated accordingly. Remaining warnings concern optional-feature imports/bindings and inactive mounter variants; they are non-blocking follow-up cleanup under this bounded continuation. Stable rustfmt ignores the repository's nightly-only imports_granularity setting; formatting still passed. Final acceptance and ready-for-review status remain pending.

## Final-gate attempt 2: succeeded

The production dependency repair passed the complete gate. [Retained evidence](artefacts/scroll-acceptance/pr-readiness-gate-2.txt) records minimal227 (`minimal.log:413`), default241 and all-features241 (`default.log:254`, `all.log:254`), no failures or ignored tests, successful acceptance-probe builds, and all three release completions (`release.log:3,7,61`). Formatting and full-diff whitespace checks passed. Source and probe checksums match the tested files. The remaining minimal-feature warnings are the optional-feature cleanup already noted; the new resolver warning is gone.

The application, desktop applet, and upstream executable thumbnailer were rebuilt successfully. No deployment or installation occurred. Real GUI/storage/remote acceptance and the separately isolated performance measurement still need final verification before a PR-ready verdict.

## Acceptance admission

The final exclusive acceptance submission timed out before admission after 50 minutes while another managed job remained active. It ran no child checks and produced no acceptance verdict. The submission was retried with the same frozen source and binaries; no unrelated job was stopped. Functional and release results remain valid.
