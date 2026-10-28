//! SceneSync server library. `main.rs` wires it to a TCP listener; tests use
//! [`app`] directly.

pub mod db;
pub mod mdns;
mod media;
mod schedules;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use axum::body::Body;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{
    DefaultBodyLimit, FromRequestParts, Multipart, Path as UrlPath, Query, Request, State,
};
use axum::http::request::Parts;
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{delete, get, post, put};
use axum::{Json, Router};
use base64::Engine as _;
use chrono::{DateTime, Utc};
use futures_util::{SinkExt, StreamExt};
use rand::Rng;
use scenesync_core::protocol::{
    DeviceConfig, DeviceReport, IdleMode, Mapping, MediaRef, PairRequest, PairResponse, PairStatus,
    ServerMessage, TimeResponse, PROTOCOL_VERSION,
};
use scenesync_core::schedule::{evaluate, Desired, EvalInput, Override, OverrideAction, Schedule};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::sync::broadcast;
use tower::ServiceExt;
use tower_http::services::ServeFile;
use tower_http::trace::TraceLayer;

use crate::db::{Db, Device, Media};

const DASHBOARD_HTML: &str = include_str!("../web/index.html");

// ---------------------------------------------------------------------------
// Config and state
// ---------------------------------------------------------------------------

pub struct Config {
    pub data_dir: PathBuf,
    pub admin_password: String,
}

impl Config {
    /// Creates the data directory if needed. The admin password comes from
    /// the argument, else `data/admin_password`, else a new random one that is
    /// written there and logged once.
    pub fn load(data_dir: &Path, password: Option<String>) -> Result<Self> {
        std::fs::create_dir_all(data_dir.join("media"))
            .with_context(|| format!("creating {}", data_dir.display()))?;
        std::fs::create_dir_all(data_dir.join("tmp"))?;
        let pw_file = data_dir.join("admin_password");
        let admin_password = match password.filter(|p| !p.is_empty()) {
            Some(p) => p,
            None => match std::fs::read_to_string(&pw_file) {
                Ok(p) if !p.trim().is_empty() => p.trim().to_string(),
                _ => {
                    let p = random_token(9);
                    std::fs::write(&pw_file, &p)?;
                    tracing::warn!(
                        "generated dashboard password {p:?} (saved in {}); set SCENESYNC_ADMIN_PASSWORD to choose your own",
                        pw_file.display()
                    );
                    p
                }
            },
        };
        Ok(Config {
            data_dir: data_dir.to_path_buf(),
            admin_password,
        })
    }
}

#[derive(Clone)]
pub struct AppState(Arc<Inner>);

pub struct Inner {
    pub config: Config,
    pub db: Db,
    /// (device_id, revision) notifications for connected players.
    changes: broadcast::Sender<(String, u64)>,
    /// Number of open WebSocket connections per device.
    online: Mutex<HashMap<String, usize>>,
}

impl std::ops::Deref for AppState {
    type Target = Inner;
    fn deref(&self) -> &Inner {
        &self.0
    }
}

impl AppState {
    pub fn new(config: Config) -> Result<Self> {
        let db = Db::open(&config.data_dir.join("scenesync.sqlite"))?;
        let (changes, _) = broadcast::channel(256);
        Ok(AppState(Arc::new(Inner {
            config,
            db,
            changes,
            online: Mutex::new(HashMap::new()),
        })))
    }

    pub(crate) fn notify(&self, device_id: &str, revision: Option<u64>) {
        if let Some(rev) = revision {
            let _ = self.changes.send((device_id.to_string(), rev));
        }
    }

    fn media_path(&self, sha256: &str) -> PathBuf {
        self.config.data_dir.join("media").join(sha256)
    }

    fn is_online(&self, device_id: &str) -> bool {
        self.online
            .lock()
            .unwrap()
            .get(device_id)
            .copied()
            .unwrap_or(0)
            > 0
    }
}

