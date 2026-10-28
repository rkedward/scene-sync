//! Schedule format and the desired-state engine.
//!
//! The engine is a pure function: given a schedule, an optional manual
//! override, clip durations, the current time, and what is currently playing,
//! it returns what should be on screen. Players call it every tick and on
//! every wake-up, so reboots, missed events, and DST changes need no special
//! handling beyond the rules below.
//!
//! Rules (decided in the v0.1 design review):
//! - A manual override beats every rule. By default it lasts until the next
//!   rule boundary (a window start or end, or a one-shot time), or until
//!   `max_duration_ms` if that comes first. `hold` keeps it until cleared.
//! - Among active rules: highest priority wins, then the most recent start,
//!   then the earlier rule in the list. Default priority: 0 for loops, 10 for
//!   one-shots.
//! - A one-shot that is more than `late_grace_ms` (default 5000) late is
//!   skipped, unless it is already playing or the rule says `join_late`.
//! - Windows whose end is not after their start run past midnight and belong
//!   to the day they start.
//! - Times are local wall-clock times in the schedule's IANA timezone. A time
//!   that does not exist (spring forward) moves to the next valid minute. A
//!   time that occurs twice (fall back) uses the first occurrence.

use std::collections::HashMap;
use std::fmt;

use chrono::{
    DateTime, Datelike, Duration, LocalResult, NaiveDate, NaiveTime, TimeZone, Utc, Weekday,
};
use chrono_tz::Tz;
use serde::{de, Deserialize, Deserializer, Serialize, Serializer};

pub const DEFAULT_LATE_GRACE_MS: u64 = 5_000;
pub const DEFAULT_LOOP_PRIORITY: i32 = 0;
pub const DEFAULT_ONE_SHOT_PRIORITY: i32 = 10;

// ---------------------------------------------------------------------------
// Format
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Schedule {
    pub schedule_id: String,
    pub revision: u64,
    /// IANA zone name, e.g. "America/Denver".
    pub timezone: String,
    #[serde(default)]
    pub rules: Vec<Rule>,
    /// Last local date on which occurrences may start. None = runs forever.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_date: Option<NaiveDate>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Rule {
    pub id: String,
    /// Days the rule starts on. Empty = every day.
    #[serde(default)]
    pub days: Vec<Day>,
    /// Window rules set `start` and `end`; one-shot rules set `at`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start: Option<LocalTime>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end: Option<LocalTime>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<LocalTime>,
    pub action: Action,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub priority: Option<i32>,
    #[serde(default)]
    pub late_policy: LatePolicy,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub late_grace_ms: Option<u64>,
    /// How a loop picks up again after something interrupted it.
    #[serde(default)]
    pub resume: ResumePolicy,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Action {
    Loop { media_id: String },
    PlayOnce { media_id: String },
}

impl Action {
    pub fn media_id(&self) -> &str {
        match self {
            Action::Loop { media_id } | Action::PlayOnce { media_id } => media_id,
        }
    }
    pub fn is_loop(&self) -> bool {
        matches!(self, Action::Loop { .. })
    }
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LatePolicy {
    #[default]
    Skip,
    JoinLate,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ResumePolicy {
    /// Start the loop from the beginning after an interruption.
    #[default]
    Restart,
    /// Jump to where the loop would have been: (now - anchor) mod duration.
    Continue,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "lowercase")]
pub enum Day {
    Mon,
    Tue,
    Wed,
    Thu,
    Fri,
    Sat,
    Sun,
}

impl Day {
    fn matches(self, wd: Weekday) -> bool {
        matches!(
            (self, wd),
            (Day::Mon, Weekday::Mon)
                | (Day::Tue, Weekday::Tue)
                | (Day::Wed, Weekday::Wed)
                | (Day::Thu, Weekday::Thu)
                | (Day::Fri, Weekday::Fri)
                | (Day::Sat, Weekday::Sat)
                | (Day::Sun, Weekday::Sun)
        )
    }
}

/// A wall-clock time written as "HH:MM" or "HH:MM:SS".
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct LocalTime(pub NaiveTime);

impl fmt::Display for LocalTime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        use chrono::Timelike;
        if self.0.second() == 0 {
            write!(f, "{}", self.0.format("%H:%M"))
        } else {
            write!(f, "{}", self.0.format("%H:%M:%S"))
        }
    }
}

