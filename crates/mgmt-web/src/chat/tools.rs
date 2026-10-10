//! JSON-schema tool definitions + dispatch for the chat tool-calling loop. Each tool is a thin
//! wrapper over [`MgmtContext`]; no new domain logic lives here. Argument failures (bad enum,
//! unknown uid, unparseable date, a read-only calendar) become tool *results* — never a panic,
//! never a stream error — so the model sees its own mistake and can recover.

use std::path::Path;

use chrono::{DateTime, Local, NaiveDateTime, TimeZone, Utc};
use serde::Deserialize;
use serde_json::{json, Value};

use mgmt_core::Uid;
use mgmt_domain::{Event, Filter, Priority, SortMode, Task};
use mgmt_service::{MgmtContext, OccurrenceScope};

/// Result of executing one tool call. `summary` and `ok`/`undo` are what the UI shows (the
/// `tool` SSE event); `content` is the richer payload fed back to the model as the tool
/// message. Both come from one place so they can never drift apart.
pub struct ToolResult {
    pub ok: bool,
    pub summary: String,
    pub undo: bool,
    pub content: Value,
}

fn ok(summary: impl Into<String>, undo: bool) -> ToolResult {
    let summary = summary.into();
    ToolResult { ok: true, content: json!({ "ok": true, "message": summary }), summary, undo }
}

fn ok_content(content: Value, summary: impl Into<String>, undo: bool) -> ToolResult {
    ToolResult { ok: true, summary: summary.into(), undo, content }
}

fn err(summary: impl Into<String>) -> ToolResult {
    let summary = summary.into();
    ToolResult { ok: false, content: json!({ "ok": false, "error": summary }), summary, undo: false }
}

/// `delete_*` calls stop the loop and ask for approval (see `chat::run_loop`).
pub fn destructive(name: &str) -> bool {
    matches!(name, "delete_event" | "delete_task")
}

