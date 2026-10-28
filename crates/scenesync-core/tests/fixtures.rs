//! Runs the shared engine fixtures in `fixtures/schedule/cases.json`.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use scenesync_core::schedule::{evaluate, Desired, EvalInput, Override, PlaybackKey, Schedule};
use serde::Deserialize;

#[derive(Deserialize)]
struct File {
    durations_ms: HashMap<String, u64>,
    schedules: HashMap<String, Schedule>,
    cases: Vec<Case>,
}

#[derive(Deserialize)]
struct Case {
    name: String,
    #[serde(default)]
    schedule: Option<String>,
    #[serde(default, rename = "override")]
    override_: Option<Override>,
    now: DateTime<Utc>,
    #[serde(default)]
    current: Option<PlaybackKey>,
    expect: Desired,
}

#[test]
fn shared_fixtures() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../fixtures/schedule/cases.json"
    );
    let file: File = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    for s in file.schedules.values() {
        s.validate().unwrap();
    }
    let mut failures = Vec::new();
    for case in &file.cases {
        let schedule = case.schedule.as_ref().map(|n| &file.schedules[n]);
        let got = evaluate(&EvalInput {
            schedule,
            override_: case.override_.as_ref(),
            durations_ms: &file.durations_ms,
            now: case.now,
            current: case.current.as_ref(),
        });
        if got != case.expect {
            failures.push(format!(
                "{}:\n  expected {:?}\n  got      {:?}",
                case.name, case.expect, got
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "{} failing case(s):\n{}",
        failures.len(),
        failures.join("\n")
    );
    println!("{} cases passed", file.cases.len());
}