pub fn app(state: AppState) -> Router {
    let device_api = Router::new()
        .route("/api/v1/device/config", get(device_config))
        .route("/api/v1/device/report", post(device_report))
        .route("/api/v1/device/ws", get(device_ws));

    let admin_api = Router::new()
        .route("/", get(dashboard))
        .route("/api/v1/admin/devices", get(admin_devices))
        .route(
            "/api/v1/admin/devices/{id}",
            delete(admin_delete_device).patch(admin_rename_device),
        )
        .route(
            "/api/v1/admin/devices/{id}/media",
            put(admin_set_device_media),
        )
        .route("/api/v1/admin/devices/{id}/command", post(admin_command))
        .route("/api/v1/admin/devices/{id}/mapping", put(admin_set_mapping))
        .route(
            "/api/v1/admin/devices/{id}/calibrate",
            put(admin_set_calibrate),
        )
        .route(
            "/api/v1/admin/devices/{id}/schedule",
            put(schedules::set_device_schedule),
        )
        .route("/api/v1/admin/schedules", get(schedules::list))
        .route(
            "/api/v1/admin/schedules/{id}",
            put(schedules::save).delete(schedules::remove),
        )
        .route(
            "/api/v1/admin/schedules/{id}/timeline",
            get(schedules::timeline),
        )
        .route("/api/v1/admin/devices/{id}/preview", get(admin_preview))
        .route("/api/v1/admin/pairings", get(admin_pairings))
        .route("/api/v1/admin/pairings/approve", post(admin_approve))
        .route("/api/v1/admin/media", get(admin_media).post(admin_upload))
        .route("/api/v1/admin/media/{id}", delete(admin_delete_media));

    Router::new()
        .route("/api/v1/time", get(time))
        .route("/api/v1/pair", post(pair))
        .route("/api/v1/media/{id}/content", get(media_content))
        .merge(device_api)
        .merge(admin_api)
        // Uploads stream to disk, so the limit only bounds a single request.
        .layer(DefaultBodyLimit::max(8 * 1024 * 1024 * 1024))
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

// ---------------------------------------------------------------------------
// Errors and auth
// ---------------------------------------------------------------------------

pub struct ApiError(StatusCode, String);

impl ApiError {
    fn new(status: StatusCode, msg: impl Into<String>) -> Self {
        ApiError(status, msg.into())
    }
    fn not_found(what: &str) -> Self {
        ApiError(StatusCode::NOT_FOUND, format!("{what} not found"))
    }
    fn bad_request(msg: impl Into<String>) -> Self {
        ApiError(StatusCode::BAD_REQUEST, msg.into())
    }
}

impl<E: Into<anyhow::Error>> From<E> for ApiError {
    fn from(e: E) -> Self {
        let e = e.into();
        tracing::error!("{e:#}");
        ApiError(StatusCode::INTERNAL_SERVER_ERROR, "internal error".into())
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let mut res = (self.0, Json(serde_json::json!({ "error": self.1 }))).into_response();
        if self.0 == StatusCode::UNAUTHORIZED {
            res.headers_mut().insert(
                header::WWW_AUTHENTICATE,
                "Basic realm=\"SceneSync\"".parse().unwrap(),
            );
        }
        res
    }
}

type ApiResult<T> = Result<T, ApiError>;

pub fn random_token(bytes: usize) -> String {
    let mut buf = vec![0u8; bytes];
    rand::rng().fill(&mut buf[..]);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(buf)
}

pub fn token_hash(token: &str) -> String {
    hex::encode(Sha256::digest(token.as_bytes()))
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

fn admin_ok(state: &AppState, headers: &HeaderMap) -> bool {
    let Some(value) = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
    else {
        return false;
    };
    let Some(b64) = value.strip_prefix("Basic ") else {
        return false;
    };
    let Ok(decoded) = base64::engine::general_purpose::STANDARD.decode(b64.trim()) else {
        return false;
    };
    let decoded = String::from_utf8_lossy(&decoded);
    let password = decoded.split_once(':').map(|(_, p)| p).unwrap_or("");
    constant_time_eq(password.as_bytes(), state.config.admin_password.as_bytes())
}

fn bearer(parts: &Parts) -> Option<String> {
    if let Some(v) = parts
        .headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
    {
        if let Some(t) = v.strip_prefix("Bearer ") {
            return Some(t.trim().to_string());
        }
    }
    // WebSocket clients that cannot set headers may pass ?token=.
    parts.uri.query().and_then(|q| {
        q.split('&')
            .find_map(|kv| kv.strip_prefix("token=").map(str::to_string))
    })
}

/// Requires the dashboard password (HTTP Basic auth, any username).
pub struct Admin;

impl FromRequestParts<AppState> for Admin {
    type Rejection = ApiError;
    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        if admin_ok(state, &parts.headers) {
            Ok(Admin)
        } else {
            Err(ApiError::new(
                StatusCode::UNAUTHORIZED,
                "dashboard password required",
            ))
        }
    }
}

/// Requires a paired device's bearer token.
pub struct AuthDevice(pub Device);

impl FromRequestParts<AppState> for AuthDevice {
    type Rejection = ApiError;
    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let token = bearer(parts)
            .ok_or_else(|| ApiError::new(StatusCode::UNAUTHORIZED, "device token required"))?;
        match state.db.device_by_token_hash(&token_hash(&token))? {
            Some(d) => Ok(AuthDevice(d)),
            None => Err(ApiError(
                StatusCode::UNAUTHORIZED,
                "unknown device token".into(),
            )),
        }
    }
}

