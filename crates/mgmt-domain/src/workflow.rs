//! The task workflow: the ordered set of statuses a task can hold. Statuses double as the
//! kanban columns, so the board is `group_by(status)` over this ordering. Unlike the rest of
//! the domain, the *set* of statuses is user-configurable (loaded from config); this module is
//! the pure value type the config builds and the views/service consult — it does no I/O.

use serde::{Deserialize, Serialize};

/// Semantic class of a status. Drives "is this open/done?" queries and the iCalendar VTODO
/// `STATUS` mapping, independent of the user's chosen id/label.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StatusKind {
    /// Not started / parked. Open work, not actively in progress.
    #[default]
    Open,
    /// Actively in progress.
    Active,
    /// Completed successfully.
    Done,
    /// Abandoned.
    Cancelled,
}

/// One status definition: a stable `id` (stored verbatim in task frontmatter and used as the
/// board-column key), a human `label`, an optional color, and a semantic `kind`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatusDef {
    pub id: String,
    /// Display label; defaults to `id` when omitted in config.
    #[serde(default)]
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    #[serde(default)]
    pub kind: StatusKind,
}

impl StatusDef {
    pub fn new(id: impl Into<String>, label: impl Into<String>, kind: StatusKind) -> Self {
        StatusDef { id: id.into(), label: label.into(), color: None, kind }
    }
}

/// An ordered, non-empty list of statuses. Construction normalizes empty labels to the id and
/// falls back to [`Workflow::builtin`] when given nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Workflow {
    defs: Vec<StatusDef>,
}

impl Default for Workflow {
    fn default() -> Self {
        Workflow::builtin()
    }
}

impl Workflow {
    /// Build from a list of definitions, filling blank labels with the id. Empty input yields
    /// the built-in workflow so the board is never column-less.
    pub fn new(mut defs: Vec<StatusDef>) -> Self {
        if defs.is_empty() {
            return Workflow::builtin();
        }
        for d in &mut defs {
            if d.label.is_empty() {
                d.label = d.id.clone();
            }
        }
        Workflow { defs }
    }

    /// The default statuses, in board order. Lean by design: `todo → doing → done → cancelled`.
    /// Add more (e.g. a parked/blocked column) via the `statuses:` list in the YAML config.
    pub fn builtin() -> Self {
        use StatusKind::*;
        Workflow {
            defs: vec![
                StatusDef::new("todo", "Todo", Open),
                StatusDef::new("doing", "Doing", Active),
                StatusDef::new("done", "Done", Done),
                StatusDef::new("cancelled", "Cancelled", Cancelled),
            ],
        }
    }

    /// Columns left-to-right.
    pub fn order(&self) -> &[StatusDef] {
        &self.defs
    }

    pub fn get(&self, id: &str) -> Option<&StatusDef> {
        self.defs.iter().find(|d| d.id == id)
    }

    /// Semantic class of `id`; unknown ids are treated as [`StatusKind::Open`] so renamed
    /// statuses still behave like open work.
    pub fn kind(&self, id: &str) -> StatusKind {
        self.get(id).map(|d| d.kind).unwrap_or(StatusKind::Open)
    }

    pub fn is_done(&self, id: &str) -> bool {
        self.kind(id) == StatusKind::Done
    }

    pub fn is_cancelled(&self, id: &str) -> bool {
        self.kind(id) == StatusKind::Cancelled
    }

    /// Open work = anything not Done or Cancelled (Open or Active).
    pub fn is_open(&self, id: &str) -> bool {
        matches!(self.kind(id), StatusKind::Open | StatusKind::Active)
    }

    /// Display label for `id`, falling back to the id itself if unknown.
    pub fn label<'a>(&'a self, id: &'a str) -> &'a str {
        self.get(id).map(|d| d.label.as_str()).unwrap_or(id)
    }

    pub fn color(&self, id: &str) -> Option<&str> {
        self.get(id).and_then(|d| d.color.as_deref())
    }

    /// The id new tasks default to (the first column).
    pub fn default_id(&self) -> &str {
        self.defs.first().map(|d| d.id.as_str()).unwrap_or("todo")
    }

    /// The first `Done`-kind status id, if any (used by toggle-done).
    pub fn first_done(&self) -> Option<&str> {
        self.defs.iter().find(|d| d.kind == StatusKind::Done).map(|d| d.id.as_str())
    }

    pub fn index_of(&self, id: &str) -> Option<usize> {
        self.defs.iter().position(|d| d.id == id)
    }

    /// The neighboring status id `dir` columns away (clamped to the ends). Drives board
    /// move-left/move-right. Returns `None` only when the workflow is somehow empty.
    pub fn neighbor(&self, id: &str, dir: i32) -> Option<&str> {
        if self.defs.is_empty() {
            return None;
        }
        let cur = self.index_of(id).unwrap_or(0) as i32;
        let idx = (cur + dir).clamp(0, self.defs.len() as i32 - 1) as usize;
        Some(self.defs[idx].id.as_str())
    }

    /// All status ids that count as open work — handy for building a [`crate::Filter`] without
    /// threading the whole workflow through pure predicates.
    pub fn open_ids(&self) -> Vec<String> {
        self.defs.iter().filter(|d| self.is_open(&d.id)).map(|d| d.id.clone()).collect()
    }

    pub fn ids(&self) -> Vec<String> {
        self.defs.iter().map(|d| d.id.clone()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_columns_in_order() {
        let w = Workflow::builtin();
        let ids: Vec<&str> = w.order().iter().map(|d| d.id.as_str()).collect();
        assert_eq!(ids, ["todo", "doing", "done", "cancelled"]);
    }

    #[test]
    fn open_excludes_done_and_cancelled() {
        let w = Workflow::builtin();
        assert!(w.is_open("todo"));
        assert!(w.is_open("doing"));
        assert!(!w.is_open("done"));
        assert!(!w.is_open("cancelled"));
    }

    #[test]
    fn unknown_status_is_open_and_labels_to_itself() {
        let w = Workflow::builtin();
        assert!(w.is_open("blocked"));
        assert_eq!(w.label("blocked"), "blocked");
    }

    #[test]
    fn neighbor_clamps_at_the_ends() {
        let w = Workflow::builtin();
        assert_eq!(w.neighbor("todo", -1), Some("todo"));
        assert_eq!(w.neighbor("todo", 1), Some("doing"));
        assert_eq!(w.neighbor("cancelled", 1), Some("cancelled"));
    }

    #[test]
    fn empty_input_falls_back_to_builtin() {
        assert_eq!(Workflow::new(vec![]), Workflow::builtin());
    }

    #[test]
    fn blank_label_defaults_to_id() {
        let w = Workflow::new(vec![StatusDef { id: "wip".into(), label: String::new(), color: None, kind: StatusKind::Active }]);
        assert_eq!(w.label("wip"), "wip");
    }
}
