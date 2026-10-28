//! Shared schedules: a schedule is saved once and assigned to any number of
//! devices. Saving bumps the revision of every device that uses it.

use std::collections::HashMap;

use axum::extract::{Path as UrlPath, Query, State};
use axum::http::StatusCode;
use axum::Json;
use chrono::{DateTime, Duration, NaiveDate, NaiveTime, Utc};
use scenesync_core::schedule::{evaluate, resolve_local, Desired, EvalInput, Schedule};
use serde::{Deserialize, Serialize};

use crate::{required_media, Admin, ApiError, ApiResult, AppState};

pub(crate) async fn list(
    _: Admin,
    State(state): State<AppState>,
) -> ApiResult<Json<Vec<ScheduleView>>> {
    let mut out = Vec::new();
    for s in state.db.schedules()? {
        let devices = state.db.schedule_users(&s.schedule_id)?;
        out.push(ScheduleView {
            schedule: s,
            devices,
        });
    }
    Ok(Json(out))
}

#[derive(Serialize)]
pub(crate) struct ScheduleView {
    #[serde(flatten)]
    schedule: Schedule,
    devices: Vec<String>,
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Creates or replaces a schedule. The URL decides its ID; the revision is
/// assigned by the server.
pub(crate) async fn save(
    _: Admin,
    State(state): State<AppState>,
    UrlPath(id): UrlPath<String>,
    Json(mut schedule): Json<Schedule>,
) -> ApiResult<Json<Schedule>> {
    if !valid_id(&id) {
        return Err(ApiError::bad_request(
            "schedule id may use letters, digits, '-' and '_' (max 64)",
        ));
    }
    schedule.schedule_id = id;
    schedule
        .validate()
        .map_err(|e| ApiError::bad_request(e.to_string()))?;
    let needed = required_media(Some(&schedule), None);
    for m in &needed {
        if !state.db.media_exists(m)? {
            return Err(ApiError::bad_request(format!(
                "schedule uses unknown clip {m:?}"
            )));
        }
    }
    let (saved, users) = state.db.save_schedule(schedule)?;
    let needed: Vec<&str> = needed.iter().map(String::as_str).collect();
    for d in users {
        state.db.add_device_media(&d, &needed)?;
        let rev = state.db.bump_revision(&d)?;
        state.notify(&d, rev);
    }
    Ok(Json(saved))
}

pub(crate) async fn remove(
    _: Admin,
    State(state): State<AppState>,
    UrlPath(id): UrlPath<String>,
) -> ApiResult<StatusCode> {
    let users = state.db.schedule_users(&id)?;
    if !users.is_empty() {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            format!("schedule is assigned to {} projector(s)", users.len()),
        ));
    }
    if state.db.delete_schedule(&id)? {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::not_found("schedule"))
    }
}

#[derive(Deserialize)]
pub(crate) struct AssignRequest {
    schedule_id: Option<String>,
}

