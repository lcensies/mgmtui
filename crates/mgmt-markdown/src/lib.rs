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
use mgmt_domain::{Priority, Project, ReminderOffset, SyncMeta, Task};

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
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    reminders: Vec<ReminderOffset>,
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
        status: normalize_status(&fm.status),
        priority: parse_priority(&fm.priority),
        project: fm.project,
        area: fm.area,
        tags: fm.tags,
        due: fm.due,
        scheduled: fm.scheduled,
        reminders: fm.reminders,
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
        status: task.status.clone(),
        priority: priority_str(task.priority).to_string(),
        project: task.project.clone(),
        area: task.area.clone(),
        tags: task.tags.clone(),
        due: task.due,
        scheduled: task.scheduled,
        reminders: task.reminders.clone(),
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

/// Normalize a hand-written status id: lowercased and trimmed, with a couple of common
/// aliases folded to mgmt's canonical built-in ids. Unknown ids pass through verbatim so
/// custom workflow statuses round-trip untouched.
fn normalize_status(s: &str) -> String {
    let s = s.trim().to_ascii_lowercase();
    match s.as_str() {
        "" => default_status(),
        "in-progress" | "in_progress" => "doing".into(),
        "completed" => "done".into(),
        "canceled" => "cancelled".into(),
        _ => s,
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

/// Frontmatter view of a project markdown file.
#[derive(Debug, Serialize, Deserialize)]
struct ProjectFrontmatter {
    name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    color: Option<String>,
}

/// Parse a project markdown document (`name`/`color` frontmatter + body description).
pub fn parse_project(input: &str) -> Result<Project> {
    let (yaml, body) = split_frontmatter(input)
        .ok_or_else(|| Error::Parse("missing YAML frontmatter (expected leading ---)".into()))?;
    let fm: ProjectFrontmatter =
        serde_yaml::from_str(yaml).map_err(|e| Error::Parse(format!("bad project frontmatter: {e}")))?;
    Ok(Project {
        name: fm.name,
        color: fm.color,
        description: body.trim_start_matches('\n').to_string(),
    })
}

/// Serialize a [`Project`] to a markdown document, preserving its description body.
pub fn serialize_project(project: &Project) -> Result<String> {
    let fm = ProjectFrontmatter { name: project.name.clone(), color: project.color.clone() };
    let yaml = serde_yaml::to_string(&fm).map_err(|e| Error::Other(format!("yaml: {e}")))?;
    let mut out = String::with_capacity(yaml.len() + project.description.len() + 16);
    out.push_str("---\n");
    out.push_str(&yaml);
    out.push_str("---\n");
    if !project.description.is_empty() {
        out.push('\n');
        out.push_str(&project.description);
        if !project.description.ends_with('\n') {
            out.push('\n');
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_a_full_task() {
        let mut t = Task::new("Write the plan");
        t.uid = Uid::from_string("task-1");
        t.body = "# Notes\n\nsome **markdown**".into();
        t.status = "blocked".into(); // custom status round-trips verbatim
        t.priority = Priority::High;
        t.project = Some("wng".into());
        t.area = Some("work".into());
        t.tags = vec!["planning".into(), "urgent".into()];
        t.reminders = vec![mgmt_domain::ReminderOffset::new(1440), mgmt_domain::ReminderOffset::new(120)];
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
        assert_eq!(parsed.reminders, t.reminders);
        assert_eq!(parsed.completion, t.completion);
    }

    #[test]
    fn parses_hand_written_frontmatter() {
        let doc = "---\nuid: abc\ntitle: Buy milk\nstatus: todo\n---\n\nremember the oat one\n";
        let t = parse_task(doc).unwrap();
        assert_eq!(t.title, "Buy milk");
        assert_eq!(t.status, "todo");
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

    #[test]
    fn project_round_trips() {
        let mut p = Project::new("wng").with_color("blue");
        p.description = "The workmux dashboard.".into();
        let text = serialize_project(&p).unwrap();
        let parsed = parse_project(&text).unwrap();
        assert_eq!(parsed.name, "wng");
        assert_eq!(parsed.color.as_deref(), Some("blue"));
        assert_eq!(parsed.description.trim_end(), "The workmux dashboard.");
    }

    #[test]
    fn project_without_color_parses() {
        let p = parse_project("---\nname: home\n---\n").unwrap();
        assert_eq!(p.name, "home");
        assert_eq!(p.color, None);
    }
}
