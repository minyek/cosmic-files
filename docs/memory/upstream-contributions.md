# Upstream contributions

- Upstream (`pop-os/cosmic-files`, System76) does not accept LLM-authored changes, so this fork
  no longer submits anything upstream: no pull requests, and no issues or comments on its
  behalf. Every fix lands only on this fork's `master`. Decided by the user on 2026-11-03; the
  README's "Why this fork exists" gives the background.
- Code still flows the other way: upstream's `master` is merged into the fork's `master` with
  `/sync-upstream`, a merge commit, never a squash.
- Each fork change is tracked as an issue in `minyek/cosmic-files`, and README.md lists every
  fix or feature with a link to its issue. A fix found while working on another issue still gets
  its own README line, linked to the issue it shipped under.
- Older notes that plan upstream pull requests (the untracked `BRANCH-LAYOUT.md` and the PR
  descriptions under `docs/reports/`) are historical.
