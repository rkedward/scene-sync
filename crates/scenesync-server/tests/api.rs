//! End-to-end API test: pair a device, upload a clip, send a command, and
//! check what the device sees.

use futures_util::StreamExt;
use scenesync_core::protocol::{
    DeviceConfig, PairResponse, PairStatus, ServerMessage, PROTOCOL_VERSION,
};
use scenesync_server::{app, AppState, Config};
use serde_json::json;

async fn start() -> (String, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let config = Config::load(dir.path(), Some("secret".into())).unwrap();
    let state = AppState::new(config).unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app(state)).await.unwrap() });
    (format!("http://{addr}"), dir)
}

#[tokio::test]
async fn pair_upload_command_download() {
    let (base, _dir) = start().await;
    let http = reqwest::Client::new();

    // Dashboard requires the password.
    let res = http
        .get(format!("{base}/api/v1/admin/devices"))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 401);
    let admin = |rb: reqwest::RequestBuilder| rb.basic_auth("admin", Some("secret"));

    // Device asks to pair and is pending.
    let pair_req = json!({ "device_id": "dev-1", "code": "123456", "name": "HY300", "model": "HY300 Pro+",
                           "app_version": "0.1.0", "protocol_version": PROTOCOL_VERSION });
    let r: PairResponse = http
        .post(format!("{base}/api/v1/pair"))
        .json(&pair_req)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(r.status, PairStatus::Pending);

    let pending: serde_json::Value = admin(http.get(format!("{base}/api/v1/admin/pairings")))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(pending[0]["code"], "123456");

    // Wrong code is rejected; right code approves.
    let res = admin(http.post(format!("{base}/api/v1/admin/pairings/approve")))
        .json(&json!({ "code": "000000" }))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 404);
    let res = admin(http.post(format!("{base}/api/v1/admin/pairings/approve")))
        .json(&json!({ "code": "123456", "name": "Front window" }))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);

    // Device gets its token exactly once.
    let r: PairResponse = http
        .post(format!("{base}/api/v1/pair"))
        .json(&pair_req)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(r.status, PairStatus::Approved);
    let token = r.token.unwrap();
    let r: PairResponse = http
        .post(format!("{base}/api/v1/pair"))
        .json(&pair_req)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(r.status, PairStatus::Pending);
    assert!(r.token.is_none());

    // Config starts empty.
    let cfg: DeviceConfig = http
        .get(format!("{base}/api/v1/device/config"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(cfg.name, "Front window");
    assert!(cfg.media.is_empty());
    let first_rev = cfg.revision;

    // Open the WebSocket; the first message is the current revision.
    let ws_url = format!(
        "{}/api/v1/device/ws?token={token}",
        base.replace("http://", "ws://")
    );
    let (mut ws, _) = tokio_tungstenite::connect_async(ws_url).await.unwrap();
    let first = ws.next().await.unwrap().unwrap();
    let msg: ServerMessage = serde_json::from_str(first.to_text().unwrap()).unwrap();
    assert_eq!(
        msg,
        ServerMessage::ConfigChanged {
            revision: first_rev
        }
    );

    // Upload a clip.
    let data: Vec<u8> = (0..100_000u32).map(|i| (i % 251) as u8).collect();
    let form = reqwest::multipart::Form::new().part(
        "file",
        reqwest::multipart::Part::bytes(data.clone()).file_name("Talking Pumpkin.mp4"),
    );
    let uploaded: serde_json::Value = admin(http.post(format!("{base}/api/v1/admin/media")))
        .multipart(form)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(uploaded[0]["id"], "talking-pumpkin");

    // Loop it on the device.
    let res = admin(http.post(format!("{base}/api/v1/admin/devices/dev-1/command")))
        .json(&json!({ "action": { "type": "play", "media_id": "talking-pumpkin", "loop": true } }))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);

    // The device is told about the new revision.
    let msg = tokio::time::timeout(std::time::Duration::from_secs(5), ws.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let ServerMessage::ConfigChanged { revision } =
        serde_json::from_str(msg.to_text().unwrap()).unwrap();
    assert!(revision > first_rev);

    let cfg: DeviceConfig = http
        .get(format!("{base}/api/v1/device/config"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(cfg.revision, revision);
    assert_eq!(cfg.media.len(), 1);
    assert!(cfg.override_.is_some());
    let m = &cfg.media[0];
    assert_eq!(m.size, data.len() as u64);

    // Download needs auth, supports Range, and matches the checksum.
    let res = http.get(format!("{base}{}", m.url)).send().await.unwrap();
    assert_eq!(res.status(), 401);
    let body = http
        .get(format!("{base}{}", m.url))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap()
        .bytes()
        .await
        .unwrap();
    use sha2::Digest;
    assert_eq!(hex::encode(sha2::Sha256::digest(&body)), m.sha256);
    let res = http
        .get(format!("{base}{}", m.url))
        .bearer_auth(&token)
        .header("Range", "bytes=1000-")
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 206);
    assert_eq!(res.bytes().await.unwrap().len(), data.len() - 1000);

    // Report and see it in the dashboard.
    let res = http.post(format!("{base}/api/v1/device/report")).bearer_auth(&token)
        .json(&json!({ "applied_revision": revision, "state": "playing", "media_id": "talking-pumpkin" }))
        .send().await.unwrap();
    assert_eq!(res.status(), 204);
    let devices: serde_json::Value = admin(http.get(format!("{base}/api/v1/admin/devices")))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(devices[0]["in_sync"], true);
    assert_eq!(devices[0]["online"], true);
    assert_eq!(devices[0]["desired"]["media_id"], "talking-pumpkin");

    // A clip in use cannot be deleted; after clearing the override it can.
    let res = admin(http.delete(format!("{base}/api/v1/admin/media/talking-pumpkin")))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 409);
    admin(http.post(format!("{base}/api/v1/admin/devices/dev-1/command")))
        .json(&json!({ "action": null }))
        .send()
        .await
        .unwrap();
    let res = admin(http.delete(format!("{base}/api/v1/admin/media/talking-pumpkin")))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 204);

    // Bad schedules are rejected with a reason.
    let res = admin(http.put(format!("{base}/api/v1/admin/schedules/bad")))
        .json(
            &json!({ "schedule_id": "s", "revision": 1, "timezone": "Mars/Olympus", "rules": [] }),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 400);
}

