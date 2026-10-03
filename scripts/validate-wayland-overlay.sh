#!/bin/sh
set -eu

runtime_dir="${XDG_RUNTIME_DIR:-/run/user/$(id -u)}"
socket="${LISNAP_WAYLAND_SOCKET:-}"
screen="${LISNAP_SCREEN:-}"

if [ -z "$socket" ] && [ -n "${WAYLAND_DISPLAY:-}" ]; then
    case "$WAYLAND_DISPLAY" in
        /*) socket="$WAYLAND_DISPLAY" ;;
        *) socket="$runtime_dir/$WAYLAND_DISPLAY" ;;
    esac
fi

if [ -z "$socket" ]; then
    for candidate in "$runtime_dir"/wayland-*; do
        if [ -S "$candidate" ]; then
            socket="$candidate"
            break
        fi
    done
fi

if [ -z "$socket" ] || [ ! -S "$socket" ]; then
    echo "LiSnap overlay validation: no Wayland socket found." >&2
    echo "Set LISNAP_WAYLAND_SOCKET=/run/user/UID/wayland-N and retry." >&2
    exit 2
fi

echo "LiSnap overlay validation socket: $socket"
echo "Drag with the left mouse button to select a region, or press Esc to cancel."

if [ -n "$screen" ]; then
    cargo run -p lisnap-platform --example wayland-overlay --         --socket "$socket" --screen "$screen"
else
    cargo run -p lisnap-platform --example wayland-overlay --         --socket "$socket"
fi
