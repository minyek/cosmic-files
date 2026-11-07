# Upstream contributions

- Upstream (`pop-os/cosmic-files`, System76) does not accept LLM-authored changes, so this fork
  no longer submits anything upstream: no pull requests, and no issues or comments on its
  behalf. Every fix lands only on this fork's `master`. Decided by the user on 2026-11-03; the
  README's "Why this fork exists" gives the background.
- Code still flows the other way: upstream's `master` is merged into the fork's `master` with
  `/sync-upstream`, a merge commit, never a squash.
- Each fork change is tracked as an issue in `minyek/cosmic-files`, and README.md lists every
  fix or feature with a link to its issue. A fix found while working on another issue gets an
  issue of its own (related fixes may share one, as #57 groups five pause and resume fixes), with
  its cause, fix and tests, its own branch and its own commit on `master`, and its README lines
  link to that issue. Defects found but not fixed get an open issue too (as #58).
- History shape (since 2026-11, the history rebuild): `master` is upstream `e03001c`, then one
  self-contained commit per issue, then the fork's records commits (tooling, packaging, evidence,
  docs). 44 of the 50 code commits hold one issue; six hold two to four issues that could not
  be separated without inventing code, and name each of them in a `Fork-issue` trailer.
  The pre-rebuild history is kept on the tag `backup/master-pre-history-2026-11`. `CLAUDE.md` holds
  the rule that keeps this shape and the landing script.
- Cherry-pick recipe, should a pull request ever be possible: read the issue's commit's
  `Depends on:` line, cherry-pick those commits (and theirs, recursively) in `master` order onto
  the upstream of the day, then the issue's own commit. Every commit was checked to apply this way
  onto `e03001c` and to build (`docs/archive/history-rebuild/`).
- Older notes that plan upstream pull requests (the untracked `BRANCH-LAYOUT.md` and the PR
  descriptions under `docs/reports/`) are historical.
