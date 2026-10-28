# SceneSync

Schedule and play video clips on several projectors around a house, controlled from a browser. Built for Halloween window effects, but general purpose.

- A **server** on a Raspberry Pi holds the clips, the schedules, and a web dashboard.
- Each projector runs the **SceneSync Player** Android app. It downloads its clips and schedule and keeps playing them if the Pi or Wi-Fi goes down.
- A **test player** (`scenesync-sim`) runs the same protocol on a laptop, so you can try everything without a projector.

Status: all planned phases built (schedules, offline resilience, fit to window, groups, app updates, synchronized playback, packaging). Hardware testing in progress; see [docs/phase0-hardware-checklist.md](docs/phase0-hardware-checklist.md). See [docs/roadmap.md](docs/roadmap.md).

## Repository layout

| Path | What it is |
| --- | --- |
| `crates/scenesync-core` | Shared protocol types and the schedule engine (Rust) |
| `crates/scenesync-server` | Server and web dashboard (Rust, Axum, SQLite) |
| `crates/scenesync-sim` | Command-line test player (Rust; plays clips with `mpv` if asked) |
| `android/engine` | Kotlin port of the schedule engine and protocol types |
| `android/app` | Android player (Kotlin, Media3/ExoPlayer) |
| `fixtures/schedule/cases.json` | Test cases both engines must pass |
| `docs/` | Protocol, decisions, roadmap, hardware checklist |

## Try it on a laptop

You need Rust (`rustup`) and, optionally, `ffprobe` (for clip length and size) and `mpv`.

```sh
# Terminal 1: the server
SCENESYNC_ADMIN_PASSWORD=changeme cargo run -p scenesync-server

# Terminal 2: a test player (add --mpv to actually play the video)
cargo run -p scenesync-sim -- --server http://127.0.0.1:8080 --name "Test window"
```

1. Open http://127.0.0.1:8080 and log in with any username and the password `changeme`.
2. Type the 6-digit code shown by the test player under **Add a projector**.
3. Upload an MP4 under **Clips**, pick it on the projector card, and press **Loop**.
4. Under **Schedules**, press **New schedule**, add rules (for example "Loop during a window" 19:00 to 22:00 and "Play once at a time" 19:15), and save. **Preview a day** shows what will play when.
5. Pick the schedule on the projector card. If you pressed **Loop** earlier, press **Return to schedule** so the schedule takes over.
6. Stop the server: the test player keeps playing. Start it again and it catches up.

## Run the server on a Raspberry Pi

On 64-bit Raspberry Pi OS:

```sh
curl -fsSL https://github.com/rkedward/scene-sync/releases/latest/download/install.sh | sudo sh
```

It installs the server as a service and prints the dashboard address and password. Run it again to update. Details, backups, logs and troubleshooting: [docs/operations.md](docs/operations.md).

The server advertises itself on the network as `_scenesync._tcp`, so players usually find it on their own. Give the Pi a fixed address (a DHCP reservation in your router) in case they don't. Recommended: fit the Pi 5 RTC battery so the Pi keeps the right time through power cuts without Internet.

## Install the player on a projector

Download `scenesync-player.apk` from the [latest release](https://github.com/rkedward/scene-sync/releases/latest). Each CI run also builds one (see the **Actions** tab, artifact `scenesync-player-debug-apk`). To build it yourself: install Android Studio or the Android SDK, then run `./gradlew :app:assembleDebug` in `android/`.

Install with `adb` over the network (enable developer options and network debugging on the projector first):

```sh
adb connect <projector-ip>:5555
adb install -r scenesync-player-debug.apk
```

Then open **SceneSync Player**. It looks for the server, shows a 6-digit code, and waits for you to enter it in the dashboard.

For a permanent setup, make SceneSync Player the projector's **home app** (Android asks which home app to use when you press the Home button). That is the most reliable way for it to come back after a reboot or power cut on Android 14. While it is the home app, press **Back three times quickly** to open system settings. Any other remote key shows the status overlay for 10 seconds.

## Groups and app updates

Under **Groups**, add a group (for example "Garage doors"), then pick it on each projector card. A group's schedule goes to all its projectors, and its command buttons control them together. The **All projectors** bar does the same for everything.

Tick **Play in sync** on a group to keep its projectors on the same frame, and pick which one plays sound. Each card then shows how close it is (for example "within about 20 ms"). If one projector still looks behind the others, raise its **Trim** a little (tens of milliseconds).

To update the player app, upload the APK under **Player app**. Each projector downloads it and installs it. The first update after installing with `adb` shows an Android "Install" prompt; press it with the remote. Later updates install on their own. For no prompts at all, make the app device owner once, on a projector with no Google account:

```sh
adb shell dpm set-device-owner dev.scenesync.player/.AdminReceiver
```

Builds from before Phase 3 were signed with a different key, so uninstall the old app once (`adb uninstall dev.scenesync.player`) before installing a new build. You'll need to pair it again.

## Fit a clip to a window

On the projector card, press **Fit to window** and follow the steps. In short: reset the projector's keystone, press **Show pattern**, take a phone photo of the wall, load it, click the four numbered corners of the pattern and then the four corners of the window. Drag the corners to fine-tune while watching the orange outline on the wall, then press **Done**. The photo stays in your browser; only the four corners are saved.

## Tests

```sh
cargo test --all                                    # server, engine fixtures, API tests, outage test
cd android && ./gradlew -DengineOnly :engine:test   # Kotlin engine fixtures (no Android SDK needed)
```

## License

MIT
