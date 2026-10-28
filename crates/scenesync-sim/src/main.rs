//! `scenesync-sim`: a command-line SceneSync player.
//!
//! It speaks the same protocol as the Android app, so the server can be
//! developed and tested on a laptop. With `--mpv` it also plays the clips,
//! which makes it a rough Linux / Raspberry Pi player.
//!
//! Behaviour mirrors the Android player:
//! 1. Pair: show a 6-digit code and wait for approval in the dashboard.
//! 2. Estimate the clock offset to the server.
//! 3. Fetch the config when told it changed; download and verify clips;
//!    apply the new revision only when every clip is present.
//! 4. Every second, evaluate the cached config and switch playback if needed.
//!    This keeps working when the server is unreachable.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use chrono::{DateTime, Utc};
use clap::Parser;
use futures_util::StreamExt;
use rand::Rng;
use scenesync_core::protocol::{
    DeviceConfig, DeviceReport, PairRequest, PairResponse, PairStatus, PlaybackState,
    ServerMessage, TimeResponse, PROTOCOL_VERSION,
};
use scenesync_core::schedule::{evaluate, Desired, EvalInput, PlaybackKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;
use tokio::sync::{watch, Mutex};

#[derive(Parser, Debug)]
#[command(version, about)]
struct Args {
    /// Server base URL, e.g. http://scenesync.local:8080
    #[arg(
        long,
        env = "SCENESYNC_SERVER",
        default_value = "http://127.0.0.1:8080"
    )]
    server: String,
    /// Where to keep the device identity, cached config, and clips.
    #[arg(long, default_value = ".scenesync-sim")]
    state_dir: PathBuf,
    /// Name shown in the dashboard while pairing.
    #[arg(long, default_value = "Test player")]
    name: String,
    /// Play clips with mpv instead of only printing what would play.
    #[arg(long)]
    mpv: bool,
}

#[derive(Serialize, Deserialize, Default)]
struct Identity {
    device_id: String,
    token: Option<String>,
}

struct Player {
    args: Args,
    http: reqwest::Client,
    token: String,
    media_dir: PathBuf,
    /// Last applied config (also cached on disk for offline restarts).
    config: Mutex<Option<DeviceConfig>>,
    /// Estimated server_time - local_time in ms.
    offset_ms: Mutex<i64>,
    last_error: Mutex<Option<String>>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    let server = args.server.trim_end_matches('/').to_string();
    let args = Args { server, ..args };
    tokio::fs::create_dir_all(args.state_dir.join("media")).await?;
    let http = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(5))
        .build()?;

    let token = pair(&args, &http).await?;
    let cached = read_json::<DeviceConfig>(&args.state_dir.join("config.json")).await;
    if let Some(c) = &cached {
        println!(
            "Loaded cached config revision {} ({} clips)",
            c.revision,
            c.media.len()
        );
    }
    let player = Arc::new(Player {
        media_dir: args.state_dir.join("media"),
        args,
        http,
        token,
        config: Mutex::new(cached),
        offset_ms: Mutex::new(0),
        last_error: Mutex::new(None),
    });

    let (now_playing_tx, now_playing_rx) =
        watch::channel::<Desired>(Desired::Idle { source: None });
    tokio::spawn(playback_loop(player.clone(), now_playing_tx));
    tokio::spawn(report_loop(player.clone(), now_playing_rx));
    connection_loop(player).await
}

// ---------------------------------------------------------------------------
// Pairing
// ---------------------------------------------------------------------------

