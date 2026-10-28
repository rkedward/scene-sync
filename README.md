# SceneSync

Schedule and play video clips on several projectors around a house, controlled from a browser. Built for Halloween window effects, but general purpose.

- A **server** on a Raspberry Pi holds the clips, the schedules, and a web dashboard.
- Each projector runs the **SceneSync Player** Android app. It downloads its clips and schedule and keeps playing them if the Pi or Wi-Fi goes down.
- A **test player** (`scenesync-sim`) runs the same protocol on a laptop, so you can try everything without a projector.

Status: Phase 2b (schedules, offline resilience, fit to window). See [docs/roadmap.md](docs/roadmap.md).

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

```sh
# On the Pi (Raspberry Pi OS 64-bit), with Rust installed:
cargo build --release -p scenesync-server
```

Then follow the comments in [`deploy/scenesync.service`](deploy/scenesync.service) to run it as a service. The server advertises itself on the network as `_scenesync._tcp`, so players usually find it on their own. Give the Pi a fixed address (a DHCP reservation in your router) in case they don't.

Recommended: fit the Pi 5 RTC battery so the Pi keeps the right time through power cuts without Internet.

## Install the player on a projector

Each CI run builds a debug APK (see the **Actions** tab, artifact `scenesync-player-debug-apk`). To build it yourself: install Android Studio or the Android SDK, then run `./gradlew :app:assembleDebug` in `android/`.

Install with `adb` over the network (enable developer options and network debugging on the projector first):

```sh
adb connect <projector-ip>:5555
adb install -r scenesync-player-debug.apk
```

Then open **SceneSync Player**. It looks for the server, shows a 6-digit code, and waits for you to enter it in the dashboard.

For a permanent setup, make SceneSync Player the projector's **home app** (Android asks which home app to use when you press the Home button). That is the most reliable way for it to come back after a reboot or power cut on Android 14. While it is the home app, press **Back three times quickly** to open system settings. Any other remote key shows the status overlay for 10 seconds.

## Fit a clip to a window

On the projector card, press **Fit to window** and follow the steps. In short: reset the projector's keystone, press **Show pattern**, take a phone photo of the wall, load it, click the four numbered corners of the pattern and then the four corners of the window. Drag the corners to fine-tune while watching the orange outline on the wall, then press **Done**. The photo stays in your browser; only the four corners are saved.

## Tests

```sh
cargo test --all                                    # server, engine fixtures, API tests, outage test
cd android && ./gradlew -DengineOnly :engine:test   # Kotlin engine fixtures (no Android SDK needed)
```

## License

MIT
