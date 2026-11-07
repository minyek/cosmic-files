#!/usr/bin/env bash
set -Eeuo pipefail
# More than one account builds in this checkout, sharing it through its group. umask 0002 covers
# what this script creates; rpmbuild resets the umask to 022 in its build steps, so on exit
# everything this account owns under target/rpm gets group write.
umask 0002

script_dir=$(dirname -- "${BASH_SOURCE[0]}")
repo_root=$(git -C "$script_dir/.." rev-parse --show-toplevel)
spec_file="$repo_root/packaging/cosmic-files.spec"
rpm_output="$repo_root/target/rpm"
topdir="$rpm_output/rpmbuild"

share_build_output() {
    if [[ -d $rpm_output ]]; then
        find "$rpm_output" -user "$(id -u)" ! -perm -g+w -exec chmod g+w {} + ||
            printf 'Warning: could not give group write to everything under %s\n' "$rpm_output" >&2
    fi
}
trap share_build_output EXIT

if ! git -C "$repo_root" diff --quiet HEAD -- . ':(exclude)docs/**' ':(exclude)packaging/**'; then
    printf 'Refusing to build from modified application files. Commit or revert them first.\n' >&2
    exit 1
fi

untracked_files=$(git -C "$repo_root" ls-files --others --exclude-standard)
untracked_application_files=$(printf '%s\n' "$untracked_files" | awk 'NF && $0 !~ /^docs\// && $0 !~ /^packaging\// && $0 !~ /^\.idea\// && $0 !~ /^\.private-journal\// && $0 !~ /\.md$/')
if [[ -n $untracked_application_files ]]; then
    printf 'Refusing to build with untracked application files; commit them first:\n%s\n' "$untracked_application_files" >&2
    exit 1
fi

commit=$(git -C "$repo_root" rev-parse HEAD)
description=$(git -C "$repo_root" describe --tags --long --abbrev=12 --match 'epoch-*' "$commit" 2>/dev/null) || {
    printf 'No reachable epoch-* Git tag was found. Fetch the upstream epoch tags, then retry:\n' >&2
    printf "  git fetch upstream 'refs/tags/epoch-*:refs/tags/epoch-*'\n" >&2
    printf 'This build does not use Cargo.toml as a version fallback.\n' >&2
    exit 1
}

if [[ ! $description =~ ^epoch-(.+)-([0-9]+)-g([[:xdigit:]]+)$ ]]; then
    printf 'Cannot parse Git description: %s\n' "$description" >&2
    exit 1
fi

tag_version=${BASH_REMATCH[1]}
distance=${BASH_REMATCH[2]}
short_commit=${BASH_REMATCH[3]}

if [[ ! $tag_version =~ ^[0-9]+(\.[0-9]+)*(-[0-9A-Za-z.-]+)?$ ]]; then
    printf 'Unsupported epoch tag version: %s\n' "$tag_version" >&2
    exit 1
fi

rpm_base_version=${tag_version/-/~}
commit_epoch=$(git -C "$repo_root" show -s --format=%ct "$commit")
tag_name="epoch-$tag_version"
snapshot_epoch=$(
    {
        git -C "$repo_root" log -1 --format=%ct "$tag_name"
        git -C "$repo_root" log --format=%ct "$tag_name..$commit"
    } | awk '$1 > newest { newest = $1 } END { if (newest) print newest }'
)
snapshot_day=$(date -u -d "@$snapshot_epoch" '+%Y%m%d')
rpm_version="${rpm_base_version}^git${snapshot_day}.${distance}.g${short_commit}"
commit_date=$(date -u -d "@$commit_epoch" '+%Y-%m-%d %H:%M:%S UTC')
arch=$(rpm --eval '%{_arch}')

printf 'RPM version: %s\n' "$rpm_version"
mkdir -p "$topdir/BUILD" "$topdir/RPMS" "$topdir/SOURCES" "$topdir/SRPMS" "$rpm_output/cargo-home"
git -C "$repo_root" archive --format=tar --prefix=cosmic-files/ "$commit" | gzip -n > "$topdir/SOURCES/cosmic-files-source.tar.gz"

env -u CARGO_TARGET_DIR CARGO_HOME="$rpm_output/cargo-home" rpmbuild -bb "$spec_file" \
    --define "_topdir $topdir" \
    --define "rpm_version $rpm_version" \
    --define "rpm_base_version $rpm_base_version" \
    --define "source_commit $commit" \
    --define "source_commit_date $commit_date"

release="1$(rpm --eval '%{?dist}').local"
rpm_file="$topdir/RPMS/$arch/cosmic-files-$rpm_version-$release.$arch.rpm"
if [[ ! -f $rpm_file ]]; then
    printf 'rpmbuild completed but did not produce the expected RPM: %s\n' "$rpm_file" >&2
    exit 1
fi

local_rpm="$rpm_output/cosmic-files-local.rpm"
install -m 0664 "$rpm_file" "$local_rpm"
printf 'Built RPM: %s\n' "$rpm_file"
printf 'Installable path: %s\n' "$local_rpm"
