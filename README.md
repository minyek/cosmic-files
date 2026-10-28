# cosmic-files
File manager for the COSMIC desktop environment

## Fork improvements

This fork adds:

- **Symlink folder badges** in list and grid views.
- **Better large-directory performance** through parallel scanning, cached sorting, and rendering only visible rows.
- **More responsive browsing** by moving filesystem and metadata work off the UI thread and coalescing filesystem watcher updates.
- **A responsive properties pane when many files are selected**, by looking up each distinct owner and group once instead of once per selected file on every frame ([#1](https://github.com/minyek/cosmic-files/issues/1)).
- **More reliable background work**, including protection against stale results, safer file-picker saves, and completing queued persistence writes before clean exit.
- **Safer file operations**, with overwrite protection, failure-safe archive creation, and visible, copyable error messages.

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
