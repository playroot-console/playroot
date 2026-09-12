#!/bin/sh
set -eu

socket_path=${1:-/run/wayland-0}
timeout_seconds=${2:-30}

while [ "$timeout_seconds" -gt 0 ]; do
    if [ -S "$socket_path" ]; then
        exit 0
    fi
    timeout_seconds=$((timeout_seconds - 1))
    sleep 1
done

echo "Timed out waiting for Wayland socket $socket_path" >&2
exit 1