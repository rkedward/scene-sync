#!/bin/sh
# Installs or updates the SceneSync server on a Raspberry Pi (64-bit
# Raspberry Pi OS) or another Linux machine with systemd.
#
#   curl -fsSL https://github.com/rkedward/scene-sync/releases/latest/download/install.sh | sudo sh
#
# or, from an unpacked release folder:  sudo ./install.sh
#
# Safe to run again to update: settings, clips and the password are kept.
set -eu

REPO="rkedward/scene-sync"
DATA=/var/lib/scenesync

if [ "$(id -u)" != 0 ]; then
    echo "Run as root (sudo)." >&2
    exit 1
fi

case "$(uname -m)" in
    aarch64 | arm64) name=linux-arm64 ;;
    x86_64 | amd64) name=linux-x64 ;;
    *) echo "Unsupported CPU $(uname -m). Use 64-bit Raspberry Pi OS." >&2; exit 1 ;;
esac

here=$(dirname "$0")
if [ -x "$here/scenesync-server" ]; then
    src="$here"
else
    tmp=$(mktemp -d)
    trap 'rm -rf "$tmp"' EXIT
    echo "Downloading the latest SceneSync for $name…"
    curl -fsSL "https://github.com/$REPO/releases/latest/download/scenesync-$name.tar.gz" | tar xz -C "$tmp"
    src="$tmp/scenesync-$name"
fi

if systemctl is-active --quiet scenesync 2>/dev/null; then
    systemctl stop scenesync
fi
install -m 755 "$src/scenesync-server" /usr/local/bin/scenesync-server
install -m 755 "$src/scenesync-sim" /usr/local/bin/scenesync-sim
id scenesync >/dev/null 2>&1 || useradd --system --home "$DATA" --create-home --shell /usr/sbin/nologin scenesync
mkdir -p "$DATA"
chown scenesync:scenesync "$DATA"
install -m 644 "$src/scenesync.service" /etc/systemd/system/scenesync.service

if ! command -v ffprobe >/dev/null 2>&1; then
    echo "Installing ffmpeg (for clip length and resolution)…"
    if command -v apt-get >/dev/null 2>&1; then
        apt-get install -y ffmpeg >/dev/null || echo "Could not install ffmpeg; clips will still work without lengths."
    fi
fi

systemctl daemon-reload
systemctl enable --now scenesync
sleep 2

ip=$(hostname -I 2>/dev/null | awk '{print $1}')
echo
echo "SceneSync $(/usr/local/bin/scenesync-server --version | awk '{print $2}') is running."
echo "Dashboard: http://${ip:-this-pi}:8080  (any username)"
if [ -f "$DATA/admin_password" ]; then
    echo "Password:  $(cat "$DATA/admin_password")   (to change it, edit $DATA/admin_password and run: sudo systemctl restart scenesync)"
fi
