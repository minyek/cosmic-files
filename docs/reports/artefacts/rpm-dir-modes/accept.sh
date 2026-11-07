#!/usr/bin/env bash
# Builds the real spec from a fresh clone of $1 (at its HEAD) inside a setgid directory, as the
# shared checkout is, with the Cargo build swapped for stand-in binaries and the desktop and
# metainfo files build.rs generates (copied from $2, a target/xdgen directory) and the debug
# subpackages turned off, since the stand-ins are not ELF, and prints every
# entry the package records (mode, owner, path), then the directories and any suspicious file.
set -uo pipefail
source_repo="$1"
xdgen_source="$2"
root=/mnt/logs/temp/scratch/rpm-dirmodes/run
chmod -R u+rwx "$root" 2>/dev/null
rm -rf "$root"
mkdir -p "$root"
chmod g+s "$root"
mkdir -p "$root/xdgen"
cp "$xdgen_source"/com.system76.CosmicFiles.desktop "$xdgen_source"/com.system76.CosmicFiles.metainfo.xml "$root/xdgen/"
git clone -q "$source_repo" "$root/clone" &&
    git -C "$root/clone" fetch -q "$source_repo" 'refs/tags/epoch-*:refs/tags/epoch-*' ||
    { echo "setup failed" >&2; exit 1; }
spec="$root/clone/packaging/cosmic-files.spec"
# The script's dirty check skips packaging/**, so the edited spec is allowed. Stand-in binaries are
# not ELF, so no debuginfo package can be made from them.
python3 -I - "$spec" "$root/xdgen" <<'EOF'
import sys
path, xdgen = sys.argv[1:]
text = open(path).read()
build = "just build-release --locked\n"
assert text.count(build) == 1
text = text.replace(build, "mkdir -p target/release\n"
    "for bin in cosmic-files cosmic-files-applet cosmic-files-thumbnailer; do\n"
    "    printf '#!/bin/sh\\n' > target/release/$bin\n"
    "done\n"
    f"mkdir -p target/xdgen && cp {xdgen}/* target/xdgen/\n")
open(path, "w").write("%global debug_package %{nil}\n" + text)
EOF
(cd "$root/clone" && bash packaging/build-rpm.sh > "$root/build.log" 2>&1)
status=$?
echo "build exit $status; commit $(git -C "$root/clone" rev-parse --short HEAD)"
[[ $status -eq 0 ]] || { tail -20 "$root/build.log"; exit 1; }
echo "every entry in the package:"
rpm -qlvp "$root/clone/target/rpm/cosmic-files-local.rpm" 2>/dev/null | awk '{ print "  " $1, $3 ":" $4, $NF }'
echo "directories in the package:"
rpm -qlvp "$root/clone/target/rpm/cosmic-files-local.rpm" 2>/dev/null | awk '/^d/ { print "  " $1, $3 ":" $4, $NF }'
echo "files with setuid/setgid or group/other write:"
rpm -qlvp "$root/clone/target/rpm/cosmic-files-local.rpm" 2>/dev/null |
    awk '/^-/ && ($1 ~ /[sS]/ || substr($1,6,1) == "w" || substr($1,9,1) == "w") { print "  " $1, $NF }'
