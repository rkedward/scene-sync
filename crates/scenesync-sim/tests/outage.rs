//! Outage test: the test player keeps following its schedule while the
//! server is down, plays from its cache after a restart with no server, and
//! catches up once the server is back.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use scenesync_server::{app, AppState, Config};
use serde_json::json;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::mpsc;

/// A server on its own runtime, so stopping it also drops every open
/// connection, like pulling the Pi's plug.
struct Server {
    stop: Option<std::sync::mpsc::Sender<()>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Server {
    fn start(data: &Path, port: u16) -> Server {
        let (stop_tx, stop_rx) = std::sync::mpsc::channel::<()>();
        let (ready_tx, ready_rx) = std::sync::mpsc::channel::<()>();
        let data = data.to_path_buf();
        let thread = std::thread::spawn(move || {
            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.spawn(async move {
                let config = Config::load(&data, Some("secret".into())).unwrap();
                let state = AppState::new(config).unwrap();
                let listener = tokio::net::TcpListener::bind(("127.0.0.1", port))
                    .await
                    .unwrap();
                ready_tx.send(()).unwrap();
                axum::serve(listener, app(state)).await.unwrap();
            });
            let _ = stop_rx.recv();
            rt.shutdown_timeout(Duration::from_secs(1));
        });
        ready_rx
            .recv_timeout(Duration::from_secs(10))
            .expect("server start");
        Server {
            stop: Some(stop_tx),
            thread: Some(thread),
        }
    }

    fn stop(mut self) {
        drop(self.stop.take());
        self.thread.take().unwrap().join().unwrap();
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        drop(self.stop.take());
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

struct Sim {
    child: Child,
    lines: mpsc::UnboundedReceiver<String>,
}

impl Sim {
    fn start(server: &str, state_dir: &Path) -> Sim {
        let mut child = Command::new(env!("CARGO_BIN_EXE_scenesync-sim"))
            .arg("--server")
            .arg(server)
            .arg("--state-dir")
            .arg(state_dir)
            .arg("--name")
            .arg("Outage test")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let (tx, lines) = mpsc::unbounded_channel();
        let outputs: [Box<dyn tokio::io::AsyncRead + Unpin + Send>; 2] = [
            Box::new(child.stdout.take().unwrap()),
            Box::new(child.stderr.take().unwrap()),
        ];
        for out in outputs {
            let tx = tx.clone();
            tokio::spawn(async move {
                let mut r = BufReader::new(out).lines();
                while let Ok(Some(line)) = r.next_line().await {
                    eprintln!("[sim] {line}");
                    let _ = tx.send(line);
                }
            });
        }
        Sim { child, lines }
    }

    /// Waits for a line containing `needle`, failing if one containing
    /// `forbidden` shows up first.
    async fn expect(&mut self, needle: &str, forbidden: Option<&str>, secs: u64) -> String {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(secs);
        loop {
            let line = tokio::time::timeout_at(deadline, self.lines.recv())
                .await
                .unwrap_or_else(|_| panic!("timed out waiting for {needle:?}"))
                .expect("player exited");
            if let Some(f) = forbidden {
                assert!(
                    !line.contains(f),
                    "unexpected {f:?} while waiting for {needle:?}: {line}"
                );
            }
            if line.contains(needle) {
                return line;
            }
        }
    }

    /// Asserts nothing containing `forbidden` is printed for `secs` seconds.
    async fn quiet(&mut self, forbidden: &str, secs: u64) {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(secs);
        while let Ok(Some(line)) = tokio::time::timeout_at(deadline, self.lines.recv()).await {
            assert!(
                !line.contains(forbidden),
                "unexpected {forbidden:?}: {line}"
            );
        }
    }
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn admin(rb: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
    rb.basic_auth("admin", Some("secret"))
}

#[tokio::test]
async fn keeps_playing_through_outages() {
    let dir = tempfile::tempdir().unwrap();
    let data: PathBuf = dir.path().join("server");
    let player_dir: PathBuf = dir.path().join("player");
    let port = free_port();
    let base = format!("http://127.0.0.1:{port}");
    let http = reqwest::Client::new();

    let server = Server::start(&data, port);

    // A clip and an all-day loop (a window ending at its start time runs 24 hours).
    let form = reqwest::multipart::Form::new().part(
        "file",
        reqwest::multipart::Part::bytes(vec![7u8; 64 * 1024]).file_name("lantern.mp4"),
    );
    let res = admin(http.post(format!("{base}/api/v1/admin/media")))
        .multipart(form)
        .send()
        .await
        .unwrap();
    assert!(res.status().is_success());
    let res = admin(http.put(format!("{base}/api/v1/admin/schedules/all-day")))
        .json(
            &json!({ "schedule_id": "all-day", "revision": 0, "timezone": "UTC",
                       "rules": [{ "id": "lantern", "start": "00:00", "end": "00:00",
                                   "action": { "type": "loop", "media_id": "lantern" } }] }),
        )
        .send()
        .await
        .unwrap();
    assert!(res.status().is_success(), "{}", res.text().await.unwrap());

    // Pair the player using the code it prints.
    let mut sim = Sim::start(&base, &player_dir);
    let line = sim.expect("Pairing code:", None, 20).await;
    let code = line
        .split("Pairing code:")
        .nth(1)
        .unwrap()
        .trim()
        .to_string();
    // The code is printed just before the player first asks to pair.
    let mut approved = false;
    for _ in 0..50 {
        let res = admin(http.post(format!("{base}/api/v1/admin/pairings/approve")))
            .json(&json!({ "code": code, "name": "Porch" }))
            .send()
            .await
            .unwrap();
        if res.status().is_success() {
            approved = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(approved, "pairing was never approved");
    sim.expect("Connected to", None, 20).await;

    let devices: serde_json::Value = admin(http.get(format!("{base}/api/v1/admin/devices")))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let device_id = devices[0]["id"].as_str().unwrap().to_string();
    let res = admin(http.put(format!("{base}/api/v1/admin/devices/{device_id}/schedule")))
        .json(&json!({ "schedule_id": "all-day" }))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 204);
    sim.expect("loop lantern", None, 20).await;

    // 1. The Pi goes down: the player keeps looping.
    server.stop();
    sim.expect("playing from cache", Some("black"), 20).await;
    sim.quiet("black", 3).await;

    // 2. The projector reboots while the Pi is still down: it plays from cache.
    sim.child.kill().await.unwrap();
    let mut sim = Sim::start(&base, &player_dir);
    sim.expect("Loaded cached config", None, 20).await;
    sim.expect("loop lantern", Some("black"), 20).await;

    // 3. The Pi comes back: the player reconnects and picks up new commands.
    let _server = Server::start(&data, port);
    let res = admin(http.post(format!("{base}/api/v1/admin/devices/{device_id}/command")))
        .json(&json!({ "action": { "type": "stop" }, "hold": true }))
        .send()
        .await
        .unwrap();
    assert!(res.status().is_success());
    // The player retries with backoff (at most 30 s between tries).
    sim.expect("black", None, 60).await;
}
