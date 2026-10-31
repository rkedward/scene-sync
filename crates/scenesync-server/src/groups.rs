//! Groups: named sets of projectors that share a schedule and take commands
//! together (and, in a later phase, play in sync).

use axum::extract::{Path as UrlPath, State};
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;

use crate::db::Group;
use crate::schedules::assign;
use crate::{Admin, ApiError, ApiResult, AppState};

pub(crate) async fn list(_: Admin, State(state): State<AppState>) -> ApiResult<Json<Vec<Group>>> {
    Ok(Json(state.db.groups()?))
}

#[derive(Deserialize)]
pub(crate) struct SaveRequest {
    name: String,
}

/// Creates or renames a group.
pub(crate) async fn save(
    _: Admin,
    State(state): State<AppState>,
    UrlPath(id): UrlPath<String>,
    Json(req): Json<SaveRequest>,
) -> ApiResult<Json<Group>> {
    if id.is_empty()
        || id.len() > 64
        || !id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err(ApiError::bad_request(
            "group id may use letters, digits, '-' and '_' (max 64)",
        ));
    }
    let name = req.name.trim();
    if name.is_empty() {
        return Err(ApiError::bad_request("name must not be empty"));
    }
    state.db.save_group(&id, name)?;
    Ok(Json(state.db.group(&id)?.expect("just saved")))
}

pub(crate) async fn remove(
    _: Admin,
    State(state): State<AppState>,
    UrlPath(id): UrlPath<String>,
) -> ApiResult<StatusCode> {
    let members = state.db.group(&id)?.map(|g| g.devices).unwrap_or_default();
    if state.db.delete_group(&id)? {
        // Former members lose the group's sync and mute settings.
        bump_all(&state, &members)?;
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::not_found("group"))
    }
}

#[derive(Deserialize)]
pub(crate) struct ScheduleRequest {
    schedule_id: Option<String>,
}

/// Sets the group's schedule and gives it to every member.
pub(crate) async fn set_schedule(
    _: Admin,
    State(state): State<AppState>,
    UrlPath(id): UrlPath<String>,
    Json(req): Json<ScheduleRequest>,
) -> ApiResult<StatusCode> {
    let group = state
        .db
        .group(&id)?
        .ok_or_else(|| ApiError::not_found("group"))?;
    if let Some(sid) = &req.schedule_id {
        state
            .db
            .schedule(sid)?
            .ok_or_else(|| ApiError::not_found("schedule"))?;
    }
    state
        .db
        .set_group_schedule(&id, req.schedule_id.as_deref())?;
    for d in &group.devices {
        assign(&state, d, req.schedule_id.as_deref())?;
    }
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
pub(crate) struct MembershipRequest {
    group_id: Option<String>,
}

/// Moves a projector into a group (taking the group's schedule, if it has
/// one) or out of all groups (keeping its schedule).
pub(crate) async fn set_device_group(
    _: Admin,
    State(state): State<AppState>,
    UrlPath(device_id): UrlPath<String>,
    Json(req): Json<MembershipRequest>,
) -> ApiResult<StatusCode> {
    let device = state
        .db
        .device(&device_id)?
        .ok_or_else(|| ApiError::not_found("device"))?;
    // Members of the old and new group: their sync settings (who plays
    // sound, who is in step) can change with membership.
    let mut affected = vec![device_id.clone()];
    if let Some(old) = device.group_id.as_deref() {
        if let Some(g) = state.db.group(old)? {
            affected.extend(g.devices);
        }
    }
    let group = match &req.group_id {
        Some(gid) => Some(
            state
                .db
                .group(gid)?
                .ok_or_else(|| ApiError::not_found("group"))?,
        ),
        None => None,
    };
    state
        .db
        .set_device_group(&device_id, req.group_id.as_deref())?;
    if let Some(g) = &group {
        affected.extend(g.devices.iter().cloned());
    }
    if let Some(sid) = group.and_then(|g| g.schedule_id) {
        assign(&state, &device_id, Some(&sid))?;
    }
    affected.sort();
    affected.dedup();
    bump_all(&state, &affected)?;
    Ok(StatusCode::NO_CONTENT)
}

/// Tells these projectors to re-read their config.
fn bump_all(state: &AppState, ids: &[String]) -> ApiResult<()> {
    for d in ids {
        let rev = state.db.bump_revision(d)?;
        state.notify(d, rev);
    }
    Ok(())
}

#[derive(Deserialize)]
pub(crate) struct SyncRequest {
    sync: bool,
    /// Member that plays sound; `null` = the first member.
    #[serde(default)]
    audio_device_id: Option<String>,
}

/// Turns synchronized playback on or off for a group.
pub(crate) async fn set_sync(
    _: Admin,
    State(state): State<AppState>,
    UrlPath(id): UrlPath<String>,
    Json(req): Json<SyncRequest>,
) -> ApiResult<StatusCode> {
    let group = state
        .db
        .group(&id)?
        .ok_or_else(|| ApiError::not_found("group"))?;
    if let Some(a) = &req.audio_device_id {
        if !group.devices.contains(a) {
            return Err(ApiError::bad_request(
                "the sound projector must be in the group",
            ));
        }
    }
    state
        .db
        .set_group_sync(&id, req.sync, req.audio_device_id.as_deref())?;
    for d in &group.devices {
        let rev = state.db.bump_revision(d)?;
        state.notify(d, rev);
    }
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
pub(crate) struct TrimRequest {
    trim_ms: i64,
}

/// Per-projector latency trim for sync groups.
pub(crate) async fn set_trim(
    _: Admin,
    State(state): State<AppState>,
    UrlPath(id): UrlPath<String>,
    Json(req): Json<TrimRequest>,
) -> ApiResult<StatusCode> {
    if !(-2000..=2000).contains(&req.trim_ms) {
        return Err(ApiError::bad_request(
            "trim must be between -2000 and 2000 ms",
        ));
    }
    let rev = state
        .db
        .set_trim(&id, req.trim_ms)?
        .ok_or_else(|| ApiError::not_found("device"))?;
    state.notify(&id, Some(rev));
    Ok(StatusCode::NO_CONTENT)
}

/// The sync settings to put in a device's config, if its group plays in sync.
pub(crate) fn sync_config(
    state: &AppState,
    d: &crate::db::Device,
) -> ApiResult<Option<scenesync_core::protocol::SyncConfig>> {
    let Some(gid) = &d.group_id else {
        return Ok(None);
    };
    let Some(g) = state.db.group(gid)? else {
        return Ok(None);
    };
    if !g.sync {
        return Ok(None);
    }
    let audio = g
        .audio_device_id
        .filter(|a| g.devices.contains(a))
        .or_else(|| g.devices.first().cloned());
    Ok(Some(scenesync_core::protocol::SyncConfig {
        group_id: g.id,
        muted: audio.as_deref() != Some(d.id.as_str()),
        trim_ms: d.trim_ms,
    }))
}
