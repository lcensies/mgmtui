//! Tasks. The source of truth for a task is a markdown file; this struct is the parsed
//! in-memory form. `status` doubles as the kanban column key, so the board view is just
//! `group_by(status)` over the task set.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use mgmt_core::Uid;

use crate::SyncMeta;

/// Task lifecycle state. Also the default kanban columns, in board order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TaskStatus {
    Todo,
    Doing,
    Done,
    Cancelled,
    /// Started but parked — distinct from Doing (actively worked) and Todo (untouched).
    Incomplete,
}

impl TaskStatus {
    /// Columns shown on the kanban board, left to right.
    pub const BOARD_ORDER: [TaskStatus; 5] = [
        TaskStatus::Todo,
        TaskStatus::Doing,
        TaskStatus::Incomplete,
        TaskStatus::Done,
        TaskStatus::Cancelled,
    ];

    pub fn label(self) -> &'static str {
        match self {
            TaskStatus::Todo => "Todo",
            TaskStatus::Doing => "Doing",
            TaskStatus::Done => "Done",
            TaskStatus::Cancelled => "Cancelled",
            TaskStatus::Incomplete => "Incomplete",
        }
    }

    pub fn is_open(self) -> bool {
        matches!(self, TaskStatus::Todo | TaskStatus::Doing | TaskStatus::Incomplete)
    }
}

impl Default for TaskStatus {
    fn default() -> Self {
        TaskStatus::Todo
    }
}

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
    #[serde(default)]
    pub status: TaskStatus,
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
            status: TaskStatus::default(),
            priority: Priority::default(),
            project: None,
            area: None,
            tags: Vec::new(),
            due: None,
            scheduled: None,
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn board_order_covers_every_status_once() {
        let mut seen = TaskStatus::BOARD_ORDER.to_vec();
        seen.sort_by_key(|s| s.label());
        seen.dedup();
        assert_eq!(seen.len(), 5);
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
    fn open_statuses_exclude_done_and_cancelled() {
        assert!(TaskStatus::Todo.is_open());
        assert!(!TaskStatus::Done.is_open());
        assert!(!TaskStatus::Cancelled.is_open());
    }
}
