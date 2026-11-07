#!/usr/bin/env bash
set -euo pipefail

output=$(realpath -m "$1")
export COSMIC_PROBE_BIN=$(realpath "$2")
server_python=$(realpath -s "$3")
repo=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
export COSMIC_TEST_REMOTE_ROOT="$output/server-root"
mkdir -p "$COSMIC_TEST_REMOTE_ROOT/nested" "$COSMIC_TEST_REMOTE_ROOT/empty-folder"
printf 'root payload\n' > "$COSMIC_TEST_REMOTE_ROOT/root-file.txt"
printf 'nested payload\n' > "$COSMIC_TEST_REMOTE_ROOT/nested/nested-file.txt"
port=$(python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')
export COSMIC_TEST_REMOTE_URI="ftp://127.0.0.1:$port/"
export XDG_RUNTIME_DIR="$output/runtime"
export XDG_CONFIG_HOME="$output/config"
export XDG_CACHE_HOME="$output/cache"
export XDG_DATA_HOME="$output/data"
export GIO_USE_VFS=gvfs
export GIO_USE_VOLUME_MONITOR=unix
export COSMIC_REQUIRE_FUSE=1
mkdir -p "$XDG_RUNTIME_DIR" "$XDG_CONFIG_HOME" "$XDG_CACHE_HOME" "$XDG_DATA_HOME"
chmod 700 "$XDG_RUNTIME_DIR"
"$server_python" -m pyftpdlib --interface 127.0.0.1 --port "$port" \
    --directory "$COSMIC_TEST_REMOTE_ROOT" --write > "$output/server.log" 2>&1 &
server_pid=$!
cleanup() {
    kill "$server_pid" 2>/dev/null || true
    wait "$server_pid" 2>/dev/null || true
}
trap cleanup EXIT
python3 - "$port" "$server_pid" <<'PY'
import os, socket, sys, time
deadline = time.monotonic() + 10
while True:
    os.kill(int(sys.argv[2]), 0)
    try:
        with socket.create_connection(("127.0.0.1", int(sys.argv[1])), timeout=1):
            break
    except OSError:
        if time.monotonic() >= deadline:
            raise
        time.sleep(0.05)
PY
dbus-run-session -- python3 "$repo/scripts/remote_acceptance.py" > "$output/result.json" 2> "$output/session.log"
sha256sum "$COSMIC_PROBE_BIN" > "$output/binary.sha256"
