# Fedora RPM detached job outcomes

Reviewed the undelivered completion records for the RPM build and scoped-gate
jobs on 2026-10-23. Resource usage was within the recorded budgets for all four
jobs.

- `cosmic-files-rpm-build-corrected` — **failed/terminated**. Its completion
  record reports `exit_code: killed` and `exit_status: TERM`; the service log at
  16:18:02 says `recipe build-debug was terminated on line 55 by signal 15`.
  This overlapping build was stopped to avoid concurrent writes to one
  worktree. The later final build from `master` succeeded and produced
  `target/rpm/cosmic-files-local.rpm` at
  `1.11.0^git20261023.9.g25e39a3cd295`.
- `cosmic-files-scoped-gate` — **failed**, exit 101. At 16:40:03 Cargo reported
  `failed to download anstream v1.0.0` because the gate was offline. After
  `cargo fetch --locked` populated the checkout-local cache, the gate was
  rerun.
- `cosmic-files-scoped-gate-final` — **succeeded**, exit 0. Its log records all
  three test profiles finishing at 16:43:48, 16:46:40, and 16:48:51.
- `cosmic-files-scoped-gate-tag-fix` — **succeeded**, exit 0. Its log records
  all three test profiles finishing at 16:54:44, 16:55:05, and 16:55:26.

The interrupted build and initial offline-gate failure were superseded by the
successful final RPM build and passing scoped gates. No human action is needed.
