# Running SceneSync

## Install the server on a Raspberry Pi

Use 64-bit Raspberry Pi OS (Bookworm or later). Give the Pi a fixed address with a DHCP reservation in your router. Fitting the Pi 5 RTC battery keeps its clock right through power cuts without Internet.

```sh
curl -fsSL https://github.com/rkedward/scene-sync/releases/latest/download/install.sh | sudo sh
```

The script downloads the latest release, installs `scenesync-server` and `scenesync-sim` to `/usr/local/bin`, creates a `scenesync` user with data in `/var/lib/scenesync`, installs ffmpeg (for clip lengths) and starts the service. It prints the dashboard address and password.

Run the same command to update. Settings, clips and the password are kept.

Without Internet on the Pi, download `scenesync-linux-arm64.tar.gz` from the [releases page](https://github.com/rkedward/scene-sync/releases) on another computer, copy it over, then:

```sh
tar xzf scenesync-linux-arm64.tar.gz && cd scenesync-linux-arm64 && sudo ./install.sh
```

## Install the player on a projector

Download `scenesync-player.apk` from the same release. The first time, install it with `adb` (see the README). After that, upload new versions under **Player app** in the dashboard and every projector updates itself.

You can also open `http://<pi-address>:8080/api/v1/app/player.apk` in a projector's browser once an APK has been uploaded.

## Settings

The service reads these from `/etc/systemd/system/scenesync.service` (`Environment=` lines). After editing, run `sudo systemctl daemon-reload && sudo systemctl restart scenesync`.

| Variable | Default | Meaning |
| --- | --- | --- |
| `SCENESYNC_DATA_DIR` | `/var/lib/scenesync` | Database, clips, backups, app |
| `SCENESYNC_LISTEN` | `0.0.0.0:8080` | Address and port |
| `SCENESYNC_ADMIN_PASSWORD` | from `admin_password` file | Dashboard password |
| `SCENESYNC_NO_MDNS` | unset | Set to stop advertising on the network |
| `RUST_LOG` | `info` | Log detail (`debug` for more) |

## Logs

```sh
journalctl -u scenesync -f          # follow
journalctl -u scenesync --since today
```

## Backups

What's where in `/var/lib/scenesync`:

| Path | Contents |
| --- | --- |
| `scenesync.sqlite` | Projectors, schedules, groups, fit-to-window corners, clip list |
| `media/` | Clip files (named by checksum) |
| `app/` | Player APK offered to projectors |
| `backups/` | A copy of the database made every day, 14 kept |
| `admin_password` | Dashboard password |

To take a backup:

- **Settings:** press **Download backup** at the top of the dashboard, or run `sudo -u scenesync SCENESYNC_DATA_DIR=/var/lib/scenesync scenesync-server backup /tmp/scenesync.sqlite`.
- **Clips:** copy `media/` (for example `rsync -a pi:/var/lib/scenesync/media/ ./media/`). You can also just upload the clips again.

To restore onto a fresh Pi: install as above, then

```sh
sudo systemctl stop scenesync
sudo cp scenesync-backup.sqlite /var/lib/scenesync/scenesync.sqlite
sudo rm -f /var/lib/scenesync/scenesync.sqlite-wal /var/lib/scenesync/scenesync.sqlite-shm
sudo rsync -a media/ /var/lib/scenesync/media/
sudo chown -R scenesync:scenesync /var/lib/scenesync
sudo systemctl start scenesync
```

Projectors keep their pairing because their tokens are in the database. If the Pi's address changed, they find it again on the network.

## Make a release

```sh
git tag v0.5.0 && git push origin v0.5.0
```

The Release workflow builds the Pi (arm64) and PC (x64) servers and the APK, and publishes them with `install.sh`. Every APK is signed with the debug key checked into `android/app/debug.keystore`, so updates install over each other. That key is public; switch to a private key before sharing builds outside your house.

## Troubleshooting

| Symptom | Check |
| --- | --- |
| Projector shows "Server not found" | Pi running (`systemctl status scenesync`), same Wi-Fi, or type the address with the remote |
| Card says "updating" for a long time | Storage on the card; errors listed on the card; clip too big for free space |
| Clip plays at the wrong time | Schedule time zone; Pi clock (`timedatectl`); projector clock offset on its card |
| Projectors drift apart in a sync group | Sync figure on each card; adjust **Trim**; prefer H.264 at 720p |
| App update never applies | Card shows "update pending": press Install on the projector once, or check errors on the card |
