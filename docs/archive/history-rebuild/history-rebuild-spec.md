# history-rebuild: rebuild `master` as upstream plus one commit per issue

Slug: `history-rebuild`. It has no ticket, and becomes a fork issue once the spec is approved.

## Goal

Every fix and feature on the fork's `master` becomes one self-contained commit on top of
upstream. Each could then become an upstream pull request without untangling, and the history
holds nothing that was added and later removed.

Today's history does not do this:
- `66dac3f` holds #2–#53 in one commit (+15,033/−1,700) on upstream's `b326e48`.
- The later single-issue commits, `8bbce58` (#1), `79a17e1` (#54) and `d70f661` (#55), need it in
  order to apply. A simulated cherry-pick conflicts for all three, both onto upstream as it was at
  the time (`4efe802`) and onto upstream today (`e03001c`). For example, #1 builds on
  `cached_metadata` from #16.
- `f4ff95d` mixes #56, #57 and fork docs.
- `d68bd59` adapts fork code to an upstream merge.

The user asked for:
- one commit per issue, with no change added and later removed;
- a project rule that makes this the default;
- a check that no issue commit carries fixes beyond its issue.

They approved a force push to fix the remote, and chose this full rebuild in the decision panel
(Q2-A, 2026-11-03).

Nothing is being submitted: upstream does not accept LLM-authored changes today
(`docs/memory/upstream-contributions.md`). The history is to be ready if that changes. The rebuild
changes history only. The code, tests and docs stay byte-identical, apart from the one AC 9 commit.

## Earlier decisions this revisits

- `docs/decisions/2026-10-02-local-integration-baseline.md` chose to land the fork's improvements
  as one squash, `66dac3f`. The rebuild reverses that. The new evidence is the user's request, and
  the cherry-pick simulation above, which shows the squash makes every later fix
  non-contributable. The decision record gets a superseding note (AC 9).
- `docs/memory/upstream-contributions.md` makes two points this changes:
  - It says upstream is merged in "a merge commit, never a squash". The rebuild linearises the
    history once (D1) and keeps merges for later syncs (D9).
  - It says a fix's README lines link to its own issue, "not the one it shipped under", which
    lets a fix ship inside another issue's commit. D10 replaces this, and AC 9 amends the note.
- `docs/reports/2026-09-22-pr-readiness.md` treated a commit without an AI disclosure as a defect.
  It left DCO sign-off as the contributor's personal act. It also rewrote a commit message while
  keeping the tree, with a backup ref. D11 follows all three.
- The untracked `BRANCH-LAYOUT.md` is historical. D1, D3, D5 and D2 follow four of its precedents:
  - PR 3 was "rebuilt rather than replayed" (D1).
  - Fix narratives stayed out of PR commits (D3).
  - Every reordered commit "was checked to build and test on its own", with no new fmt or clippy
    findings (D5, which runs clippy per commit).
  - A dependency that "can never stand alone" was combined, not stacked (D2).
- `docs/guides/local-build-and-install.md` describes the squash ancestry. AC 9 updates it.

## Inputs

- **Base:** upstream `e03001c`, pinned. It is today's `upstream/master` (per `ls-remote`) and the
  merge-base with `master`.
- **Target tree:** `master` when the rebuild starts (now `765f9d0`; see D12).
- **The net diff `e03001c..master`, by area:**
  - Upstream-shaped code: `src/` (31 files, +9,518/−1,693), `tests/` (+471), `Cargo.toml` and
    `Cargo.lock`, `TESTING.md`, `i18n/` and `res/icons/`.
  - Fork-only material: `examples/storage_acceptance.rs`, `examples/remote_scan_acceptance.rs`,
    `examples/scroll_frame_probe.rs`, `scripts/` (+609), `packaging/` (+146), `docs/` (+1,524),
    `docs/reports/artefacts/` (142 files, +3,952), the README fork sections (+130) and `CLAUDE.md`.
- **The 57 issues** in `minyek/cosmic-files`. Each names its cause, its fix (functions and files)
  and its tests: 181 named tests in all, every one present on `master`. Six name no tests: #14,
  #22, #29, #31 and #34, plus #56, which was checked by hand. The issue texts are the main key for
  assigning code to issues.
- **The old single-issue commits** `8bbce58` (#1), `79a17e1` (#54), `d70f661` (#55) and
  `f4ff95d` (#56, #57): the exact record of what those issues changed.
- **Guides only:** the three old PR branches, of 12, 20 and 58 commits grouped by theme, on
  epoch-1.7.0 and epoch-1.8.0 upstream commits.

## Decisions

| # | Decision | Options | Recommendation | Why |
|---|---|---|---|---|
| D1 | Shape and base | (a) One linear series on `e03001c`; the old upstream merge commits disappear. (b) Replay the old history with its merges, splitting only the fork commits. | (a) | The commits are wanted for PRs against upstream as it is now. The target tree already contains upstream's refactor, so the split partitions one net diff rather than replaying across the split of `tab.rs`. The adaptation in `d68bd59` folds into the issues it adapts. (b) keeps merge commits that a PR series cannot use. |
| D2 | Commit unit and intermediate lines *(user)* | (a) Exactly one issue per commit, and every line added once only. (b) One issue per commit, and a line may be added and later changed in three listed cases: *mechanical* (a partial `use`, enum-variant, struct-field or match-arm list that a later issue extends, or rustfmt re-wrapping); *issue-driven* (a later issue changes code an earlier issue added, as #55 extends #54's test); *grouped* (where splitting two issues would mean inventing behaviour that exists in neither `e03001c` nor the final tree, they share one commit that names both). An earlier issue's code is taken from the final tree, minus what later issues demonstrably added (from their issue texts, the old single-issue commits and the old PR branches); no other intermediate version is invented. | (b) | (a) is not achievable. Shared `use` and variant lists in `app.rs` and `tab.rs` would force groups everywhere; every conflict in BRANCH-LAYOUT's PR 3 rebuild was such a list. Each case is listed and checked (AC 5). (b) departs from the panel option's literal "one commit per issue" and "nothing added then removed" only for these listed cases. |
| D3 | Fork-only material | (a) Code commits touch only upstream-shaped paths, and fork material follows in four trailing commits: acceptance tooling (`scripts/`, the three examples, and the `src/lib.rs` change `pub(crate) mod trash` → `pub mod trash`, which only `examples/storage_acceptance.rs` needs); packaging; evidence (`docs/reports/artefacts/`); docs (the rest of `docs/`, the README fork sections and `CLAUDE.md`). (b) Each issue's README line goes in its code commit. | (a) | A README or `docs/` hunk in a fix commit conflicts upstream, which has no fork section. The `lib.rs` hunk is fork tooling in an upstream path, so it travels with that tooling (AC 4's one exception). Fork tooling (packaging, acceptance scripts) has no issue, and the rule exempts it (AC 9). |
| D4 | Order and test ownership | Dependency order, then issue number. A dependency is semantic (the code calls or tests it) or textual (adjacent lines), and the plan labels each. Shared fixtures (`app::test_utils`, `operation::test_faults`) go in the first commit that needs them. A test belongs to the commit that adds it, and a later issue that extends it is an issue-driven rewrite (D2). #57 goes before #56 if it builds and tests on compio 0.18; otherwise the two are grouped, because #56's compio 0.19 bump without #57's Delete fix brings in #57's first cause. | as stated | Each commit must build on its predecessors alone. |
| D5 | Per-commit verification | (a) Per commit, `cargo check` in three configs plus the issue's named tests, about 60 × 4 min ≈ 4 h. (b) Per commit, the full three-config gate plus clippy, about 60 × 9 min ≈ 9 h. (c) Per commit, everything below, about 60 × 7 min ≈ 7 h; the final tree also gets the full gate (AC 8). | (c) | `cargo test <filter>` exits 0 when the filter matches nothing, so (a) passes silently for a test moved to the wrong commit. (c) meets BRANCH-LAYOUT's bar (every commit builds and tests on its own, with no new fmt or clippy findings) below (b)'s cost. The final gate covers the other two test configurations. |
| D6 | Self-contained check | For each code commit, simulate cherry-picking it onto `e03001c` plus only its declared dependencies (`git merge-tree --write-tree`). It must apply cleanly, and the resulting tree must pass default-config `cargo check --locked --all-targets`. | as stated | This is the property the goal is about. A clean merge does not prove the code builds, and without `--all-targets` the test code is not compiled. |
| D7 | Landing *(user)* | (0) Before anything is pushed, check the main checkout: `git symbolic-ref HEAD` is `refs/heads/master`, nothing is staged (`git diff --cached --quiet`), and no modified or untracked file sits at an AC 9 path; record `git status --porcelain=v1`. Any failure stops the landing with nothing pushed. (1) Push the tag `backup/master-pre-history-2026-11` on today's `master` to origin, and confirm it with `ls-remote`. (2) Run `git push --force-with-lease=master:<target sha> origin history-rebuild-series:master`. (3) In the main checkout, repeat step 0's checks, then run `git checkout -B master <series tip>`; only the AC 9 files change. Compare `git status --porcelain=v1` before and after, both columns. The old PR worktrees sit on `06278ee` and `b326e48`, not on `master`, so they and their origin branches stay as they are. Any other clone must reset, not `git pull`, which would merge the old history back in. | as stated | Step 0 makes a local refusal happen before origin changes. The lease refuses if `master` moved, and the tag keeps the old history recoverable. `reset --keep` would unstage other sessions' staged work in the shared checkout; `checkout -B` keeps it. |
| D8 | Old SHAs in issue bodies *(user, with D7)* | (a) Edit each issue body's "Landed in <old>" to "Landed in <new> (before the history rebuild: <old>, tag `backup/master-pre-history-2026-11`)". Add the line where it is missing (for example #56). A subsumed issue names the commit that subsumes it. (b) Comment on each issue instead. | (a) | The body is where readers look. A script makes each edit, and each is checked against the commit map. |
| D9 | Syncing afterwards | (a) Keep `/sync-upstream`'s merge. For a PR, cherry-pick an issue's commit and its dependencies onto the upstream of the day. An adaptation that a sync forces on fork code, and that does not conflict, lands right after the merge as one commit per adapted issue, following D11. A conflict resolution has to live in the merge commit (as in `8ef65e4`); the affected issue's commit then no longer cherry-picks cleanly onto later upstream, and is re-resolved when a PR is made. (b) Rebase the series at every sync and force-push. | (a) | (b) makes force pushes routine and rewrites every SHA each time. (a) keeps each fix's commit intact for syncs that do not conflict with it, and records where one did (the merge). |
| D10 | Rule for future work (project `CLAUDE.md`) *(user)* | (a) One issue per branch. A fix found along the way gets its own issue (related fixes may share one, as #57 does) and its own branch, landed before the branch that needs it. Each landing makes two commits on `master`, through a project-local landing procedure that replaces `/sqmerge` step 7: a code commit with only upstream-shaped paths and a D11 message, then a records commit with the branch's fork-only paths (README line, docs, archived spec). Work in progress never reaches `master`. D9's adaptation rule applies to syncs. The rule states that it replaces, in this project, the global rule that folds a gate-blocking or related clean-up fix into the current ticket (`delivery-process.md`). A `boy-scout` clean-up stays in the issue's commit only when it is inside the code the issue touches and changes no behaviour; any other clean-up is its own issue. The landing procedure keeps `/sqmerge` step 7's guards (main-moved check, change-set comparison, `--ff-only`, plain push), and makes one commit when the branch has no fork-only paths. It is tried once on a throwaway branch before AC 9 is judged. (b) As (a), but a defect that blocks the current branch may land from that branch as its own code commit, just before the branch's own (three commits from one landing). | (a) | It keeps the shape this rebuild creates at the cost of one extra commit per landing, and the landing procedure stays simple. (b) saves a branch for the rare blocking defect, but needs a three-way split of one branch. |
| D11 | Messages and trailers *(user)* | Subject: an imperative summary with no `(#N)`, because upstream uses that for its own PR numbers. Body: the issue's cause and fix, the tests that pin it, and its dependencies, with fork issues always written `minyek/cosmic-files#N`, never a bare `#N`. Trailers: `Fork-issue: minyek/cosmic-files#N` (one per grouped issue). Provenance: (a) `Assisted-by: Claude`, with no session URL; (b) the old PR branches' body line, "Developed with AI assistance (Claude Code); all changes have been reviewed and are understood by the author."; (c) none. A session URL, including the `Claude-Session` line this harness asks for, is ruled out by the user's own rule. No `Signed-off-by`: that is the user's personal DCO certification, which they add if they ever submit. Author: the repository's configured user. Date: the time of the rebuild. | (a) | The 2026-09-22 review treated a missing AI disclosure as a defect. A trailer is machine-readable and short; (b)'s sentence asserts a review by the author that the rebuild cannot claim for them, and the 12 PR-branch commits that carried it also carried the user's own `Signed-off-by`. A bare `#N` links to upstream's own issue in an upstream PR. |
| D12 | `master` during the rebuild | Hold every other landing on `master`, including #58's follow-up work, until the rebuild lands. Run no `/sync-upstream`. A records-only commit that lands meanwhile is replayed into the series' docs commit, and the AC 2 target and D7 lease move to the new tip. A code landing that gets through anyway is split into the series the same way as the rest. The hold is written into the #58 kickoff file (done at spec time). | as stated | A code landing meanwhile would mix code and docs at the tip, breaking D3, AC 4 and D11. Holding is cheap: #58 has not started. |

**D5(c), per commit:**
- `cargo check --locked --all-targets` in all three configs, and `cargo clippy --locked
  --all-targets --keep-going` in the default config. Each may give only findings (warnings or
  errors) in the *baseline*: the union of what `e03001c` and the final tree give for that command
  and config, matched by lint, file and message, never by line number. Clippy is judged by its
  findings, not its exit code, since upstream's `examples/copy.rs` may fail a lint (BRANCH-LAYOUT).
  Task 4 first measures the baseline for each of the four command and config pairs, at `e03001c`
  and at the final tree. (The final
  tree gives 32 check warnings with no default features and 0 in the other two configs; that
  they are all upstream's is inferred, not yet measured.)
- `cargo test --locked` in the default config, in full.
- Each of the issue's named tests, found by its full path from `cargo test -- --list` and run
  with `--exact`, must report "1 passed".
- `cargo fmt --check` may report no hunk other than upstream's own, which is the `ItemMetadata::Trash`
  `if let` in `src/app.rs`, matched by content and not by line number.

## Acceptance criteria

1. `history-rebuild-series` is `e03001c` followed by a linear series of commits, with no merges:
   `git merge-base --is-ancestor e03001c series` holds and `git rev-list --merges e03001c..series`
   is empty.
2. Before the AC 9 commit, the series' final tree equals the target tree exactly:
   `git diff --exit-code <target> <series~1>` is empty.
3. Every code commit names exactly one issue, or one D2 group from the plan, in its `Fork-issue`
   trailers, plus a trailer for each issue it subsumes. Every issue #1–#57 with code in the net diff appears in exactly one commit or group.
   - An issue whose lines a later issue fully rewrote is "subsumed". The plan records it, the
     subsuming commit names it in its body and a `Fork-issue` trailer, and its issue body names
     that commit (D8). Its README line still links to its own issue.
   - A hunk that belongs to no existing issue is an unrecorded fix. It gets an issue of its own,
     with its cause, fix and tests, before its commit is built, and the plan lists it.
4. Code commits touch only upstream-shaped paths: `src/`, `tests/`, `Cargo.toml`, `Cargo.lock`,
   `TESTING.md`, `i18n/` and `res/`. Fork-only paths appear only in the four trailing commits
   (D3). The one exception is the `src/lib.rs` `pub mod trash` hunk, which goes in the tooling
   commit.
5. Lines change after they are added only in D2(b)'s listed cases:
   - Lines added across the series' commits equal the final tree's lines that blame attributes
     to series commits, plus the non-surviving lines (`added-identity.py`). *(Amended 2026-11-07,
     Q2-A: the original numstat equation could not hold exactly, because the net diff's line count
     depends on how the diff algorithm aligns lines.)*
   - Every added line that does not survive to the final tree is on the list. Lines are tracked
     with `git blame --reverse --diff-algorithm=histogram`, and `--numstat` uses the same
     algorithm.
   - A line counts as *mechanical* only if either of these holds, which the check script applies
     independently of the list:
     - its replacement keeps every token of the removed line, within the same `use`, variant,
       field or arm list;
     - the change only re-wraps whitespace.
   - Every *issue-driven* or *grouped* entry names the issue whose text requires the change.
6. Every commit passes D5(c). `Cargo.lock` hunks travel with the `Cargo.toml` change that needs
   them. A failure is fixed by moving hunks between commits, reordering, or forming a D2 group,
   never by a fix-up commit.
7. Every code commit passes D6.
8. The full gate passes on the final tree, with 0 failed in every config. The gate is the three CI
   commands, run detached under `arm-run -m 55G --io light` from the worktree:
   - `cargo test --locked --verbose --no-default-features`
   - `cargo test --locked --verbose`
   - `cargo test --locked --verbose --all-features`

   One runner script, kept beside the output under `/mnt/logs/temp/history-rebuild/gate/` (not
   in swept scratch) and pinned with `.keep-until`, runs the three and keeps each one's output.
   Each config's pass and fail counts and the tree SHA are also recorded in the history-rebuild
   issue, since those logs expire.
9. One commit after the AC 2 check carries:
   - the D10 rule and its landing procedure, D11's message convention, D9's adaptation rule and the
     fork-tooling exemption, all in `CLAUDE.md`;
   - an amended `docs/memory/upstream-contributions.md`: the history shape, the cherry-pick recipe,
     and the replaced "shipped under" line;
   - a superseding note on the 2026-10-02 baseline decision;
   - the ancestry line in `docs/guides/local-build-and-install.md`;
   - any README lines the AC 13 check finds missing or wrongly linked;
   - this spec, its plan and the mapping file, archived under `docs/archive/history-rebuild/` and
     indexed in `docs/README.md`.
10. `backup/master-pre-history-2026-11` is on origin before the push. `origin/master` is the
    series tip, pushed by D7's command with a lease on the target SHA. The main checkout's
    `master` is the series tip. Its `git status --porcelain=v1` is unchanged in both columns, apart
    from the AC 9 files.
11. Every issue #1–#57, and every issue AC 3 creates, names its new commit in its body (D8). #58
    stays open and untouched.
12. Code commits carry nothing fork-specific in code or comments. That means no bare `#N`, no
    `minyek` URL and no `docs/` path. Today's net diff has none, and the check keeps it that way.
    Every series commit message has no `(#N)` in its subject, no bare `#N` and no
    `Claude-Session` line (D11).
13. README.md has at least one line per issue that changed code. Every README issue link points to
    the issue whose code the line describes.
14. The RPM builds from the new `master`, and its version sorts above the last built RPM,
    `1.11.0^git20261103.35.g1330aec3f28f`.

## Verification

| AC | How |
|---|---|
| 1, 2 | The AC 1 commands, and the AC 2 tree diff is empty. |
| 3, 4 | A script over `git log --name-only` and the trailers checks both against the plan's issue map and D3's path lists. |
| 5 | A script compares the `--numstat` totals with the net diff plus twice the listed lines. It runs `blame --reverse` on every non-surviving added line, and applies the mechanical rule independently of the list. An independent review reads the list. |
| 6 | A per-commit runner under `arm-run` logs every result to `/mnt/logs/temp/history-rebuild/commits/`. |
| 7 | `merge-tree`, then `cargo check --locked --all-targets` of the resulting tree. |
| 8 | The AC 8 commands. |
| 9, 13 | A review reading, plus a script that matches README links against the commit map and `gh issue list`. |
| 10 | `git ls-remote origin master refs/tags/backup/master-pre-history-2026-11`, and porcelain status before and after. |
| 11 | A script compares each issue body with the commit map. |
| 12 | A grep of every code commit's added lines for `#[0-9]{1,3}\b` (so hex colours such as `#303030` do not match), `minyek` and `docs/` paths, and a grep of every series message for `(#[0-9]+)` in the subject, `(^\|[^[:alnum:]_/-])#[0-9]+` (so `minyek/cosmic-files#12` passes and `See #7` fails) and `Claude-Session`. |
| 14 | `rpmdev-vercmp` of the old and new versions. This holds by construction: the snapshot day is the newest commit's date, and the distance only grows (10 + the series length, against 35). It is checked anyway. |

## Reviews and their order

No code behaviour changes, so the per-task reviews and the per-file clean-code stage do not apply.
These run instead:
- **Series review** after task 5. It covers architecture and coverage: AC 1–7 and 12. It reads a
  sample of commits end to end, plus all of the following in full:
  - every D2 group and listed line;
  - the commits of the six issues that name no tests (#14, #22, #29, #31, #34, #56), whose
    assignment no named test pins.
- **Final review**, fresh and clause by clause, of AC 1–14. It runs before the gate and the push.
  It judges AC 10, 11 and 14 on their dry-run evidence, and those three are confirmed after the
  push.
- **Gate** last, on the reviewed final tree (AC 8).

## Task list

1. **Inventory.** Assign every hunk of the net diff to an issue, a D2 group or a fork-only bucket,
   and split a hunk where two issues share it. The result is a mapping file in `docs/plans/`.
   Subagents work per file, guided by the issue texts.
   - A line that fits no issue but is a fix: create its issue (AC 3), then go on.
   - A line that fits nothing and is not a fix: this is an unmet-AC question for the user, under
     the unmet-AC gate.
2. **Dependencies and order.** Build the dependency graph and fix the order (D4, including the
   #56/#57 check). List the groups, the listed lines and the subsumed issues. All of this goes in
   the plan.
3. **Build the series** from `e03001c` by the mapping, editing by hand the hunks that must be split.
4. **Per-commit checks** (D5(c), D6) under `arm-run`, after measuring the D5(c) warning
   baselines. Fix a failure by moving hunks (AC 6), then
   rerun the checks from that commit on.
5. **Fork-only commits** (D3). Run D5(c) on each of them, then the AC 2 tree check.
6. **Series review.**
7. **The AC 9 commit.**
8. **Final review.**
9. **Gate** (AC 8).
10. **Land** (D7, AC 10).
11. **Issues and RPM** (AC 11, 14), and confirm AC 13.
12. **Cleanup**, which `/sqmerge` would otherwise do: harvest the journal, remove the handoff
    file, then remove this worktree and its branches.

This needs a plan ([the plan](history-rebuild-plan.md)), because tasks 1 and 3 run as subagent
work per file and per issue.

## Risks

- **A hunk assigned to the wrong issue.** AC 6 contains this: the issue's own tests must run and
  pass at its commit, and dead code fails the warnings check. AC 7 and the series review back it
  up. AC 2 cannot catch it.
- **Issues too intertwined to split.** D2's groups handle these, each one named and justified.
- **Cost.** Several days of agent time, about 7 h of machine time for the per-commit checks, about
  1 h for the D6 builds, and one final gate of about 8 min.
- **A stale clone that pulls.** Pulling would merge the old history back in. Every such clone
  resets instead (D7).
- **Other landings held for several days** (D12).

## Docs impact

- `CLAUDE.md`: the D10 rule and landing procedure, D11 and D9, and the fork-tooling exemption.
- `docs/memory/upstream-contributions.md`: the history shape, the cherry-pick recipe, and the
  amended "shipped under" line.
- `docs/decisions/2026-10-02-local-integration-baseline.md`: a superseding note.
- `docs/guides/local-build-and-install.md`: the new ancestry.
- `docs/README.md`: index lines for the archived spec and plan.
- `README.md`: any fix lines AC 13 finds missing or wrongly linked.

## Record

Decision panel answers, 2026-11-03:
- Q4 approve the spec as a whole: A, approved.
- Q5 D2: A, D2(b) listed exceptions.
- Q6 D7 exact force push: **B, stop before the push.** Build and verify the series, then stop and
  ask again with the finished series. D7's steps 0-4 run only on a fresh, explicit approval.
- Q7 D8: A, edit the issue bodies (after the landing).
- Q8 D10 and D9: A, D10(a) with D9(a).
- Q9 D11: A, `Assisted-by: Claude`, no `Signed-off-by`.

Tasks 1-9 and 11's dry run proceed. Task 10 (land), the D8 edits and the cleanup wait for the
user's renewed approval of D7.

Later answers and decisions:
- Q10-A (2026-11-03): issues #59–#61 for the four changes that belonged to no issue.
- Q11-B (2026-11-04): thematic groups where issues were cyclically entangled.
- M1 (2026-11-06): #10 shares #49's commit; #10 alone fails clippy's collapsible-if lint, and no
  old commit has one without the other.
- M2 (2026-11-06): #37 joins #32, because its fix lives in #32's module and #32's exit test needs it.
- Review triage (2026-11-07): Q1-A apply all review fixes, including splitting the groups where
  old commits hold the earlier code; Q2-A AC 5's first bullet replaced as above; Q3-A the target
  gains two fixes (restore upstream's `// just one quadrant with overlap` comment; format #45's
  Compress closure); Q4-B the target also moves #42's `is_dir()` check into the blocking rename
  task. The target is therefore `master` plus those three changes, not `master` itself.
- M3 (2026-11-07): #54 stays with #55; #55 rewrote #54's delete entry, callers and test, and
  #54's old form does not fit the final structure without new glue code.
- M4 (2026-11-07): #32, #37, #46 and #33 share a commit; `persistence::flush` was born draining both
  writers, and the picker's Open and Save handlers interleave #32's and #33's lines.
- Landing (2026-11-07): D7's lease and AC 10 use `master`'s tip at landing (765f9d0 today), not
  the target 569ef7d, which is not on origin. D7 step 0 also checks `src/app.rs` and
  `src/operation/mod.rs`, which the checkout changes.
- Round 2 review (2026-11-07): #27 and #36 split (#27's form from f80f57df; app.rs also a4a0c806), #6 split from the
  rescan group, #14 keeps its `Self::delete` call, #1's inserts stay inside `#[cfg(unix)]`.
- Q6-C (2026-11-09): after the 2026-11-08 upstream sync, the commit for #32, #37, #46 and #33 was
  rewritten to collapse its nested `if` in the Save dialog's filename update (`clippy::collapsible_if`,
  carried from the old history), and every later commit was rebuilt on it; `master` was force-pushed
  again. The SHAs in this archive's `results/` and `map/` files are the rebuilt ones; the force-pushed
  tip before Q6 was 8724329a.
