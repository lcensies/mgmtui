//! Service layer for mgmt: the `MgmtContext` aggregate over the local stores, plus the
//! time-management (pomodoro/flowtime) engine.

mod context;
pub mod reminders;
pub mod time;

pub use context::MgmtContext;
pub use reminders::{pending as pending_reminders, HitAction, ReminderHit};
pub use time::{Flowtime, Phase, Pomodoro, Technique};
