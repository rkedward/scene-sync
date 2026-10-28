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
- **Phase 1, single projector MVP (this release).** Server with dashboard, pairing, upload, download with checksum, play/loop/stop/black remotely, offline playback from cache. Schedule engine and shared fixtures are in place already; schedules can be set through the API.
- **Phase 2, scheduling and offline resilience.** Schedule editor in the dashboard, Wi-Fi and Pi outage tests, reboot tests, DST tests.
- **Phase 2b, fit to window (projection mapping).** Project a calibration pattern, photograph it with a phone, mark the window corners, and warp every clip onto the window with black around it. The player applies the warp on the GPU (Media3 video effects with a 4-point perspective matrix).
- **Phase 3, multiple projectors.** Groups, per-device schedules at scale, concurrent downloads, silent app updates (device-owner mode).
- **Phase 4, synchronized groups.** Preload-then-play, per-device latency trim, drift correction by playback speed, measured tolerance.
- **Phase 5, packaging.** Pi image or container, signed APK releases, backups, docs.
