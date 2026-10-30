//! Updates from GitHub releases. The Pi checks the latest release; it can
//! fetch the player APK for every projector, and ask the root updater
//! (deploy/scenesync-update.*) to install a newer server. With automatic
//! updates on, both happen once a day at 04:00 local time.

use std::time::Duration;

use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;
use chrono::{DateTime, Local, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::app_update::{self, ReleaseInfo};
use crate::{random_token, Admin, ApiError, ApiResult, AppState};

pub const DEFAULT_URL: &str = "https://api.github.com/repos/rkedward/scene-sync/releases/latest";
const APK_ASSET: &str = "scenesync-player.apk";
/// Written for the root updater, which watches for it with a systemd path unit.
const REQUEST_FILE: &str = "update-requested";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Release {
    pub tag: String,
    pub url: String,
    pub published_at: Option<DateTime<Utc>>,
    /// Download link for the player APK, if the release has one.
    pub apk_url: Option<String>,
}

/// Saved in `data/updates.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Saved {
    auto: bool,
    checked_at: Option<DateTime<Utc>>,
    latest: Option<Release>,
    error: Option<String>,
}

impl Default for Saved {
    fn default() -> Self {
        Saved {
            auto: true,
            checked_at: None,
            latest: None,
            error: None,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct Status {
    auto: bool,
    checked_at: Option<DateTime<Utc>>,
    latest: Option<Release>,
    error: Option<String>,
    server: ServerStatus,
    player: PlayerStatus,
}

#[derive(Debug, Serialize)]
struct ServerStatus {
    version: String,
    update_available: bool,
    /// The root updater is installed, so the dashboard can update the server.
    can_update: bool,
    /// An update was requested and hasn't run yet.
    updating: bool,
}

#[derive(Debug, Serialize)]
struct PlayerStatus {
    /// Release tag of the APK being offered; None for an uploaded build.
    tag: Option<String>,
    offered: bool,
    update_available: bool,
}

fn load(state: &AppState) -> Saved {
    std::fs::read_to_string(state.config.data_dir.join("updates.json"))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

fn save(state: &AppState, saved: &Saved) -> anyhow::Result<()> {
    std::fs::write(
        state.config.data_dir.join("updates.json"),
        serde_json::to_vec_pretty(saved)?,
    )?;
    Ok(())
}

/// "v0.5.1" or "0.5.1" -> [0, 5, 1]. Anything after a '-' is ignored.
fn parse_version(v: &str) -> Vec<u64> {
    v.trim_start_matches('v')
        .split('-')
        .next()
        .unwrap_or("")
        .split('.')
        .map(|p| p.parse().unwrap_or(0))
        .collect()
}

pub(crate) fn newer(candidate: &str, than: &str) -> bool {
    parse_version(candidate) > parse_version(than)
}

fn status(state: &AppState) -> Status {
    let saved = load(state);
    let version = env!("CARGO_PKG_VERSION").to_string();
    let offered = app_update::current(state);
    let latest_tag = saved.latest.as_ref().map(|r| r.tag.as_str());
    let player_tag = offered.as_ref().and_then(|o| o.tag.clone());
    let has_apk = saved.latest.as_ref().is_some_and(|r| r.apk_url.is_some());
    Status {
        auto: saved.auto,
        checked_at: saved.checked_at,
        error: saved.error.clone(),
        server: ServerStatus {
            update_available: latest_tag.is_some_and(|t| newer(t, &version)),
            can_update: state.config.updater_unit.exists(),
            updating: state.config.data_dir.join(REQUEST_FILE).exists(),
            version,
        },
        player: PlayerStatus {
            update_available: has_apk
                && match (&player_tag, latest_tag) {
                    (Some(p), Some(l)) => newer(l, p),
                    _ => true,
                },
            offered: offered.is_some(),
            tag: player_tag,
        },
        latest: saved.latest,
    }
}

fn client() -> anyhow::Result<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .user_agent(concat!("scenesync-server/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(300))
        .build()?)
}

/// Asks GitHub for the latest release and saves the answer.
async fn check(state: &AppState) -> Saved {
    let mut saved = load(state);
    saved.checked_at = Some(Utc::now());
    match fetch_latest(&state.config.update_url).await {
        Ok(r) => {
            saved.latest = Some(r);
            saved.error = None;
        }
        Err(e) => saved.error = Some(format!("Could not check GitHub: {e:#}")),
    }
    if let Err(e) = save(state, &saved) {
        tracing::warn!("saving update status: {e:#}");
    }
    saved
}

async fn fetch_latest(url: &str) -> anyhow::Result<Release> {
    #[derive(Deserialize)]
    struct Asset {
        name: String,
        browser_download_url: String,
    }
    #[derive(Deserialize)]
    struct GhRelease {
        tag_name: String,
        html_url: String,
        published_at: Option<DateTime<Utc>>,
        #[serde(default)]
        assets: Vec<Asset>,
    }
    let r: GhRelease = client()?
        .get(url)
        .header("Accept", "application/vnd.github+json")
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    Ok(Release {
        apk_url: r
            .assets
            .iter()
            .find(|a| a.name == APK_ASSET)
            .map(|a| a.browser_download_url.clone()),
        tag: r.tag_name,
        url: r.html_url,
        published_at: r.published_at,
    })
}

/// Downloads the release's APK and offers it to every projector.
async fn install_player(state: &AppState, release: &Release) -> ApiResult<ReleaseInfo> {
    let url = release
        .apk_url
        .as_deref()
        .ok_or_else(|| ApiError::bad_request(format!("{} has no player APK", release.tag)))?;
    let bytes = client()?
        .get(url)
        .send()
        .await
        .and_then(|r| r.error_for_status())
        .map_err(|e| ApiError::new(StatusCode::BAD_GATEWAY, format!("Download failed: {e}")))?
        .bytes()
        .await
        .map_err(|e| ApiError::new(StatusCode::BAD_GATEWAY, format!("Download failed: {e}")))?;
    if !bytes.starts_with(b"PK\x03\x04") {
        return Err(ApiError::new(
            StatusCode::BAD_GATEWAY,
            format!("{APK_ASSET} from {} is not an APK", release.tag),
        ));
    }
    let tmp = state.config.data_dir.join("tmp").join(random_token(12));
    tokio::fs::write(&tmp, &bytes).await?;
    let info = ReleaseInfo {
        sha256: hex::encode(Sha256::digest(&bytes)),
        size: bytes.len() as u64,
        filename: format!("{APK_ASSET} ({})", release.tag),
        uploaded_at: Utc::now(),
        tag: Some(release.tag.clone()),
    };
    let result = app_update::offer(state, &tmp, info).await;
    if result.is_err() {
        let _ = tokio::fs::remove_file(&tmp).await;
    }
    tracing::info!("player app updated to {}", release.tag);
    result
}

/// Leaves a note for the root updater. It replaces the server and restarts it.
fn request_server_update(state: &AppState, tag: &str) -> ApiResult<()> {
    if !state.config.updater_unit.exists() {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "The updater isn't installed on this Pi. Run the install command again to add it.",
        ));
    }
    std::fs::write(state.config.data_dir.join(REQUEST_FILE), tag)?;
    tracing::info!("server update to {tag} requested");
    Ok(())
}

// --- Dashboard API ---------------------------------------------------------

pub(crate) async fn get_status(_: Admin, State(state): State<AppState>) -> Json<Status> {
    Json(status(&state))
}

pub(crate) async fn check_now(_: Admin, State(state): State<AppState>) -> Json<Status> {
    check(&state).await;
    Json(status(&state))
}

#[derive(Deserialize)]
pub(crate) struct Settings {
    auto: bool,
}

pub(crate) async fn set_settings(
    _: Admin,
    State(state): State<AppState>,
    Json(s): Json<Settings>,
) -> ApiResult<Json<Status>> {
    let mut saved = load(&state);
    saved.auto = s.auto;
    save(&state, &saved)?;
    Ok(Json(status(&state)))
}

/// Offers the latest release's APK to every projector (replacing an upload).
pub(crate) async fn update_player(
    _: Admin,
    State(state): State<AppState>,
) -> ApiResult<Json<Status>> {
    let saved = check(&state).await;
    let release = saved.latest.ok_or_else(|| {
        ApiError::new(
            StatusCode::BAD_GATEWAY,
            saved.error.unwrap_or_else(|| "No release found".into()),
        )
    })?;
    install_player(&state, &release).await?;
    Ok(Json(status(&state)))
}

pub(crate) async fn update_server(
    _: Admin,
    State(state): State<AppState>,
) -> ApiResult<(StatusCode, Json<Status>)> {
    let saved = check(&state).await;
    let release = saved.latest.ok_or_else(|| {
        ApiError::new(
            StatusCode::BAD_GATEWAY,
            saved.error.unwrap_or_else(|| "No release found".into()),
        )
    })?;
    if !newer(&release.tag, env!("CARGO_PKG_VERSION")) {
        return Err(ApiError::bad_request(format!(
            "The server is already on the latest version ({}).",
            env!("CARGO_PKG_VERSION")
        )));
    }
    request_server_update(&state, &release.tag)?;
    Ok((StatusCode::ACCEPTED, Json(status(&state))))
}

// --- Daily check -------------------------------------------------------------

/// Checks GitHub a minute after start (just to show what's available), then
/// at 04:00 every day installs updates if automatic updates are on.
pub fn spawn_daily(state: AppState) {
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(60)).await;
        check(&state).await;
        loop {
            tokio::time::sleep(until_next_4am()).await;
            if let Err(e) = auto_update(&state).await {
                tracing::warn!("automatic update failed: {e}");
            }
        }
    });
}

