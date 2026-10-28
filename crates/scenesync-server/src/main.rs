use std::net::SocketAddr;
use std::path::PathBuf;

use anyhow::{Context, Result};
use scenesync_server::{app, AppState, Config};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<()> {
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

    let listener = tokio::net::TcpListener::bind(listen).await?;
    tracing::info!(
        "SceneSync server listening on http://{listen} (data in {})",
        data_dir.display()
    );
    axum::serve(listener, app(state)).await?;
    Ok(())
}
