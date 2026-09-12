#!/bin/sh
set -eu

SEED_ROOT=/usr/share/openconsole-seed
OPENCONSOLE_DST=/data/games/project/openconsole
GAME_DST=/data/games/project
ROOT_HOME=/data/home/root
OPENCONSOLE_DATA=/data/openconsole
IWD_DATA_DIR=$OPENCONSOLE_DATA/iwd
SEED_MARKER=$OPENCONSOLE_DST/.openconsole-seeded

mkdir -p /data/games
mkdir -p "$GAME_DST"
mkdir -p "$OPENCONSOLE_DST"
mkdir -p "$OPENCONSOLE_DATA"
mkdir -p "$IWD_DATA_DIR"
mkdir -p "$ROOT_HOME/.cache" "$ROOT_HOME/.config" "$ROOT_HOME/.local/share"

chmod 0755 /data/games "$GAME_DST" "$OPENCONSOLE_DST"
chmod 0700 "$OPENCONSOLE_DATA" "$IWD_DATA_DIR"
chmod 0700 "$ROOT_HOME" "$ROOT_HOME/.cache" "$ROOT_HOME/.config" "$ROOT_HOME/.local" "$ROOT_HOME/.local/share"
chown root:root /data/games "$GAME_DST" "$OPENCONSOLE_DST" "$OPENCONSOLE_DATA" "$IWD_DATA_DIR" "$ROOT_HOME" "$ROOT_HOME/.cache" "$ROOT_HOME/.config" "$ROOT_HOME/.local" "$ROOT_HOME/.local/share"

mkdir -p /var/lib
if [ -d /var/lib/iwd ] && [ ! -L /var/lib/iwd ]; then
    if [ -n "$(ls -A /var/lib/iwd 2>/dev/null || true)" ]; then
        cp -a /var/lib/iwd/. "$IWD_DATA_DIR"/
    fi
    rm -rf /var/lib/iwd
fi
ln -sfn "$IWD_DATA_DIR" /var/lib/iwd

find "$SEED_ROOT/openconsole" -type f | while IFS= read -r seed_file; do
    relative_path=${seed_file#"$SEED_ROOT/openconsole/"}
    destination_file="$OPENCONSOLE_DST/$relative_path"
    destination_dir=$(dirname "$destination_file")
    mkdir -p "$destination_dir"
    cp -p "$seed_file" "$destination_file"
done

if [ ! -e "$SEED_MARKER" ]; then
    if [ -d "$SEED_ROOT/game" ]; then
        find "$SEED_ROOT/game" -maxdepth 1 -type f | while IFS= read -r seed_file; do
            game_name=$(basename "$seed_file")
            destination_file="$GAME_DST/$game_name"
            cp -f "$seed_file" "$destination_file"
            case "$game_name" in
                *.arm64)
                    chmod 0755 "$destination_file"
                    ;;
                *)
                    chmod 0644 "$destination_file"
                    ;;
            esac
            chown root:root "$destination_file"
        done
    fi

    touch "$SEED_MARKER"
    chmod 0644 "$SEED_MARKER"
    chown root:root "$SEED_MARKER"
fi

find "$OPENCONSOLE_DST/assets" -type d -exec chmod 755 {} + 2>/dev/null || true
find "$OPENCONSOLE_DST/assets" -type f -exec chmod 644 {} + 2>/dev/null || true
chown -R root:root "$OPENCONSOLE_DST"

for log_file in /data/openconsole-launch.log /data/openconsole-supervisor-systemd.log /data/openconsole-ui-current.log /data/openconsole-ui-previous.log /data/godot-current.log /data/godot-previous.log; do
    if [ ! -e "$log_file" ]; then
        : > "$log_file"
    fi
done
chmod 664 /data/openconsole-launch.log /data/openconsole-supervisor-systemd.log /data/openconsole-ui-current.log /data/openconsole-ui-previous.log /data/godot-current.log /data/godot-previous.log
chown root:root /data/openconsole-launch.log /data/openconsole-supervisor-systemd.log /data/openconsole-ui-current.log /data/openconsole-ui-previous.log /data/godot-current.log /data/godot-previous.log