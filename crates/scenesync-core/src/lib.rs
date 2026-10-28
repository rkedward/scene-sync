//! Shared SceneSync types and logic.
//!
//! - [`protocol`]: messages exchanged between the server and players.
//! - [`schedule`]: the schedule format and the engine that turns a schedule,
//!   an optional manual override, and the current time into "what should be
//!   playing right now".
//!
//! The Android player re-implements the engine in Kotlin. Both
//! implementations must pass the JSON fixtures in `fixtures/schedule/`.

pub mod protocol;
pub mod schedule;

pub use schedule::{evaluate, Desired, EvalInput, PlaybackKey};
