# Local integration and release delivery

All local improvements were squash-merged into `master` at `66dac3f` after
[acceptance closure](2026-10-02-acceptance-closure.md). The resulting tree exactly
matches accepted preparation commit `fd49259`.

The squash was created in a clean, root-owned worktree and main advanced by
fast-forward. The [baseline decision](../decisions/2026-10-02-local-integration-baseline.md)
records the preserved upstream history and the scope of this merge. No push was
performed.

The release build ran from `master` at `66dac3f` with `just build-release
--locked --offline -j8`, reusing the completed preparation cache. All three
optimized executables built successfully and were copied into main's standard
`target/release/` directory. The default install recipe passed in a temporary
root; installed executable hashes matched the release files and required
application resources were present. Main now also has its own offline Cargo
cache under `.cargo/home/`.

The [build and install evidence](artefacts/main-release/) records the full
source commit, commands, binary sizes and SHA-256 hashes. An initial launch
stopped before building because its commit guard received an abbreviated ID;
the successful launch used the full ID.

The main checkout's tracked source still matches the accepted source. Final
report updates are documentation only. No live installation was performed.
From the main checkout, install the completed release with:

```sh
sudo just install
```

Seven clean, idle temporary worktrees were removed. The original GUI-thread,
preexisting-bug and large-directory worktrees were preserved with their local
notes, acceptance files and IDE state. Historical improvement branches were
retained. All three release binaries still match the recorded hashes, and their
dynamic dependencies resolve after temporary cache/worktree removal.

The shared resource launcher's empty-slice counting defect remains outside this
repository. The isolated correction used for the exclusive measurement and the
exact owner follow-up are recorded in the [acceptance report](2026-10-02-acceptance-closure.md).
