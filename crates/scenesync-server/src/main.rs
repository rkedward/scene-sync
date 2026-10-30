use std::net::SocketAddr;
use std::path::PathBuf;

use anyhow::{Context, Result};
use scenesync_server::{app, AppState, Config};
use tracing_subscriber::EnvFilter;

const USAGE: &str = "SceneSync server

Usage:
  scenesync-server                 run the server
  scenesync-server backup <file>   write a copy of the settings database
  scenesync-server --version

Environment:
  SCENESYNC_DATA_DIR        data folder (default ./data)
  SCENESYNC_LISTEN          address to listen on (default 0.0.0.0:8080)
  SCENESYNC_ADMIN_PASSWORD  dashboard password (default: generated, saved in the data folder)
  SCENESYNC_NO_MDNS         set to skip advertising on the local network
  SCENESYNC_NO_UPDATE_CHECK set to never contact GitHub for updates
  SCENESYNC_UPDATE_URL      release API to check (default: the GitHub repo)
  RUST_LOG                  log detail, e.g. debug";

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        None => {}
        Some("--version" | "-V") => {
            println!("scenesync-server {}", env!("CARGO_PKG_VERSION"));
            return Ok(());
        }
        Some("backup") if args.len() == 2 => {
            let data_dir = PathBuf::from(
                std::env::var("SCENESYNC_DATA_DIR").unwrap_or_else(|_| "data".into()),
            );
            let db = scenesync_server::db::Db::open(&data_dir.join("scenesync.sqlite"))?;
            db.backup_to(std::path::Path::new(&args[1]))?;
            println!("Wrote {}", args[1]);
            return Ok(());
        }
        _ => {
            println!("{USAGE}");
            return Ok(());
        }
    }

    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| "info,tower_http=info".into()),
        )
        .init();

    let data_dir =
        PathBuf::from(std::env::var("SCENESYNC_DATA_DIR").unwrap_or_else(|_| "data".into()));
    let listen: SocketAddr = std::env::var("SCENESYNC_LISTEN")
        .unwrap_or_else(|_| "0.0.0.0:8080".into())
        .parse()
        .context("SCENESYNC_LISTEN must look like 0.0.0.0:8080")?;

    let config = Config::load(&data_dir, std::env::var("SCENESYNC_ADMIN_PASSWORD").ok())?;
    let state = AppState::new(config)?;

    if std::env::var("SCENESYNC_NO_MDNS").is_err() {
        if let Err(e) = scenesync_server::mdns::advertise(listen.port()) {
            tracing::warn!(
                "mDNS advertisement failed (players can still use the address directly): {e:#}"
            );
        }
    }

    scenesync_server::backup::spawn_daily(state.clone());
    if std::env::var("SCENESYNC_NO_UPDATE_CHECK").is_err() {
        scenesync_server::updates::spawn_daily(state.clone());
    }

    let listener = tokio::net::TcpListener::bind(listen).await?;
    tracing::info!(
        "SceneSync server listening on http://{listen} (data in {})",
        data_dir.display()
    );
    axum::serve(listener, app(state)).await?;
    Ok(())
}
