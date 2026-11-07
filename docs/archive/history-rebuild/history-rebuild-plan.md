# history-rebuild: implementation plan

Spec: [the spec](history-rebuild-spec.md) (approved 2026-11-03, Record section). The spec is
decided; this plan only orders the work. D7's force push is **not** approved (Q6-B): tasks 10, the
D8 edits and cleanup wait for a renewed approval.

No code behaviour changes, so "tests first" means the checkers: T1 builds every checker and proves
each fails on a deliberately wrong input before any series commit exists. Checkers and run logs live
in `/mnt/logs/temp/history-rebuild/` (`bin/`, `commits/`, `gate/`, `map/`), pinned with `.keep-until`.
They are copied to `docs/archive/history-rebuild/` in the AC 9 commit, not into the series.

## Module map

There is no product code. The tooling is shell and Python under `/mnt/logs/temp/history-rebuild/bin/`:

| File | Responsibility | Size |
|---|---|---|
| `check-series.py` | AC 1, 3, 4, 12 (paths, trailers, messages, added-line greps) over `git log` | ~200 |
| `check-lines.py` | AC 5: numstat totals, `blame --reverse` of non-surviving lines, the mechanical rule | ~200 |
| `check-commit.sh` | D5(c) for one commit, run in a throwaway worktree | ~80 |
| `check-selfcontained.sh` | D6 for one commit: `merge-tree` plus `cargo check --all-targets` | ~60 |
| `run-series.sh` | loops the two commit checkers under `arm-run`, resumable per commit | ~60 |
| `baseline.sh` | measures the D5(c) baselines at `e03001c` and the target | ~60 |
| `gate.sh` | the three AC 8 commands | ~30 |
| `check-readme.py`, `check-issues.py` | AC 13 and AC 11 against the commit map and `gh` | ~100 each |

## Tasks

Each task lists dependencies (Dep) and the files it touches (Touch).

**T0. Worktrees and scaffolding.** Dep: none. Create `history-rebuild-series` as a branch at
`e03001c` in its own worktree `.worktrees/history-rebuild-series` (the docs branch stays separate).
Create the `/mnt/logs/temp/history-rebuild/` tree. Touch: new worktree and log dirs only.

**T1. Checkers, proven to fail.** Dep: T0. Write the checkers in the table. For each, write a
fixture that must make it fail (an issue in two commits, a `docs/` path in a code commit, a
`(#N)` subject, `Claude-Session`, a non-listed added-then-removed line, a wrong README link) and show
it failing, then passing on a good fixture. The regex `(^|[^[:alnum:]_/-])#[0-9]+` is tested on the
sample lines from the spec. Touch: `bin/`. Independent of T2.

**T2. Baselines.** Dep: T0. Run `baseline.sh`: for `e03001c` and `master` (target), collect
check findings in all three configs and clippy findings (`--keep-going`) in the default config,
normalised by lint, file and message. The union is the D5(c) baseline. This also settles whether
`examples/copy.rs` errors under clippy, and whether #57 builds on compio 0.18 (D4 order).
Run under `arm-run`. Touch: `map/baseline-*.txt`. Independent of T1.

**T3. Inventory (spec task 1).** Dep: T0. Per source file, one subagent assigns every hunk of
`git diff e03001c..765f9d0` to an issue, a D2 group or a fork-only bucket, guided by the issue texts
(`gh issue view`), the old single-issue commits and the three old PR branches. Output
[the commit map](history-rebuild-map.md) (hunk to issue, plus the list of D2 cases: mechanical,
issue-driven, grouped, and subsumed issues). Hunks that fit no issue but are fixes get a new issue
(AC 3); a non-fix leftover goes to the user. Fan-out is by file over `src/` (31 files), `tests/`,
`Cargo.*`, `i18n/`, `res/`; the fork-only buckets are one subagent. A synthesis pass checks every hunk
is assigned exactly once. Touch: map file only. Dep T1 is not required.

