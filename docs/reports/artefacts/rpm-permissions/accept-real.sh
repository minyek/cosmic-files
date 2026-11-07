#!/usr/bin/env bash
# Runs packaging/build-rpm.sh from a fresh clone of $1 (at its HEAD) for each case, under umask 022
# with the real rpm and rpmbuild, the spec swapped for a small one shaped like it (%setup, a
# CARGO_HOME write, %install). Cases: a build failing in %build, a passing build, and a passing build
# whose target/rpm holds a directory the build cannot read. Each case reports the exit status and
# what the build left under target/rpm that the group cannot write.
set -uo pipefail
source_repo="$1"
root=/mnt/logs/temp/scratch/rpm-umask/real
chmod -R u+rwx "$root" 2>/dev/null
rm -rf "$root"
mkdir -p "$root"

spec() {
    cat <<'EOF'
%global debug_package %{nil}
Name:           cosmic-files
Version:        %{rpm_version}
Release:        1%{?dist}.local
Summary:        Permission acceptance stand-in
License:        GPL-3.0-only
Source0:        cosmic-files-source.tar.gz

%description
Permission acceptance stand-in.

%prep
%setup -q -n cosmic-files

%build
mkdir -p "$CARGO_HOME/registry/cache/index" target/release
echo crate > "$CARGO_HOME/registry/cache/index/demo.crate"
echo binary > target/release/demo
[ -z "${DEMO_FAIL:-}" ] || exit 1

%install
mkdir -p %{buildroot}%{_bindir}
install -m 0755 target/release/demo %{buildroot}%{_bindir}/demo

%files
%{_bindir}/demo
EOF
}

run_case() {
    local name="$1" fail="$2" unreadable="$3" clone="$root/$1"
    git clone -q "$source_repo" "$clone" &&
        git -C "$clone" fetch -q "$source_repo" 'refs/tags/epoch-*:refs/tags/epoch-*' ||
        { echo "$name: setup failed, no result" >&2; return 1; }
    # The script's dirty check skips packaging/**, so the swapped spec is allowed.
    spec > "$clone/packaging/cosmic-files.spec"
    if [[ $unreadable == yes ]]; then
        mkdir -p "$clone/target/rpm/locked/inner"
        chmod 0700 "$clone/target/rpm/locked/inner"
        chmod 0000 "$clone/target/rpm/locked"
    fi
    (umask 022 && cd "$clone" && DEMO_FAIL="$fail" bash packaging/build-rpm.sh > "$root/$name.log" 2>&1)
    local status=$?
    [[ $unreadable == yes ]] && chmod 0755 "$clone/target/rpm/locked"
    local left
    left=$(find "$clone/target/rpm" ! -path '*/locked*' ! -perm -g+w | wc -l)
    echo "$name: exit $status; entries $(find "$clone/target/rpm" | wc -l); not group-writable $left"
    grep -h '^Warning' "$root/$name.log" | sed 's/^/  /'
}

run_case failing 1 no
run_case passing "" no
run_case unreadable-dir "" yes
