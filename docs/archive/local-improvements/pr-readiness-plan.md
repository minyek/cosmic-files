# PR readiness implementation plan

> Use the executing-plans workflow for the root's sequential repairs. A bounded worker owns only the remote MIME repair; no concurrent writes to shared paths.

**Goal:** Make the complete branch, including pending repairs, reproducible and understandable for an upstream reviewer.

**Architecture:** Keep asynchronous request ownership with the UI lifecycle, filesystem publication with storage collaborators, and menu action identity with the parsed menu invocation. Retain the existing commit sequence and add focused, tested repairs and acceptance evidence.

**Tech stack:** Rust, libcosmic/Iced, GIO/GVFS, tempfile, Python acceptance drivers.

**Spec:** [PR readiness requirements](pr-readiness-requirements.md).

**Requirements:** Preserve files on failures; discard obsolete asynchronous responses; keep displayed actions bound to their intended commands; make acceptance assertions match the claims. No pushes, installation, user-store mutation, or personal contributor attestations.

## Tasks and dependencies

1. **Remote MIME parity** — owns `src/mounter/gvfs.rs` and `examples/remote_scan_acceptance.rs`. Add a failing regular-file MIME test, derive MIME from content type/name, and compare MIME in the live URI/FUSE probe. Run focused tests; root reruns the real server acceptance after rebuilding. This task is independent; one bounded worker implements it.
2. **Atomic storage publication** — owns `src/operation/mod.rs` and `src/mime_app.rs`. Reproduce a failed compression leaving its destination and a failed MIME-default write truncating its existing file. Write archives into destination-directory temporaries, finish and flush every encoder, and publish without replacement only on success. Atomically replace MIME defaults while retaining existing permissions and symlink target semantics. Tests assert original bytes survive injected failure and no partial archive survives failure/cancellation. Run focused tests before proceeding.
3. **UI request identity** — owns `src/app.rs`, `src/dialog.rs`, `src/navigation.rs`, `src/watcher.rs`, and `src/recents.rs`. Reproduce same-path repeated save checks, direct navigation superseding a pending request, repeated same-entity sidebar loads, and failed watch registration followed by retry. Add request tokens and invalidate at each owning transition. Record only successful watch registrations. Trace recents shutdown and arrange completion before a clean exit/picker return without blocking the GUI thread. Each behavior gets a failing regression before its repair.
4. **Search and menu identity** — owns `src/tab.rs`, `src/menu.rs`, and the necessary app action mapping. Reproduce cached capped-search insertion and desktop-file mutation between menu display and activation. Repair a demonstrated stale-cache defect if reproduced; otherwise retain the boundary regression and record the corrected finding. Bind activation to the parsed action shown by the invocation. Preserve existing selection/work identity checks. Task follows UI edits because action types can touch the same consumers.
5. **Upstream integration and history** — inspect the two new upstream commits, including drag selection. The isolated merge already preserves upstream's drag callbacks and removes `watch_drag`; do not treat a textual conflict as established. Integrate current upstream once repairs are ready, then verify the merged behavior. Correct the one missing AI disclosure with a backup ref and tree-preserving message rewrite; leave personal DCO/understanding certification to the author. Keep local IDE/journal files outside commits.
6. **Final verification and review packet** — run the canonical CI feature matrix and release recipe on the final source, rerun affected real acceptance, and collect the separately queued exclusive performance probe. Refresh artifacts, source hashes, test counts, PR description, and the review report. Checkpoint source fixes with explicit paths after focused verification to permit a clean upstream merge. Commit the final acceptance packet only after its fresh evidence is complete. Report remaining environmental limits accurately.

## Review focus and decisions

- Equal paths/entities do not establish equal asynchronous requests: exercise reverse completion and A→B→A changes.
- Publication failures must leave either the previous valid file or no new destination: exercise errors after writes begin and cancellation.
- Menu labels and commands must share immutable identity even if the backing desktop file changes.
- Full search results can change without changing vector length: exercise insertion at the 200-result cap with equal modification times; verify the comparator before assuming a filename tie-break.
- Failed watch registration must remain eligible for retry.
- Rename must never replace an existing destination. Retain fail-closed behavior where a backend lacks atomic no-replace support unless a verified safe primitive is available; a check-then-rename fallback would reintroduce data loss. Determine actual supported-backend impact before accepting the reviewer's portability severity.

Existing 206-test results and release artifacts describe the earlier source snapshot. New repairs require fresh final results; earlier remote parity evidence covered names/types but omitted MIME and is insufficient for the stronger claim.
