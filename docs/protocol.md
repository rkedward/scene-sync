# SceneSync protocol (version 1)

JSON over HTTP, plus a WebSocket for change notifications. Types are defined in
[`crates/scenesync-core/src/protocol.rs`](../crates/scenesync-core/src/protocol.rs)
and mirrored in [`android/engine/.../Model.kt`](../android/engine/src/main/kotlin/dev/scenesync/engine/Model.kt).

The WebSocket only carries hints. A player always fetches the full config over HTTP, so a missed message never leaves it in the wrong state.

## Authentication

| Caller | How |
| --- | --- |
| Dashboard | HTTP Basic auth. Any username, password = the server's admin password. |
| Player | `Authorization: Bearer <token>`, issued once at pairing. The WebSocket also accepts `?token=`. |

## Pairing

1. The player picks a random 6-digit code, shows it on screen, and repeats `POST /api/v1/pair` every few seconds:
   ```json
   { "device_id": "uuid", "code": "123456", "name": "HY300", "model": "...", "app_version": "...", "protocol_version": 1 }
   ```
   Response: `{ "status": "pending" }`.
2. The user enters the code in the dashboard (`POST /api/v1/admin/pairings/approve {"code": "123456", "name": "Front window"}`).
3. The next `POST /api/v1/pair` returns `{ "status": "approved", "token": "..." }`. The token is returned only once.

Removing a device in the dashboard invalidates its token; the player gets `401` and pairs again.

## Player endpoints

