#!/usr/bin/bash
# parse-stage.sh <order-file> <stage-index> <file>: stage the order's first <index> units into the
# series worktree (at e03001c) and show rustfmt's parse errors for <file>, with context.
W=/mnt/work/projects/cosmic-de/cosmic-files/.worktrees/history-rebuild-series
done=$(head -n "$2" "$1" | paste -sd,)
git -C "$W" checkout -q -- . && git -C "$W" clean -fdq
/mnt/logs/temp/history-rebuild/bin/stage.py "$W" "$done" || exit 2
rustfmt --edition 2024 --check "$W/$3" 2>&1 | grep -A6 "^error" | head -30
