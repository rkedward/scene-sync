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
    /// Fit to window: where the corners of the video land on the projector
    /// image. None = full frame, as the projector shows it normally.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mapping: Option<Mapping>,
    /// Show the calibration pattern instead of video (used while fitting).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub calibrate: bool,
    /// The player app every projector should run. A player whose own APK has
    /// a different SHA-256 downloads and installs this one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_update: Option<AppRelease>,
    /// Present when this projector plays in sync with its group.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sync: Option<SyncConfig>,
}

/// Sync group settings for one projector. Projectors in a sync group play
/// every clip against the same anchor time on the server clock, so their
/// positions line up; each one corrects drift by adjusting playback speed.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SyncConfig {
    pub group_id: String,
    /// Only one projector in a group plays sound.
    #[serde(default)]
    pub muted: bool,
    /// Shifts this projector's timeline: positive plays ahead, to make up for
    /// slower display latency.
    #[serde(default)]
    pub trim_ms: i64,
}

/// An Android player APK served by the server.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AppRelease {
    pub sha256: String,
    pub size: u64,
    /// Path on the server, relative to the server base URL.
    pub url: String,
}

/// A four-corner warp. `corners` are the top-left, top-right, bottom-right
/// and bottom-left corners of the video, as fractions of the projector
/// image (0,0 = top-left of the image, 1,1 = bottom-right). Everything
/// outside the shape is black.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Mapping {
    pub corners: [[f64; 2]; 4],
}

impl Mapping {
    /// Corners must be finite, roughly on screen, and form a convex shape in
    /// clockwise order (as seen on the wall).
    pub fn validate(&self) -> Result<(), String> {
        for [x, y] in self.corners {
            if !x.is_finite()
                || !y.is_finite()
                || !(-0.5..=1.5).contains(&x)
                || !(-0.5..=1.5).contains(&y)
            {
                return Err("corners must be between -0.5 and 1.5".into());
            }
        }
        // With y pointing down, clockwise order gives positive cross products.
        let c = self.corners;
        for i in 0..4 {
            let [ax, ay] = c[i];
            let [bx, by] = c[(i + 1) % 4];
            let [cx, cy] = c[(i + 2) % 4];
            let cross = (bx - ax) * (cy - by) - (by - ay) * (cx - bx);
            if cross <= 1e-6 {
                return Err(
                    "corners must form a convex shape in order top-left, top-right, bottom-right, bottom-left".into(),
                );
            }
        }
        Ok(())
    }
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
    /// SHA-256 of the running app package, to tell whether an update applied.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_sha256: Option<String>,
    /// Average distance from the sync group's timeline.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sync_error_ms: Option<i64>,
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

#[cfg(test)]
mod tests {
    use super::Mapping;

    #[test]
    fn mapping_validation() {
        let full = Mapping {
            corners: [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
        };
        assert!(full.validate().is_ok());
        let window = Mapping {
            corners: [[0.3, 0.2], [0.62, 0.25], [0.6, 0.8], [0.31, 0.75]],
        };
        assert!(window.validate().is_ok());
        let backwards = Mapping {
            corners: [[0.0, 0.0], [0.0, 1.0], [1.0, 1.0], [1.0, 0.0]],
        };
        assert!(backwards.validate().is_err());
        let bowtie = Mapping {
            corners: [[0.0, 0.0], [1.0, 1.0], [1.0, 0.0], [0.0, 1.0]],
        };
        assert!(bowtie.validate().is_err());
        let far = Mapping {
            corners: [[0.0, 0.0], [3.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
        };
        assert!(far.validate().is_err());
    }
}
