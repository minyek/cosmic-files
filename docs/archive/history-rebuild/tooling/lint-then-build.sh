#!/usr/bin/bash
# lint-then-build.sh <order-file>: lint every stage; only when all are OK, rebuild the series from
# e03001c and run the per-commit check (build-and-check.sh).
set -u
H=/mnt/logs/temp/history-rebuild
W=/mnt/work/projects/cosmic-de/cosmic-files/.worktrees/history-rebuild-series
# The stage tools write onto a checkout of the base, so the series must sit at e03001c first.
git -C "$W" checkout -q -- . && git -C "$W" clean -fdq && git -C "$W" reset -q --hard e03001c || exit 2
before=$(wc -l < "$H/commits/lint-summary.txt")
"$H/bin/lint-order.py" "$1" || exit 3
bad=$(tail -n +"$((before + 1))" "$H/commits/lint-summary.txt" | grep -c " BAD$")
echo "lint: $bad bad stages"
[ "$bad" -eq 0 ] || exit 4
W=/mnt/work/projects/cosmic-de/cosmic-files/.worktrees/history-rebuild-series
git -C "$W" checkout -q -- . && git -C "$W" clean -fdq && git -C "$W" reset -q --hard e03001c
exec "$H/bin/build-and-check.sh" "$1"