#[tokio::test]
async fn shared_schedules() {
    let (base, _dir) = start().await;
    let http = reqwest::Client::new();
    let admin = |rb: reqwest::RequestBuilder| rb.basic_auth("admin", Some("secret"));

    // Pair a device.
    let pair_req = json!({ "device_id": "dev-2", "code": "654321", "name": "Side", "protocol_version": PROTOCOL_VERSION });
    http.post(format!("{base}/api/v1/pair"))
        .json(&pair_req)
        .send()
        .await
        .unwrap();
    admin(http.post(format!("{base}/api/v1/admin/pairings/approve")))
        .json(&json!({ "code": "654321" }))
        .send()
        .await
        .unwrap();
    let r: PairResponse = http
        .post(format!("{base}/api/v1/pair"))
        .json(&pair_req)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let token = r.token.unwrap();

    // Upload a clip and save a schedule that uses it.
    let form = reqwest::multipart::Form::new().part(
        "file",
        reqwest::multipart::Part::bytes(vec![1u8; 1000]).file_name("bats.mp4"),
    );
    admin(http.post(format!("{base}/api/v1/admin/media")))
        .multipart(form)
        .send()
        .await
        .unwrap();
    let schedule = json!({ "schedule_id": "ignored", "revision": 99, "timezone": "America/Denver", "rules": [
        { "id": "bats", "start": "00:00", "end": "23:59", "action": { "type": "loop", "media_id": "bats" } }
    ]});
    let saved: serde_json::Value =
        admin(http.put(format!("{base}/api/v1/admin/schedules/evening")))
            .json(&schedule)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
    assert_eq!(saved["schedule_id"], "evening");
    assert_eq!(saved["revision"], 1);

    // Unknown clips are rejected.
    let bad = json!({ "schedule_id": "x", "revision": 1, "timezone": "America/Denver", "rules": [
        { "id": "r", "at": "19:00", "action": { "type": "play_once", "media_id": "nope" } } ]});
    let res = admin(http.put(format!("{base}/api/v1/admin/schedules/x")))
        .json(&bad)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 400);

    // Assign it: the device config now carries the schedule and its clip.
    let res = admin(http.put(format!("{base}/api/v1/admin/devices/dev-2/schedule")))
        .json(&json!({ "schedule_id": "evening" }))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 204);
    let cfg: DeviceConfig = http
        .get(format!("{base}/api/v1/device/config"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(cfg.schedule.as_ref().unwrap().schedule_id, "evening");
    assert_eq!(cfg.media[0].media_id, "bats");
    let rev = cfg.revision;

    // Editing the schedule bumps the device revision and the schedule revision.
    let saved: serde_json::Value =
        admin(http.put(format!("{base}/api/v1/admin/schedules/evening")))
            .json(&schedule)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
    assert_eq!(saved["revision"], 2);
    let cfg: DeviceConfig = http
        .get(format!("{base}/api/v1/device/config"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(cfg.revision > rev);
    assert_eq!(cfg.schedule.as_ref().unwrap().revision, 2);

    // In-use schedules and clips cannot be deleted.
    let res = admin(http.delete(format!("{base}/api/v1/admin/schedules/evening")))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 409);
    let res = admin(http.delete(format!("{base}/api/v1/admin/media/bats")))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 409);

    // Timeline and list.
    let tl: serde_json::Value = admin(http.get(format!(
        "{base}/api/v1/admin/schedules/evening/timeline?date=2026-10-30"
    )))
    .send()
    .await
    .unwrap()
    .json()
    .await
    .unwrap();
    assert_eq!(tl[0]["desired"]["media_id"], "bats");
    let list: serde_json::Value = admin(http.get(format!("{base}/api/v1/admin/schedules")))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(list[0]["devices"][0], "dev-2");

    // Unassign, then delete works.
    admin(http.put(format!("{base}/api/v1/admin/devices/dev-2/schedule")))
        .json(&json!({ "schedule_id": null }))
        .send()
        .await
        .unwrap();
    let res = admin(http.delete(format!("{base}/api/v1/admin/schedules/evening")))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 204);
}

