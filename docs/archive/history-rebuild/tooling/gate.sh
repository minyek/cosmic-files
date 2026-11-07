#!/usr/bin/bash
# AC 8: CI's three test commands. Usage: gate.sh <worktree>
cd "$1" || exit 1
out=/mnt/logs/temp/history-rebuild/gate
status=0
for cfg in no-default default all; do
    case $cfg in
        no-default) flags=(--no-default-features) ;;
        default) flags=() ;;
        all) flags=(--all-features) ;;
    esac
    cargo test --locked --verbose "${flags[@]}" > "$out/test-$cfg.txt" 2>&1
    rc=$?
    echo "exit=$rc" >> "$out/test-$cfg.txt"
    [ $rc -ne 0 ] && status=$rc
done
exit $status