fn until_next_4am() -> Duration {
    let now = Local::now();
    let today = now.date_naive().and_hms_opt(4, 0, 0).unwrap();
    let next = if now.naive_local() < today {
        today
    } else {
        today + chrono::Duration::days(1)
    };
    (next - now.naive_local())
        .to_std()
        .unwrap_or(Duration::from_secs(3600))
}

/// One automatic pass: update the player app (unless a hand-uploaded build is
/// being offered, which stays until you choose otherwise), then the server.
/// Public for tests; the daily task calls it.
pub async fn auto_update(state: &AppState) -> Result<(), String> {
    auto_update_inner(state).await.map_err(|e| e.1)
}

async fn auto_update_inner(state: &AppState) -> ApiResult<()> {
    let saved = check(state).await;
    if !saved.auto {
        return Ok(());
    }
    let Some(release) = saved.latest else {
        return Ok(());
    };
    let offered = app_update::current(state);
    let player_follows_github = offered.as_ref().is_none_or(|o| o.tag.is_some());
    let player_behind = offered
        .as_ref()
        .and_then(|o| o.tag.as_deref())
        .is_none_or(|t| newer(&release.tag, t));
    if release.apk_url.is_some() && player_follows_github && player_behind {
        install_player(state, &release).await?;
    }
    if newer(&release.tag, env!("CARGO_PKG_VERSION")) && state.config.updater_unit.exists() {
        request_server_update(state, &release.tag)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::newer;

    #[test]
    fn compares_versions() {
        assert!(newer("v0.5.1", "0.5.0"));
        assert!(newer("v0.10.0", "v0.9.9"));
        assert!(!newer("v0.5.0", "0.5.0"));
        assert!(!newer("v0.4.9", "0.5.0"));
        assert!(newer("v1.0", "0.9.9"));
    }
}