/// OpenAI-style `tools` array sent upstream with every request.
pub fn tool_defs() -> Vec<Value> {
    vec![
        func("now", "The current instant (also already in the system prompt). Call this if you need a fresh read.", json!({ "type": "object", "properties": {} })),
        func(
            "list_events",
            "List calendar events whose occurrence overlaps [from, to).",
            json!({
                "type": "object",
                "properties": {
                    "from": { "type": "string", "description": "local wall-clock start, YYYY-MM-DDTHH:MM (no Z, no offset); inclusive" },
                    "to": { "type": "string", "description": "local wall-clock end, YYYY-MM-DDTHH:MM (no Z, no offset); exclusive" },
                    "calendar": { "type": "string", "description": "restrict to one calendar name" }
                },
                "required": ["from", "to"]
            }),
        ),
        func(
            "create_event",
            "Create a calendar event.",
            json!({
                "type": "object",
                "properties": {
                    "calendar": { "type": "string", "description": "must be one of the writable calendars named in the system prompt" },
                    "summary": { "type": "string" },
                    "start": { "type": "string", "description": "local wall-clock time, YYYY-MM-DDTHH:MM (no Z, no offset)" },
                    "end": { "type": "string", "description": "local wall-clock time, YYYY-MM-DDTHH:MM (no Z, no offset); after start" },
                    "all_day": { "type": "boolean" },
                    "description": { "type": "string" },
                    "location": { "type": "string" },
                    "project": { "type": "string" }
                },
                "required": ["calendar", "summary", "start", "end"]
            }),
        ),
        func(
            "update_event",
            "Change fields of an existing event, or (with 'at') one occurrence of a recurring series.",
            json!({
                "type": "object",
                "properties": {
                    "uid": { "type": "string" },
                    "at": { "type": "string", "description": "occurrence start as local wall clock, YYYY-MM-DDTHH:MM; omit to edit the whole series" },
                    "scope": { "type": "string", "enum": ["this", "following", "all"], "description": "with 'at': which occurrences to change" },
                    "summary": { "type": "string" },
                    "start": { "type": "string" },
                    "end": { "type": "string" },
                    "description": { "type": "string" },
                    "location": { "type": "string" },
                    "project": { "type": "string" },
                    "calendar": { "type": "string" }
                },
                "required": ["uid"]
            }),
        ),
        func(
            "delete_event",
            "Delete an event, or (with 'at') one occurrence of a series. Destructive: requires the user's explicit approval before it executes.",
            json!({
                "type": "object",
                "properties": {
                    "uid": { "type": "string" },
                    "at": { "type": "string", "description": "occurrence start as local wall clock, YYYY-MM-DDTHH:MM; omit to delete the whole series" },
                    "scope": { "type": "string", "enum": ["this", "following", "all"] }
                },
                "required": ["uid"]
            }),
        ),
        func(
            "list_tasks",
            "List tasks, optionally filtered by project/status.",
            json!({
                "type": "object",
                "properties": {
                    "project": { "type": "string" },
                    "status": { "type": "string", "description": "one of the status ids named in the system prompt" },
                    "limit": { "type": "integer" }
                }
            }),
        ),
        func(
            "create_task",
            "Create a task.",
            json!({
                "type": "object",
                "properties": {
                    "title": { "type": "string" },
                    "project": { "type": "string" },
                    "due": { "type": "string", "description": "local wall-clock time, YYYY-MM-DDTHH:MM (no Z, no offset)" },
                    "scheduled": { "type": "string", "description": "local wall-clock time, YYYY-MM-DDTHH:MM (no Z, no offset)" },
                    "priority": { "type": "string", "enum": ["none", "low", "medium", "high"] }
                },
                "required": ["title"]
            }),
        ),
        func(
            "update_task",
            "Change fields of an existing task (not its status \u{2014} use set_task_status for that).",
            json!({
                "type": "object",
                "properties": {
                    "uid": { "type": "string" },
                    "title": { "type": "string" },
                    "project": { "type": "string" },
                    "due": { "type": "string" },
                    "scheduled": { "type": "string" },
                    "priority": { "type": "string", "enum": ["none", "low", "medium", "high"] }
                },
                "required": ["uid"]
            }),
        ),
        func(
            "set_task_status",
            "Move a task to a different status/kanban column.",
            json!({
                "type": "object",
                "properties": {
                    "uid": { "type": "string" },
                    "status": { "type": "string", "description": "one of the status ids named in the system prompt" }
                },
                "required": ["uid", "status"]
            }),
        ),
        func(
            "complete_task",
            "Mark a task done.",
            json!({ "type": "object", "properties": { "uid": { "type": "string" } }, "required": ["uid"] }),
        ),
        func(
            "delete_task",
            "Delete a task. Destructive: requires the user's explicit approval before it executes.",
            json!({ "type": "object", "properties": { "uid": { "type": "string" } }, "required": ["uid"] }),
        ),
        func("list_projects", "List known projects and their colors.", json!({ "type": "object", "properties": {} })),
        func(
            "create_project",
            "Register a new project.",
            json!({ "type": "object", "properties": { "name": { "type": "string" } }, "required": ["name"] }),
        ),
        func(
            "list_calendars",
            "List calendar names, flagging which are read-only subscriptions (writes to those fail).",
            json!({ "type": "object", "properties": {} }),
        ),
        func(
            "get_settings",
            "Read the configured task workflow (status ids/labels).",
            json!({ "type": "object", "properties": {} }),
        ),
    ]
}

fn func(name: &str, description: &str, parameters: Value) -> Value {
    json!({ "type": "function", "function": { "name": name, "description": description, "parameters": parameters } })
}

