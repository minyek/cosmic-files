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

## Build and install a Fedora 45 RPM

Prepare the Fedora build dependencies:

```sh
sudo dnf5 install git rpm-build rust cargo just glib2-devel libxkbcommon-devel lld desktop-file-utils
```

From the checkout root, refresh upstream version tags if needed and build the
RPM:

```sh
git fetch upstream 'refs/tags/epoch-*:refs/tags/epoch-*'
./packaging/build-rpm.sh
```

The build uses the nearest reachable `epoch-*` tag and the current Git commit.
For example, commits after `epoch-1.11.0` receive versions such as
`1.11.0^git20261023.7.g<commit>`. The UTC commit date follows COSMIC's
date-based snapshot convention; the Git distance orders snapshots from the
same day. The date uses the newest timestamp from the tag and its descendants,
so it cannot move backward from the tag as the branch advances. These versions
sort above `1.11.0`. The
source archive comes from this checkout's committed files, so the fork's
committed changes are included. The
build stops if application files are modified or untracked, and writes its RPM
build files and Cargo cache under `target/rpm/`. Cargo downloads Rust
dependencies on the first build when they are not cached.

The named RPM is under `target/rpm/rpmbuild/RPMS/`, in the directory for the
current architecture. The build also copies it to the stable path
`target/rpm/cosmic-files-local.rpm`. Install or update it from the checkout root:

```sh
sudo dnf5 install ./target/rpm/cosmic-files-local.rpm
```

If no reachable `epoch-*` tag is available, the build stops with a message to
fetch the upstream tags. It does not use `Cargo.toml` as a fallback version
source; a source archive without Git tags cannot produce a versioned RPM.

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
