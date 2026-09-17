//! Service layer for mgmt: the `MgmtContext` aggregate over the local stores, the
//! time-management (pomodoro/flowtime) engine, and the status surfaces (pomodoro + next event)
//! that the daemon pushes to desktop status bars.

mod context;
mod ics;
pub mod reminders;
pub mod status;
pub mod time;

pub use context::{MgmtContext, OccurrenceScope};
pub use ics::{export_ics, import_ics};
pub use reminders::{pending as pending_reminders, HitAction, ReminderHit};
pub use status::{pomodoro_path, wire_payload, PomodoroState, StatusSnapshot};
pub use time::{Engine, Flowtime, Phase, Pomodoro, Technique};
