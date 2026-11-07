#!/usr/bin/bash
# check-stage.sh <order-file> <stage-index> [cargo flags]: stage the first <index> units, prune imports,
# and print cargo check's errors (default config unless flags are given).
W=/mnt/work/projects/cosmic-de/cosmic-files/.worktrees/history-rebuild-series
O=$1; N=$2; shift 2
done=$(head -n "$N" "$O" | paste -sd,)
git -C "$W" checkout -q -- . && git -C "$W" clean -fdq
/mnt/logs/temp/history-rebuild/bin/stage.py "$W" "$done" || exit 2
/mnt/logs/temp/history-rebuild/bin/prune.py "$W" "$@" >/dev/null 2>&1
cd "$W" && cargo check --locked --all-targets "$@" 2>&1 | grep -E -A12 "^error" | head -60
