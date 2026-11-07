# Squash-merge readiness

This is the initial audit snapshot. The later [acceptance closure](2026-10-02-acceptance-closure.md)
closes its merge-blocking findings and records the final-source evidence.

Status: not merged. Main remains `master` at `06278ee`. The user requested
integration of all local improvements and a release build from main after
testing the combined application. Preparation is isolated on
`integration/main-preparation`; the tested integration source is `4b2c08a`.

The [symlink audit](2026-10-02-symlink-badge-ac-audit.md) passed. The
[existing-improvements audit](2026-10-02-existing-improvements-ac-audit.md)
found acceptance evidence gaps. Neither audit reproduced a production defect.

## Closure decisions

All listed items are **FIX before merge**:

1. Include the existing acceptance examples, scripts, reports and evidence,
   subject to the pending uncommitted-file approval. Archive the five completed
   planning documents under `docs/archive/local-improvements/` only when their
   acceptance is complete and the proposed move is approved.
2. Add operation-level ZIP/TGZ finalization-failure coverage. Assert a reported
   error and no published destination or leftover stage.
3. Exercise real drag selection in grid/list views after viewport culling.
4. Reproduce picker framework-close behavior with queued persistence. If it
   bypasses the drain, fix that lifecycle path and retain the failing regression.
5. Pin fail-closed behavior for unsupported atomic rename, watcher recovery
   after a panic, and usable startup after failed launch resolution. These
   boundary claims currently depend on source inference.
6. Refresh isolated storage/failure/recovery, GUI, picker and remote/FUSE
   acceptance against the final source. Preserve the historical evidence as
   evidence of its own snapshot.
7. Run the planned frame measurement separately through an exclusive managed
   job, with explicit CPU-only measurement limits.
8. Obtain a fresh independent closure verdict, run the canonical functional
   matrix, finish documentation/journal harvest, squash through an isolated
   worktree, advance `master`, and build all three release executables there.

The prior passing functional matrix and release build remain valid for their
recorded source. They do not establish the missing boundary assertions or
refresh older desktop/storage evidence. No user profile, installed binary,
remote service, existing worktree source or main branch was changed by this
audit. No push is authorized.

## Approved closure work

The user approved the complete recommendation with “yes, go”: include the
uncommitted acceptance tools and evidence, close the listed gaps, and archive
the completed planning documents before integration. Main must not advance
while actual acceptance clauses remain untested or unmet.
