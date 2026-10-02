# Local build and installation

Use the main checkout on branch `master` to build the combined local
improvements after the squash merge. Keep new feature development in isolated
worktrees and merge verified changes back into `master`.

The combined source includes the GUI-thread, file-operation, large-directory
and symlink-folder-badge improvements. Backup branches are historical snapshots,
not additional improvements to merge.

## Run

From the main checkout, launch the release build:

```sh
./target/release/cosmic-files --no-daemon
```

The application uses your normal preferences and files. Building it does not
replace the installed application.

To install a completed release build, run from the main checkout:

```sh
sudo just install
```

This installs the application, applet, thumbnailer and application resources.
Close and reopen Files afterward to run the installed build.

## Build and verify

Keep the Cargo cache and build directory within the checkout:

```sh
export CARGO_HOME="$PWD/.cargo/home"
export CARGO_TARGET_DIR="$PWD/target"
just build-release --locked --offline -j8
```

The release recipe builds the application, applet and thumbnailer. The canonical
test matrix is:

```sh
cargo test --locked --offline --verbose --no-default-features
cargo test --locked --offline --verbose
cargo test --locked --offline --verbose --all-features
```

Run full gates through the resource-managed detached workflow required by the
project's operating instructions. Dependencies must be available in the local
cache for offline builds.

For future updates, check branch contents as well as commit ancestry: the
original improvement branches contain rebased copies, and the squash merge
does not retain those commits as ancestors. Repeat the test matrix and release
build after integrating new changes.