impl std::str::FromStr for LocalTime {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        NaiveTime::parse_from_str(s, "%H:%M:%S")
            .or_else(|_| NaiveTime::parse_from_str(s, "%H:%M"))
            .map(LocalTime)
            .map_err(|_| format!("invalid time {s:?}, expected HH:MM or HH:MM:SS"))
    }
}

impl Serialize for LocalTime {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for LocalTime {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        s.parse().map_err(de::Error::custom)
    }
}

/// A manual command from the dashboard, delivered to players as state so it
/// survives a server outage.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Override {
    pub override_id: String,
    pub action: OverrideAction,
    pub issued_at: DateTime<Utc>,
    /// Keep until cleared, ignoring rule boundaries and max duration.
    #[serde(default)]
    pub hold: bool,
    /// Upper bound on how long the override lasts, if not held.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_duration_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum OverrideAction {
    Play {
        media_id: String,
        #[serde(default, rename = "loop")]
        looping: bool,
    },
    /// Show the idle screen (black).
    Stop,
}

#[derive(Debug)]
pub struct ScheduleError(pub String);

impl fmt::Display for ScheduleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ScheduleError {}

impl Schedule {
    pub fn tz(&self) -> Result<Tz, ScheduleError> {
        self.timezone
            .parse::<Tz>()
            .map_err(|_| ScheduleError(format!("unknown timezone {:?}", self.timezone)))
    }

