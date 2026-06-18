//! Markdown-first task persistence: one task = one `.md` file with a YAML frontmatter block
//! followed by a free-form markdown body (the task's notes).
//!
//! ```text
//! ---
//! uid: 9f3...
//! title: Write the plan
//! status: doing
//! priority: high
//! project: wng
//! tags: [planning, urgent]
//! due: 2026-06-20T09:00:00Z
//! ---
//!
//! Body notes in **markdown** go here.
//! ```

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use mgmt_core::{Error, Result, Uid};
use mgmt_domain::{Priority, SyncMeta, Task, TaskStatus};

/// The frontmatter view of a task. Enums are represented as lowercase strings for
/// human-friendly, hand-editable files.
#[derive(Debug, Serialize, Deserialize)]
struct Frontmatter {
    uid: String,
    title: String,
    #[serde(default = "default_status")]
    status: String,
    #[serde(default = "default_priority")]
    priority: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    project: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    area: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    tags: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    due: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    scheduled: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    completion: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    created: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    href: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    etag: Option<String>,
}

fn default_status() -> String {
    "todo".into()
}
fn default_priority() -> String {
    "none".into()
}

/// Parse a markdown task document into a [`Task`].
pub fn parse_task(input: &str) -> Result<Task> {
    let (yaml, body) = split_frontmatter(input)
        .ok_or_else(|| Error::Parse("missing YAML frontmatter (expected leading ---)".into()))?;
    let fm: Frontmatter =
        serde_yaml::from_str(yaml).map_err(|e| Error::Parse(format!("bad frontmatter: {e}")))?;

    Ok(Task {
        uid: Uid::from_string(fm.uid),
        title: fm.title,
        body: body.trim_start_matches('\n').to_string(),
        status: parse_status(&fm.status),
        priority: parse_priority(&fm.priority),
        project: fm.project,
        area: fm.area,
        tags: fm.tags,
        due: fm.due,
        scheduled: fm.scheduled,
        completion: fm.completion,
        created: fm.created,
        sync: SyncMeta {
            href: fm.href,
            etag: fm.etag,
        },
    })
}

/// Serialize a [`Task`] back to a markdown document, preserving its body verbatim.
pub fn serialize_task(task: &Task) -> Result<String> {
    let fm = Frontmatter {
        uid: task.uid.to_string(),
        title: task.title.clone(),
        status: status_str(task.status).to_string(),
        priority: priority_str(task.priority).to_string(),
        project: task.project.clone(),
        area: task.area.clone(),
        tags: task.tags.clone(),
        due: task.due,
        scheduled: task.scheduled,
        completion: task.completion,
        created: task.created,
        href: task.sync.href.clone(),
        etag: task.sync.etag.clone(),
    };
    let yaml = serde_yaml::to_string(&fm).map_err(|e| Error::Other(format!("yaml: {e}")))?;
    let mut out = String::with_capacity(yaml.len() + task.body.len() + 16);
    out.push_str("---\n");
    out.push_str(&yaml);
    out.push_str("---\n");
    if !task.body.is_empty() {
        out.push('\n');
        out.push_str(&task.body);
        if !task.body.ends_with('\n') {
            out.push('\n');
        }
    }
    Ok(out)
}

/// Split a document into its frontmatter YAML and the remaining body. Returns `None` when
/// the document does not begin with a `---` fence.
fn split_frontmatter(input: &str) -> Option<(&str, &str)> {
    let rest = input.strip_prefix("---\n").or_else(|| input.strip_prefix("---\r\n"))?;
    // Find the closing fence at the start of a line.
    let mut search_from = 0;
    loop {
        let idx = rest[search_from..].find("\n---")?;
        let abs = search_from + idx + 1; // position of the '-' starting the fence
        let after = &rest[abs + 3..];
        if after.starts_with('\n') || after.starts_with("\r\n") || after.is_empty() {
            let yaml = &rest[..abs];
            let body = after.strip_prefix('\n').or_else(|| after.strip_prefix("\r\n")).unwrap_or(after);
            return Some((yaml, body));
        }
        search_from = abs + 3;
    }
}

fn status_str(s: TaskStatus) -> &'static str {
    match s {
        TaskStatus::Todo => "todo",
        TaskStatus::Doing => "doing",
        TaskStatus::Done => "done",
        TaskStatus::Cancelled => "cancelled",
        TaskStatus::Incomplete => "incomplete",
    }
}

fn parse_status(s: &str) -> TaskStatus {
    match s.to_ascii_lowercase().as_str() {
        "doing" | "in-progress" | "in_progress" => TaskStatus::Doing,
        "done" | "completed" => TaskStatus::Done,
        "cancelled" | "canceled" => TaskStatus::Cancelled,
        "incomplete" => TaskStatus::Incomplete,
        _ => TaskStatus::Todo,
    }
}

fn priority_str(p: Priority) -> &'static str {
    match p {
        Priority::None => "none",
        Priority::Low => "low",
        Priority::Medium => "medium",
        Priority::High => "high",
    }
}

fn parse_priority(s: &str) -> Priority {
    match s.to_ascii_lowercase().as_str() {
        "low" => Priority::Low,
        "medium" | "med" => Priority::Medium,
        "high" => Priority::High,
        _ => Priority::None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_a_full_task() {
        let mut t = Task::new("Write the plan");
        t.uid = Uid::from_string("task-1");
        t.body = "# Notes\n\nsome **markdown**".into();
        t.status = TaskStatus::Doing;
        t.priority = Priority::High;
        t.project = Some("wng".into());
        t.area = Some("work".into());
        t.tags = vec!["planning".into(), "urgent".into()];
        t.completion = Some(40);

        let text = serialize_task(&t).unwrap();
        let parsed = parse_task(&text).unwrap();
        assert_eq!(parsed.uid, t.uid);
        assert_eq!(parsed.title, t.title);
        assert_eq!(parsed.body.trim_end(), t.body);
        assert_eq!(parsed.status, t.status);
        assert_eq!(parsed.priority, t.priority);
        assert_eq!(parsed.project, t.project);
        assert_eq!(parsed.area, t.area);
        assert_eq!(parsed.tags, t.tags);
        assert_eq!(parsed.completion, t.completion);
    }

    #[test]
    fn parses_hand_written_frontmatter() {
        let doc = "---\nuid: abc\ntitle: Buy milk\nstatus: todo\n---\n\nremember the oat one\n";
        let t = parse_task(doc).unwrap();
        assert_eq!(t.title, "Buy milk");
        assert_eq!(t.status, TaskStatus::Todo);
        assert_eq!(t.body.trim(), "remember the oat one");
    }

    #[test]
    fn body_with_horizontal_rule_is_not_treated_as_fence_end() {
        let mut t = Task::new("x");
        t.uid = Uid::from_string("u");
        t.body = "above\n---\nbelow".into();
        let text = serialize_task(&t).unwrap();
        let parsed = parse_task(&text).unwrap();
        assert_eq!(parsed.body.trim_end(), "above\n---\nbelow");
    }

    #[test]
    fn missing_frontmatter_is_error() {
        assert!(parse_task("just text").is_err());
    }
}