async fn pair(args: &Args, http: &reqwest::Client) -> Result<String> {
    let path = args.state_dir.join("identity.json");
    let mut id = read_json::<Identity>(&path).await.unwrap_or_default();
    if id.device_id.is_empty() {
        id.device_id = format!("sim-{:08x}", rand::rng().random::<u32>());
        write_json(&path, &id).await?;
    }
    if let Some(t) = &id.token {
        return Ok(t.clone());
    }
    let code = format!("{:06}", rand::rng().random_range(0..1_000_000));
    println!(
        "\n  Pairing code: {code}\n  Enter it in the SceneSync dashboard at {}\n",
        args.server
    );
    let req = PairRequest {
        device_id: id.device_id.clone(),
        code,
        name: args.name.clone(),
        model: Some("scenesync-sim".into()),
        app_version: Some(env!("CARGO_PKG_VERSION").into()),
        protocol_version: PROTOCOL_VERSION,
    };
    loop {
        match http
            .post(format!("{}/api/v1/pair", args.server))
            .json(&req)
            .send()
            .await
        {
            Ok(res) if res.status().is_success() => {
                let r: PairResponse = res.json().await?;
                if let (PairStatus::Approved, Some(token)) = (r.status, r.token) {
                    id.token = Some(token.clone());
                    write_json(&path, &id).await?;
                    println!("Paired as {}", id.device_id);
                    return Ok(token);
                }
            }
            Ok(res) => eprintln!("pairing: server said {}", res.status()),
            Err(e) => eprintln!("pairing: {e}"),
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}

// ---------------------------------------------------------------------------
// Server connection
// ---------------------------------------------------------------------------

async fn connection_loop(p: Arc<Player>) -> Result<()> {
    let mut backoff = 1u64;
    loop {
        match connect_once(&p).await {
            Ok(()) => backoff = 1,
            Err(e) => {
                eprintln!("server connection: {e:#} (playing from cache, retrying in {backoff}s)");
            }
        }
        tokio::time::sleep(Duration::from_secs(backoff)).await;
        backoff = (backoff * 2).min(30);
    }
}

async fn connect_once(p: &Arc<Player>) -> Result<()> {
    sync_clock(p).await?;
    let ws_url = format!(
        "{}/api/v1/device/ws?token={}",
        p.args.server.replacen("http", "ws", 1),
        p.token
    );
    let (mut ws, _) = tokio_tungstenite::connect_async(ws_url)
        .await
        .context("websocket connect")?;
    println!("Connected to {}", p.args.server);
    let mut resync = tokio::time::interval_at(
        tokio::time::Instant::now() + Duration::from_secs(600),
        Duration::from_secs(600),
    );
    loop {
        tokio::select! {
            msg = ws.next() => {
                let Some(msg) = msg else { bail!("server closed the connection") };
                let msg = msg?;
                if let Ok(text) = msg.to_text() {
                    if let Ok(ServerMessage::ConfigChanged { revision }) = serde_json::from_str(text) {
                        let applied = p.config.lock().await.as_ref().map(|c| c.revision);
                        if applied != Some(revision) {
                            if let Err(e) = update_config(p).await {
                                eprintln!("update failed, keeping revision {applied:?}: {e:#}");
                                *p.last_error.lock().await = Some(format!("{e:#}"));
                            }
                        }
                    }
                }
            }
            _ = resync.tick() => { let _ = sync_clock(p).await; }
        }
    }
}

/// NTP-style offset: keep the sample with the shortest round trip.
async fn sync_clock(p: &Player) -> Result<()> {
    let mut best: Option<(i64, i64)> = None; // (rtt, offset)
    for _ in 0..8 {
        let t0 = Utc::now().timestamp_millis();
        let r: TimeResponse = p
            .http
            .get(format!("{}/api/v1/time", p.args.server))
            .send()
            .await?
            .json()
            .await?;
        let t1 = Utc::now().timestamp_millis();
        let rtt = t1 - t0;
        let offset = r.server_time_ms - (t0 + rtt / 2);
        if best.is_none_or(|(b, _)| rtt < b) {
            best = Some((rtt, offset));
        }
    }
    if let Some((rtt, offset)) = best {
        *p.offset_ms.lock().await = offset;
        println!("Clock offset {offset} ms (round trip {rtt} ms)");
    }
    Ok(())
}

async fn update_config(p: &Player) -> Result<()> {
    let cfg: DeviceConfig = p
        .http
        .get(format!("{}/api/v1/device/config", p.args.server))
        .bearer_auth(&p.token)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    for m in &cfg.media {
        let dest = p.media_dir.join(&m.sha256);
        if tokio::fs::try_exists(&dest).await.unwrap_or(false) {
            continue;
        }
        println!("Downloading {} ({} bytes)", m.media_id, m.size);
        download(p, &format!("{}{}", p.args.server, m.url), &dest, &m.sha256)
            .await
            .with_context(|| format!("downloading {}", m.media_id))?;
    }
    // Every clip is present: apply atomically.
    write_json(&p.args.state_dir.join("config.json"), &cfg).await?;
    println!("Applied config revision {}", cfg.revision);
    if let Some(sync) = &cfg.sync {
        println!(
            "  In sync group {} ({}, trim {} ms)",
            sync.group_id,
            if sync.muted { "muted" } else { "plays sound" },
            sync.trim_ms
        );
    }
    if cfg.calibrate {
        println!("  Showing the calibration pattern (fit to window)");
    }
    if let Some(m) = &cfg.mapping {
        let pts: Vec<String> = m
            .corners
            .iter()
            .map(|[x, y]| format!("({x:.3}, {y:.3})"))
            .collect();
        println!("  Fit to window: corners {}", pts.join(" "));
    }
    *p.config.lock().await = Some(cfg);
    *p.last_error.lock().await = None;
    garbage_collect(p).await;
    Ok(())
}

/// Downloads to `<dest>.part`, resuming with a Range request if a partial
/// file exists, then verifies the checksum before renaming into place.
async fn download(p: &Player, url: &str, dest: &Path, sha256: &str) -> Result<()> {
    let part = dest.with_extension("part");
    let have = tokio::fs::metadata(&part)
        .await
        .map(|m| m.len())
        .unwrap_or(0);
    let mut req = p.http.get(url).bearer_auth(&p.token);
    if have > 0 {
        req = req.header("Range", format!("bytes={have}-"));
    }
    let res = req.send().await?.error_for_status()?;
    let resumed = res.status() == reqwest::StatusCode::PARTIAL_CONTENT;
    let mut file = if resumed {
        tokio::fs::OpenOptions::new()
            .append(true)
            .open(&part)
            .await?
    } else {
        tokio::fs::File::create(&part).await?
    };
    let mut stream = res.bytes_stream();
    while let Some(chunk) = stream.next().await {
        file.write_all(&chunk?).await?;
    }
    file.flush().await?;
    drop(file);

    let data = tokio::fs::read(&part).await?;
    let got = hex::encode(Sha256::digest(&data));
    if got != sha256 {
        tokio::fs::remove_file(&part).await?;
        bail!("checksum mismatch (expected {sha256}, got {got})");
    }
    tokio::fs::rename(&part, dest).await?;
    Ok(())
}

/// Deletes clips the current config no longer needs.
async fn garbage_collect(p: &Player) {
    let keep: Vec<String> = match &*p.config.lock().await {
        Some(c) => c.media.iter().map(|m| m.sha256.clone()).collect(),
        None => return,
    };
    let Ok(mut dir) = tokio::fs::read_dir(&p.media_dir).await else {
        return;
    };
    while let Ok(Some(entry)) = dir.next_entry().await {
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.ends_with(".part") && !keep.contains(&name) {
            let _ = tokio::fs::remove_file(entry.path()).await;
        }
    }
}

// ---------------------------------------------------------------------------
// Playback
// ---------------------------------------------------------------------------

async fn playback_loop(p: Arc<Player>, now_playing: watch::Sender<Desired>) {
    let mut current: Option<PlaybackKey> = None;
    let mut shown: Desired = Desired::Idle { source: None };
    let mut mpv: Option<tokio::process::Child> = None;
    let mut tick = tokio::time::interval(Duration::from_millis(250));
    loop {
        tick.tick().await;
        let now = Utc::now() + chrono::Duration::milliseconds(*p.offset_ms.lock().await);
        let desired = {
            let cfg = p.config.lock().await;
            let durations: HashMap<String, u64> = cfg
                .as_ref()
                .map(|c| {
                    c.media
                        .iter()
                        .filter_map(|m| m.duration_ms.map(|d| (m.media_id.clone(), d)))
                        .collect()
                })
                .unwrap_or_default();
            evaluate(&EvalInput {
                schedule: cfg.as_ref().and_then(|c| c.schedule.as_ref()),
                override_: cfg.as_ref().and_then(|c| c.override_.as_ref()),
                durations_ms: &durations,
                now,
                current: current.as_ref(),
            })
        };
        if desired == shown {
            continue;
        }
        if let Some(mut child) = mpv.take() {
            let _ = child.kill().await;
        }
        match &desired {
            Desired::Idle { .. } => println!("{} ■ black", stamp(now)),
            Desired::Play {
                media_id,
                looping,
                source,
                ..
            } => {
                println!(
                    "{} ▶ {} {media_id} ({source})",
                    stamp(now),
                    if *looping { "loop" } else { "play" }
                );
                if p.args.mpv {
                    mpv = spawn_mpv(&p, media_id, *looping).await;
                }
            }
        }
        current = desired.key();
        shown = desired.clone();
        let _ = now_playing.send(desired);
    }
}

async fn spawn_mpv(p: &Player, media_id: &str, looping: bool) -> Option<tokio::process::Child> {
    let sha = p
        .config
        .lock()
        .await
        .as_ref()?
        .media
        .iter()
        .find(|m| m.media_id == media_id)?
        .sha256
        .clone();
    let mut cmd = tokio::process::Command::new("mpv");
    cmd.arg("--fs")
        .arg("--really-quiet")
        .arg("--keep-open=no")
        .arg(p.media_dir.join(sha));
    if looping {
        cmd.arg("--loop-file=inf");
    }
    match cmd.kill_on_drop(true).spawn() {
        Ok(c) => Some(c),
        Err(e) => {
            eprintln!("could not start mpv: {e}");
            None
        }
    }
}

fn stamp(t: DateTime<Utc>) -> String {
    t.with_timezone(&chrono::Local)
        .format("%H:%M:%S")
        .to_string()
}

async fn report_loop(p: Arc<Player>, mut now_playing: watch::Receiver<Desired>) {
    loop {
        let desired = now_playing.borrow().clone();
        let (state, media_id) = match &desired {
            Desired::Idle { .. } => (PlaybackState::Idle, None),
            Desired::Play { media_id, .. } => (PlaybackState::Playing, Some(media_id.clone())),
        };
        let media_bytes = {
            let cfg = p.config.lock().await;
            cfg.as_ref().map(|c| c.media.iter().map(|m| m.size).sum())
        };
        let report = DeviceReport {
            applied_revision: p.config.lock().await.as_ref().map(|c| c.revision),
            state,
            media_id,
            media_bytes,
            clock_offset_ms: Some(*p.offset_ms.lock().await),
            app_version: Some(format!("sim {}", env!("CARGO_PKG_VERSION"))),
            errors: p.last_error.lock().await.clone().into_iter().collect(),
            ..Default::default()
        };
        let _ = p
            .http
            .post(format!("{}/api/v1/device/report", p.args.server))
            .bearer_auth(&p.token)
            .json(&report)
            .send()
            .await;
        // Report every 10 s, or sooner when playback changes.
        let _ = tokio::time::timeout(Duration::from_secs(10), now_playing.changed()).await;
    }
}

// ---------------------------------------------------------------------------

async fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Option<T> {
    let bytes = tokio::fs::read(path).await.ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// Writes via a temp file and rename so a crash never leaves half a file.
async fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let tmp = path.with_extension("tmp");
    tokio::fs::write(&tmp, serde_json::to_vec_pretty(value)?).await?;
    tokio::fs::rename(&tmp, path).await?;
    Ok(())
}
