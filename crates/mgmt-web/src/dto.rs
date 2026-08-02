//! Request-shaping helpers: turn query strings into domain `Filter`/`SortMode` values.

use std::collections::HashMap;

use chrono::{DateTime, Utc};

use mgmt_domain::{Filter, SmartView, SortMode};
use mgmt_service::MgmtContext;

/// Parse the `sort` query param, defaulting to due-date order.
pub fn sort_from_query(q: &HashMap<String, String>) -> SortMode {
    q.get("sort").and_then(|s| SortMode::from_id(s)).unwrap_or(SortMode::DueDate)
}

/// Build a task [`Filter`] from query params. A `view=<id>` selects a smart list (today/inbox/…)
/// resolved against the current day and the workflow's open statuses; `project`/`text` refine it.
/// Without a `view`, the individual `project`/`status`/`tag`/`area`/`text` params apply directly.
pub fn filter_from_query(ctx: &MgmtContext, q: &HashMap<String, String>) -> Filter {
    if let Some(view) = q.get("view").and_then(|v| SmartView::from_id(v)) {
        // The server's local day — "today" is a wall-clock notion, not a UTC one.
        let today = chrono::Local::now().date_naive();
        let mut f = view.to_filter(today, ctx.workflow().open_ids());
        if let Some(p) = q.get("project").filter(|s| !s.is_empty()) {
            f.project = Some(p.clone());
        }
        f.projects = projects_from_query(q);
        if let Some(t) = q.get("text").filter(|s| !s.is_empty()) {
            f.text = Some(t.clone());
        }
        return f;
    }
    Filter {
        project: non_empty(q, "project"),
        projects: projects_from_query(q),
        area: non_empty(q, "area"),
        tag: non_empty(q, "tag"),
        status: non_empty(q, "status"),
        text: non_empty(q, "text"),
        ..Default::default()
    }
}

/// `projects=work,home` (with the literal `none` selecting the no-project bucket) → the domain
/// filter's project set. Absent or empty means "every project", i.e. no constraint.
pub fn projects_from_query(q: &HashMap<String, String>) -> Option<Vec<String>> {
    let raw = q.get("projects").filter(|s| !s.is_empty())?;
    let list: Vec<String> = raw
        .split(',')
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(|s| if s == NO_PROJECT { String::new() } else { s.to_string() })
        .collect();
    (!list.is_empty()).then_some(list)
}

/// Reserved query value naming the "tasks without a project" bucket.
pub const NO_PROJECT: &str = "none";

/// Does this task/event project pass the selected project set?
pub fn project_selected(selected: Option<&Vec<String>>, project: Option<&str>) -> bool {
    match selected {
        None => true,
        Some(list) => list.iter().any(|x| x == project.unwrap_or("")),
    }
}

fn non_empty(q: &HashMap<String, String>, key: &str) -> Option<String> {
    q.get(key).filter(|s| !s.is_empty()).cloned()
}

/// Parse an RFC 3339 timestamp query param into UTC.
pub fn parse_rfc3339(s: Option<&String>) -> Option<DateTime<Utc>> {
    let s = s?;
    DateTime::parse_from_rfc3339(s).ok().map(|d| d.with_timezone(&Utc))
}
