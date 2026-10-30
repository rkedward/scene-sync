//! Player app updates. The dashboard uploads an APK; every player whose own
//! package differs downloads and installs it.

use axum::body::Body;
use axum::extract::{Multipart, Request, State};
use axum::http::StatusCode;
use axum::response::Response;
use axum::Json;
use chrono::{DateTime, Utc};
use scenesync_core::protocol::AppRelease;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;
use tower::ServiceExt;
use tower_http::services::ServeFile;

use crate::{random_token, Admin, ApiError, ApiResult, AppState};

pub(crate) const APK_URL: &str = "/api/v1/app/player.apk";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ReleaseInfo {
    pub sha256: String,
    pub size: u64,
    pub filename: String,
    pub uploaded_at: DateTime<Utc>,
    /// The GitHub release it came from (e.g. "v0.5.1"); None for an upload.
    #[serde(default)]
    pub tag: Option<String>,
}

fn dir(state: &AppState) -> std::path::PathBuf {
    state.config.data_dir.join("app")
}

/// The current release, if an APK has been uploaded.
pub(crate) fn current(state: &AppState) -> Option<ReleaseInfo> {
    let text = std::fs::read_to_string(dir(state).join("release.json")).ok()?;
    serde_json::from_str(&text).ok()
}

pub(crate) fn release_for_config(state: &AppState) -> Option<AppRelease> {
    current(state).map(|r| AppRelease {
        sha256: r.sha256,
        size: r.size,
        url: APK_URL.into(),
    })
}

pub(crate) async fn info(_: Admin, State(state): State<AppState>) -> Json<Option<ReleaseInfo>> {
    Json(current(&state))
}

/// Tells every player to re-read its config.
fn notify_all(state: &AppState) -> ApiResult<()> {
    for d in state.db.devices()? {
        let rev = state.db.bump_revision(&d.id)?;
        state.notify(&d.id, rev);
    }
    Ok(())
}

pub(crate) async fn upload(
    _: Admin,
    State(state): State<AppState>,
    mut multipart: Multipart,
) -> ApiResult<Json<ReleaseInfo>> {
    tokio::fs::create_dir_all(dir(&state)).await?;
    while let Some(mut field) = multipart
        .next_field()
        .await
        .map_err(|e| ApiError::bad_request(format!("bad upload: {e}")))?
    {
        if field.name() != Some("file") {
            continue;
        }
        let filename = field.file_name().unwrap_or("player.apk").to_string();
        let tmp = state.config.data_dir.join("tmp").join(random_token(12));
        let mut out = tokio::fs::File::create(&tmp).await?;
        let mut hasher = Sha256::new();
        let mut size = 0u64;
        let mut head = Vec::new();
        let result: ApiResult<()> = async {
            while let Some(chunk) = field
                .chunk()
                .await
                .map_err(|e| ApiError::bad_request(format!("upload interrupted: {e}")))?
            {
                if head.len() < 4 {
                    head.extend_from_slice(&chunk[..chunk.len().min(4)]);
                }
                hasher.update(&chunk);
                size += chunk.len() as u64;
                out.write_all(&chunk).await?;
            }
            out.flush().await?;
            Ok(())
        }
        .await;
        drop(out);
        if let Err(e) = result {
            let _ = tokio::fs::remove_file(&tmp).await;
            return Err(e);
        }
        // An APK is a zip file.
        if !head.starts_with(b"PK\x03\x04") {
            let _ = tokio::fs::remove_file(&tmp).await;
            return Err(ApiError::bad_request(format!("{filename} is not an APK")));
        }
        let info = ReleaseInfo {
            sha256: hex::encode(hasher.finalize()),
            size,
            filename,
            uploaded_at: Utc::now(),
            tag: None,
        };
        return Ok(Json(offer(&state, &tmp, info).await?));
    }
    Err(ApiError::bad_request("no file in upload"))
}

/// Makes the APK at `file` (already checked) the one every projector runs.
pub(crate) async fn offer(
    state: &AppState,
    file: &std::path::Path,
    info: ReleaseInfo,
) -> ApiResult<ReleaseInfo> {
    tokio::fs::create_dir_all(dir(state)).await?;
    tokio::fs::rename(file, dir(state).join("player.apk")).await?;
    tokio::fs::write(
        dir(state).join("release.json"),
        serde_json::to_vec_pretty(&info).map_err(anyhow::Error::from)?,
    )
    .await?;
    notify_all(state)?;
    Ok(info)
}

pub(crate) async fn remove(_: Admin, State(state): State<AppState>) -> ApiResult<StatusCode> {
    let _ = tokio::fs::remove_file(dir(&state).join("release.json")).await;
    let _ = tokio::fs::remove_file(dir(&state).join("player.apk")).await;
    notify_all(&state)?;
    Ok(StatusCode::NO_CONTENT)
}

/// Serves the APK without authentication, so it can also be opened in a
/// browser on a projector for the first install.
pub(crate) async fn download(State(state): State<AppState>, req: Request) -> ApiResult<Response> {
    if current(&state).is_none() {
        return Err(ApiError::not_found("app"));
    }
    let res = ServeFile::new_with_mime(
        dir(&state).join("player.apk"),
        &"application/vnd.android.package-archive".parse().unwrap(),
    )
    .oneshot(req)
    .await
    .map_err(|e| anyhow::anyhow!("serving app: {e}"))?;
    Ok(res.map(Body::new))
}
