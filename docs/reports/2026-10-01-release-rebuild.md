# Release rebuild

Outcome: succeeded. `just build-release --locked -j12` exited with status 0 after 623 seconds and rebuilt all three executables in this worktree's `target/release/`.

The [complete build log](artefacts/2026-10-01-release-rebuild/build.log) records successful release compilation of the application at line 555, the applet at line 705, and the thumbnailer at line 759. No compiler warnings or errors appeared.

| Executable | Bytes |
| --- | ---: |
| `cosmic-files` | 66037696 |
| `cosmic-files-applet` | 51772680 |
| `cosmic-files-thumbnailer` | 6376336 |

Acceptance: ran the complete `just install` recipe with a temporary staging root. It exited with status 0 and installed all three binaries, desktop metadata, thumbnailer registration, and icons. The [installation check log](artefacts/2026-10-01-release-rebuild/install-check.log) retains the output.

The missing thumbnailer source is now present in the default build directory. System installation remains a user action: run `sudo just install` from this worktree. No system files were replaced, no source code changed, and no test-suite or GUI validation was performed as part of this rebuild.
