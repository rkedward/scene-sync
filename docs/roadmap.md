# Roadmap and decisions

From the v0.1 design draft and its review (October 2026).

## Decisions

| # | Question | Decision |
| --- | --- | --- |
| 1 | Hardware | Magcubic HY300 Pro+: Android 14, 1280×720, 1 GB RAM, 8 GB storage (about 2.4 GB free) |
| 2 | Storage | Internal storage only for v1. Revisit USB only if storage proves too tight. |
| 3 | Offline schedules | Run indefinitely, with an optional per-schedule `end_date` |
| 4 | Manual commands | Override until the next rule boundary or `max_duration_ms`, whichever is first; `hold` keeps it until cleared |
| 5 | Sync audio | Visuals only; in a group, one device plays audio |
| 6 | Sync tolerance | Target about 100 ms, measured in Phase 4 |
| 7 | Other players | Android only for v1, with a device-neutral protocol (the test player shows it works) |
| 8 | Missed one-shots | Skipped if more than 5 s late, unless already playing or the rule says `join_late`. DST gaps move to the next valid minute; repeated hours use the first occurrence. |

## Phases

- **Phase 0, hardware feasibility.** See [phase0-hardware-checklist.md](phase0-hardware-checklist.md).
- **Phase 1, single projector MVP (done).** Server with dashboard, pairing, upload, download with checksum, play/loop/stop/black remotely, offline playback from cache. Schedule engine and shared fixtures.
- **Phase 2, scheduling and offline resilience (done).** Shared schedules with an editor and day preview in the dashboard. Automated test: the test player keeps playing while the server is down, plays from cache after a restart with no server, and catches up when it returns. DST cases are in the shared fixtures. Reboot and Wi-Fi tests on a real projector are in the Phase 0 checklist.
- **Phase 2b, fit to window (done).** Project a calibration pattern, photograph it with a phone, click the pattern's corners and the window's corners in the dashboard, and every clip is warped onto the window with black around it. The dashboard computes the warp from the photo (a homography), and the player applies it on the GPU as a perspective transform of its video view. Corners can be fine-tuned by dragging while the projector shows the outline live.
- **Phase 3, multiple projectors (done).** Groups with a shared schedule and group commands, an "all projectors" command bar, and app updates served by the Pi (compared by SHA-256; one on-screen confirmation the first time, silent after that or with device owner). Every build is signed with the same checked-in debug key so updates install in place.
- **Phase 4, synchronized groups (done, to be measured on hardware).** A group can play in sync: commands start at a shared moment 1.5 s ahead, every player follows the same timeline on the server clock, corrects drift by playback speed (up to 5%) or a learned seek, and reports its average error, shown on each card. Per-projector trim for display latency; sound from one projector only.
- **Phase 5, packaging (done).** Tagged releases build Pi (arm64) and PC (x64) servers and the APK; a one-line installer sets up the service; daily database backups (14 kept) plus a dashboard download; operations guide in [operations.md](operations.md).