/// Execute one tool call against `ctx`. `root` is the vault root (needed only for the calendar
/// listing, which `MgmtContext` doesn't cache).
pub fn dispatch(ctx: &mut MgmtContext, root: &Path, name: &str, args: &Value) -> ToolResult {
    match name {
        "now" => now(),
        "list_events" => list_events(ctx, args),
        "create_event" => create_event(ctx, args),
        "update_event" => update_event(ctx, args),
        "delete_event" => delete_event(ctx, args),
        "list_tasks" => list_tasks(ctx, args),
        "create_task" => create_task(ctx, args),
        "update_task" => update_task(ctx, args),
        "set_task_status" => set_task_status(ctx, args),
        "complete_task" => complete_task(ctx, args),
        "delete_task" => delete_task(ctx, args),
        "list_projects" => list_projects(ctx),
        "create_project" => create_project(ctx, args),
        "list_calendars" => list_calendars(root),
        "get_settings" => get_settings(ctx),
        other => err(format!("unknown tool '{other}'")),
    }
}

/// A human description of what a destructive call would do, without executing it (the
/// `confirm` prompt). `Err` means there is nothing to confirm (e.g. the target is already
/// gone) — the caller then runs it through [`dispatch`] as usual, which reports that as a
/// normal tool error.
pub fn describe_destructive(ctx: &MgmtContext, name: &str, args: &Value) -> Result<String, String> {
    match name {
        "delete_event" => {
            let a: DeleteEventArgs = serde_json::from_value(args.clone()).map_err(|e| e.to_string())?;
            let uid = Uid::from(a.uid.as_str());
            let ev = ctx.event(&uid).ok_or("event not found")?;
            let at = a.at.as_deref().and_then(parse_dt).unwrap_or(ev.start);
            Ok(format!("delete '{}' ({})", ev.summary, fmt_local(at)))
        }
        "delete_task" => {
            let a: UidArgs = serde_json::from_value(args.clone()).map_err(|e| e.to_string())?;
            let t = ctx.task(&Uid::from(a.uid.as_str())).ok_or("task not found")?;
            Ok(format!("delete task '{}'", t.title))
        }
        _ => Err("not a destructive call".into()),
    }
}

/// Calendar names (vdir collections on disk) alongside whether each is a read-only subscription.
fn all_calendars(root: &Path) -> Vec<(String, bool)> {
    let store = mgmt_store::VdirStore::new(mgmt_store::calendars_dir(root));
    let all = store.collections().unwrap_or_default();
    let ro: std::collections::HashSet<String> = mgmt_store::load_calendars(root)
        .unwrap_or_default()
        .into_iter()
        .filter(|c| c.is_read_only())
        .map(|c| c.id)
        .collect();
    all.into_iter().map(|id| { let read_only = ro.contains(&id); (id, read_only) }).collect()
}

/// Writable calendar names only (for the system prompt's vocabulary).
pub fn writable_calendars(root: &Path) -> Vec<String> {
    all_calendars(root).into_iter().filter(|(_, ro)| !ro).map(|(id, _)| id).collect()
}

/// Accepts an RFC3339 instant (offset honoured) or a naive `YYYY-MM-DDTHH:MM[:SS]` /
/// `YYYY-MM-DD HH:MM[:SS]` wall clock, which is read in the host's local zone — the convention
/// every other mgmt surface uses (see `mgmt-cli::datetime::local_to_utc`). Models emit naive or
/// `Z`-suffixed local wall clock far more often than a correct offset, so a naive parse that
/// silently meant UTC shifted every event by the host offset.
fn parse_dt(s: &str) -> Option<DateTime<Utc>> {
    if let Ok(d) = DateTime::parse_from_rfc3339(s) {
        return Some(d.with_timezone(&Utc));
    }
    let naive = ["%Y-%m-%dT%H:%M:%S", "%Y-%m-%dT%H:%M", "%Y-%m-%d %H:%M:%S", "%Y-%m-%d %H:%M"]
        .iter()
        .find_map(|f| NaiveDateTime::parse_from_str(s, f).ok())?;
    // DST gap/fold: take the earliest candidate, as local_to_utc does.
    Some(Local.from_local_datetime(&naive).earliest().map_or_else(|| naive.and_utc(), |d| d.with_timezone(&Utc)))
}

fn fmt_local(dt: DateTime<Utc>) -> String {
    dt.with_timezone(&Local).format("%Y-%m-%d %H:%M").to_string()
}