// ---------------------------------------------------------------------------
// Public endpoints
// ---------------------------------------------------------------------------

async fn time() -> Json<TimeResponse> {
    Json(TimeResponse {
        server_time_ms: Utc::now().timestamp_millis(),
    })
}

async fn pair(
    State(state): State<AppState>,
    Json(req): Json<PairRequest>,
) -> ApiResult<Json<PairResponse>> {
    if req.protocol_version != PROTOCOL_VERSION {
        return Err(ApiError::bad_request(format!(
            "protocol version {} not supported (server speaks {PROTOCOL_VERSION})",
            req.protocol_version
        )));
    }
    if req.device_id.is_empty() || req.device_id.len() > 128 {
        return Err(ApiError::bad_request("device_id must be 1-128 characters"));
    }
    if req.code.len() != 6 || !req.code.chars().all(|c| c.is_ascii_digit()) {
        return Err(ApiError::bad_request("code must be 6 digits"));
    }
    if let Some(token) = state.db.take_pairing_token(&req.device_id)? {
        return Ok(Json(PairResponse {
            status: PairStatus::Approved,
            token: Some(token),
        }));
    }
    state.db.upsert_pairing(
        &req.device_id,
        &req.code,
        &req.name,
        req.model.as_deref(),
        req.app_version.as_deref(),
    )?;
    Ok(Json(PairResponse {
        status: PairStatus::Pending,
        token: None,
    }))
}

async fn media_content(
    State(state): State<AppState>,
    UrlPath(id): UrlPath<String>,
    req: Request,
) -> ApiResult<Response> {
    let (mut parts, body) = req.into_parts();
    let allowed = admin_ok(&state, &parts.headers)
        || AuthDevice::from_request_parts(&mut parts, &state)
            .await
            .is_ok();
    if !allowed {
        return Err(ApiError::new(
            StatusCode::UNAUTHORIZED,
            "device token or dashboard password required",
        ));
    }
    let media = state
        .db
        .media(&id)?
        .ok_or_else(|| ApiError::not_found("media"))?;
    let req = Request::from_parts(parts, body);
    // ServeFile handles Range requests, so interrupted downloads can resume.
    let res = ServeFile::new_with_mime(
        state.media_path(&media.sha256),
        &"video/mp4".parse().unwrap(),
    )
    .oneshot(req)
    .await
    .map_err(|e| anyhow::anyhow!("serving media: {e}"))?;
    Ok(res.map(Body::new))
}

// ---------------------------------------------------------------------------
// Device endpoints
// ---------------------------------------------------------------------------

fn media_ref(m: &Media) -> MediaRef {
    MediaRef {
        media_id: m.id.clone(),
        sha256: m.sha256.clone(),
        size: m.size,
        url: format!("/api/v1/media/{}/content", m.id),
        duration_ms: m.duration_ms,
    }
}

fn build_config(state: &AppState, d: &Device) -> ApiResult<DeviceConfig> {
    let media = state.db.device_media(&d.id)?;
    Ok(DeviceConfig {
        protocol_version: PROTOCOL_VERSION,
        device_id: d.id.clone(),
        name: d.name.clone(),
        revision: d.revision,
        media: media.iter().map(media_ref).collect(),
        schedule: d.schedule.clone(),
        override_: d.override_.clone(),
        idle: IdleMode::Black,
        mapping: d.mapping.clone(),
        calibrate: d.calibrate,
    })
}

