# Project memory

- [Background work and acceptance](docs/memory/background-work-and-acceptance.md): request ownership, persistence ordering, metadata provenance and isolated validation.
- [Upstream contributions](docs/memory/upstream-contributions.md): no PRs or issues upstream; fixes stay in the fork, each with a README line linked to its issue.

# History shape: upstream plus one commit per issue

`master` is upstream plus one self-contained commit per fork issue, then the fork's own records.
Keep it that way, so any fix can be cherry-picked onto upstream with only the commits it depends on.

- **One issue per branch.** A fix found along the way gets its own issue (related fixes may share
  one) and its own branch, landed before the branch that needs it. In this project this replaces
  the global rule that folds a gate-blocking or related clean-up fix into the current ticket. A
  boy-scout clean-up stays in the issue's commit only when it is inside the code the issue touches
  and changes no behaviour; any other clean-up is its own issue. Work in progress never reaches
  `master`: no change is added and then changed or removed by a later commit of the same branch.
- **Landing replaces `/sqmerge` step 7.** Run `/sqmerge` steps 1–6, land with
  `bash scripts/land.sh <main-checkout> <worktree> <code-message-file> <records-message-file> --push`
  in place of step 7, then run steps 8–11. It makes a code commit with only the branch's
  upstream-shaped paths (`src/`, `tests/`, `i18n/`, `res/`, `Cargo.toml`, `Cargo.lock`, `TESTING.md`)
  and then a records commit with the fork-only paths (`docs/`, `packaging/`, `README.md`,
  `CLAUDE.md`, and the fork's acceptance scripts and examples); a branch with only one kind lands as
  one commit. A path on neither list (upstream's `build.rs`, applet, `debian/`, `.github/` and so
  on) stops the landing until it is added to one. It
  keeps step 7's guards: `master` must be an ancestor of the branch, the landed changes must equal
  the branch's, the main checkout must be on `master`, which moves only by `merge --ff-only`, and
  the push is plain.
- **Code commit messages.** Subject: an imperative summary with no `(#N)`, since upstream uses that
  for its own pull requests. Body: the issue's problem, cause and fix, the tests that pin it, and
  `Depends on:` the issues of the commits it needs. Write fork issues as `minyek/cosmic-files#N`,
  never a bare `#N`. Trailers: one `Fork-issue: minyek/cosmic-files#N` per issue, then
  `Assisted-by: Claude`. No session URL and no `Signed-off-by`: that is the user's own sign-off,
  added only if they ever submit. Code and comments carry nothing fork-specific (no issue numbers,
  no `minyek` URL, no `docs/` path).
- **Syncing upstream.** Keep `/sync-upstream`'s merge. An adaptation the sync forces on fork code
  that does not conflict lands right after the merge, one commit per adapted issue, by the rules
  above. A conflict resolution has to live in the merge commit; the affected issue's commit then
  needs re-resolving whenever it is cherry-picked for a pull request.
- **Fork tooling is exempt.** Packaging, acceptance scripts and their examples, and docs have no
  issue; they land as records commits.
- **To prepare a pull request** for an issue: cherry-pick the commits named by its `Depends on:`
  line (recursively) and then its own commit onto the upstream of the day.

# Install after merge

After every change lands on `master` (`scripts/land.sh`, `/sync-upstream`), build the RPM from the main
checkout without waiting to be asked. The build takes several minutes, so launch it detached and
wait with the watcher `arm-run` prints:

```sh
git -C /mnt/work/projects/cosmic-de/cosmic-files fetch upstream 'refs/tags/epoch-*:refs/tags/epoch-*'
arm-run -m 20G -l cosmic-files-rpm -C /mnt/work/projects/cosmic-de/cosmic-files -- /mnt/work/projects/cosmic-de/cosmic-files/packaging/build-rpm.sh
```

Installing needs root, so it is the user's step: report it as not live until they run
`sudo dnf5 install /mnt/work/projects/cosmic-de/cosmic-files/target/rpm/cosmic-files-local.rpm`,
then close and reopen Files. Details: [local build and install](docs/guides/local-build-and-install.md).
