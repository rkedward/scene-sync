//! Backups of the settings database (projectors, schedules, groups, clip
//! list). Clips themselves live in `data/media` and are copied separately.

use std::path::{Path, PathBuf};
use std::time::Duration;

use axum::extract::State;
use axum::http::header;
use axum::response::IntoResponse;
use chrono::Utc;

use crate::{random_token, Admin, ApiResult, AppState};

/// How many daily backups to keep.
const KEEP: usize = 14;

/// Copies the database into `data/backups/` once a day and prunes old copies.
pub fn spawn_daily(state: AppState) {
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(60)).await;
        loop {
            if let Err(e) = daily(&state) {
                tracing::warn!("daily backup failed: {e:#}");
            }
            tokio::time::sleep(Duration::from_secs(24 * 3600)).await;
        }
    });
}

fn daily(state: &AppState) -> anyhow::Result<PathBuf> {
    let dir = state.config.data_dir.join("backups");
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!(
        "scenesync-{}.sqlite",
        Utc::now().format("%Y-%m-%d")
    ));
    if !path.exists() {
        state.db.backup_to(&path)?;
        tracing::info!("backup written to {}", path.display());
    }
    prune(&dir, KEEP)?;
    Ok(path)
}

fn prune(dir: &Path, keep: usize) -> anyhow::Result<()> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("scenesync-") && n.ends_with(".sqlite"))
        })
        .collect();
    // Names sort by date.
    files.sort();
    let excess = files.len().saturating_sub(keep);
    for f in &files[..excess] {
        let _ = std::fs::remove_file(f);
    }
    Ok(())
}

/// `GET /api/v1/admin/backup`: the settings database as a download.
pub(crate) async fn download(
    _: Admin,
    State(state): State<AppState>,
) -> ApiResult<impl IntoResponse> {
    let tmp = state.config.data_dir.join("tmp").join(random_token(12));
    state.db.backup_to(&tmp)?;
    let bytes = tokio::fs::read(&tmp).await;
    let _ = tokio::fs::remove_file(&tmp).await;
    let name = format!("scenesync-backup-{}.sqlite", Utc::now().format("%Y-%m-%d"));
    Ok((
        [
            (header::CONTENT_TYPE, "application/vnd.sqlite3".to_string()),
            (
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"{name}\""),
            ),
        ],
        bytes?,
    ))
}

#[cfg(test)]
mod tests {
    use super::prune;

    #[test]
    fn prune_keeps_newest() {
        let dir = tempfile::tempdir().unwrap();
        for d in 1..=5 {
            std::fs::write(
                dir.path().join(format!("scenesync-2026-10-0{d}.sqlite")),
                b"x",
            )
            .unwrap();
        }
        std::fs::write(dir.path().join("other.txt"), b"x").unwrap();
        prune(dir.path(), 2).unwrap();
        let mut left: Vec<String> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        left.sort();
        assert_eq!(
            left,
            [
                "other.txt",
                "scenesync-2026-10-04.sqlite",
                "scenesync-2026-10-05.sqlite"
            ]
        );
    }
}
