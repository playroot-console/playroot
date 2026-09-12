#!/bin/sh
set -eu

detect_audio_device() {
    if [ -n "${OPENCONSOLE_AUDIO_DEVICE:-}" ]; then
        printf '%s\n' "$OPENCONSOLE_AUDIO_DEVICE"
        return 0
    fi

    if grep -qs '^connected$' /sys/class/drm/*HDMI-A-2/status 2>/dev/null; then
        printf '%s\n' 'hdmi:CARD=vc4hdmi1,DEV=0'
        return 0
    fi

    if grep -qs '^connected$' /sys/class/drm/*HDMI-A-1/status 2>/dev/null; then
        printf '%s\n' 'hdmi:CARD=vc4hdmi0,DEV=0'
        return 0
    fi

    printf '%s\n' 'hdmi:CARD=vc4hdmi1,DEV=0'
}

write_asoundrc() {
    target_file=$1
    audio_device=$2
    card_name=$(printf '%s\n' "$audio_device" | sed -n 's/.*CARD=\([^,]*\).*/\1/p')

    cat >"$target_file" <<EOF
pcm.!default {
    type plug
    slave.pcm "$audio_device"
}

ctl.!default {
    type hw
    card ${card_name:-vc4hdmi1}
}
EOF
}

GAME_DIR=/data/games/project
GAME_BIN_NAME="${OPENCONSOLE_GAME_BIN:-}"
if [ -z "$GAME_BIN_NAME" ]; then
    echo "openconsole: OPENCONSOLE_GAME_BIN must be set" >&2
    exit 1
fi
case "$GAME_BIN_NAME" in
    ""|*/*)
        echo "openconsole: invalid OPENCONSOLE_GAME_BIN value: $GAME_BIN_NAME" >&2
        exit 1
        ;;
esac
GAME_BIN="$GAME_DIR/$GAME_BIN_NAME"
GAME_PACK_NAME="${OPENCONSOLE_GAME_PACK:-}"
if [ -n "$GAME_PACK_NAME" ]; then
    case "$GAME_PACK_NAME" in
        ""|*/*)
            echo "openconsole: invalid OPENCONSOLE_GAME_PACK value: $GAME_PACK_NAME" >&2
            exit 1
            ;;
    esac
    if [ ! -f "$GAME_DIR/$GAME_PACK_NAME" ]; then
        echo "openconsole: missing game pack at $GAME_DIR/$GAME_PACK_NAME" >&2
        exit 1
    fi
    echo "openconsole: OPENCONSOLE_GAME_PACK is set to $GAME_PACK_NAME but will be ignored by this Godot build" >&2
fi
TMP_HOME=/tmp/oc-weston-home
AUDIO_DEVICE=$(detect_audio_device)

if [ ! -x "$GAME_BIN" ]; then
    echo "openconsole: missing executable game binary at $GAME_BIN" >&2
    exit 1
fi

mkdir -p "$TMP_HOME/.cache" "$TMP_HOME/.config" "$TMP_HOME/.local/share"
write_asoundrc "$TMP_HOME/.asoundrc" "$AUDIO_DEVICE"
chmod 0755 "$TMP_HOME"
chmod 0700 "$TMP_HOME/.cache" "$TMP_HOME/.config" "$TMP_HOME/.local" "$TMP_HOME/.local/share"
chmod 0644 "$TMP_HOME/.asoundrc"
chown -R weston:weston "$TMP_HOME"

echo "openconsole: using ALSA audio device $AUDIO_DEVICE" >&2

exec su weston -s /bin/sh -c '
    export HOME="'$TMP_HOME'"
    export XDG_CACHE_HOME="'$TMP_HOME'/.cache"
    export XDG_CONFIG_HOME="'$TMP_HOME'/.config"
    export XDG_DATA_HOME="'$TMP_HOME'/.local/share"
    export XDG_RUNTIME_DIR="/run"
    export WAYLAND_DISPLAY="wayland-0"
    export SDL_AUDIODRIVER="alsa"
    export AUDIODEV="default"
    unset DISPLAY
    unset DRI_PRIME
    unset SLINT_BACKEND
    cd "'$GAME_DIR'"
    exec "'$GAME_BIN'" "$@"
' -- sh "$@"