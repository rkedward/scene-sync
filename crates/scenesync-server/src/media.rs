//! Media uploads: stream to disk, hash, inspect with ffprobe when available.

use std::path::Path;

use axum::extract::Multipart;
use axum::http::StatusCode;
use axum::Json;
use chrono::Utc;
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;

use crate::db::Media;
use crate::{random_token, ApiError, ApiResult, AppState};

/// Accepts one or more `file` fields. Each becomes a clip whose ID is derived
/// from its file name ("Talking Pumpkin.mp4" -> "talking-pumpkin").
pub(crate) async fn upload(
    state: &AppState,
    mut multipart: Multipart,
) -> ApiResult<Json<Vec<Media>>> {
    let mut created = Vec::new();
    while let Some(mut field) = multipart
        .next_field()
        .await
        .map_err(|e| ApiError::bad_request(format!("bad upload: {e}")))?
    {
        if field.name() != Some("file") {
            continue;
        }
        let filename = field.file_name().unwrap_or("clip.mp4").to_string();
        let tmp = state.config.data_dir.join("tmp").join(random_token(12));
        let mut out = tokio::fs::File::create(&tmp).await?;
        let mut hasher = Sha256::new();
        let mut size: u64 = 0;
        let result: ApiResult<()> = async {
            while let Some(chunk) = field
                .chunk()
                .await
                .map_err(|e| ApiError::bad_request(format!("upload interrupted: {e}")))?
            {
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
        if size == 0 {
            let _ = tokio::fs::remove_file(&tmp).await;
            return Err(ApiError::bad_request(format!("{filename} is empty")));
        }

        let sha256 = hex::encode(hasher.finalize());
        let dest = state.media_path(&sha256);
        if tokio::fs::try_exists(&dest).await.unwrap_or(false) {
            tokio::fs::remove_file(&tmp).await?;
        } else {
            tokio::fs::rename(&tmp, &dest).await?;
        }

        let probe = probe(&dest).await;
        let id = unique_id(state, &slug(&filename))?;
        let media = Media {
            id,
            filename,
            sha256,
            size,
            duration_ms: probe.duration_ms,
            width: probe.width,
            height: probe.height,
            codec: probe.codec,
            created_at: Utc::now(),
        };
        state.db.insert_media(&media)?;
        tracing::info!(
            "uploaded {} ({} bytes) as {}",
            media.filename,
            media.size,
            media.id
        );
        created.push(media);
    }
    if created.is_empty() {
        return Err(ApiError(
            StatusCode::BAD_REQUEST,
            "no file field in upload".into(),
        ));
    }
    Ok(Json(created))
}

pub(crate) fn slug(filename: &str) -> String {
    let stem = Path::new(filename)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("clip");
    let mut out = String::new();
    for c in stem.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    let out = out
        .trim_end_matches('-')
        .chars()
        .take(48)
        .collect::<String>();
    if out.is_empty() {
        "clip".into()
    } else {
        out
    }
}

fn unique_id(state: &AppState, base: &str) -> ApiResult<String> {
    if !state.db.media_exists(base)? {
        return Ok(base.to_string());
    }
    for n in 2.. {
        let candidate = format!("{base}-{n}");
        if !state.db.media_exists(&candidate)? {
            return Ok(candidate);
        }
    }
    unreachable!()
}

#[derive(Default)]
struct Probe {
    duration_ms: Option<u64>,
    width: Option<u32>,
    height: Option<u32>,
    codec: Option<String>,
}

/// Reads clip metadata with ffprobe if it is installed. This only inspects
/// the file; the server never transcodes.
async fn probe(path: &Path) -> Probe {
    let output = tokio::process::Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=codec_name,width,height:format=duration",
            "-of",
            "json",
        ])
        .arg(path)
        .output()
        .await;
    let Ok(output) = output else {
        return Probe::default();
    };
    if !output.status.success() {
        return Probe::default();
    }
    let Ok(v) = serde_json::from_slice::<serde_json::Value>(&output.stdout) else {
        return Probe::default();
    };
    let stream = &v["streams"][0];
    Probe {
        duration_ms: v["format"]["duration"]
            .as_str()
            .and_then(|s| s.parse::<f64>().ok())
            .map(|s| (s * 1000.0).round() as u64),
        width: stream["width"].as_u64().map(|w| w as u32),
        height: stream["height"].as_u64().map(|h| h as u32),
        codec: stream["codec_name"].as_str().map(str::to_string),
    }
}

#[cfg(test)]
mod tests {
    use super::slug;

    #[test]
    fn slugs() {
        assert_eq!(slug("Talking Pumpkin.mp4"), "talking-pumpkin");
        assert_eq!(slug("ghost__v2 (final).MP4"), "ghost-v2-final");
        assert_eq!(slug(".mp4"), "mp4");
        assert_eq!(slug("!!!.mp4"), "clip");
    }
}
