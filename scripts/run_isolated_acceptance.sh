#!/usr/bin/env bash
set -euo pipefail

mode=$1
output=$(realpath -m "$2")
binary=$(realpath "$3")
shift 3
repo=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
mkdir -p "$output/profile/.local/share" "$output/runtime"
chmod 700 "$output/runtime"
profile_home=$(getent passwd "$(id -u)" | cut -d: -f6)
args=(
    --ro-bind / / --bind "$output/profile" "$profile_home"
    --ro-bind "$profile_home/.cargo/git/checkouts" "$profile_home/.cargo/git/checkouts"
    --bind "$output" "$output" --tmpfs /tmp --dev /dev --proc /proc
    --unshare-pid --die-with-parent --chdir "$repo"
    --setenv COSMIC_ACCEPTANCE_PROFILE "$profile_home"
    --setenv XDG_DATA_HOME "$profile_home/.local/share"
    --setenv XDG_CONFIG_HOME "$profile_home/.config"
    --setenv XDG_CACHE_HOME "$profile_home/.cache"
    --setenv XDG_RUNTIME_DIR "$output/runtime"
)
sha256sum "$binary" > "$output/binary.sha256"
case "$mode" in
    storage-failure)
        if bwrap "${args[@]}" bash -c 'trap "" XFSZ; ulimit -f 0; exec "$1" --initialize-only' bash "$binary" 2>&1 | tee "$output/failure.log"; then
            echo "limited writes unexpectedly succeeded" >&2
            exit 1
        fi
        test ! -e "$output/profile/.local/share/recently-used.xbel"
        rg -q 'failed to initialize recents history' "$output/failure.log"
        bwrap "${args[@]}" "$binary" --initialize-only | tee "$output/recovery.log"
        ;;
    storage)
        bwrap "${args[@]}" "$binary" | tee "$output/result.log"
        ;;
    gui)
        xvfb=$(realpath "$1")
        shift
        python_packages=$(python3 -c 'import pathlib, Xlib; print(pathlib.Path(Xlib.__file__).parent.parent)')
        bwrap "${args[@]}" --tmpfs /opt --ro-bind "$python_packages" /opt/acceptance-python \
            --setenv PYTHONPATH /opt/acceptance-python dbus-run-session -- \
            python3 "$repo/scripts/gui_acceptance.py" --binary "$binary" \
            --xvfb "$xvfb" --output "$output" "$@"
        ;;
    *)
        echo "unknown acceptance mode: $mode" >&2
        exit 2
        ;;
esac
