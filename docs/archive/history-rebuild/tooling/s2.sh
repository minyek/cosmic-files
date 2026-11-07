#!/usr/bin/bash
# s2.sh: #7's cfg(not wgpu) named test at #7's own commit, without default features.
W=/mnt/work/projects/cosmic-de/cosmic-files/.worktrees/history-rebuild-check
git -C "$W" checkout -q --detach 1a7483b985d85b75eeb3d79c543fb08c5f198f74 && git -C "$W" clean -fdq
cd "$W" && cargo test --locked --no-default-features --lib -- --exact tab::tests::virtual_rows_match_actual_layout_at_each_scroll_position > /mnt/logs/temp/history-rebuild/commits/s2-issue7-test.txt 2>&1
echo "rc=$? at 1a7483b985d85b75eeb3d79c543fb08c5f198f74"; grep -E "test result|virtual_rows" /mnt/logs/temp/history-rebuild/commits/s2-issue7-test.txt
