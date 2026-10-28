//! Wire types for the server <-> player protocol (JSON over HTTP and WebSocket).
//!
//! The WebSocket only carries hints ("config changed, revision N"). The
//! authoritative state is always fetched over HTTP from `GET /api/v1/device/config`.

use serde::{Deserialize, Serialize};

use crate::schedule::{Override, Schedule};

/// Bumped when a change would break older players or servers.
pub const PROTOCOL_VERSION: u32 = 1;

/// `POST /api/v1/pair` — sent by an unpaired player. The player shows `code`
/// on screen; the user types it into the dashboard to approve the device.
/// The player repeats the request until the response contains a token.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PairRequest {
    pub device_id: String,
    pub code: String,
    pub name: String,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub app_version: Option<String>,
    pub protocol_version: u32,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PairStatus {
    Pending,
    Approved,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PairResponse {
    pub status: PairStatus,
    /// Present once, on the first poll after approval.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
}

/// One clip a device must have locally before its config can be applied.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MediaRef {
    pub media_id: String,
    /// Lowercase hex SHA-256 of the file contents.
    pub sha256: String,
    pub size: u64,
    /// Path on the server, relative to the server base URL. Supports HTTP Range.
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
}

/// What the player shows when nothing is scheduled.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum IdleMode {
    #[default]
    Black,
}

/// `GET /api/v1/device/config` — everything a player needs to run offline.
/// A player applies a revision only once every clip in `media` is downloaded
/// and verified; until then it keeps running its previous revision.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DeviceConfig {
    pub protocol_version: u32,
    pub device_id: String,
    pub name: String,
    pub revision: u64,
    pub media: Vec<MediaRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schedule: Option<Schedule>,
    #[serde(default, rename = "override", skip_serializing_if = "Option::is_none")]
    pub override_: Option<Override>,
    #[serde(default)]
    pub idle: IdleMode,
}

/// `GET /api/v1/time` — used for NTP-style clock offset estimation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TimeResponse {
    pub server_time_ms: i64,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PlaybackState {
    #[default]
    Idle,
    Playing,
    Error,
}

/// `POST /api/v1/device/report` — the player's observed state.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct DeviceReport {
    #[serde(default)]
    pub applied_revision: Option<u64>,
    #[serde(default)]
    pub state: PlaybackState,
    #[serde(default)]
    pub media_id: Option<String>,
    #[serde(default)]
    pub position_ms: Option<u64>,
    #[serde(default)]
    pub storage_free_bytes: Option<u64>,
    #[serde(default)]
    pub storage_total_bytes: Option<u64>,
    /// Bytes used by SceneSync's own downloaded clips.
    #[serde(default)]
    pub media_bytes: Option<u64>,
    /// Estimated server_time - device_time, in milliseconds.
    #[serde(default)]
    pub clock_offset_ms: Option<i64>,
    #[serde(default)]
    pub app_version: Option<String>,
    #[serde(default)]
    pub errors: Vec<String>,
}

/// Messages the server sends to a player over `GET /api/v1/device/ws`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMessage {
    /// A new config revision exists; fetch it over HTTP.
    ConfigChanged { revision: u64 },
}
