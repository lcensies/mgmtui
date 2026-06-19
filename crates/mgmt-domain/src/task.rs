//! Tasks. The source of truth for a task is a markdown file; this struct is the parsed
//! in-memory form. `status` is a free-form status id (see [`crate::Workflow`]) that doubles as
//! the kanban column key, so the board view is just `group_by(status)` over the task set.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use mgmt_core::Uid;

use crate::{ReminderOffset, SyncMeta};

/// The id new tasks default to when no workflow override is known. Matches the first column of
/// [`crate::Workflow::builtin`].
pub const DEFAULT_STATUS: &str = "todo";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Priority {
    None,
    Low,
    Medium,
    High,
}

impl Default for Priority {
    fn default() -> Self {
        Priority::None
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Task {
    pub uid: Uid,
    pub title: String,
    /// Free-form markdown notes (everything below the frontmatter in the source file).
    #[serde(default)]
    pub body: String,
    /// Status id — a kanban column key resolved against the configured [`crate::Workflow`].
    /// Stored verbatim so a renamed/custom status round-trips even if the workflow changes.
    #[serde(default = "default_status")]
    pub status: String,
    #[serde(default)]
    pub priority: Priority,
    /// Project this task belongs to — also the inbox "bucket" for quick-add.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    /// GTD/PARA-style area of responsibility.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub area: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub due: Option<DateTime<Utc>>,
    /// When the task should appear on the calendar (may differ from `due`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scheduled: Option<DateTime<Utc>>,
    /// Reminder offsets fired before `due` (e.g. `1d`, `2h`). Inert without a `due` date.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reminders: Vec<ReminderOffset>,
    /// Completion percentage 0..=100, if tracked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completion: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created: Option<DateTime<Utc>>,
    #[serde(default)]
    pub sync: SyncMeta,
}

impl Task {
    pub fn new(title: impl Into<String>) -> Self {
        Task {
            uid: Uid::new(),
            title: title.into(),
            body: String::new(),
            status: default_status(),
            priority: Priority::default(),
            project: None,
            area: None,
            tags: Vec::new(),
            due: None,
            scheduled: None,
            reminders: Vec::new(),
            completion: None,
            created: None,
            sync: SyncMeta::default(),
        }
    }

    /// The date this task should surface on the calendar, preferring `scheduled` then `due`.
    pub fn calendar_date(&self) -> Option<DateTime<Utc>> {
        self.scheduled.or(self.due)
    }

    pub fn with_project(mut self, project: impl Into<String>) -> Self {
        self.project = Some(project.into());
        self
    }

    pub fn with_status(mut self, status: impl Into<String>) -> Self {
        self.status = status.into();
        self
    }
}

fn default_status() -> String {
    DEFAULT_STATUS.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_task_defaults_to_todo_status() {
        assert_eq!(Task::new("x").status, "todo");
    }

    #[test]
    fn calendar_date_prefers_scheduled_over_due() {
        let mut t = Task::new("x");
        let due = Utc::now();
        let sched = due + chrono::Duration::days(1);
        t.due = Some(due);
        t.scheduled = Some(sched);
        assert_eq!(t.calendar_date(), Some(sched));
        t.scheduled = None;
        assert_eq!(t.calendar_date(), Some(due));
    }

    #[test]
    fn reminders_default_empty() {
        assert!(Task::new("x").reminders.is_empty());
    }
}
