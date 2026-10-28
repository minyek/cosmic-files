# cosmic-files
File manager for the COSMIC desktop environment

## Fork improvements

Each change below is tracked as an issue in this repository, with its cause, its fix and the tests that pin it.

### Symlink badges

- Symlinked folders carry a link badge in list, grid and compact views and while dragging, so a link can be told from a real folder. ([#2](https://github.com/minyek/cosmic-files/issues/2))

### Large-directory performance

Large folders open, scroll and update faster.

- Reading a file in the open folder no longer rescans it; a write refreshes only that entry, and only added, removed or renamed entries rescan. ([#3](https://github.com/minyek/cosmic-files/issues/3))
- Subfolder item counts are read only for visible rows (or all rows when sorting by size), not for every subfolder during the scan. ([#4](https://github.com/minyek/cosmic-files/issues/4))
- A rescan keeps the thumbnails, sizes and checksums of entries that did not change. ([#5](https://github.com/minyek/cosmic-files/issues/5))
- The sort order is cached and rebuilt only when the sorted field changes, not on every frame. ([#6](https://github.com/minyek/cosmic-files/issues/6))
- List and grid views build widgets only for visible rows, with stable ids so focus follows the item while scrolling. ([#7](https://github.com/minyek/cosmic-files/issues/7))
- Text previews are built only for the gallery and the preview pane, and capped at 16 KiB, so scrolling past text files no longer freezes the window. ([#8](https://github.com/minyek/cosmic-files/issues/8))
- Folder entries are processed in parallel, so large folders on slow disks list sooner. ([#9](https://github.com/minyek/cosmic-files/issues/9))
- A file whose name decides its type is typed from the name, without opening it. ([#10](https://github.com/minyek/cosmic-files/issues/10))
- Drawing icons never waits for a file being read by a scan. ([#11](https://github.com/minyek/cosmic-files/issues/11))

### Responsive browsing

The filesystem and metadata work below moved off the UI thread, so slow disks and network mounts no longer freeze the window for it.

- The details pane looks up each distinct owner and group once, instead of once per selected file on every frame, so a large selection no longer freezes the window. ([#1](https://github.com/minyek/cosmic-files/issues/1))
- Whether the Trash is empty is cached and refreshed in the background, so startup and the sidebar no longer walk every mount's trash. ([#12](https://github.com/minyek/cosmic-files/issues/12))
- Navigation resolves paths in the background, and a symlinked favourite stays highlighted. ([#13](https://github.com/minyek/cosmic-files/issues/13))
- Deciding whether files go to the Trash runs in the background, so deleting no longer freezes the window. ([#14](https://github.com/minyek/cosmic-files/issues/14))
- The details pane and the gallery reuse image dimensions instead of re-reading the file. ([#15](https://github.com/minyek/cosmic-files/issues/15))
- The details pane shows network files from cached metadata, without touching the network on every frame. ([#16](https://github.com/minyek/cosmic-files/issues/16))
- A sidebar context menu loads its entry's details once, in the background. ([#17](https://github.com/minyek/cosmic-files/issues/17))
- Name dialogs check for clashes against the listing instead of the disk on every keystroke. ([#18](https://github.com/minyek/cosmic-files/issues/18))
- A launcher's context menu parses its .desktop file once, in the background. ([#19](https://github.com/minyek/cosmic-files/issues/19))
- Opening files reads their types in the background. ([#20](https://github.com/minyek/cosmic-files/issues/20))
- Pasted images, video and text are written in the background, and two pastes never take the same name. ([#21](https://github.com/minyek/cosmic-files/issues/21))
- Clicking a network favourite checks it in the background, and a favourite that is a file opens the file. ([#22](https://github.com/minyek/cosmic-files/issues/22))
- The Open-with application list is built and saved in the background. ([#23](https://github.com/minyek/cosmic-files/issues/23))
- Open in terminal uses a cached default terminal instead of running xdg-mime each time. ([#24](https://github.com/minyek/cosmic-files/issues/24))
- A path typed in the location bar is resolved in the background. ([#25](https://github.com/minyek/cosmic-files/issues/25))
- Open in new tab or window, Rename, Open in the preview and drag-and-drop use the scanned metadata instead of checking the disk again. ([#26](https://github.com/minyek/cosmic-files/issues/26))
- The file watcher is updated in the background, in the main window and the file picker. ([#27](https://github.com/minyek/cosmic-files/issues/27))
- Paths given at launch are resolved in the background, and a missing path opens an empty tab. ([#28](https://github.com/minyek/cosmic-files/issues/28))
- Search results are prepared in the search worker, so large searches no longer stutter. ([#29](https://github.com/minyek/cosmic-files/issues/29))
- Sidebar Open with and Preview, and breadcrumb Preview, read the item in the background. ([#30](https://github.com/minyek/cosmic-files/issues/30))
- Launching a .desktop file parses it in the background. ([#31](https://github.com/minyek/cosmic-files/issues/31))
- Recent files are recorded by one ordered background writer that finishes before the app or the picker closes. ([#32](https://github.com/minyek/cosmic-files/issues/32))
- The open and save dialogs do their file checks in the background. ([#33](https://github.com/minyek/cosmic-files/issues/33))
- The overwrite dialog and the trash watcher keep blocking file calls off the async workers. ([#34](https://github.com/minyek/cosmic-files/issues/34))

### Reliable background work

Background work no longer piles up, applies stale results or grows without bound.

- At most one rescan runs per folder with one follow-up queued, and an older scan can never replace a newer listing. ([#35](https://github.com/minyek/cosmic-files/issues/35))
- A folder whose file watch failed is watched again on the next update. ([#36](https://github.com/minyek/cosmic-files/issues/36))
- Recent files are recorded on a fresh profile, by creating the history file first. ([#37](https://github.com/minyek/cosmic-files/issues/37))
- The operation history keeps the newest 256 entries. ([#38](https://github.com/minyek/cosmic-files/issues/38))
- The gallery clears all of its per-image state together, so memory no longer grows with every image viewed. ([#39](https://github.com/minyek/cosmic-files/issues/39))

### Safe file operations

File operations no longer overwrite or truncate files unexpectedly, and they report what actually happened.

- Move to Trash reports an error when the file could not be trashed, instead of success. ([#40](https://github.com/minyek/cosmic-files/issues/40))
- The failed-operations dialog has a Copy to clipboard button for its error text. ([#41](https://github.com/minyek/cosmic-files/issues/41))
- Rename refuses to replace a file that appeared under the new name. ([#42](https://github.com/minyek/cosmic-files/issues/42))
- Compress never overwrites an existing file, and a failed or cancelled compress leaves no partial archive. ([#43](https://github.com/minyek/cosmic-files/issues/43))
- A compress that fails while finishing the archive reports the error. ([#44](https://github.com/minyek/cosmic-files/issues/44))
- Archives keep files whose names are not valid UTF-8 (tar.gz), or refuse with an error (zip). ([#45](https://github.com/minyek/cosmic-files/issues/45))
- Default-application changes replace mimeapps.list atomically, in order, keeping its permissions and symlink. ([#46](https://github.com/minyek/cosmic-files/issues/46))
- The file picker shows an error when it cannot create a folder. ([#47](https://github.com/minyek/cosmic-files/issues/47))
- Every "Delete permanently" menu entry deletes permanently, and holding Shift offers it in every menu, matching Shift+Delete. ([#54](https://github.com/minyek/cosmic-files/issues/54))
- Files the Trash cannot take, on network mounts or in a folder opened from the Trash, read "Delete permanently" in every menu, and the Delete key no longer moves trashed files back into the Trash. ([#55](https://github.com/minyek/cosmic-files/issues/55))

### Other fixes

- Trashed images show their own dimensions. ([#48](https://github.com/minyek/cosmic-files/issues/48))
- Files without an extension on network mounts and in the Trash are no longer shown as empty files. ([#49](https://github.com/minyek/cosmic-files/issues/49))
- Files in network listings get their real type instead of all being folders. ([#50](https://github.com/minyek/cosmic-files/issues/50))
- The launcher menu appears only for a single selected .desktop file. ([#51](https://github.com/minyek/cosmic-files/issues/51))
- Default terminal ids keep a .desktop segment in the middle of the name. ([#52](https://github.com/minyek/cosmic-files/issues/52))
- A launcher action always runs the command its menu showed. ([#53](https://github.com/minyek/cosmic-files/issues/53))

See the [local build and installation guide](docs/guides/local-build-and-install.md) for this fork's release workflow.

## Build the project from source

```sh
# Clone this fork using `git`
git clone https://github.com/minyek/cosmic-files.git
# Change to the directory that was created by `git`
cd cosmic-files
# Build an optimized version using `cargo`, this may take a while
cargo build --release
# Run the optimized version using `cargo`
cargo run --release
```

## Community and Contributing

The COSMIC desktop environment is maintained by System76 for use in Pop!_OS. A list of all COSMIC projects can be found in the
[cosmic-epoch](https://github.com/pop-os/cosmic-epoch) project's README. If you would like to discuss COSMIC and Pop!_OS, please
consider joining the [Pop!_OS Chat](https://chat.pop-os.org/). More information and links can be found on the
[Pop!_OS Website](https://pop.system76.com).

## License

This project is licensed under [GPLv3](LICENSE)
