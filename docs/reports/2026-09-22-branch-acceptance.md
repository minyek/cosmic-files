# Branch acceptance validation

**Earlier acceptance snapshot:** the PR-readiness repairs in `3778fee` require the fresh matrix and real acceptance reruns tracked in [the readiness report](2026-09-22-pr-readiness.md). The results below are provisional for the final PR until those reruns complete.

This validation extends the six review repairs beyond their isolated tests. The checks use private profiles, displays, D-Bus sessions, and loopback server fixtures. They do not change the user's desktop session, recents history, or trash contents.

## Additional defects found

**Fresh-profile recents initialization.** The real XBEL integration initially failed because `recently-used-xbel` requires an existing history file. The writer now initializes an empty history without replacing existing contents. Independent review identified partial-write poisoning and missing diagnostics in the first initializer; initialization now writes a temporary file and publishes it atomically without clobbering an existing file, and failures are logged. This affects first-use recents operations. No deployed binary or existing history was changed; failed historical recordings cannot be reconstructed automatically.

**Changing an open context menu.** The private X11 session reproduced a panic when a launcher parse changed the menu's action list while its old layout remained active. Each X11 menu invocation now retains a stable action snapshot, and the next opening uses completed metadata. Unit coverage checks this lifecycle, and the GUI sequence exercises pending, invalid, directory, and successfully parsed launcher menus. The trade-off is that an already-open pending X11 menu stays ordinary until reopened. Native Wayland popups use the toolkit's corrected refresh behavior instead.

**Rapid-click Wayland popup state.** Nested COSMIC testing exposed a further stale-popup-tree panic with back-to-back synthetic press/release events. The lockfile now selects the [upstream libcosmic correction](https://github.com/pop-os/libcosmic/commit/34caca138738af92f4b8fc60db52a82812161345) from 2026-09-21. It brings eight toolkit commits since the old revision, including an Iced update and updated AccessKit, winit, and settings-config revisions. The full feature matrix and rapid-click replay pass with the updated dependency. Vendoring the toolkit was rejected because it would duplicate several megabytes of dependency source and assets.

## Checks and current evidence

| Surface | Acceptance check | Status |
| --- | --- | --- |
| Recents | Real record/clear/record/remove queue against XBEL, then the application's recents scan | Passed; injected write failures leave no partial history and normal initialization recovers |
| Trash | Real delete, application's trash listing, restore, and byte equality | Passed |
| X11 GUI | Directory/invalid/valid launcher menus, image details `320×180`, 20,000-file list/grid scrolling, watcher creation/removal | Passed, including back-to-back press/release events |
| File picker | Navigate a 20,000-file directory, reach its final sorted row, observe watcher mutation, return a selected file | Passed; returned the expected final row under modified-descending sorting |
| Live remote filesystem | Private FTP/GVFS: initial/nested/create/remove scans through both production URI entry points | Passed all four stages, including equality with the mounted FUSE-path scan |
| Wayland GUI | Private nested COSMIC compositor using software rendering | Passed rapid-click sequence, launcher menu, image dimensions, last list/grid rows and watcher mutation |
| Performance | Three repetitions, 600 measured frames after 60 warm-up frames, 1,000 versus 20,000 files, list and grid | Pending exclusive run |
| Functional matrix | Minimal, default, all features; current application and acceptance examples rebuilt | Passed: 206 tests per configuration, no failures or ignored tests; all acceptance example builds succeeded |

The frame probe measures element construction, tree diff, layout, wheel/motion dispatch, and drawing-command generation. It does not measure GPU presentation or physical display latency. Before running, the interpretation is: a 95th-percentile frame cost below 16.7 ms in both sizes supports staying within a 60 Hz CPU frame budget in this probe; approximately linear 20-fold growth would contradict the intended viewport-bound work. Timing tests remain outside the routine functional gate.

## Reproduction

- Build `storage_acceptance`, `remote_scan_acceptance`, and `dialog` examples with the default features.
- `scripts/run_isolated_acceptance.sh storage OUTPUT STORAGE_BINARY` runs real storage checks under Bubblewrap. `storage-failure` injects failed writes and checks that initialization can recover.
- `scripts/run_isolated_acceptance.sh gui OUTPUT APP_BINARY XVFB_BINARY` runs the private X11 sequence. Add `--wayland` for nested COSMIC and `--click-hold 0` for a back-to-back press/release replay. Use the dialog example binary with `--dialog` for the picker sequence.
- `scripts/run_remote_acceptance.sh OUTPUT REMOTE_PROBE_BINARY SERVER_PYTHON` uses a Python environment with `pyftpdlib` 2.2.0 and the system's GIO bindings. It requires real GVFS FUSE-path coverage.
- Build `scroll_frame_probe` with `--no-default-features`, then launch `scripts/run_scroll_performance.sh BINARY SMALL_DIRECTORY LARGE_DIRECTORY` using `arm-run --exclusive`; output goes to the job's log directory.

The GUI runner requires Xvfb, Python Xlib/Pillow, Bubblewrap, and D-Bus; the Wayland variant also requires COSMIC compositor and Mesa software EGL. The Xvfb package was extracted into scratch storage, not installed into the system. Dependency translation assets are mounted read-only because debug builds load them from their source checkout.

## Limits

Screenshots were inspected for actual UI state, rather than treating process survival as proof. This caught a harness selection error and an unmaximized window; the corrected replay selects the image with the keyboard and verifies window setup. Earlier misleading screenshots are excluded from the accepted evidence. The harness automatically checks process survival and picker return; content checks additionally require visual inspection.

Durable evidence is in [the acceptance artifacts](artefacts/scroll-acceptance/): real storage and failure/recovery logs, remote scan JSON, and selected GUI screenshots. The final Wayland-only menu change does not affect the already-passing X11, picker, storage, or remote paths. No deployed binary, user's history/trash, external run, data/tensor cache, or shipped artifact was modified. No existing performance conclusion is silently replaced: the fresh measurements below have their own stated scope.

Loopback FTP covers real protocol, GVFS, and mount behavior, but not WAN latency or every SMB/SFTP/NFS server. Software-rendered X11 and nested Wayland exercise real widgets and input dispatch but do not establish physical-GPU performance. These distinctions remain explicit even after the acceptance checks pass.
