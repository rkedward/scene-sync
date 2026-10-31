#!/bin/sh
# Installs or updates the SceneSync server on a Raspberry Pi (64-bit
# Raspberry Pi OS) or another Linux machine with systemd.
#
#   curl -fsSL https://github.com/rkedward/scene-sync/releases/latest/download/install.sh | sudo sh
#
# or, from an unpacked release folder:  sudo ./install.sh
#
# Safe to run again to update: settings, clips and the password are kept.
# It also installs /usr/local/bin/scenesync-update (this script), which the
# dashboard uses to update the server: `scenesync-update --update` always
# downloads the latest release and runs that release's installer.
set -eu

REPO="rkedward/scene-sync"
DATA=/var/lib/scenesync

if [ "$(id -u)" != 0 ]; then
    echo "Run as root (sudo)." >&2
    exit 1
fi

case "$(uname -m)" in
    aarch64 | arm64)
        # 32-bit Raspberry Pi OS can run a 64-bit kernel; the binary needs a 64-bit system.
        if [ "$(getconf LONG_BIT 2>/dev/null)" != 64 ]; then
            echo "This is 32-bit Raspberry Pi OS. Install the 64-bit version." >&2
            exit 1
        fi
        name=linux-arm64 ;;
    x86_64 | amd64) name=linux-x64 ;;
    *) echo "Unsupported CPU $(uname -m). Use 64-bit Raspberry Pi OS." >&2; exit 1 ;;
esac

here=$(dirname "$0")
if [ "${1:-}" = "--update" ]; then
    rm -f "$DATA/update-requested"
fi
if [ "${1:-}" != "--update" ] && [ -x "$here/scenesync-server" ]; then
    src="$here"
else
    tmp=$(mktemp -d)
    trap 'rm -rf "$tmp"' EXIT
    echo "Downloading the latest SceneSync for $name…"
    curl -fsSL --connect-timeout 15 --max-time 900 --retry 2 \
        "https://github.com/$REPO/releases/latest/download/scenesync-$name.tar.gz" | tar xz -C "$tmp"
    src="$tmp/scenesync-$name"
    # Let the new release's own installer finish the job.
    if [ -f "$src/install.sh" ]; then
        # (--quiet, not --update, or the new installer would download again.)
        if [ "${1:-}" = "--update" ]; then sh "$src/install.sh" --quiet; else sh "$src/install.sh"; fi
        exit $?
    fi
fi

# Files are replaced while the server runs (install unlinks first), and it is
# restarted only at the end, so a failure part way leaves the old one running.
if ! command -v ffprobe >/dev/null 2>&1; then
    echo "Installing ffmpeg (for clip length and resolution)…"
    if command -v apt-get >/dev/null 2>&1; then
        DEBIAN_FRONTEND=noninteractive apt-get -o DPkg::Lock::Timeout=60 install -y ffmpeg >/dev/null ||
            echo "Could not install ffmpeg; clips will still work without lengths."
    fi
fi

install -m 755 "$src/scenesync-server" /usr/local/bin/scenesync-server
install -m 755 "$src/scenesync-sim" /usr/local/bin/scenesync-sim
id scenesync >/dev/null 2>&1 || useradd --system --home "$DATA" --create-home --shell /usr/sbin/nologin scenesync
mkdir -p "$DATA"
chown scenesync:scenesync "$DATA"
install -m 644 "$src/scenesync.service" /etc/systemd/system/scenesync.service
# Updates from the dashboard.
install -m 755 "$src/install.sh" /usr/local/bin/scenesync-update
for unit in scenesync-update.service scenesync-update.path; do
    if [ -f "$src/$unit" ]; then
        install -m 644 "$src/$unit" "/etc/systemd/system/$unit"
    fi
done

systemctl daemon-reload
systemctl enable scenesync
systemctl restart scenesync
if [ -f /etc/systemd/system/scenesync-update.path ]; then
    systemctl enable --now scenesync-update.path
fi
sleep 2

ip=$(hostname -I 2>/dev/null | awk '{print $1}')
echo
echo "SceneSync $(/usr/local/bin/scenesync-server --version | awk '{print $2}') is running."
echo "Dashboard: http://${ip:-this-pi}:8080  (any username)"
# Not when run by the updater: its output goes to the system log.
if [ "${1:-}" != "--quiet" ] && [ -f "$DATA/admin_password" ]; then
    echo "Password:  $(cat "$DATA/admin_password")   (to change it, edit $DATA/admin_password and run: sudo systemctl restart scenesync)"
fi
