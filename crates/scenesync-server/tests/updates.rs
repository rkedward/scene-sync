//! Updates from GitHub, against a fake GitHub release API.

use std::sync::{Arc, Mutex};

use axum::extract::State;
use axum::routing::get;
use axum::{Json, Router};
use scenesync_server::{app, updates, AppState, Config};
use serde_json::json;

/// A fake "latest release" endpoint plus its APK download.
async fn fake_github(tag: Arc<Mutex<String>>) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let api_base = base.clone();
    let router = Router::new()
        .route(
            "/latest",
            get(move |State(tag): State<Arc<Mutex<String>>>| {
                let base = api_base.clone();
                async move {
                    let tag = tag.lock().unwrap().clone();
                    Json(json!({
                        "tag_name": tag,
                        "html_url": format!("https://example.com/{tag}"),
                        "published_at": "2026-10-30T12:00:00Z",
                        "assets": [
                            { "name": "scenesync-linux-arm64.tar.gz", "browser_download_url": format!("{base}/x.tar.gz") },
                            { "name": "scenesync-player.apk", "browser_download_url": format!("{base}/apk/{tag}") },
                        ]
                    }))
                }
            }),
        )
        .route(
            "/apk/{tag}",
            get(|axum::extract::Path(tag): axum::extract::Path<String>| async move {
                let mut body = b"PK\x03\x04".to_vec();
                body.extend_from_slice(tag.as_bytes());
                body
            }),
        )
        .with_state(tag);
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    base
}

#[tokio::test]
async fn player_and_server_updates_from_github() {
    let tag = Arc::new(Mutex::new("v99.0.0".to_string()));
    let gh = fake_github(tag.clone()).await;

    let dir = tempfile::tempdir().unwrap();
    let mut config = Config::load(dir.path(), Some("secret".into())).unwrap();
    config.update_url = format!("{gh}/latest");
    let unit = dir.path().join("scenesync-update.path");
    config.updater_unit = unit.clone();
    let state = AppState::new(config).unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let st = state.clone();
    tokio::spawn(async move { axum::serve(listener, app(st)).await.unwrap() });

    let http = reqwest::Client::new();
    let admin = |rb: reqwest::RequestBuilder| rb.basic_auth("admin", Some("secret"));

    // Check: a newer release exists for both.
    let s: serde_json::Value = admin(http.post(format!("{base}/api/v1/admin/updates/check")))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(s["latest"]["tag"], "v99.0.0");
    assert_eq!(s["auto"], true);
    assert_eq!(s["server"]["update_available"], true);
    assert_eq!(s["server"]["can_update"], false);
    assert_eq!(s["player"]["update_available"], true);
    assert_eq!(s["player"]["offered"], false);

    // Without the root updater the server can't update itself.
    let res = admin(http.post(format!("{base}/api/v1/admin/updates/server")))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 409);

    // Player: fetch the release APK and offer it to projectors.
    let s: serde_json::Value = admin(http.post(format!("{base}/api/v1/admin/updates/player")))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(s["player"]["tag"], "v99.0.0");
    assert_eq!(s["player"]["update_available"], false);
    let apk = http
        .get(format!("{base}/api/v1/app/player.apk"))
        .send()
        .await
        .unwrap()
        .bytes()
        .await
        .unwrap();
    assert_eq!(&apk[..], b"PK\x03\x04v99.0.0");

    // A hand-uploaded build is left alone by the automatic update.
    let form = reqwest::multipart::Form::new().part(
        "file",
        reqwest::multipart::Part::bytes(b"PK\x03\x04debug".to_vec()).file_name("debug.apk"),
    );
    let res = admin(http.post(format!("{base}/api/v1/admin/app")))
        .multipart(form)
        .send()
        .await
        .unwrap();
    assert!(res.status().is_success());
    *tag.lock().unwrap() = "v99.1.0".into();
    std::fs::write(&unit, "").unwrap();
    updates::auto_update(&state).await.unwrap();
    let apk = http
        .get(format!("{base}/api/v1/app/player.apk"))
        .send()
        .await
        .unwrap()
        .bytes()
        .await
        .unwrap();
    assert_eq!(&apk[..], b"PK\x03\x04debug");
    // ...but the server update is requested for the root updater.
    assert_eq!(
        std::fs::read_to_string(dir.path().join("update-requested")).unwrap(),
        "v99.1.0"
    );
    std::fs::remove_file(dir.path().join("update-requested")).unwrap();

    // Once the projectors follow GitHub again, the automatic update moves them along.
    admin(http.post(format!("{base}/api/v1/admin/updates/player")))
        .send()
        .await
        .unwrap();
    *tag.lock().unwrap() = "v99.2.0".into();
    updates::auto_update(&state).await.unwrap();
    let apk = http
        .get(format!("{base}/api/v1/app/player.apk"))
        .send()
        .await
        .unwrap()
        .bytes()
        .await
        .unwrap();
    assert_eq!(&apk[..], b"PK\x03\x04v99.2.0");
    std::fs::remove_file(dir.path().join("update-requested")).unwrap();

    // Automatic updates off: nothing changes.
    let s: serde_json::Value = admin(http.put(format!("{base}/api/v1/admin/updates")))
        .json(&json!({ "auto": false }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(s["auto"], false);
    *tag.lock().unwrap() = "v99.3.0".into();
    updates::auto_update(&state).await.unwrap();
    let apk = http
        .get(format!("{base}/api/v1/app/player.apk"))
        .send()
        .await
        .unwrap()
        .bytes()
        .await
        .unwrap();
    assert_eq!(&apk[..], b"PK\x03\x04v99.2.0");
    assert!(!dir.path().join("update-requested").exists());

    // The dashboard button asks for a server update when the updater exists.
    let res = admin(http.post(format!("{base}/api/v1/admin/updates/server")))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 202);
    let s: serde_json::Value = res.json().await.unwrap();
    assert_eq!(s["server"]["updating"], true);
}

#[tokio::test]
async fn github_unreachable_is_reported() {
    let dir = tempfile::tempdir().unwrap();
    let mut config = Config::load(dir.path(), Some("secret".into())).unwrap();
    config.update_url = "http://127.0.0.1:1/latest".into();
    let state = AppState::new(config).unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app(state)).await.unwrap() });
    let http = reqwest::Client::new();
    let s: serde_json::Value = http
        .post(format!("{base}/api/v1/admin/updates/check"))
        .basic_auth("admin", Some("secret"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(s["error"]
        .as_str()
        .unwrap()
        .contains("Could not check GitHub"));
    assert_eq!(s["server"]["update_available"], false);
}