#[tokio::test]
async fn fit_to_window() {
    let (base, _dir) = start().await;
    let http = reqwest::Client::new();
    let admin = |rb: reqwest::RequestBuilder| rb.basic_auth("admin", Some("secret"));

    let pair_req = json!({ "device_id": "dev-3", "code": "111222", "name": "Garage", "protocol_version": PROTOCOL_VERSION });
    http.post(format!("{base}/api/v1/pair"))
        .json(&pair_req)
        .send()
        .await
        .unwrap();
    admin(http.post(format!("{base}/api/v1/admin/pairings/approve")))
        .json(&json!({ "code": "111222" }))
        .send()
        .await
        .unwrap();
    let r: PairResponse = http
        .post(format!("{base}/api/v1/pair"))
        .json(&pair_req)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let token = r.token.unwrap();
    let config = || async {
        http.get(format!("{base}/api/v1/device/config"))
            .bearer_auth(&token)
            .send()
            .await
            .unwrap()
            .json::<DeviceConfig>()
            .await
            .unwrap()
    };
    let before = config().await;
    assert!(before.mapping.is_none() && !before.calibrate);

    // Pattern on.
    let res = admin(http.put(format!("{base}/api/v1/admin/devices/dev-3/calibrate")))
        .json(&json!({ "on": true }))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 204);
    let c = config().await;
    assert!(c.calibrate);
    assert!(c.revision > before.revision);

    // A bow-tie shape is rejected; a window shape is stored.
    let res = admin(http.put(format!("{base}/api/v1/admin/devices/dev-3/mapping")))
        .json(&json!({ "mapping": { "corners": [[0,0],[1,1],[1,0],[0,1]] } }))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 400);
    let corners = [[0.3, 0.2], [0.62, 0.25], [0.6, 0.8], [0.31, 0.75]];
    let res = admin(http.put(format!("{base}/api/v1/admin/devices/dev-3/mapping")))
        .json(&json!({ "mapping": { "corners": corners } }))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 204);
    assert_eq!(config().await.mapping.unwrap().corners, corners);

    // Pattern off, mapping cleared.
    admin(http.put(format!("{base}/api/v1/admin/devices/dev-3/calibrate")))
        .json(&json!({ "on": false }))
        .send()
        .await
        .unwrap();
    admin(http.put(format!("{base}/api/v1/admin/devices/dev-3/mapping")))
        .json(&json!({ "mapping": null }))
        .send()
        .await
        .unwrap();
    let c = config().await;
    assert!(c.mapping.is_none() && !c.calibrate);

    // Unknown device.
    let res = admin(http.put(format!("{base}/api/v1/admin/devices/nope/calibrate")))
        .json(&json!({ "on": true }))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 404);
}

async fn pair(base: &str, http: &reqwest::Client, device_id: &str, code: &str) -> String {
    let req = json!({ "device_id": device_id, "code": code, "name": device_id, "protocol_version": PROTOCOL_VERSION });
    http.post(format!("{base}/api/v1/pair"))
        .json(&req)
        .send()
        .await
        .unwrap();
    let res = http
        .post(format!("{base}/api/v1/admin/pairings/approve"))
        .basic_auth("admin", Some("secret"))
        .json(&json!({ "code": code }))
        .send()
        .await
        .unwrap();
    assert!(res.status().is_success());
    let r: PairResponse = http
        .post(format!("{base}/api/v1/pair"))
        .json(&req)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    r.token.unwrap()
}