**T4. Dependencies and order (spec task 2).** Dep: T2, T3. Build the dependency graph (semantic and
textual labels), order by D4, place shared fixtures, decide #57 before or with #56, list groups,
listed lines and subsumed issues, and write them into the map file. Touch: map file.

**T5. Build the series (spec task 3).** Dep: T4. In the series worktree, build each commit from the
map in order, editing by hand where a hunk is split, with D11 messages (`Fork-issue` trailers,
`Assisted-by: Claude`, no `Signed-off-by`, no session URL). Build code commits by waves of issues
whose commits are independent only if the map says their touch-sets are disjoint; otherwise serial.
Touch: series branch only.

**T6. Per-commit checks (spec task 4).** Dep: T1, T2, T5. Run `run-series.sh` under
`arm-run -m 20G`: D5(c) and D6 per commit. A failure is fixed by moving hunks or reordering, never a
fix-up commit; rerun from that commit onward. Touch: series branch, `commits/` logs.

**T7. Fork-only commits (spec task 5).** Dep: T6. Add the four trailing commits (tooling with the
`lib.rs` `pub mod trash` hunk, packaging, evidence, docs), run D5(c) on each, and then the AC 2
check `git diff --exit-code 765f9d0 <series>`. Touch: series branch.

**T8. Series checks.** Dep: T7. Run `check-series.py` and `check-lines.py` (AC 1, 3, 4, 5, 12) and
the AC 13 README check. Fix by moving hunks. Touch: logs.

**T9. Series review (spec).** Dep: T8. A fresh `reviewer` agent reads a sample of commits end to
end, every D2 group and listed line, and the commits of #14, #22, #29, #31, #34 and #56. Apply
findings, rerunning T6 from the first changed commit. Touch: series branch.

**T10. The AC 9 commit.** Dep: T9. `CLAUDE.md` (D10 rule, landing procedure, D11, D9, tooling
exemption), amended `docs/memory/upstream-contributions.md`, the superseding note on the 2026-10-02
decision, the ancestry line in `docs/guides/local-build-and-install.md`, README fixes, and the spec,
plan, map and checkers archived under `docs/archive/history-rebuild/` with `docs/README.md` indexed.
Then try the new landing procedure once on a throwaway branch. Touch: the series branch's docs paths.

**T11. Final review.** Dep: T10. A fresh `reviewer` agent, clause by clause over AC 1-14, naming the
check that pins each. AC 10, 11 and 14 are judged on their dry-run evidence. Touch: none.

**T12. Gate.** Dep: T11. `gate.sh` under `arm-run -m 55G --io light` from the series worktree (AC 8).
Record the counts and tree SHA in a history-rebuild issue. Touch: `gate/` logs.

**T13. Stop for the user.** Dep: T12. Present the finished series (tip, counts, review verdicts, the
exact D7 steps with the target SHA re-read from `master`) and ask for D7 again. End the turn.
**Nothing is pushed or edited on GitHub before that approval.**

After approval (not part of this approval): land by D7 steps 0-4; D8 issue-body edits; RPM build
from the new `master` and `rpmdev-vercmp` (AC 14); confirm AC 10, 11 and 13; cleanup (harvest the
journal, remove the handoff file, worktrees and branches).

## Parallelism

Independent groups: {T1, T2, T3}. T3 fans out per file. T5 is serial unless the map proves disjoint
touch-sets. T6 runs commits in order, since each builds on its predecessor.

## End to end

The smallest end-to-end case is T5's first commit through T6's checks on it, run as soon as the first
commit exists; it proves the runner, the baselines and D6 together before the other commits are built.
The last end-to-end case is T12 on the finished series.

## AC coverage

| AC | Task and check |
|---|---|
| 1, 3, 4, 12 | T8 `check-series.py` |
| 2 | T7 tree diff, repeated in T11 |
| 5 | T8 `check-lines.py`, read by T9 |
| 6, 7 | T6 `run-series.sh` |
| 8 | T12 |
| 9 | T10, judged by T11 |
| 10, 11, 14 | dry run in T10/T11; confirmed after approval and landing |
| 13 | T8 `check-readme.py` |
