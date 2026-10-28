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
    if state.db.delete_group(&id)? {
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
    state
        .db
        .device(&device_id)?
        .ok_or_else(|| ApiError::not_found("device"))?;
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
    if let Some(sid) = group.and_then(|g| g.schedule_id) {
        assign(&state, &device_id, Some(&sid))?;
    }
    Ok(StatusCode::NO_CONTENT)
}