#[tokio::test]
async fn groups_bulk_commands_and_app_updates() {
    let (base, _dir) = start().await;
    let http = reqwest::Client::new();
    let admin = |rb: reqwest::RequestBuilder| rb.basic_auth("admin", Some("secret"));
    let t1 = pair(&base, &http, "left", "100001").await;
    let t2 = pair(&base, &http, "right", "100002").await;
    let _t3 = pair(&base, &http, "porch", "100003").await;
    let config = |token: String| {
        let http = http.clone();
        let base = base.clone();
        async move {
            http.get(format!("{base}/api/v1/device/config"))
                .bearer_auth(token)
                .send()
                .await
                .unwrap()
                .json::<DeviceConfig>()
                .await
                .unwrap()
        }
    };

    let form = reqwest::multipart::Form::new().part(
        "file",
        reqwest::multipart::Part::bytes(vec![3u8; 2000]).file_name("skeleton.mp4"),
    );
    admin(http.post(format!("{base}/api/v1/admin/media")))
        .multipart(form)
        .send()
        .await
        .unwrap();
    admin(http.put(format!("{base}/api/v1/admin/schedules/night")))
        .json(&json!({ "schedule_id": "night", "revision": 0, "timezone": "UTC", "rules": [
            { "id": "s", "start": "18:00", "end": "23:00", "action": { "type": "loop", "media_id": "skeleton" } } ] }))
        .send()
        .await
        .unwrap();

    // A group with a schedule; members get it, including ones that join later.
    let res = admin(http.put(format!("{base}/api/v1/admin/groups/garage")))
        .json(&json!({ "name": "Garage doors" }))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    let res = admin(http.put(format!("{base}/api/v1/admin/devices/left/group")))
        .json(&json!({ "group_id": "garage" }))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 204);
    let res = admin(http.put(format!("{base}/api/v1/admin/groups/garage/schedule")))
        .json(&json!({ "schedule_id": "night" }))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 204);
    admin(http.put(format!("{base}/api/v1/admin/devices/right/group")))
        .json(&json!({ "group_id": "garage" }))
        .send()
        .await
        .unwrap();
    for t in [&t1, &t2] {
        let c = config(t.clone()).await;
        assert_eq!(c.schedule.unwrap().schedule_id, "night");
        assert_eq!(c.media[0].media_id, "skeleton");
    }
    let groups: serde_json::Value = admin(http.get(format!("{base}/api/v1/admin/groups")))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(groups[0]["devices"], json!(["left", "right"]));
    // The schedule can't be deleted while the group uses it.
    let res = admin(http.delete(format!("{base}/api/v1/admin/schedules/night")))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 409);

    // One command for the group: both get the same override.
    let res: serde_json::Value = admin(http.post(format!("{base}/api/v1/admin/command")))
        .json(&json!({ "device_ids": ["left", "right"], "action": { "type": "play", "media_id": "skeleton", "loop": false } }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(res["devices"], 2);
    let (c1, c2) = (config(t1.clone()).await, config(t2.clone()).await);
    assert_eq!(c1.override_, c2.override_);
    assert!(c1.override_.is_some());
    // Everyone back to schedule.
    let res: serde_json::Value = admin(http.post(format!("{base}/api/v1/admin/command")))
        .json(&json!({ "device_ids": null, "action": null }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(res["devices"], 3);
    assert!(config(t1.clone()).await.override_.is_none());

    // App updates: a non-APK is refused; an APK is offered to every player.
    let form = reqwest::multipart::Form::new().part(
        "file",
        reqwest::multipart::Part::bytes(b"hello".to_vec()).file_name("x.apk"),
    );
    let res = admin(http.post(format!("{base}/api/v1/admin/app")))
        .multipart(form)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 400);
    let mut apk = b"PK\x03\x04".to_vec();
    apk.extend(vec![0u8; 5000]);
    let form = reqwest::multipart::Form::new().part(
        "file",
        reqwest::multipart::Part::bytes(apk.clone()).file_name("scenesync-player.apk"),
    );
    let before = config(t1.clone()).await.revision;
    let res = admin(http.post(format!("{base}/api/v1/admin/app")))
        .multipart(form)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    let c = config(t1.clone()).await;
    assert!(c.revision > before);
    let rel = c.app_update.unwrap();
    assert_eq!(rel.size, apk.len() as u64);
    let body = http
        .get(format!("{base}{}", rel.url))
        .send()
        .await
        .unwrap()
        .bytes()
        .await
        .unwrap();
    assert_eq!(body.as_ref(), apk.as_slice());

    // Deleting the group keeps members' schedules.
    let res = admin(http.delete(format!("{base}/api/v1/admin/groups/garage")))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 204);
    assert!(config(t2).await.schedule.is_some());
}