pub(crate) async fn set_device_schedule(
    _: Admin,
    State(state): State<AppState>,
    UrlPath(id): UrlPath<String>,
    Json(req): Json<AssignRequest>,
) -> ApiResult<StatusCode> {
    state
        .db
        .device(&id)?
        .ok_or_else(|| ApiError::not_found("device"))?;
    if let Some(sid) = &req.schedule_id {
        let s = state
            .db
            .schedule(sid)?
            .ok_or_else(|| ApiError::not_found("schedule"))?;
        let needed = required_media(Some(&s), None);
        state
            .db
            .add_device_media(&id, &needed.iter().map(String::as_str).collect::<Vec<_>>())?;
    }
    let rev = state
        .db
        .set_device_schedule(&id, req.schedule_id.as_deref())?;
    state.notify(&id, rev);
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
pub(crate) struct TimelineQuery {
    /// Local date in the schedule's timezone; defaults to today there.
    date: Option<NaiveDate>,
}

#[derive(Serialize, Debug, PartialEq)]
pub struct Segment {
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
    /// Local wall-clock times, for display.
    pub start_local: String,
    pub end_local: String,
    pub desired: Desired,
}

/// What the schedule plays over one local day, as contiguous segments.
pub(crate) async fn timeline(
    _: Admin,
    State(state): State<AppState>,
    UrlPath(id): UrlPath<String>,
    Query(q): Query<TimelineQuery>,
) -> ApiResult<Json<Vec<Segment>>> {
    let s = state
        .db
        .schedule(&id)?
        .ok_or_else(|| ApiError::not_found("schedule"))?;
    let durations: HashMap<String, u64> = state
        .db
        .all_media()?
        .into_iter()
        .filter_map(|m| m.duration_ms.map(|d| (m.id, d)))
        .collect();
    let tz = s.tz().map_err(|e| ApiError::bad_request(e.to_string()))?;
    let date = q
        .date
        .unwrap_or_else(|| Utc::now().with_timezone(&tz).date_naive());
    Ok(Json(day_timeline(&s, &durations, date)))
}

/// Simulates a player over the day in 5-second steps, carrying what is
/// playing forward so one-shots play to the end, and merges equal states.
pub fn day_timeline(
    s: &Schedule,
    durations: &HashMap<String, u64>,
    date: NaiveDate,
) -> Vec<Segment> {
    let Ok(tz) = s.tz() else { return Vec::new() };
    let midnight = NaiveTime::from_hms_opt(0, 0, 0).unwrap();
    let start = resolve_local(&tz, date, midnight);
    let end = resolve_local(&tz, date.succ_opt().unwrap_or(date), midnight);
    let step = Duration::seconds(5);
    let local = |t: DateTime<Utc>| t.with_timezone(&tz).format("%H:%M:%S").to_string();

    let mut out: Vec<Segment> = Vec::new();
    let mut current = None;
    let mut t = start;
    while t < end {
        let d = evaluate(&EvalInput {
            schedule: Some(s),
            override_: None,
            durations_ms: durations,
            now: t,
            current: current.as_ref(),
        });
        current = d.key();
        let next = (t + step).min(end);
        match out.last_mut() {
            Some(last) if last.desired == d => {
                last.end = next;
                last.end_local = local(next);
            }
            _ => out.push(Segment {
                start: t,
                end: next,
                start_local: local(t),
                end_local: local(next),
                desired: d,
            }),
        }
        t = next;
    }
    if let Some(last) = out.last_mut() {
        if last.end == end {
            last.end_local = "24:00:00".into();
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timeline_for_halloween_friday() {
        let s: Schedule = serde_json::from_value(serde_json::json!({
            "schedule_id": "evening", "revision": 1, "timezone": "America/Denver",
            "rules": [
                { "id": "pumpkin", "days": ["fri"], "start": "19:00", "end": "22:00",
                  "action": { "type": "loop", "media_id": "pumpkin" } },
                { "id": "ghost", "days": ["fri"], "at": "19:15",
                  "action": { "type": "play_once", "media_id": "ghost" } }
            ]
        }))
        .unwrap();
        let durations = HashMap::from([
            ("pumpkin".to_string(), 60_000),
            ("ghost".to_string(), 30_000),
        ]);
        let segs = day_timeline(
            &s,
            &durations,
            NaiveDate::from_ymd_opt(2026, 10, 30).unwrap(),
        );
        let summary: Vec<(String, String, String)> = segs
            .iter()
            .map(|g| {
                let what = match &g.desired {
                    Desired::Idle { .. } => "idle".to_string(),
                    Desired::Play { media_id, .. } => media_id.clone(),
                };
                (g.start_local.clone(), g.end_local.clone(), what)
            })
            .collect();
        assert_eq!(
            summary,
            vec![
                ("00:00:00".into(), "19:00:00".into(), "idle".into()),
                ("19:00:00".into(), "19:15:00".into(), "pumpkin".into()),
                ("19:15:00".into(), "19:15:30".into(), "ghost".into()),
                ("19:15:30".into(), "22:00:00".into(), "pumpkin".into()),
                ("22:00:00".into(), "24:00:00".into(), "idle".into()),
            ]
        );
    }
}