fn parse_priority(s: &str) -> Option<Priority> {
    match s.to_lowercase().as_str() {
        "none" => Some(Priority::None),
        "low" => Some(Priority::Low),
        "medium" | "med" => Some(Priority::Medium),
        "high" => Some(Priority::High),
        _ => None,
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

fn parse_scope(s: Option<&str>) -> Result<OccurrenceScope, String> {
    s.unwrap_or("all").parse().map_err(|_| "'scope' must be this, following, or all".to_string())
}

// ---- argument shapes ---------------------------------------------------------------

#[derive(Deserialize)]
struct UidArgs {
    uid: String,
}

#[derive(Deserialize)]
struct RangeArgs {
    from: String,
    to: String,
    #[serde(default)]
    calendar: Option<String>,
}

#[derive(Deserialize)]
struct NewEventArgs {
    calendar: String,
    summary: String,
    start: String,
    end: String,
    #[serde(default)]
    all_day: bool,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    location: Option<String>,
    #[serde(default)]
    project: Option<String>,
}

#[derive(Deserialize)]
struct UpdateEventArgs {
    uid: String,
    #[serde(default)]
    at: Option<String>,
    #[serde(default)]
    scope: Option<String>,
    #[serde(default)]
    summary: Option<String>,
    #[serde(default)]
    start: Option<String>,
    #[serde(default)]
    end: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    location: Option<String>,
    #[serde(default)]
    project: Option<String>,
    #[serde(default)]
    calendar: Option<String>,
}

#[derive(Deserialize)]
struct DeleteEventArgs {
    uid: String,
    #[serde(default)]
    at: Option<String>,
    #[serde(default)]
    scope: Option<String>,
}

#[derive(Deserialize)]
struct ListTasksArgs {
    #[serde(default)]
    project: Option<String>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    limit: Option<usize>,
}

#[derive(Deserialize)]
struct NewTaskArgs {
    title: String,
    #[serde(default)]
    project: Option<String>,
    #[serde(default)]
    due: Option<String>,
    #[serde(default)]
    scheduled: Option<String>,
    #[serde(default)]
    priority: Option<String>,
}

#[derive(Deserialize)]
struct UpdateTaskArgs {
    uid: String,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    project: Option<String>,
    #[serde(default)]
    due: Option<String>,
    #[serde(default)]
    scheduled: Option<String>,
    #[serde(default)]
    priority: Option<String>,
}

#[derive(Deserialize)]
struct SetStatusArgs {
    uid: String,
    status: String,
}

#[derive(Deserialize)]
struct NewProjectArgs {
    name: String,
}

// ---- reads ---------------------------------------------------------------------------

fn now() -> ToolResult {
    let now = Utc::now();
    let local = now.with_timezone(&Local);
    ok_content(
        json!({ "now": now.to_rfc3339(), "local": local.format("%Y-%m-%d %H:%M").to_string(), "utc_offset": local.format("%:z").to_string() }),
        format!("now is {} ({})", now.to_rfc3339(), local.format("%Y-%m-%d %H:%M")),
        false,
    )
}

fn event_summary_json(e: &Event) -> Value {
    json!({
        "uid": e.uid.as_str(),
        "calendar": e.calendar,
        "summary": e.summary,
        "start": e.start,
        "end": e.end,
        "all_day": e.all_day,
        "project": e.project,
        "occurrence_start": e.recurrence_id.unwrap_or(e.start),
    })
}

fn list_events(ctx: &MgmtContext, args: &Value) -> ToolResult {
    let a: RangeArgs = match serde_json::from_value(args.clone()) {
        Ok(a) => a,
        Err(e) => return err(format!("invalid arguments: {e}")),
    };
    let Some(from) = parse_dt(&a.from) else { return err("'from' is not a date-time (use YYYY-MM-DDTHH:MM local wall clock)") };
    let Some(to) = parse_dt(&a.to) else { return err("'to' is not a date-time (use YYYY-MM-DDTHH:MM local wall clock)") };
    let events: Vec<Value> = ctx
        .events_in_range(from, to)
        .iter()
        .filter(|e| a.calendar.as_deref().map(|c| e.calendar == c).unwrap_or(true))
        .map(event_summary_json)
        .collect();
    let n = events.len();
    ok_content(json!(events), format!("found {n} event(s) between {} and {}", fmt_local(from), fmt_local(to)), false)
}

fn task_summary_json(t: &Task) -> Value {
    json!({
        "uid": t.uid.as_str(),
        "title": t.title,
        "status": t.status,
        "project": t.project,
        "priority": priority_str(t.priority),
        "due": t.due,
        "scheduled": t.scheduled,
    })
}

fn list_tasks(ctx: &MgmtContext, args: &Value) -> ToolResult {
    let a: ListTasksArgs = match serde_json::from_value(args.clone()) {
        Ok(a) => a,
        Err(e) => return err(format!("invalid arguments: {e}")),
    };
    let filter = Filter { project: a.project, status: a.status, ..Default::default() };
    let mut tasks = ctx.filtered_tasks(&filter, SortMode::DueDate);
    let limit = a.limit.unwrap_or(50).min(200);
    tasks.truncate(limit);
    let n = tasks.len();
    let data: Vec<Value> = tasks.iter().map(task_summary_json).collect();
    ok_content(json!(data), format!("{n} task(s)"), false)
}

fn list_projects(ctx: &MgmtContext) -> ToolResult {
    let projects = ctx.projects();
    let n = projects.len();
    let data: Vec<Value> = projects.iter().map(|p| json!({ "name": p, "color": ctx.project_color(p) })).collect();
    ok_content(json!(data), format!("{n} project(s)"), false)
}

fn list_calendars(root: &Path) -> ToolResult {
    let cals = all_calendars(root);
    let n = cals.len();
    let data: Vec<Value> = cals.iter().map(|(id, ro)| json!({ "id": id, "read_only": ro })).collect();
    ok_content(json!(data), format!("{n} calendar(s)"), false)
}

fn get_settings(ctx: &MgmtContext) -> ToolResult {
    let wf = ctx.workflow();
    let statuses: Vec<Value> = wf.order().iter().map(|d| json!({ "id": d.id, "label": d.label })).collect();
    let data = json!({ "statuses": statuses, "default_status": wf.default_id() });
    ok_content(data, "read workflow settings", false)
}

// ---- event mutations -------------------------------------------------------------------

fn create_event(ctx: &mut MgmtContext, args: &Value) -> ToolResult {
    let a: NewEventArgs = match serde_json::from_value(args.clone()) {
        Ok(a) => a,
        Err(e) => return err(format!("invalid arguments: {e}")),
    };
    let Some(start) = parse_dt(&a.start) else { return err("'start' is not a date-time (use YYYY-MM-DDTHH:MM local wall clock)") };
    let Some(end) = parse_dt(&a.end) else { return err("'end' is not a date-time (use YYYY-MM-DDTHH:MM local wall clock)") };
    if end <= start {
        return err("'end' must be after 'start'");
    }
    let mut ev = Event::new(a.calendar, a.summary, start, end);
    ev.all_day = a.all_day;
    ev.description = a.description;
    ev.location = a.location;
    ev.project = a.project;
    match ctx.put_event(ev.clone()) {
        Ok(()) => ok_content(
            json!({ "uid": ev.uid.as_str() }),
            format!("created '{}' on {} ({} \u{2013} {})", ev.summary, ev.calendar, fmt_local(start), fmt_local(end)),
            true,
        ),
        Err(e) => err(e.to_string()),
    }
}

fn update_event(ctx: &mut MgmtContext, args: &Value) -> ToolResult {
    let a: UpdateEventArgs = match serde_json::from_value(args.clone()) {
        Ok(a) => a,
        Err(e) => return err(format!("invalid arguments: {e}")),
    };
    let uid = Uid::from(a.uid.as_str());
    let Some(mut ev) = ctx.event(&uid).cloned() else { return err(format!("no event '{}'", a.uid)) };
    if let Some(s) = a.summary {
        ev.summary = s;
    }
    if let Some(s) = &a.start {
        match parse_dt(s) {
            Some(d) => ev.start = d,
            None => return err("'start' is not a date-time (use YYYY-MM-DDTHH:MM local wall clock)"),
        }
    }
    if let Some(s) = &a.end {
        match parse_dt(s) {
            Some(d) => ev.end = d,
            None => return err("'end' is not a date-time (use YYYY-MM-DDTHH:MM local wall clock)"),
        }
    }
    if let Some(d) = a.description {
        ev.description = Some(d);
    }
    if let Some(l) = a.location {
        ev.location = Some(l);
    }
    if let Some(p) = a.project {
        ev.project = Some(p);
    }
    if let Some(c) = a.calendar {
        ev.calendar = c;
    }
    let target = match &a.at {
        Some(at) => {
            let Some(at) = parse_dt(at) else { return err("'at' is not a date-time (use YYYY-MM-DDTHH:MM local wall clock)") };
            match parse_scope(a.scope.as_deref()) {
                Ok(scope) => Some((at, scope)),
                Err(e) => return err(e),
            }
        }
        None => None,
    };
    let result = match target {
        Some((at, scope)) => ctx.update_occurrence(&uid, at, ev.clone(), scope),
        None => ctx.put_event(ev.clone()),
    };
    match result {
        Ok(()) => ok_content(json!({ "uid": ev.uid.as_str() }), format!("updated '{}'", ev.summary), true),
        Err(e) => err(e.to_string()),
    }
}

fn delete_event(ctx: &mut MgmtContext, args: &Value) -> ToolResult {
    let a: DeleteEventArgs = match serde_json::from_value(args.clone()) {
        Ok(a) => a,
        Err(e) => return err(format!("invalid arguments: {e}")),
    };
    let uid = Uid::from(a.uid.as_str());
    let Some(ev) = ctx.event(&uid).cloned() else { return err(format!("no event '{}'", a.uid)) };
    let result = match &a.at {
        Some(at) => {
            let Some(at) = parse_dt(at) else { return err("'at' is not a date-time (use YYYY-MM-DDTHH:MM local wall clock)") };
            match parse_scope(a.scope.as_deref()) {
                Ok(scope) => ctx.delete_occurrence(&uid, at, scope),
                Err(e) => return err(e),
            }
        }
        None => ctx.delete_event(&uid),
    };
    match result {
        Ok(()) => ok(format!("deleted '{}' ({})", ev.summary, fmt_local(ev.start)), true),
        Err(e) => err(e.to_string()),
    }
}

// ---- task mutations ---------------------------------------------------------------------

fn create_task(ctx: &mut MgmtContext, args: &Value) -> ToolResult {
    let a: NewTaskArgs = match serde_json::from_value(args.clone()) {
        Ok(a) => a,
        Err(e) => return err(format!("invalid arguments: {e}")),
    };
    if a.title.trim().is_empty() {
        return err("title is empty");
    }
    let due = match &a.due {
        Some(s) => match parse_dt(s) {
            Some(d) => Some(d),
            None => return err("'due' is not a date-time (use YYYY-MM-DDTHH:MM local wall clock)"),
        },
        None => None,
    };
    let scheduled = match &a.scheduled {
        Some(s) => match parse_dt(s) {
            Some(d) => Some(d),
            None => return err("'scheduled' is not a date-time (use YYYY-MM-DDTHH:MM local wall clock)"),
        },
        None => None,
    };
    let priority = match &a.priority {
        Some(s) => match parse_priority(s) {
            Some(p) => Some(p),
            None => return err(format!("unknown priority '{s}' (use none, low, medium, high)")),
        },
        None => None,
    };
    let mut t = Task::new(a.title.clone());
    t.status = ctx.workflow().default_id().to_string();
    t.project = a.project;
    t.created = Some(Utc::now());
    t.due = due;
    t.scheduled = scheduled;
    if let Some(p) = priority {
        t.priority = p;
    }
    let uid = t.uid.clone();
    match ctx.put_task(t) {
        Ok(()) => ok_content(json!({ "uid": uid.as_str() }), format!("created task '{}'", a.title), true),
        Err(e) => err(e.to_string()),
    }
}

fn update_task(ctx: &mut MgmtContext, args: &Value) -> ToolResult {
    let a: UpdateTaskArgs = match serde_json::from_value(args.clone()) {
        Ok(a) => a,
        Err(e) => return err(format!("invalid arguments: {e}")),
    };
    let uid = Uid::from(a.uid.as_str());
    let Some(mut t) = ctx.task(&uid).cloned() else { return err(format!("no task '{}'", a.uid)) };
    if let Some(title) = a.title {
        t.title = title;
    }
    if let Some(project) = a.project {
        t.project = if project.is_empty() { None } else { Some(project) };
    }
    if let Some(due) = &a.due {
        match parse_dt(due) {
            Some(d) => t.due = Some(d),
            None => return err("'due' is not a date-time (use YYYY-MM-DDTHH:MM local wall clock)"),
        }
    }
    if let Some(sch) = &a.scheduled {
        match parse_dt(sch) {
            Some(d) => t.scheduled = Some(d),
            None => return err("'scheduled' is not a date-time (use YYYY-MM-DDTHH:MM local wall clock)"),
        }
    }
    if let Some(p) = &a.priority {
        match parse_priority(p) {
            Some(pr) => t.priority = pr,
            None => return err(format!("unknown priority '{p}' (use none, low, medium, high)")),
        }
    }
    match ctx.put_task(t.clone()) {
        Ok(()) => ok_content(json!({ "uid": t.uid.as_str() }), format!("updated task '{}'", t.title), true),
        Err(e) => err(e.to_string()),
    }
}

fn set_task_status(ctx: &mut MgmtContext, args: &Value) -> ToolResult {
    let a: SetStatusArgs = match serde_json::from_value(args.clone()) {
        Ok(a) => a,
        Err(e) => return err(format!("invalid arguments: {e}")),
    };
    let uid = Uid::from(a.uid.as_str());
    let Some(t) = ctx.task(&uid) else { return err(format!("no task '{}'", a.uid)) };
    let valid = ctx.workflow().ids();
    if !valid.iter().any(|v| v == &a.status) {
        return err(format!("unknown status '{}'; valid: {}", a.status, valid.join(", ")));
    }
    let title = t.title.clone();
    match ctx.set_task_status(&uid, a.status.clone()) {
        Ok(()) => ok(format!("moved '{title}' to {}", a.status), true),
        Err(e) => err(e.to_string()),
    }
}

fn complete_task(ctx: &mut MgmtContext, args: &Value) -> ToolResult {
    let a: UidArgs = match serde_json::from_value(args.clone()) {
        Ok(a) => a,
        Err(e) => return err(format!("invalid arguments: {e}")),
    };
    let uid = Uid::from(a.uid.as_str());
    let Some(t) = ctx.task(&uid) else { return err(format!("no task '{}'", a.uid)) };
    let title = t.title.clone();
    let done = ctx.workflow().first_done().unwrap_or("done").to_string();
    match ctx.set_task_status(&uid, done) {
        Ok(()) => ok(format!("completed '{title}'"), true),
        Err(e) => err(e.to_string()),
    }
}

fn delete_task(ctx: &mut MgmtContext, args: &Value) -> ToolResult {
    let a: UidArgs = match serde_json::from_value(args.clone()) {
        Ok(a) => a,
        Err(e) => return err(format!("invalid arguments: {e}")),
    };
    let uid = Uid::from(a.uid.as_str());
    let Some(t) = ctx.task(&uid).cloned() else { return err(format!("no task '{}'", a.uid)) };
    match ctx.delete_task(&uid) {
        Ok(()) => ok(format!("deleted task '{}'", t.title), true),
        Err(e) => err(e.to_string()),
    }
}

// ---- project mutations ------------------------------------------------------------------

fn create_project(ctx: &mut MgmtContext, args: &Value) -> ToolResult {
    let a: NewProjectArgs = match serde_json::from_value(args.clone()) {
        Ok(a) => a,
        Err(e) => return err(format!("invalid arguments: {e}")),
    };
    if a.name.trim().is_empty() {
        return err("name is empty");
    }
    match ctx.add_project(a.name.clone()) {
        Ok(()) => ok_content(json!({ "name": a.name }), format!("created project '{}'", a.name), false),
        Err(e) => err(e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mgmt_store::{VaultStore, VdirStore};

    #[test]
    fn naive_datetimes_are_host_local_offsets_are_honoured() {
        // The bug this pins: a naive wall clock parsed as UTC shifted every written event by the
        // host offset ("9am" landed at 09:00Z = 12:00 local on a UTC+3 host).
        let naive = parse_dt("2026-10-11T09:00").unwrap();
        assert_eq!(naive.with_timezone(&Local).format("%H:%M").to_string(), "09:00");
        assert_eq!(parse_dt("2026-10-11T09:00:00").unwrap(), naive);
        assert_eq!(parse_dt("2026-10-11 09:00").unwrap(), naive);
        // An explicit offset still means that instant, not local wall clock.
        assert_eq!(parse_dt("2026-10-11T09:00:00Z").unwrap().format("%H:%M").to_string(), "09:00");
        assert!(parse_dt("tomorrow at nine").is_none());
    }

    fn ctx() -> (MgmtContext, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        let vault = VaultStore::new(mgmt_store::tasks_dir(&root));
        let vdir = VdirStore::new(mgmt_store::calendars_dir(&root));
        (MgmtContext::open(vault, vdir).unwrap(), dir)
    }

    #[test]
    fn create_then_list_task_round_trips() {
        let (mut c, _d) = ctx();
        let r = create_task(&mut c, &json!({ "title": "buy milk", "project": "home" }));
        assert!(r.ok, "{}", r.summary);
        let r = list_tasks(&c, &json!({}));
        assert!(r.ok);
        let tasks = r.content.as_array().unwrap();
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0]["title"], "buy milk");
    }

    #[test]
    fn set_task_status_rejects_unknown_id_and_names_valid_ones() {
        let (mut c, _d) = ctx();
        let r = create_task(&mut c, &json!({ "title": "x" }));
        let uid = r.content["uid"].as_str().unwrap().to_string();
        let r = set_task_status(&mut c, &json!({ "uid": uid, "status": "nope" }));
        assert!(!r.ok);
        assert!(r.summary.contains("todo"), "{}", r.summary);
    }

    #[test]
    fn delete_event_is_reported_destructive() {
        assert!(destructive("delete_event"));
        assert!(destructive("delete_task"));
        assert!(!destructive("create_event"));
        assert!(!destructive("update_task"));
    }

    #[test]
    fn read_only_calendar_write_fails_with_existing_error() {
        let (mut c, dir) = ctx();
        let mut col = mgmt_domain::Collection::local("holidays", mgmt_domain::CollectionKind::Events);
        col.remote = Some(mgmt_domain::RemoteSource::Ics { url: "https://ex.org/h.ics".into(), refresh_minutes: 60 });
        mgmt_store::save_calendars(dir.path(), &[col]).unwrap();
        c.reload().unwrap();
        let r = create_event(
            &mut c,
            &json!({ "calendar": "holidays", "summary": "x", "start": "2026-01-01T09:00:00Z", "end": "2026-01-01T10:00:00Z" }),
        );
        assert!(!r.ok);
        assert!(r.summary.contains("read-only"), "{}", r.summary);
    }

    #[test]
    fn describe_destructive_names_the_event() {
        let (mut c, _d) = ctx();
        let r = create_event(
            &mut c,
            &json!({ "calendar": "default", "summary": "standup", "start": "2026-01-01T09:00:00Z", "end": "2026-01-01T09:30:00Z" }),
        );
        let uid = r.content["uid"].as_str().unwrap().to_string();
        let summary = describe_destructive(&c, "delete_event", &json!({ "uid": uid })).unwrap();
        assert!(summary.contains("standup"), "{summary}");
    }
}
