#!/usr/bin/bash
# stage-all-diff.sh <target>: stage every issue onto e03001c in the check worktree and diff the result,
# untracked files included (through a throwaway index), against <target>.
W=/mnt/work/projects/cosmic-de/cosmic-files/.worktrees/history-rebuild-check
IDX=$(mktemp -p /mnt/logs/temp/scratch stage-all-idx.XXXXXX)
git -C "$W" checkout -q --detach e03001c && git -C "$W" clean -fdq
/mnt/logs/temp/history-rebuild/bin/stage.py "$W" ALL || exit 2
GIT_INDEX_FILE=$IDX git -C "$W" read-tree HEAD
GIT_INDEX_FILE=$IDX git -C "$W" add -- src tests Cargo.toml Cargo.lock i18n res TESTING.md
GIT_INDEX_FILE=$IDX git -C "$W" diff --cached --stat "$1" -- src tests Cargo.toml Cargo.lock i18n res TESTING.md
rm -f "$IDX"; git -C "$W" checkout -q -- . && git -C "$W" clean -fdq
