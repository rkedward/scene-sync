//! SQLite storage. Calls are short, so a single connection behind a mutex is
//! enough for a home installation.

use std::path::Path;
use std::sync::Mutex;

use anyhow::Result;
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use scenesync_core::protocol::{DeviceReport, Mapping};
use scenesync_core::schedule::{Override, Schedule};
use serde::Serialize;

pub struct Db {
    conn: Mutex<Connection>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Device {
    pub id: String,
    pub name: String,
    pub model: Option<String>,
    pub app_version: Option<String>,
    pub paired_at: DateTime<Utc>,
    pub last_seen: Option<DateTime<Utc>>,
    pub revision: u64,
    pub report: Option<DeviceReport>,
    /// The assigned schedule, resolved from `schedule_id`.
    pub schedule: Option<Schedule>,
    pub schedule_id: Option<String>,
    #[serde(rename = "override")]
    pub override_: Option<Override>,
    pub mapping: Option<Mapping>,
    pub calibrate: bool,
    pub group_id: Option<String>,
    pub trim_ms: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct Group {
    pub id: String,
    pub name: String,
    pub schedule_id: Option<String>,
    /// Members play in sync.
    pub sync: bool,
    /// The member that plays sound when in sync (default: the first).
    pub audio_device_id: Option<String>,
    /// Member device IDs.
    pub devices: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Pairing {
    pub device_id: String,
    pub code: String,
    pub name: String,
    pub model: Option<String>,
    pub app_version: Option<String>,
    pub requested_at: DateTime<Utc>,
    pub approved: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct Media {
    pub id: String,
    pub filename: String,
    pub sha256: String,
    pub size: u64,
    pub duration_ms: Option<u64>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub codec: Option<String>,
    pub created_at: DateTime<Utc>,
}

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS devices (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    model TEXT,
    app_version TEXT,
    token_hash TEXT NOT NULL UNIQUE,
    paired_at TEXT NOT NULL,
    last_seen TEXT,
    revision INTEGER NOT NULL DEFAULT 1,
    report_json TEXT,
    schedule_json TEXT,
    override_json TEXT
);
CREATE TABLE IF NOT EXISTS pairings (
    device_id TEXT PRIMARY KEY,
    code TEXT NOT NULL,
    name TEXT NOT NULL,
    model TEXT,
    app_version TEXT,
    requested_at TEXT NOT NULL,
    approved INTEGER NOT NULL DEFAULT 0,
    token TEXT
);
CREATE TABLE IF NOT EXISTS media (
    id TEXT PRIMARY KEY,
    filename TEXT NOT NULL,
    sha256 TEXT NOT NULL,
    size INTEGER NOT NULL,
    duration_ms INTEGER,
    width INTEGER,
    height INTEGER,
    codec TEXT,
    created_at TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS groups (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    schedule_id TEXT
);
CREATE TABLE IF NOT EXISTS schedules (
    id TEXT PRIMARY KEY,
    json TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS device_media (
    device_id TEXT NOT NULL REFERENCES devices(id) ON DELETE CASCADE,
    media_id TEXT NOT NULL REFERENCES media(id) ON DELETE CASCADE,
    PRIMARY KEY (device_id, media_id)
);
"#;

fn json_col<T: serde::de::DeserializeOwned>(s: Option<String>) -> Option<T> {
    s.and_then(|s| serde_json::from_str(&s).ok())
}

/// Additive migrations for databases created by earlier versions.
fn migrate(conn: &Connection) -> Result<()> {
    let has_column = |table: &str, column: &str| -> rusqlite::Result<bool> {
        let mut st = conn.prepare(&format!("PRAGMA table_info({table})"))?;
        let names = st
            .query_map([], |r| r.get::<_, String>(1))?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(names.iter().any(|n| n == column))
    };
    if !has_column("devices", "schedule_id")? {
        conn.execute("ALTER TABLE devices ADD COLUMN schedule_id TEXT", [])?;
    }
    if !has_column("devices", "group_id")? {
        conn.execute("ALTER TABLE devices ADD COLUMN group_id TEXT", [])?;
    }
    if !has_column("devices", "trim_ms")? {
        conn.execute(
            "ALTER TABLE devices ADD COLUMN trim_ms INTEGER NOT NULL DEFAULT 0",
            [],
        )?;
    }
    if !has_column("groups", "sync")? {
        conn.execute(
            "ALTER TABLE groups ADD COLUMN sync INTEGER NOT NULL DEFAULT 0",
            [],
        )?;
        conn.execute("ALTER TABLE groups ADD COLUMN audio_device TEXT", [])?;
    }
    if !has_column("devices", "mapping_json")? {
        conn.execute("ALTER TABLE devices ADD COLUMN mapping_json TEXT", [])?;
    }
    if !has_column("devices", "calibrate")? {
        conn.execute(
            "ALTER TABLE devices ADD COLUMN calibrate INTEGER NOT NULL DEFAULT 0",
            [],
        )?;
    }
    Ok(())
}

fn time_col(s: String) -> DateTime<Utc> {
    s.parse().unwrap_or_else(|_| Utc::now())
}

impl Db {
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.execute_batch(SCHEMA)?;
        migrate(&conn)?;
        Ok(Db {
            conn: Mutex::new(conn),
        })
    }

    fn conn(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.conn.lock().expect("db mutex poisoned")
    }

    // ---- pairing ---------------------------------------------------------

    /// Records or refreshes a pairing request. Returns the pairing row.
    pub fn upsert_pairing(
        &self,
        device_id: &str,
        code: &str,
        name: &str,
        model: Option<&str>,
        app_version: Option<&str>,
    ) -> Result<Pairing> {
        let c = self.conn();
        let existing: Option<(String, bool)> = c
            .query_row(
                "SELECT code, approved FROM pairings WHERE device_id = ?1",
                params![device_id],
                |r| Ok((r.get(0)?, r.get::<_, i64>(1)? != 0)),
            )
            .optional()?;
        match existing {
            // Keep an approval only if the device still shows the same code.
            Some((old_code, true)) if old_code == code => {}
            _ => {
                c.execute(
                    "INSERT INTO pairings (device_id, code, name, model, app_version, requested_at, approved, token)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, 0, NULL)
                     ON CONFLICT(device_id) DO UPDATE SET code = ?2, name = ?3, model = ?4, app_version = ?5,
                        requested_at = ?6, approved = 0, token = NULL",
                    params![device_id, code, name, model, app_version, Utc::now().to_rfc3339()],
                )?;
            }
        }
        drop(c);
        Ok(self.pairing(device_id)?.expect("pairing just written"))
    }

    pub fn pairing(&self, device_id: &str) -> Result<Option<Pairing>> {
        Ok(self
            .conn()
            .query_row(
                "SELECT device_id, code, name, model, app_version, requested_at, approved FROM pairings WHERE device_id = ?1",
                params![device_id],
                pairing_row,
            )
            .optional()?)
    }

    pub fn pending_pairings(&self) -> Result<Vec<Pairing>> {
        let c = self.conn();
        let mut st = c.prepare(
            "SELECT device_id, code, name, model, app_version, requested_at, approved FROM pairings
             WHERE approved = 0 ORDER BY requested_at DESC",
        )?;
        let rows = st
            .query_map([], pairing_row)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Approves the pending pairing with this code: creates the device and
    /// stores the token for the device's next poll. Returns the device ID.
    pub fn approve_pairing(
        &self,
        code: &str,
        name: Option<&str>,
        token: &str,
        token_hash: &str,
    ) -> Result<Option<String>> {
        let mut c = self.conn();
        let tx = c.transaction()?;
        let row: Option<(String, String, Option<String>, Option<String>)> = tx
            .query_row(
                "SELECT device_id, name, model, app_version FROM pairings WHERE code = ?1 AND approved = 0
                 ORDER BY requested_at DESC LIMIT 1",
                params![code],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .optional()?;
        let Some((device_id, req_name, model, app_version)) = row else {
            return Ok(None);
        };
        let name = name
            .filter(|n| !n.trim().is_empty())
            .map(str::to_string)
            .unwrap_or(req_name);
        tx.execute(
            "INSERT INTO devices (id, name, model, app_version, token_hash, paired_at, revision)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, 1)
             ON CONFLICT(id) DO UPDATE SET name = ?2, model = ?3, app_version = ?4, token_hash = ?5,
                revision = devices.revision + 1",
            params![
                device_id,
                name,
                model,
                app_version,
                token_hash,
                Utc::now().to_rfc3339()
            ],
        )?;
        tx.execute(
            "UPDATE pairings SET approved = 1, token = ?2 WHERE device_id = ?1",
            params![device_id, token],
        )?;
        tx.commit()?;
        Ok(Some(device_id))
    }

    /// Hands out the token once, then forgets it.
    pub fn take_pairing_token(&self, device_id: &str) -> Result<Option<String>> {
        let c = self.conn();
        let token: Option<Option<String>> = c
            .query_row(
                "SELECT token FROM pairings WHERE device_id = ?1 AND approved = 1",
                params![device_id],
                |r| r.get(0),
            )
            .optional()?;
        let token = token.flatten();
        if token.is_some() {
            c.execute(
                "DELETE FROM pairings WHERE device_id = ?1",
                params![device_id],
            )?;
        }
        Ok(token)
    }

    // ---- devices ---------------------------------------------------------

    pub fn device_by_token_hash(&self, token_hash: &str) -> Result<Option<Device>> {
        let id: Option<String> = self
            .conn()
            .query_row(
                "SELECT id FROM devices WHERE token_hash = ?1",
                params![token_hash],
                |r| r.get(0),
            )
            .optional()?;
        match id {
            Some(id) => self.device(&id),
            None => Ok(None),
        }
    }

    pub fn device(&self, id: &str) -> Result<Option<Device>> {
        Ok(self
            .conn()
            .query_row(
                "SELECT d.id, d.name, d.model, d.app_version, d.paired_at, d.last_seen, d.revision, d.report_json, s.json, d.override_json, d.schedule_id, d.mapping_json, d.calibrate, d.group_id, d.trim_ms
                 FROM devices d LEFT JOIN schedules s ON s.id = d.schedule_id WHERE d.id = ?1",
                params![id],
                device_row,
            )
            .optional()?)
    }

    pub fn devices(&self) -> Result<Vec<Device>> {
        let c = self.conn();
        let mut st = c.prepare(
            "SELECT d.id, d.name, d.model, d.app_version, d.paired_at, d.last_seen, d.revision, d.report_json, s.json, d.override_json, d.schedule_id, d.mapping_json, d.calibrate, d.group_id, d.trim_ms
             FROM devices d LEFT JOIN schedules s ON s.id = d.schedule_id ORDER BY d.name",
        )?;
        let rows = st
            .query_map([], device_row)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn delete_device(&self, id: &str) -> Result<bool> {
        Ok(self
            .conn()
            .execute("DELETE FROM devices WHERE id = ?1", params![id])?
            > 0)
    }

    pub fn rename_device(&self, id: &str, name: &str) -> Result<Option<u64>> {
        self.conn().execute(
            "UPDATE devices SET name = ?2 WHERE id = ?1",
            params![id, name],
        )?;
        self.bump_revision(id)
    }

    pub fn touch_device(&self, id: &str) -> Result<()> {
        self.conn().execute(
            "UPDATE devices SET last_seen = ?2 WHERE id = ?1",
            params![id, Utc::now().to_rfc3339()],
        )?;
        Ok(())
    }

    pub fn save_report(&self, id: &str, report: &DeviceReport) -> Result<()> {
        self.conn().execute(
            "UPDATE devices SET report_json = ?2, last_seen = ?3, app_version = COALESCE(?4, app_version) WHERE id = ?1",
            params![id, serde_json::to_string(report)?, Utc::now().to_rfc3339(), report.app_version],
        )?;
        Ok(())
    }

    /// Increments the device's config revision. Returns the new revision.
    pub fn bump_revision(&self, id: &str) -> Result<Option<u64>> {
        let c = self.conn();
        c.execute(
            "UPDATE devices SET revision = revision + 1 WHERE id = ?1",
            params![id],
        )?;
        Ok(c.query_row(
            "SELECT revision FROM devices WHERE id = ?1",
            params![id],
            |r| r.get::<_, i64>(0),
        )
        .optional()?
        .map(|v| v as u64))
    }

    pub fn set_override(&self, id: &str, ov: Option<&Override>) -> Result<Option<u64>> {
        let json = ov.map(serde_json::to_string).transpose()?;
        self.conn().execute(
            "UPDATE devices SET override_json = ?2 WHERE id = ?1",
            params![id, json],
        )?;
        self.bump_revision(id)
    }

    /// Sets or clears the fit-to-window mapping. Returns the new revision.
    pub fn set_mapping(&self, id: &str, mapping: Option<&Mapping>) -> Result<Option<u64>> {
        let json = mapping.map(serde_json::to_string).transpose()?;
        self.conn().execute(
            "UPDATE devices SET mapping_json = ?2 WHERE id = ?1",
            params![id, json],
        )?;
        self.bump_revision(id)
    }

    /// Turns the calibration pattern on or off. Returns the new revision.
    pub fn set_calibrate(&self, id: &str, on: bool) -> Result<Option<u64>> {
        self.conn().execute(
            "UPDATE devices SET calibrate = ?2 WHERE id = ?1",
            params![id, on as i64],
        )?;
        self.bump_revision(id)
    }

    /// Assigns a schedule (or none) to a device. Returns the new revision.
    pub fn set_device_schedule(&self, id: &str, schedule_id: Option<&str>) -> Result<Option<u64>> {
        self.conn().execute(
            "UPDATE devices SET schedule_id = ?2 WHERE id = ?1",
            params![id, schedule_id],
        )?;
        self.bump_revision(id)
    }

    // ---- schedules -------------------------------------------------------

    pub fn schedules(&self) -> Result<Vec<Schedule>> {
        let c = self.conn();
        let mut st = c.prepare("SELECT json FROM schedules ORDER BY id")?;
        let rows = st
            .query_map([], |r| r.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows
            .into_iter()
            .filter_map(|j| serde_json::from_str(&j).ok())
            .collect())
    }

    pub fn schedule(&self, id: &str) -> Result<Option<Schedule>> {
        let json: Option<String> = self
            .conn()
            .query_row(
                "SELECT json FROM schedules WHERE id = ?1",
                params![id],
                |r| r.get(0),
            )
            .optional()?;
        Ok(json.and_then(|j| serde_json::from_str(&j).ok()))
    }

    /// Creates or replaces a schedule, giving it the next revision number.
    /// Returns the stored schedule and the devices that use it.
    pub fn save_schedule(&self, mut schedule: Schedule) -> Result<(Schedule, Vec<String>)> {
        let previous = self
            .schedule(&schedule.schedule_id)?
            .map(|s| s.revision)
            .unwrap_or(0);
        schedule.revision = previous + 1;
        let json = serde_json::to_string(&schedule)?;
        let users = {
            let c = self.conn();
            c.execute(
                "INSERT INTO schedules (id, json, updated_at) VALUES (?1, ?2, ?3)
                 ON CONFLICT(id) DO UPDATE SET json = ?2, updated_at = ?3",
                params![schedule.schedule_id, json, Utc::now().to_rfc3339()],
            )?;
            let mut st = c.prepare("SELECT id FROM devices WHERE schedule_id = ?1")?;
            let ids = st
                .query_map(params![schedule.schedule_id], |r| r.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()?;
            ids
        };
        Ok((schedule, users))
    }

    /// Devices using a schedule.
    pub fn schedule_users(&self, id: &str) -> Result<Vec<String>> {
        let c = self.conn();
        let mut st = c.prepare("SELECT id FROM devices WHERE schedule_id = ?1")?;
        let ids = st
            .query_map(params![id], |r| r.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(ids)
    }

    pub fn delete_schedule(&self, id: &str) -> Result<bool> {
        Ok(self
            .conn()
            .execute("DELETE FROM schedules WHERE id = ?1", params![id])?
            > 0)
    }

    // ---- groups ----------------------------------------------------------

    pub fn groups(&self) -> Result<Vec<Group>> {
        let c = self.conn();
        let mut st = c.prepare(
            "SELECT id, name, schedule_id, sync, audio_device FROM groups ORDER BY name",
        )?;
        let mut groups = st
            .query_map([], |r| {
                Ok(Group {
                    id: r.get(0)?,
                    name: r.get(1)?,
                    schedule_id: r.get(2)?,
                    sync: r.get::<_, i64>(3)? != 0,
                    audio_device_id: r.get(4)?,
                    devices: Vec::new(),
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        let mut st =
            c.prepare("SELECT id, group_id FROM devices WHERE group_id IS NOT NULL ORDER BY name")?;
        let members = st
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
            .collect::<Result<Vec<_>, _>>()?;
        for (device, group) in members {
            if let Some(g) = groups.iter_mut().find(|g| g.id == group) {
                g.devices.push(device);
            }
        }
        Ok(groups)
    }

    pub fn group(&self, id: &str) -> Result<Option<Group>> {
        Ok(self.groups()?.into_iter().find(|g| g.id == id))
    }

    /// Creates or renames a group.
    pub fn save_group(&self, id: &str, name: &str) -> Result<()> {
        self.conn().execute(
            "INSERT INTO groups (id, name) VALUES (?1, ?2) ON CONFLICT(id) DO UPDATE SET name = excluded.name",
            params![id, name],
        )?;
        Ok(())
    }

    /// Deletes a group; its projectors keep their schedules.
    pub fn delete_group(&self, id: &str) -> Result<bool> {
        let c = self.conn();
        c.execute(
            "UPDATE devices SET group_id = NULL WHERE group_id = ?1",
            params![id],
        )?;
        Ok(c.execute("DELETE FROM groups WHERE id = ?1", params![id])? > 0)
    }

    pub fn set_device_group(&self, device_id: &str, group_id: Option<&str>) -> Result<()> {
        self.conn().execute(
            "UPDATE devices SET group_id = ?2 WHERE id = ?1",
            params![device_id, group_id],
        )?;
        Ok(())
    }

    pub fn set_group_schedule(&self, id: &str, schedule_id: Option<&str>) -> Result<()> {
        self.conn().execute(
            "UPDATE groups SET schedule_id = ?2 WHERE id = ?1",
            params![id, schedule_id],
        )?;
        Ok(())
    }

    pub fn set_group_sync(&self, id: &str, sync: bool, audio_device: Option<&str>) -> Result<()> {
        self.conn().execute(
            "UPDATE groups SET sync = ?2, audio_device = ?3 WHERE id = ?1",
            params![id, sync as i64, audio_device],
        )?;
        Ok(())
    }

    pub fn set_trim(&self, id: &str, trim_ms: i64) -> Result<Option<u64>> {
        self.conn().execute(
            "UPDATE devices SET trim_ms = ?2 WHERE id = ?1",
            params![id, trim_ms],
        )?;
        self.bump_revision(id)
    }

    /// Groups whose schedule is `schedule_id`.
    pub fn schedule_groups(&self, schedule_id: &str) -> Result<Vec<String>> {
        let c = self.conn();
        let mut st = c.prepare("SELECT id FROM groups WHERE schedule_id = ?1")?;
        let ids = st
            .query_map(params![schedule_id], |r| r.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(ids)
    }

    // ---- media -----------------------------------------------------------

    pub fn insert_media(&self, m: &Media) -> Result<()> {
        self.conn().execute(
            "INSERT INTO media (id, filename, sha256, size, duration_ms, width, height, codec, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                m.id,
                m.filename,
                m.sha256,
                m.size as i64,
                m.duration_ms.map(|v| v as i64),
                m.width,
                m.height,
                m.codec,
                m.created_at.to_rfc3339()
            ],
        )?;
        Ok(())
    }

    pub fn media_exists(&self, id: &str) -> Result<bool> {
        Ok(self
            .conn()
            .query_row("SELECT 1 FROM media WHERE id = ?1", params![id], |_| Ok(()))
            .optional()?
            .is_some())
    }

    pub fn media(&self, id: &str) -> Result<Option<Media>> {
        Ok(self
            .conn()
            .query_row(
                "SELECT id, filename, sha256, size, duration_ms, width, height, codec, created_at FROM media WHERE id = ?1",
                params![id],
                media_row,
            )
            .optional()?)
    }

    pub fn all_media(&self) -> Result<Vec<Media>> {
        let c = self.conn();
        let mut st = c.prepare(
            "SELECT id, filename, sha256, size, duration_ms, width, height, codec, created_at FROM media ORDER BY created_at DESC",
        )?;
        let rows = st
            .query_map([], media_row)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Deletes a clip. Returns the IDs of devices whose config changed.
    pub fn delete_media(&self, id: &str) -> Result<Option<(Media, Vec<String>)>> {
        let Some(m) = self.media(id)? else {
            return Ok(None);
        };
        let devices = {
            let c = self.conn();
            let mut st = c.prepare("SELECT device_id FROM device_media WHERE media_id = ?1")?;
            let ids = st
                .query_map(params![id], |r| r.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()?;
            drop(st);
            c.execute("DELETE FROM media WHERE id = ?1", params![id])?;
            ids
        };
        for d in &devices {
            self.bump_revision(d)?;
        }
        Ok(Some((m, devices)))
    }

    /// Clips assigned to a device.
    pub fn device_media(&self, device_id: &str) -> Result<Vec<Media>> {
        let c = self.conn();
        let mut st = c.prepare(
            "SELECT m.id, m.filename, m.sha256, m.size, m.duration_ms, m.width, m.height, m.codec, m.created_at
             FROM media m JOIN device_media dm ON dm.media_id = m.id WHERE dm.device_id = ?1 ORDER BY m.id",
        )?;
        let rows = st
            .query_map(params![device_id], media_row)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Replaces the device's assigned clips. Returns the new revision.
    pub fn set_device_media(&self, device_id: &str, media_ids: &[String]) -> Result<Option<u64>> {
        {
            let mut c = self.conn();
            let tx = c.transaction()?;
            tx.execute(
                "DELETE FROM device_media WHERE device_id = ?1",
                params![device_id],
            )?;
            for m in media_ids {
                tx.execute(
                    "INSERT OR IGNORE INTO device_media (device_id, media_id) VALUES (?1, ?2)",
                    params![device_id, m],
                )?;
            }
            tx.commit()?;
        }
        self.bump_revision(device_id)
    }

    /// Adds clips to a device's assignment without bumping the revision.
    pub fn add_device_media(&self, device_id: &str, media_ids: &[&str]) -> Result<()> {
        let c = self.conn();
        for m in media_ids {
            c.execute(
                "INSERT OR IGNORE INTO device_media (device_id, media_id) VALUES (?1, ?2)",
                params![device_id, m],
            )?;
        }
        Ok(())
    }
}

fn pairing_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Pairing> {
    Ok(Pairing {
        device_id: r.get(0)?,
        code: r.get(1)?,
        name: r.get(2)?,
        model: r.get(3)?,
        app_version: r.get(4)?,
        requested_at: time_col(r.get(5)?),
        approved: r.get::<_, i64>(6)? != 0,
    })
}

fn device_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Device> {
    Ok(Device {
        id: r.get(0)?,
        name: r.get(1)?,
        model: r.get(2)?,
        app_version: r.get(3)?,
        paired_at: time_col(r.get(4)?),
        last_seen: r.get::<_, Option<String>>(5)?.map(time_col),
        revision: r.get::<_, i64>(6)? as u64,
        report: json_col(r.get(7)?),
        schedule: json_col(r.get(8)?),
        override_: json_col(r.get(9)?),
        schedule_id: r.get(10)?,
        mapping: json_col(r.get(11)?),
        calibrate: r.get::<_, i64>(12)? != 0,
        group_id: r.get(13)?,
        trim_ms: r.get(14)?,
    })
}

fn media_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Media> {
    Ok(Media {
        id: r.get(0)?,
        filename: r.get(1)?,
        sha256: r.get(2)?,
        size: r.get::<_, i64>(3)? as u64,
        duration_ms: r.get::<_, Option<i64>>(4)?.map(|v| v as u64),
        width: r.get(5)?,
        height: r.get(6)?,
        codec: r.get(7)?,
        created_at: time_col(r.get(8)?),
    })
}
