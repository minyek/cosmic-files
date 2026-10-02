#!/usr/bin/env bash
set -euo pipefail

binary=$(realpath "$1")
shift
output=${ARM_LOG_DIR:?run the performance probe through arm-run}
export XDG_CONFIG_HOME="$output/config"
export XDG_CACHE_HOME="$output/cache"
mkdir -p "$XDG_CONFIG_HOME" "$XDG_CACHE_HOME"
{
    uname -srmo
    rustc --version
    sha256sum "$binary"
    for repeat in 1 2 3; do
        for directory in "$@"; do
            printf 'repeat=%s directory=%s\n' "$repeat" "$directory"
            "$binary" "$directory"
        done
    done
} > "$output/performance.log" 2>&1
