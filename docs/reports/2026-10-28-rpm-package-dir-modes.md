# RPM package directory modes

The RPM is built inside the shared checkout, whose directories carry the setgid bit, so the two
directories the package owns were recorded and installed as `drwxr-sr-x`:
`/usr/share/doc/cosmic-files` and `/usr/share/licenses/cosmic-files`. The installed package built on
2026-10-28 shows them so (`rpm -qlv cosmic-files`). Directories rpm records with a fixed mode, such as
`/usr/lib/.build-id/*`, were already `drwxr-xr-x`, although they are setgid in the build root too.

`%defattr(-,root,root,0755)` in the spec's `%files` records the package's directories as `0755` and
keeps each file's own mode.

Evidence: [`artefacts/rpm-dir-modes/`](artefacts/rpm-dir-modes/). `accept.sh <repo> <xdgen-dir>`
builds the real spec with the real script, rpm and rpmbuild from a fresh clone inside a setgid
directory. `%build` is swapped for three stand-in binaries plus the desktop and metainfo files that
`build.rs` generates, copied from a `target/xdgen` directory, and the debug subpackages are turned off
(`%global debug_package %{nil}`), since the stand-ins are not ELF. The `%install`, `%check` and `%files`
sections are the real ones. `rpm -qlvp` of the result, as mode, owner and path:

- `before.list` (master, before this change): both package directories `drwxr-sr-x`.
- `after.list` (with `%defattr`): both `drwxr-xr-x`; the 15 files' modes and owners are identical.

Limits. The debug subpackages were checked with a small C package built in a setgid tree with
debuginfo on, not with the real Cargo build. To reproduce: in a directory with `chmod g+s`, write a
spec whose `%build` compiles a one-file C program with `%{optflags}` and whose `%files` has
`%defattr(-,root,root,0755)`, build it with `rpmbuild -bb --define "_topdir $PWD/top"`, and read the
subpackages with `rpm -qlvp`.

- The debuginfo subpackage records its directories as `0755`, with or without this change.
- The debugsource subpackage records its `/usr/src/debug/<name>-*` directories as `drwxr-sr-x`, with or
  without this change. Its `%files` comes from rpm's `%_debugsource_template` (`/usr/lib/rpm/macros`),
  which has no `%defattr`, and `find-debuginfo` (debugedit 5.3) resets directory modes with
  `chmod 0755`, which GNU chmod applies without clearing a directory's setgid bit (`00755` would). The
  same holds for `cosmic-files-debugsource` by inference. Clearing
  setgid on the build root in `%install` would fix it but gives new entries the builder's primary
  group, which breaks the shared-checkout cleanup, so the fix belongs in debugedit. The debugsource
  RPM is not the one `build-rpm.sh` copies to `cosmic-files-local.rpm`.
