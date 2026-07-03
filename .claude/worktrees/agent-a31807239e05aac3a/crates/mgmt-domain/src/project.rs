//! Projects. Like tasks, a project's source of truth is a markdown file (one `.md` per project),
//! so the project list is portable and hand-editable. This struct is the parsed in-memory form.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Project {
    /// Stable display name; also the key tasks/events reference via their `project` field.
    pub name: String,
    /// Optional display color (a name, `#rrggbb`, or ANSI index). `None` → auto-assigned.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    /// Free-form markdown notes (the file body).
    #[serde(default)]
    pub description: String,
}

impl Project {
    pub fn new(name: impl Into<String>) -> Self {
        Project { name: name.into(), color: None, description: String::new() }
    }

    pub fn with_color(mut self, color: impl Into<String>) -> Self {
        self.color = Some(color.into());
        self
    }
}