    /// Checks the schedule for mistakes a person could make when writing it.
    pub fn validate(&self) -> Result<(), ScheduleError> {
        self.tz()?;
        let mut seen = std::collections::HashSet::new();
        for r in &self.rules {
            if !seen.insert(r.id.as_str()) {
                return Err(ScheduleError(format!("duplicate rule id {:?}", r.id)));
            }
            match (r.start, r.end, r.at) {
                (Some(_), Some(_), None) => {}
                (None, None, Some(_)) => {
                    if r.action.is_loop() {
                        return Err(ScheduleError(format!(
                            "rule {:?}: a loop needs start and end, not at",
                            r.id
                        )));
                    }
                }
                _ => {
                    return Err(ScheduleError(format!(
                        "rule {:?}: set either start and end, or at",
                        r.id
                    )))
                }
            }
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Engine
// ---------------------------------------------------------------------------

/// Identifies one occurrence of something playing, so the engine can tell
/// "already playing this" from "about to start this late".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlaybackKey {
    /// "rule:<id>" or "override:<id>".
    pub source: String,
    /// When this occurrence started (or should have started).
    pub anchor: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Desired {
    Idle {
        /// "override:<id>" when a Stop override caused it, otherwise absent.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        source: Option<String>,
    },
    Play {
        source: String,
        anchor: DateTime<Utc>,
        media_id: String,
        looping: bool,
        resume: ResumePolicy,
    },
}

impl Desired {
    pub fn key(&self) -> Option<PlaybackKey> {
        match self {
            Desired::Idle { .. } => None,
            Desired::Play { source, anchor, .. } => Some(PlaybackKey {
                source: source.clone(),
                anchor: *anchor,
            }),
        }
    }
}

pub struct EvalInput<'a> {
    pub schedule: Option<&'a Schedule>,
    pub override_: Option<&'a Override>,
    /// Clip durations in milliseconds, by media ID. One-shots whose duration
    /// is unknown are treated as already finished.
    pub durations_ms: &'a HashMap<String, u64>,
    pub now: DateTime<Utc>,
    /// What the player is showing right now, if anything.
    pub current: Option<&'a PlaybackKey>,
}

#[derive(Debug, Clone)]
struct Occurrence {
    rule_index: usize,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
}

/// Resolves a local wall-clock time to UTC, applying the DST rules.
pub fn resolve_local(tz: &Tz, date: NaiveDate, time: NaiveTime) -> DateTime<Utc> {
    let mut naive = date.and_time(time);
    // Gaps are at most a few hours; step forward a minute at a time.
    for _ in 0..(24 * 60) {
        match tz.from_local_datetime(&naive) {
            LocalResult::Single(t) => return t.with_timezone(&Utc),
            LocalResult::Ambiguous(first, _) => return first.with_timezone(&Utc),
            LocalResult::None => {
                // Move to the start of the next minute.
                use chrono::Timelike;
                naive = naive
                    .with_second(0)
                    .and_then(|n| n.with_nanosecond(0))
                    .unwrap_or(naive)
                    + Duration::minutes(1);
            }
        }
    }
    // Unreachable for real zones; fall back to treating the time as UTC.
    Utc.from_utc_datetime(&date.and_time(time))
}

fn rule_occurrence(
    schedule: &Schedule,
    tz: &Tz,
    index: usize,
    rule: &Rule,
    date: NaiveDate,
    durations: &HashMap<String, u64>,
) -> Option<Occurrence> {
    if !rule.days.is_empty() && !rule.days.iter().any(|d| d.matches(date.weekday())) {
        return None;
    }
    if let Some(end_date) = schedule.end_date {
        if date > end_date {
            return None;
        }
    }
    let clip_ms = durations.get(rule.action.media_id()).copied();
    match (rule.start, rule.end, rule.at) {
        (Some(s), Some(e), None) => {
            let start = resolve_local(tz, date, s.0);
            let end_date = if e.0 > s.0 { date } else { date.succ_opt()? };
            let mut end = resolve_local(tz, end_date, e.0);
            if !rule.action.is_loop() {
                // A one-shot inside a window plays once from the window start.
                let clip_end = start + Duration::milliseconds(clip_ms.unwrap_or(0) as i64);
                end = end.min(clip_end);
            }
            Some(Occurrence {
                rule_index: index,
                start,
                end,
            })
        }
        (None, None, Some(a)) => {
            let start = resolve_local(tz, date, a.0);
            let end = start + Duration::milliseconds(clip_ms.unwrap_or(0) as i64);
            Some(Occurrence {
                rule_index: index,
                start,
                end,
            })
        }
        _ => None,
    }
}

/// All occurrences whose start date lies within `days_before..=days_after`
/// of the local date of `around`.
fn occurrences_near(
    schedule: &Schedule,
    tz: &Tz,
    around: DateTime<Utc>,
    days_before: i64,
    days_after: i64,
    durations: &HashMap<String, u64>,
) -> Vec<Occurrence> {
    let local_date = around.with_timezone(tz).date_naive();
    let mut out = Vec::new();
    for offset in -days_before..=days_after {
        let Some(date) = local_date.checked_add_signed(Duration::days(offset)) else {
            continue;
        };
        for (i, rule) in schedule.rules.iter().enumerate() {
            if let Some(o) = rule_occurrence(schedule, tz, i, rule, date, durations) {
                out.push(o);
            }
        }
    }
    out
}

/// The first rule boundary (window start, window end, or one-shot time)
/// strictly after `after`, looking up to 8 days ahead.
pub fn next_boundary(
    schedule: &Schedule,
    after: DateTime<Utc>,
    durations: &HashMap<String, u64>,
) -> Option<DateTime<Utc>> {
    let tz = schedule.tz().ok()?;
    occurrences_near(schedule, &tz, after, 1, 8, durations)
        .into_iter()
        .flat_map(|o| {
            let rule = &schedule.rules[o.rule_index];
            if rule.at.is_some() {
                vec![o.start]
            } else {
                vec![o.start, o.end]
            }
        })
        .filter(|t| *t > after)
        .min()
}

/// When an override stops applying, or None if it never expires on its own.
pub fn override_expiry(
    ov: &Override,
    schedule: Option<&Schedule>,
    durations: &HashMap<String, u64>,
) -> Option<DateTime<Utc>> {
    let mut ends: Vec<DateTime<Utc>> = Vec::new();
    if let OverrideAction::Play {
        media_id,
        looping: false,
    } = &ov.action
    {
        // A one-time play hands back to the schedule when the clip ends.
        let ms = durations.get(media_id).copied().unwrap_or(0);
        ends.push(ov.issued_at + Duration::milliseconds(ms as i64));
    }
    if !ov.hold {
        if let Some(max) = ov.max_duration_ms {
            ends.push(ov.issued_at + Duration::milliseconds(max as i64));
        }
        if let Some(s) = schedule {
            if let Some(b) = next_boundary(s, ov.issued_at, durations) {
                ends.push(b);
            }
        }
    }
    ends.into_iter().min()
}

/// Returns what should be playing at `input.now`.
pub fn evaluate(input: &EvalInput<'_>) -> Desired {
    let now = input.now;

    if let Some(ov) = input.override_ {
        let expired =
            override_expiry(ov, input.schedule, input.durations_ms).is_some_and(|e| now >= e);
        if now >= ov.issued_at && !expired {
            let source = format!("override:{}", ov.override_id);
            return match &ov.action {
                OverrideAction::Stop => Desired::Idle {
                    source: Some(source),
                },
                OverrideAction::Play { media_id, looping } => Desired::Play {
                    source,
                    anchor: ov.issued_at,
                    media_id: media_id.clone(),
                    looping: *looping,
                    resume: ResumePolicy::Restart,
                },
            };
        }
    }

    let Some(schedule) = input.schedule else {
        return Desired::Idle { source: None };
    };
    let Ok(tz) = schedule.tz() else {
        return Desired::Idle { source: None };
    };

    let mut best: Option<(i32, DateTime<Utc>, usize, Occurrence)> = None;
    for o in occurrences_near(schedule, &tz, now, 1, 0, input.durations_ms) {
        if !(o.start <= now && now < o.end) {
            continue;
        }
        let rule = &schedule.rules[o.rule_index];
        let source = format!("rule:{}", rule.id);
        if !rule.action.is_loop() && rule.late_policy == LatePolicy::Skip {
            let already = input
                .current
                .is_some_and(|c| c.source == source && c.anchor == o.start);
            let grace =
                Duration::milliseconds(rule.late_grace_ms.unwrap_or(DEFAULT_LATE_GRACE_MS) as i64);
            if !already && now - o.start > grace {
                continue;
            }
        }
        let priority = rule.priority.unwrap_or(if rule.action.is_loop() {
            DEFAULT_LOOP_PRIORITY
        } else {
            DEFAULT_ONE_SHOT_PRIORITY
        });
        let better = match &best {
            None => true,
            Some((bp, bs, bi, _)) => {
                (priority, o.start, std::cmp::Reverse(o.rule_index))
                    > (*bp, *bs, std::cmp::Reverse(*bi))
            }
        };
        if better {
            best = Some((priority, o.start, o.rule_index, o));
        }
    }

    match best {
        None => Desired::Idle { source: None },
        Some((_, _, _, o)) => {
            let rule = &schedule.rules[o.rule_index];
            Desired::Play {
                source: format!("rule:{}", rule.id),
                anchor: o.start,
                media_id: rule.action.media_id().to_string(),
                looping: rule.action.is_loop(),
                resume: rule.resume,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spring_forward_gap_moves_to_next_valid_minute() {
        let tz: Tz = "America/Denver".parse().unwrap();
        // 2026-03-08 02:30 does not exist in Denver.
        let t = resolve_local(
            &tz,
            NaiveDate::from_ymd_opt(2026, 3, 8).unwrap(),
            NaiveTime::from_hms_opt(2, 30, 0).unwrap(),
        );
        assert_eq!(t.to_rfc3339(), "2026-03-08T09:00:00+00:00"); // 03:00 MDT
    }

    #[test]
    fn fall_back_uses_first_occurrence() {
        let tz: Tz = "America/Denver".parse().unwrap();
        let t = resolve_local(
            &tz,
            NaiveDate::from_ymd_opt(2026, 11, 1).unwrap(),
            NaiveTime::from_hms_opt(1, 30, 0).unwrap(),
        );
        assert_eq!(t.to_rfc3339(), "2026-11-01T07:30:00+00:00"); // 01:30 MDT
    }
}
