#!/usr/bin/bash
# land.sh <main-checkout> <worktree> <code-message-file> <records-message-file> [--push]
#
# Lands one issue branch on master as at most two commits, replacing /sqmerge step 7 in this fork:
#   1. a code commit: master plus the branch's changes to upstream-shaped paths, its message checked
#      by check_code_message;
#   2. a records commit: the rest of the branch (README line, docs/, archived spec), on top.
# A branch with only one kind of change lands as one commit; a path on neither list stops it. It
# refuses a branch that does not descend from master or that holds a merge commit, and prints the
# restack command. The landed changes must equal the branch's own, the main checkout must be on
# master and moves only by `merge --ff-only`, and --push pushes plainly to origin, only where an
# upstream remote exists and origin is not it.
set -euo pipefail

MAIN=$1 WT=$2 CODE_MSG=$3 RECORDS_MSG=$4 PUSH=${5:-}
UPSTREAM_SHAPED='^(src/|tests/|i18n/|res/|Cargo\.toml$|Cargo\.lock$|TESTING\.md$)'
# The fork's own records. A path on neither list stops the landing: upstream also owns build.rs, the
# applet and thumbnailer crates, debian/, justfile and .github/, which need a deliberate choice.
FORK_ONLY='^(docs/|packaging/|README\.md$|CLAUDE\.md$|scripts/(land\.sh|tests/land_test\.sh|gui_acceptance\.py|remote_acceptance\.py|run_isolated_acceptance\.sh|run_remote_acceptance\.sh|run_scroll_performance\.sh)$|examples/(storage_acceptance|remote_scan_acceptance|scroll_frame_probe)\.rs$)'

cd "$WT"
M=$(git rev-parse refs/heads/master)
# The restack command: <old base> is the parent of the branch's own commits on its first-parent line.
# Backup tags hold every master a sync replaced, so a branch cut before a sync counts its own commits
# by them.
restack_command() {
    local own
    own=$(git rev-list --count --first-parent HEAD --not "$M" --tags='backup/main-pre-sync-*')
    echo "git rebase --autosquash --onto master $(git rev-parse "HEAD~$own")"
}
git merge-base --is-ancestor "$M" HEAD || {
    echo "master moved: restack the branch onto it ($(restack_command)), never merge master into it"
    exit 1
}
[ -z "$(git rev-list --merges "$M"..HEAD)" ] || {
    echo "the branch holds a merge commit: restack it linear ($(restack_command)) before landing"
    exit 1
}

check_code_message() {
    local subject
    subject=$(head -n1 "$1")
    if grep -qE '\(#[0-9]+\)' <<<"$subject"; then echo "code message: (#N) in the subject"; exit 1; fi
    if grep -qE '(^|[^[:alnum:]_/-])#[0-9]+' "$1"; then echo "code message: bare #N; write minyek/cosmic-files#N"; exit 1; fi
    if grep -q '^Claude-Session' "$1"; then echo "code message: Claude-Session line"; exit 1; fi
    if grep -qi '^Signed-off-by:' "$1"; then echo "code message: Signed-off-by is the user's own sign-off"; exit 1; fi
    grep -qx 'Assisted-by: Claude' "$1" || { echo "code message: no Assisted-by: Claude trailer"; exit 1; }
    grep -qE '^Fork-issue: minyek/cosmic-files#[0-9]+$' "$1" || { echo "code message: no Fork-issue trailer"; exit 1; }
}

mapfile -d '' -t CHANGED < <(git diff -z --name-only --no-renames "$M" HEAD)
CODE=() RECORDS=()
for p in "${CHANGED[@]}"; do
    if grep -qE "$UPSTREAM_SHAPED" <<<"$p"; then
        CODE+=("$p")
    elif grep -qE "$FORK_ONLY" <<<"$p"; then
        RECORDS+=("$p")
    else
        echo "unclassified path $p: add it to UPSTREAM_SHAPED or FORK_ONLY in scripts/land.sh"; exit 1
    fi
done
[ ${#CHANGED[@]} -gt 0 ] || { echo "nothing to land"; exit 1; }

TIP=$M
if [ ${#CODE[@]} -gt 0 ]; then
    check_code_message "$CODE_MSG"
    INDEX=$(mktemp -p "$(git rev-parse --absolute-git-dir)" land-index.XXXXXX)
    trap 'rm -f "$INDEX"' EXIT
    GIT_INDEX_FILE=$INDEX git read-tree "$M"
    for p in "${CODE[@]}"; do
        entry=$(git ls-tree HEAD -- "$p")
        if [ -z "$entry" ]; then
            GIT_INDEX_FILE=$INDEX git update-index --force-remove -- "$p"
        else
            read -r mode _ blob _ <<<"$entry"
            GIT_INDEX_FILE=$INDEX git update-index --add --cacheinfo "$mode,$blob,$p"
        fi
    done
    TIP=$(git commit-tree "$(GIT_INDEX_FILE=$INDEX git write-tree)" -p "$TIP" -F "$CODE_MSG")
    echo "code commit: $TIP"
fi
if [ ${#RECORDS[@]} -gt 0 ]; then
    TIP=$(git commit-tree "HEAD^{tree}" -p "$TIP" -F "$RECORDS_MSG")
    echo "records commit: $TIP"
fi

RAW=(diff --raw --no-abbrev --no-relative --ignore-submodules=none)
[ "$(git "${RAW[@]}" "$M" "$TIP")" = "$(git "${RAW[@]}" "$M" HEAD)" ] \
    || { echo "the landed commits differ from the branch's own changes"; exit 1; }
[ "$(git rev-parse "$TIP^{tree}")" = "$(git rev-parse "HEAD^{tree}")" ] \
    || { echo "the landed tree is not the branch's tree"; exit 1; }

[ "$(git -C "$MAIN" symbolic-ref -q HEAD)" = refs/heads/master ] || { echo "the main checkout is not on master"; exit 1; }
git -C "$MAIN" merge --ff-only "$TIP"
echo "master is now $TIP"
if [ "$PUSH" = --push ]; then
    # Push only where the fork has an upstream and origin is not it (the skill's Landing, step 7).
    upstream_url=$(git -C "$MAIN" remote get-url upstream 2>/dev/null || true)
    origin_url=$(git -C "$MAIN" remote get-url origin 2>/dev/null || true)
    if [ -z "$upstream_url" ] || [ -z "$origin_url" ] || [ "$upstream_url" = "$origin_url" ]; then
        echo "not pushed: this checkout needs an upstream remote and an origin that is not it"
    elif ! git -C "$MAIN" push origin master; then
        echo "push failed (git's reason is above). If git rejected it (non-fast-forward or fetch first), another clone moved origin's master, by a landing or a sync: tag $TIP as backup/main-pre-sync-<stamp> before anything moves it, and land its commits again once this clone takes origin's master"
        exit 1
    fi
fi
