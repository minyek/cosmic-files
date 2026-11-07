# RPM build permissions acceptance

`packaging/build-rpm.sh` now leaves everything it creates under `target/rpm/` group-writable, so the
accounts that share the checkout through its group can each build. Before, whichever account built
second failed with "Permission denied" in `rpmbuild/RPMS/x86_64`.

rpmbuild runs its build steps under `umask 022`, strips group write from the unpacked source in
`%setup` and creates `RPMS/<arch>` itself, so a umask in the script cannot reach those entries. The
script sets `umask 0002` for its own files and, on exit, gives group write to every entry the running
account owns under `target/rpm/`.

Evidence: [`artefacts/rpm-permissions/`](artefacts/rpm-permissions/). `accept-real.sh` runs the real
script with the real rpm and rpmbuild in a fresh scratch clone per case, under `umask 022`, with the
spec swapped for a small one shaped like it. `results.txt` holds the runs:

| Case | master f6ae0a3: entries without group write | c96bf5b |
|---|---|---|
| Build failing in `%build` (exit 1) | 519 of 519 | 0 |
| Passing build (exit 0) | 15 of 15 | 0 |
| Passing build with an unreadable directory in `target/rpm` (exit 0, warning) | 14 of 17 | 0 |

A mutant without the trap's warning fallback turned the unreadable case's exit 0 into 1.

Limits: one account cannot run a second account's build, so the runs show the group-writable property,
not a cross-account build. A build killed with SIGKILL skips the exit step. The full Cargo build was not
run; the stand-in spec writes into `$CARGO_HOME` and the unpacked tree the same way.
