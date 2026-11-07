# Branch review fixes implementation plan

> **For agentic workers:** Use superpowers:executing-plans for serial implementation and an independent review after the fixes.

**Goal:** Fix the six accepted branch-review findings and verify their regression coverage.

**Architecture:** Request identities govern asynchronous listing and trash publication. A single worker owns recents mutations. Menus preserve unknown/invalid metadata states, and thumbnails provide image dimensions.

**Tech stack:** Rust, Tokio, Iced/libcosmic, Cargo.

**Spec:** [Requirements](branch-review-fixes-requirements.md).

## Execution

The existing worktree is isolated. Implement serially because the repairs share `src/app.rs` and `src/tab.rs`; parallel implementers would compete over those files. Review the final stable diff independently. Do not push or merge this upstream-based branch.

- [x] Add failing local-image, desktop-directory, and mixed-scan regression cases using existing test fixtures. Run targeted tests with `CARGO_TARGET_DIR` under `/mnt/logs/temp`.
- [x] Fix image dimension propagation in `src/tab.rs`; test both current and obsolete thumbnail work IDs.
- [x] Fix launcher classification in `src/menu.rs`, `src/tab.rs`, and `src/app.rs`; retain successful empty action lists as valid entries and preserve parse failure as `None`.
- [x] Extract sidebar menu item construction in `src/app.rs` so tests can assert actual actions. Test unknown, file, directory, and virtual locations; suppress only metadata-dependent actions while loading.
- [x] Replace rescan booleans with a request-owning coordinator in `src/scan.rs`, integrated through all app/picker scan starts and success/failure completions. Test reverse completion, pending bursts, navigation supersession, and failed current/obsolete requests. Preserve existing coalescing test contracts.
- [x] Add latest-request publication to the trash cache in `src/trash.rs` and carry its token/value through `src/app.rs`. Test out-of-order and duplicate completion on a local cache instance.
- [x] Replace recents threads/mutex with a FIFO writer in `src/recents.rs`, following `DefaultAppWriter`. Keep record/clear fire-and-forget; wait for remove's result only in its existing blocking caller. Test record/clear/remove ordering, failure continuation, and returned errors with isolated sinks.
- [x] Apply targeted placement/comment cleanup; run an independent review and resolve its findings.
- [x] Run the checked-in CI matrix serially, detached through `arm-run --resume-on-done`, with external target storage. Record results and impact in `docs/reports/`.

## Review focus

- An old scan arriving after a new location's scan must not release current work.
- A failed worker must not leave watcher refresh permanently pending.
- An invalid desktop file must not be confused with a valid launcher with zero actions.
- An image write must invalidate dimensions until current thumbnail work completes.
- Recents tests must never access the user's XBEL store; trash tests must not mutate the real bins.

## Validation and impact

Targeted regression tests establish each boundary. The final matrix is `cargo test --locked --offline --no-default-features`, `cargo test --locked --offline`, and `cargo test --locked --offline --all-features`. Use an external target directory because the review's first matrix exhausted `/mnt/work` during compilation. No prior performance figures are revalidated by functional tests; no deployment or user-data regeneration is part of these fixes.
