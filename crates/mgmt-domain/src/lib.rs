//! Pure domain models for mgmt. No I/O, no ratatui, no networking — just data and the
//! behaviour that belongs to it (durations, shifting events, matching filters).

mod calendar;
mod event;
mod expand;
mod filter;
mod palette;
mod project;
mod recurrence;
mod reminder;
mod task;
mod workflow;

pub use calendar::{Collection, CollectionKind, RemoteSource};
pub use event::{Alarm, AlarmAction, AlarmTrigger, Event, EventStatus};
pub use filter::{Filter, SmartView, SortMode};
pub use palette::{auto_color, PALETTE};
pub use project::Project;
pub use recurrence::{Frequency, RecurrenceRule, Weekday};
pub use reminder::ReminderOffset;
pub use task::{Priority, Task, DEFAULT_STATUS};
pub use workflow::{StatusDef, StatusKind, Workflow};

use serde::{Deserialize, Serialize};

/// Sync bookkeeping shared by events and tasks. Empty for purely-local items; populated
/// once an item has been seen on a remote CalDAV collection.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncMeta {
    /// Server path of the resource on its CalDAV collection, if synced.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub href: Option<String>,
    /// Last-seen server etag, used for conflict detection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub etag: Option<String>,
}

impl SyncMeta {
    pub fn is_synced(&self) -> bool {
        self.href.is_some()
    }
}
