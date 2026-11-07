# Combined testing worktree

Outcome: succeeded. The combined worktree is
`.worktrees/integration-testing`, on `integration/local-testing`.

## Included changes

- `fix-large-directory-scroll-lag` at `2ab787e`: scrolling improvements,
  GUI-thread work, file-operation fixes and their subsequent corrections.
- `feat/symlink-folder-badges` at `5783c59`: directory symlink badges.

Independent source review confirmed that all 20 commits from the current
GUI-thread branch and all 12 commits from the preexisting-fixes branch have
counterparts in the scrolling branch. Differing patch identities reflect
rebased context and native context-menu adaptation. All 57 preexisting-fix test
names remain; one of 78 GUI test names was deliberately replaced by a later
fix that preserves parent components across symlinks.

The merge retained both Item methods and both sets of GIO tests where additions
overlapped. The review found no missing improvement or merge blocker. The
existing source worktrees and installed application were preserved.

## Verification

The detached gate ran the symlink integration tests, the repository's complete
CI feature matrix, and `just build-release --locked --offline -j8`. It finished
with status 0 after 738 seconds.

| Check | Result |
| --- | --- |
| Symlink integration acceptance | 6 passed |
| No default features | 227 unit + 5 integration tests passed |
| Default features | 242 unit + 6 integration tests passed |
| All features | 242 unit + 6 integration tests passed |
| Release build | Application, applet and thumbnailer built |
| Changed Rust files formatting | Passed; stable rustfmt reports the existing nightly-only configuration setting |
| Staged diff whitespace | Passed |
| Installation into a temporary staging root | Passed; all three executable hashes match the build outputs |

The acceptance tests render the real grid and list views at three zoom levels,
and cover local, search, GIO and Trash directory symlinks. The no-default-feature
build emits unused-import, unused-variable and dead-code warnings. No compiler
warnings were emitted by the default, all-features or release builds.

The [retained validation summary](artefacts/integration-testing/validation.txt)
records the test results and exit statuses. The
[staged installation log](artefacts/integration-testing/install.log) records the
packaging check. The [binary manifest](artefacts/integration-testing/binaries.json)
identifies the release artifacts. No interactive desktop session was launched;
the release build is ready for user testing using the
[local build guide](../guides/local-build-and-install.md).