async fn device_config(
    State(state): State<AppState>,
    AuthDevice(d): AuthDevice,
) -> ApiResult<Json<DeviceConfig>> {
    state.db.touch_device(&d.id)?;
    Ok(Json(build_config(&state, &d)?))
}

async fn device_report(
    State(state): State<AppState>,
    AuthDevice(d): AuthDevice,
    Json(report): Json<DeviceReport>,
) -> ApiResult<StatusCode> {
    state.db.save_report(&d.id, &report)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn device_ws(
    State(state): State<AppState>,
    AuthDevice(d): AuthDevice,
    ws: WebSocketUpgrade,
) -> Response {
    ws.on_upgrade(move |socket| device_socket(state, d, socket))
}

async fn device_socket(state: AppState, device: Device, socket: WebSocket) {
    let id = device.id.clone();
    *state.online.lock().unwrap().entry(id.clone()).or_default() += 1;
    let _ = state.db.touch_device(&id);
    tracing::info!("device {} ({}) connected", device.name, id);

    let (mut tx, mut rx) = socket.split();
    let mut changes = state.changes.subscribe();

    // Tell the player the current revision right away so it can catch up.
    let hello = ServerMessage::ConfigChanged {
        revision: device.revision,
    };
    let mut ok = tx
        .send(Message::Text(serde_json::to_string(&hello).unwrap().into()))
        .await
        .is_ok();
    let ping_every = std::time::Duration::from_secs(20);
    let mut ping = tokio::time::interval_at(tokio::time::Instant::now() + ping_every, ping_every);

    while ok {
        tokio::select! {
            msg = rx.next() => match msg {
                Some(Ok(Message::Text(text))) => {
                    // Players may send reports over the socket too.
                    if let Ok(report) = serde_json::from_str::<DeviceReport>(&text) {
                        let _ = state.db.save_report(&id, &report);
                    }
                }
                Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                Some(Ok(_)) => { let _ = state.db.touch_device(&id); }
            },
            change = changes.recv() => match change {
                Ok((dev, revision)) if dev == id => {
                    let msg = ServerMessage::ConfigChanged { revision };
                    ok = tx.send(Message::Text(serde_json::to_string(&msg).unwrap().into())).await.is_ok();
                }
                Ok(_) => {}
                Err(broadcast::error::RecvError::Lagged(_)) => {
                    // Missed some notifications; resend the latest revision.
                    if let Ok(Some(d)) = state.db.device(&id) {
                        let msg = ServerMessage::ConfigChanged { revision: d.revision };
                        ok = tx.send(Message::Text(serde_json::to_string(&msg).unwrap().into())).await.is_ok();
                    }
                }
                Err(broadcast::error::RecvError::Closed) => break,
            },
            _ = ping.tick() => {
                ok = tx.send(Message::Ping(Vec::new().into())).await.is_ok();
            }
        }
    }

    let mut online = state.online.lock().unwrap();
    if let Some(n) = online.get_mut(&id) {
        *n = n.saturating_sub(1);
    }
    tracing::info!("device {} ({}) disconnected", device.name, id);
}

// ---------------------------------------------------------------------------
// Dashboard endpoints
// ---------------------------------------------------------------------------

async fn dashboard(_: Admin) -> Html<&'static str> {
    Html(DASHBOARD_HTML)
}

#[derive(Serialize)]
struct DeviceView {
    #[serde(flatten)]
    device: Device,
    online: bool,
    in_sync: bool,
    media: Vec<String>,
    desired: Desired,
}

fn device_view(state: &AppState, d: Device) -> ApiResult<DeviceView> {
    let media = state.db.device_media(&d.id)?;
    let durations: HashMap<String, u64> = media
        .iter()
        .filter_map(|m| m.duration_ms.map(|v| (m.id.clone(), v)))
        .collect();
    let desired = evaluate(&EvalInput {
        schedule: d.schedule.as_ref(),
        override_: d.override_.as_ref(),
        durations_ms: &durations,
        now: Utc::now(),
        current: None,
    });
    Ok(DeviceView {
        online: state.is_online(&d.id),
        in_sync: d.report.as_ref().and_then(|r| r.applied_revision) == Some(d.revision),
        media: media.into_iter().map(|m| m.id).collect(),
        desired,
        device: d,
    })
}