| Method and path | Purpose |
| --- | --- |
| `GET /api/v1/time` | `{ "server_time_ms": 1761625593000 }` for clock offset estimation. No auth. |
| `GET /api/v1/device/config` | The device's full config (below). |
| `GET /api/v1/media/{id}/content` | Clip bytes. Supports `Range` for resuming. |
| `POST /api/v1/device/report` | Status report (below). |
| `GET /api/v1/app/player.apk` | The current player APK (no auth, so it can be opened in a projector's browser for the first install). |
| `GET /api/v1/device/ws` | WebSocket. Server sends `{"type":"config_changed","revision":N}` on connect and on every change. |

### Device config

```json
{
  "protocol_version": 1,
  "device_id": "uuid",
  "name": "Front window",
  "revision": 7,
  "media": [
    { "media_id": "talking-pumpkin", "sha256": "…", "size": 48493,
      "url": "/api/v1/media/talking-pumpkin/content", "duration_ms": 60000 }
  ],
  "schedule": { "schedule_id": "evening-effects", "revision": 12, "timezone": "America/Denver", "rules": [ … ] },
  "override": { "override_id": "ab12", "action": { "type": "play", "media_id": "ghost", "loop": true },
                "issued_at": "2026-10-31T01:05:00Z", "hold": false },
  "idle": "black",
  "mapping": { "corners": [[0.30, 0.20], [0.62, 0.25], [0.60, 0.80], [0.31, 0.75]] },
  "calibrate": false,
  "app_update": { "sha256": "…", "size": 9123456, "url": "/api/v1/app/player.apk" }
}
```

`app_update` (optional) is the player APK every projector should run. A player compares its SHA-256 with that of its own installed package; if they differ, it downloads, verifies, and installs it (Android may ask once on screen; later updates are silent, and fully silent if the app is device owner). A release that fails to install is not retried until a different one is uploaded. Players report their package hash as `app_sha256`.

`sync` (optional) is present when the projector's group plays in sync: `{"group_id": "windows", "muted": true, "trim_ms": 0}`. In a sync group, a player treats every clip as running on a shared timeline: the clip should be at `(server_now - anchor + trim_ms)`, modulo its length for loops. Each player compares that with its real position a few times a second, plays up to 5% faster or slower to close small gaps, and seeks (a little ahead, learning how long its seeks take) for gaps over 400 ms. It reports its average gap as `sync_error_ms`. Only the unmuted projector plays sound.

`mapping` (optional) is fit to window: where the video's top-left, top-right, bottom-right and bottom-left corners land, as fractions of the projector image (0,0 is its top-left). The player warps the video onto that shape and leaves everything else black. Without it, clips are letterboxed to fill the image. `calibrate: true` asks the player to show the calibration pattern (white border, 10% grid, corners numbered 1 to 4 clockwise from top-left, and the current `mapping` outlined in orange) instead of video.

A player downloads every clip in `media`, checks each SHA-256, and only then switches to the new revision. Until then it keeps the previous one. Clips not listed in the applied config may be deleted.

### Report

```json
{ "applied_revision": 7, "state": "playing", "media_id": "ghost",
  "storage_free_bytes": 2500000000, "storage_total_bytes": 8000000000, "media_bytes": 48493,
  "clock_offset_ms": 12, "app_version": "android 0.1.0", "errors": [] }
```

## Schedule format

See the module docs in [`schedule.rs`](../crates/scenesync-core/src/schedule.rs) for the full rules and [`fixtures/schedule/cases.json`](../fixtures/schedule/cases.json) for worked examples.

```json
{
  "schedule_id": "evening-effects",
  "revision": 12,
  "timezone": "America/Denver",
  "end_date": "2026-11-02",
  "rules": [
    { "id": "pumpkin-evening", "days": ["fri", "sat"], "start": "19:00", "end": "22:00",
      "action": { "type": "loop", "media_id": "talking-pumpkin" } },
    { "id": "ghost-appearance", "days": ["fri", "sat"], "at": "19:15",
      "action": { "type": "play_once", "media_id": "ghost" }, "priority": 10, "late_policy": "skip" }
  ]
}
```

## Dashboard endpoints

All under `/api/v1/admin/`, Basic auth.

| Method and path | Purpose |
| --- | --- |
| `GET devices` | Devices with online state, applied vs. desired revision, report, and what should play now |
| `PATCH devices/{id}` | Rename: `{"name": "…"}` |
| `DELETE devices/{id}` | Unpair |
| `PUT devices/{id}/media` | Set assigned clips: `{"media_ids": [...]}` (clips in use are kept) |
| `POST devices/{id}/command` | Manual override: `{"action": {"type":"play","media_id":"x","loop":true} \| {"type":"stop"} \| null, "hold": false, "max_duration_ms": null}`. `null` returns to the schedule. |
| `PUT devices/{id}/schedule` | Assign a saved schedule, or clear it: `{"schedule_id": "evening" \| null}`. Clips it uses are assigned automatically. |
| `PUT devices/{id}/mapping` | Fit to window: `{"mapping": {"corners": [[x,y] ×4]}}` or `{"mapping": null}`. Corners must form a convex shape, clockwise from top-left, within -0.5 to 1.5. |
| `PUT devices/{id}/calibrate` | Show or hide the calibration pattern: `{"on": true}` |
| `GET devices/{id}/preview?at=…` | What the engine says should play at a given time |
| `GET schedules` | Saved schedules, each with `devices` (IDs of projectors using it) |
| `PUT schedules/{id}` | Create or replace a schedule (body as in "Schedule format"; `schedule_id` and `revision` are set by the server). Every projector using it gets the new version. Unknown clips or invalid rules return `400`. |
| `DELETE schedules/{id}` | Delete a schedule. Returns `409` while a projector uses it. |
| `GET schedules/{id}/timeline?date=YYYY-MM-DD` | What the schedule plays over one local day, as segments `{start, end, start_local, end_local, desired}` |
| `GET groups` | Groups: `{id, name, schedule_id, devices}` |
| `PUT groups/{id}` | Create or rename: `{"name": "Garage doors"}` |
| `DELETE groups/{id}` | Delete a group; its projectors keep their schedules |
| `PUT groups/{id}/schedule` | `{"schedule_id": "night" \| null}`: sets the group's schedule and gives it to every member |
| `PUT devices/{id}/group` | `{"group_id": "garage" \| null}`: joining a group with a schedule takes that schedule |
| `POST command` | One command for many projectors: `{"device_ids": [...] \| null (all), "action": …, "hold": false, "start_in_ms": 1500}`. All get the same override, so clips start from the same moment; `start_in_ms` (up to 10000) sets that moment slightly in the future so every projector has the command first. |
| `PUT groups/{id}/sync` | `{"sync": true, "audio_device_id": "…" \| null}`: play in sync; sound from one member (default the first) |
| `PUT devices/{id}/trim` | `{"trim_ms": 40}`: shift one projector's timeline (-2000 to 2000) to make up for display latency |
| `GET app`, `POST app` (multipart `file`), `DELETE app` | The player APK offered to every projector |
| `GET pairings` | Pending pairing requests |
| `POST pairings/approve` | `{"code": "123456", "name": "optional"}` |
| `GET media`, `POST media` (multipart `file`), `DELETE media/{id}` | Clip library. Deleting a clip that a saved schedule or an override uses returns `409`. |