async fn admin_devices(
    _: Admin,
    State(state): State<AppState>,
) -> ApiResult<Json<Vec<DeviceView>>> {
    let devices = state.db.devices()?;
    let views = devices
        .into_iter()
        .map(|d| device_view(&state, d))
        .collect::<ApiResult<Vec<_>>>()?;
    Ok(Json(views))
}

async fn admin_delete_device(
    _: Admin,
    State(state): State<AppState>,
    UrlPath(id): UrlPath<String>,
) -> ApiResult<StatusCode> {
    if state.db.delete_device(&id)? {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::not_found("device"))
    }
}

#[derive(Deserialize)]
struct RenameRequest {
    name: String,
}

async fn admin_rename_device(
    _: Admin,
    State(state): State<AppState>,
    UrlPath(id): UrlPath<String>,
    Json(req): Json<RenameRequest>,
) -> ApiResult<StatusCode> {
    let name = req.name.trim();
    if name.is_empty() {
        return Err(ApiError::bad_request("name must not be empty"));
    }
    let rev = state
        .db
        .rename_device(&id, name)?
        .ok_or_else(|| ApiError::not_found("device"))?;
    state.notify(&id, Some(rev));
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
struct MappingRequest {
    mapping: Option<Mapping>,
}

/// Sets (or clears, with `null`) the fit-to-window corners.
async fn admin_set_mapping(
    _: Admin,
    State(state): State<AppState>,
    UrlPath(id): UrlPath<String>,
    Json(req): Json<MappingRequest>,
) -> ApiResult<StatusCode> {
    if let Some(m) = &req.mapping {
        m.validate().map_err(ApiError::bad_request)?;
    }
    let rev = state
        .db
        .set_mapping(&id, req.mapping.as_ref())?
        .ok_or_else(|| ApiError::not_found("device"))?;
    state.notify(&id, Some(rev));
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
struct CalibrateRequest {
    on: bool,
}

/// Shows or hides the calibration pattern on the projector.
async fn admin_set_calibrate(
    _: Admin,
    State(state): State<AppState>,
    UrlPath(id): UrlPath<String>,
    Json(req): Json<CalibrateRequest>,
) -> ApiResult<StatusCode> {
    let rev = state
        .db
        .set_calibrate(&id, req.on)?
        .ok_or_else(|| ApiError::not_found("device"))?;
    state.notify(&id, Some(rev));
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
struct SetMediaRequest {
    media_ids: Vec<String>,
}

async fn admin_set_device_media(
    _: Admin,
    State(state): State<AppState>,
    UrlPath(id): UrlPath<String>,
    Json(req): Json<SetMediaRequest>,
) -> ApiResult<StatusCode> {
    let device = state
        .db
        .device(&id)?
        .ok_or_else(|| ApiError::not_found("device"))?;
    for m in &req.media_ids {
        if !state.db.media_exists(m)? {
            return Err(ApiError::bad_request(format!("unknown media {m:?}")));
        }
    }
    // Keep clips the current schedule or override still needs.
    let mut ids = req.media_ids.clone();
    ids.extend(required_media(
        device.schedule.as_ref(),
        device.override_.as_ref(),
    ));
    ids.sort();
    ids.dedup();
    let rev = state.db.set_device_media(&id, &ids)?;
    state.notify(&id, rev);
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) fn required_media(schedule: Option<&Schedule>, ov: Option<&Override>) -> Vec<String> {
    let mut out: Vec<String> = schedule
        .map(|s| {
            s.rules
                .iter()
                .map(|r| r.action.media_id().to_string())
                .collect()
        })
        .unwrap_or_default();
    if let Some(Override {
        action: OverrideAction::Play { media_id, .. },
        ..
    }) = ov
    {
        out.push(media_id.clone());
    }
    out
}

/// A manual command. `action: null` clears the override ("Return to schedule").
#[derive(Deserialize)]
struct CommandRequest {
    action: Option<OverrideAction>,
    #[serde(default)]
    hold: bool,
    #[serde(default)]
    max_duration_ms: Option<u64>,
}

#[derive(Serialize)]
struct CommandResponse {
    revision: u64,
    #[serde(rename = "override")]
    override_: Option<Override>,
}

async fn admin_command(
    _: Admin,
    State(state): State<AppState>,
    UrlPath(id): UrlPath<String>,
    Json(req): Json<CommandRequest>,
) -> ApiResult<Json<CommandResponse>> {
    state
        .db
        .device(&id)?
        .ok_or_else(|| ApiError::not_found("device"))?;
    let ov = match req.action {
        None => None,
        Some(action) => {
            if let OverrideAction::Play { media_id, .. } = &action {
                if !state.db.media_exists(media_id)? {
                    return Err(ApiError::bad_request(format!("unknown media {media_id:?}")));
                }
                state.db.add_device_media(&id, &[media_id])?;
            }
            Some(Override {
                override_id: random_token(6),
                action,
                issued_at: Utc::now(),
                hold: req.hold,
                max_duration_ms: req.max_duration_ms,
            })
        }
    };
    let rev = state
        .db
        .set_override(&id, ov.as_ref())?
        .ok_or_else(|| ApiError::not_found("device"))?;
    state.notify(&id, Some(rev));
    Ok(Json(CommandResponse {
        revision: rev,
        override_: ov,
    }))
}

#[derive(Deserialize)]
struct PreviewQuery {
    at: Option<DateTime<Utc>>,
}

async fn admin_preview(
    _: Admin,
    State(state): State<AppState>,
    UrlPath(id): UrlPath<String>,
    Query(q): Query<PreviewQuery>,
) -> ApiResult<Json<Desired>> {
    let d = state
        .db
        .device(&id)?
        .ok_or_else(|| ApiError::not_found("device"))?;
    let media = state.db.device_media(&id)?;
    let durations: HashMap<String, u64> = media
        .iter()
        .filter_map(|m| m.duration_ms.map(|v| (m.id.clone(), v)))
        .collect();
    Ok(Json(evaluate(&EvalInput {
        schedule: d.schedule.as_ref(),
        override_: d.override_.as_ref(),
        durations_ms: &durations,
        now: q.at.unwrap_or_else(Utc::now),
        current: None,
    })))
}

async fn admin_pairings(
    _: Admin,
    State(state): State<AppState>,
) -> ApiResult<Json<Vec<db::Pairing>>> {
    Ok(Json(state.db.pending_pairings()?))
}

#[derive(Deserialize)]
struct ApproveRequest {
    code: String,
    #[serde(default)]
    name: Option<String>,
}

#[derive(Serialize)]
struct ApproveResponse {
    device_id: String,
}

async fn admin_approve(
    _: Admin,
    State(state): State<AppState>,
    Json(req): Json<ApproveRequest>,
) -> ApiResult<Json<ApproveResponse>> {
    let token = random_token(32);
    let code = req.code.trim();
    match state
        .db
        .approve_pairing(code, req.name.as_deref(), &token, &token_hash(&token))?
    {
        Some(device_id) => Ok(Json(ApproveResponse { device_id })),
        None => Err(ApiError::not_found("pending pairing with that code")),
    }
}

async fn admin_media(_: Admin, State(state): State<AppState>) -> ApiResult<Json<Vec<Media>>> {
    Ok(Json(state.db.all_media()?))
}

async fn admin_upload(
    _: Admin,
    State(state): State<AppState>,
    multipart: Multipart,
) -> ApiResult<Json<Vec<Media>>> {
    media::upload(&state, multipart).await
}

async fn admin_delete_media(
    _: Admin,
    State(state): State<AppState>,
    UrlPath(id): UrlPath<String>,
) -> ApiResult<StatusCode> {
    let in_use = state.db.devices()?.into_iter().any(|d| {
        required_media(d.schedule.as_ref(), d.override_.as_ref())
            .iter()
            .any(|m| m == &id)
    });
    let in_schedule = state
        .db
        .schedules()?
        .iter()
        .any(|s| required_media(Some(s), None).iter().any(|m| m == &id));
    if in_use || in_schedule {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "clip is used by a schedule or a live command",
        ));
    }
    let (media, devices) = state
        .db
        .delete_media(&id)?
        .ok_or_else(|| ApiError::not_found("media"))?;
    for d in devices {
        let rev = state.db.device(&d)?.map(|d| d.revision);
        state.notify(&d, rev);
    }
    // Several media IDs may share one file if the same clip was uploaded twice.
    if !state
        .db
        .all_media()?
        .iter()
        .any(|m| m.sha256 == media.sha256)
    {
        let _ = tokio::fs::remove_file(state.media_path(&media.sha256)).await;
    }
    Ok(StatusCode::NO_CONTENT)
}
