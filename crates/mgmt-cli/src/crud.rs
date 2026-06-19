//! Structured create/edit/delete/list for events and tasks — the scriptable surface an
//! agent uses to manage the schedule. Quick task capture stays in `main::cmd_add`; this
//! module is the full-field CRUD.
//!
//! Conventions:
//! - Items are addressed by UID *prefix*: any unambiguous leading substring resolves.
//! - All times are UTC (see [`crate::datetime`]).
//! - `--json` on the `list`/`show` paths emits machine-readable output for agents; the
//!   default is a compact human line.

use anyhow::{Result, anyhow, bail};
use chrono::{Duration, Utc};
use clap::Subcommand;

use mgmt_core::Uid;
use mgmt_domain::{
    Alarm, AlarmTrigger, Event, EventStatus, Priority, ReminderOffset, Task,
};
use mgmt_service::MgmtContext;

// ---- subcommand trees --------------------------------------------------------------

#[derive(Subcommand)]
pub enum EventCmd {
    /// Create a calendar event.
    Add {
        /// Event title / summary.
        summary: String,
        /// Calendar (local collection) to file it under.
        #[arg(short, long, default_value = "default")]
        calendar: String,
        /// Start time (UTC). 'YYYY-MM-DD', 'YYYY-MM-DD HH:MM', or RFC3339.
        #[arg(short, long)]
        start: String,
        /// End time. Mutually exclusive with --duration.
        #[arg(short, long, conflicts_with = "duration")]
        end: Option<String>,
        /// Duration in minutes (alternative to --end). Defaults to 60 for timed events.
        #[arg(long)]
        duration: Option<i64>,
        /// Mark as an all-day event (end defaults to start + 1 day).
        #[arg(long)]
        all_day: bool,
        #[arg(long)]
        location: Option<String>,
        #[arg(long)]
        description: Option<String>,
        /// Project to bind the event to.
        #[arg(short, long)]
        project: Option<String>,
        /// Recurrence as a raw RRULE, e.g. "FREQ=WEEKLY;BYDAY=MO,WE;COUNT=10".
        #[arg(long)]
        rrule: Option<String>,
        /// Add an alarm N minutes before start (repeatable).
        #[arg(long = "alarm", value_name = "MINUTES")]
        alarms: Vec<i64>,
        /// confirmed | tentative | cancelled.
        #[arg(long, default_value = "confirmed")]
        status: String,
    },
    /// Edit an existing event by UID prefix. Only the flags you pass change.
    Edit {
        id: String,
        #[arg(long)]
        summary: Option<String>,
        #[arg(long)]
        calendar: Option<String>,
        #[arg(short, long)]
        start: Option<String>,
        #[arg(short, long)]
        end: Option<String>,
        #[arg(long)]
        duration: Option<i64>,
        #[arg(long)]
        location: Option<String>,
        #[arg(long)]
        description: Option<String>,
        /// Bind the event to a project.
        #[arg(short, long)]
        project: Option<String>,
        /// Clear the event's project.
        #[arg(long, conflicts_with = "project")]
        clear_project: bool,
        /// Replace the recurrence rule (raw RRULE).
        #[arg(long)]
        rrule: Option<String>,
        /// Drop the recurrence rule entirely.
        #[arg(long, conflicts_with = "rrule")]
        clear_rrule: bool,
        /// Replace all alarms with these (minutes-before; repeatable). Pass --clear-alarms to remove.
        #[arg(long = "alarm", value_name = "MINUTES")]
        alarms: Vec<i64>,
        #[arg(long, conflicts_with = "alarms")]
        clear_alarms: bool,
        #[arg(long)]
        status: Option<String>,
        /// Make the event all-day.
        #[arg(long, conflicts_with = "timed")]
        all_day: bool,
        /// Make the event timed (clear all-day).
        #[arg(long)]
        timed: bool,
    },
    /// Delete an event by UID prefix.
    Rm { id: String },
    /// Show one event by UID prefix.
    Show {
        id: String,
        #[arg(long)]
        json: bool,
    },
    /// List events, optionally restricted to a calendar and/or date window.
    List {
        #[arg(short, long)]
        calendar: Option<String>,
        /// Lower bound (inclusive). Without --from/--to, all stored events are listed.
        #[arg(long)]
        from: Option<String>,
        /// Upper bound (exclusive).
        #[arg(long)]
        to: Option<String>,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
pub enum TaskCmd {
    /// Create a task with full fields (use top-level `add` for quick capture).
    Add {
        title: String,
        #[arg(short, long)]
        project: Option<String>,
        /// none | low | medium | high.
        #[arg(long, default_value = "none")]
        priority: String,
        /// todo | doing | done | cancelled | incomplete.
        #[arg(long, default_value = "todo")]
        status: String,
        #[arg(long)]
        due: Option<String>,
        #[arg(long)]
        scheduled: Option<String>,
        #[arg(long)]
        area: Option<String>,
        /// Comma-separated tags.
        #[arg(long)]
        tags: Option<String>,
        /// Reminder offset before due (e.g. 1d, 2h, 30m); repeatable. Defaults from config apply
        /// when --due is set and no reminders are given.
        #[arg(long = "reminder", value_name = "OFFSET")]
        reminders: Vec<String>,
        /// Markdown body / notes.
        #[arg(long)]
        body: Option<String>,
    },
    /// Edit a task by UID prefix. Only the flags you pass change.
    Edit {
        id: String,
        #[arg(long)]
        title: Option<String>,
        #[arg(long)]
        status: Option<String>,
        #[arg(long)]
        priority: Option<String>,
        #[arg(long)]
        project: Option<String>,
        #[arg(long, conflicts_with = "project")]
        clear_project: bool,
        #[arg(long)]
        due: Option<String>,
        #[arg(long, conflicts_with = "due")]
        clear_due: bool,
        #[arg(long)]
        scheduled: Option<String>,
        #[arg(long, conflicts_with = "scheduled")]
        clear_scheduled: bool,
        #[arg(long)]
        area: Option<String>,
        #[arg(long)]
        tags: Option<String>,
        /// Replace reminders with these offsets (e.g. 1d, 2h); repeatable.
        #[arg(long = "reminder", value_name = "OFFSET")]
        reminders: Vec<String>,
        /// Clear all reminders.
        #[arg(long, conflicts_with = "reminders")]
        clear_reminders: bool,
        #[arg(long)]
        body: Option<String>,
    },
    /// Delete a task by UID prefix.
    Rm { id: String },
    /// Show one task by UID prefix.
    Show {
        id: String,
        #[arg(long)]
        json: bool,
    },
    /// List tasks, optionally filtered by status and/or project.
    List {
        #[arg(long)]
        status: Option<String>,
        #[arg(long)]
        project: Option<String>,
        #[arg(long)]
        json: bool,
    },
}

// ---- dispatch ----------------------------------------------------------------------

pub fn run_event(ctx: &mut MgmtContext, cmd: EventCmd) -> Result<()> {
    match cmd {
        EventCmd::Add {
            summary,
            calendar,
            start,
            end,
            duration,
            all_day,
            location,
            description,
            project,
            rrule,
            alarms,
            status,
        } => {
            let start = crate::datetime::parse_when(&start)?;
            let end = resolve_end(start, end.as_deref(), duration, all_day)?;
            if end < start {
                bail!("event end is before its start");
            }
            if let Some(p) = &project {
                ctx.add_project(p.clone()).map_err(anyerr)?;
            }
            let mut ev = Event::new(calendar, summary, start, end);
            ev.all_day = all_day;
            ev.location = location;
            ev.description = description;
            ev.project = project;
            ev.status = parse_event_status(&status)?;
            if let Some(r) = rrule {
                ev.rrule = Some(mgmt_ical::from_rrule(&r).map_err(anyerr)?);
            }
            ev.alarms = alarms.into_iter().map(alarm_minutes_before).collect();
            let uid = ev.uid.clone();
            ctx.put_event(ev).map_err(anyerr)?;
            println!("added event {uid}");
            Ok(())
        }
        EventCmd::Edit {
            id,
            summary,
            calendar,
            start,
            end,
            duration,
            location,
            description,
            project,
            clear_project,
            rrule,
            clear_rrule,
            alarms,
            clear_alarms,
            status,
            all_day,
            timed,
        } => {
            let uid = resolve_event(ctx, &id)?;
            let mut ev = ctx.event(&uid).cloned().unwrap();
            if let Some(s) = summary {
                ev.summary = s;
            }
            if let Some(c) = calendar {
                ev.calendar = c;
            }
            if let Some(s) = start {
                let new_start = crate::datetime::parse_when(&s)?;
                // Preserve duration unless an explicit end/duration is also given below.
                let span = ev.end - ev.start;
                ev.start = new_start;
                ev.end = new_start + span;
            }
            if let Some(e) = end {
                ev.end = crate::datetime::parse_when(&e)?;
            } else if let Some(mins) = duration {
                ev.end = ev.start + Duration::minutes(mins);
            }
            if ev.end < ev.start {
                bail!("event end is before its start");
            }
            if all_day {
                ev.all_day = true;
            }
            if timed {
                ev.all_day = false;
            }
            if let Some(l) = location {
                ev.location = Some(l);
            }
            if let Some(d) = description {
                ev.description = Some(d);
            }
            if clear_project {
                ev.project = None;
            } else if let Some(p) = project {
                ctx.add_project(p.clone()).map_err(anyerr)?;
                ev.project = Some(p);
            }
            if clear_rrule {
                ev.rrule = None;
            } else if let Some(r) = rrule {
                ev.rrule = Some(mgmt_ical::from_rrule(&r).map_err(anyerr)?);
            }
            if clear_alarms {
                ev.alarms.clear();
            } else if !alarms.is_empty() {
                ev.alarms = alarms.into_iter().map(alarm_minutes_before).collect();
            }
            if let Some(s) = status {
                ev.status = parse_event_status(&s)?;
            }
            ctx.put_event(ev).map_err(anyerr)?;
            println!("updated event {uid}");
            Ok(())
        }
        EventCmd::Rm { id } => {
            let uid = resolve_event(ctx, &id)?;
            ctx.delete_event(&uid).map_err(anyerr)?;
            println!("deleted event {uid}");
            Ok(())
        }
        EventCmd::Show { id, json } => {
            let uid = resolve_event(ctx, &id)?;
            let ev = ctx.event(&uid).unwrap();
            if json {
                println!("{}", event_json(ev));
            } else {
                println!("{}", event_line(ev));
            }
            Ok(())
        }
        EventCmd::List {
            calendar,
            from,
            to,
            json,
        } => {
            let mut events: Vec<Event> = match (from, to) {
                (Some(f), Some(t)) => {
                    ctx.events_in_range(crate::datetime::parse_when(&f)?, crate::datetime::parse_when(&t)?)
                }
                (from, to) => {
                    // Open-ended: filter the raw set by whichever bound was given.
                    let lo = from.map(|f| crate::datetime::parse_when(&f)).transpose()?;
                    let hi = to.map(|t| crate::datetime::parse_when(&t)).transpose()?;
                    let mut v: Vec<Event> = ctx
                        .events()
                        .iter()
                        .filter(|e| lo.map(|l| e.end > l).unwrap_or(true) && hi.map(|h| e.start < h).unwrap_or(true))
                        .cloned()
                        .collect();
                    v.sort_by_key(|e| e.start);
                    v
                }
            };
            if let Some(c) = &calendar {
                events.retain(|e| &e.calendar == c);
            }
            print_list(&events, json, event_json, event_line);
            Ok(())
        }
    }
}

pub fn run_task(ctx: &mut MgmtContext, cmd: TaskCmd) -> Result<()> {
    match cmd {
        TaskCmd::Add {
            title,
            project,
            priority,
            status,
            due,
            scheduled,
            area,
            tags,
            reminders,
            body,
        } => {
            let mut t = Task::new(title);
            t.created = Some(Utc::now());
            t.project = project;
            t.priority = parse_priority(&priority)?;
            t.status = parse_task_status(&status)?;
            t.due = due.map(|d| crate::datetime::parse_when(&d)).transpose()?;
            t.scheduled = scheduled.map(|d| crate::datetime::parse_when(&d)).transpose()?;
            t.area = area;
            if let Some(tg) = tags {
                t.tags = split_tags(&tg);
            }
            t.reminders = parse_reminders(&reminders)?;
            // Fall back to the configured default reminders when a due date is set and none given.
            if t.reminders.is_empty() && t.due.is_some() {
                t.reminders = ctx.default_reminders();
            }
            if let Some(b) = body {
                t.body = b;
            }
            if let Some(p) = t.project.clone() {
                ctx.add_project(p).map_err(anyerr)?;
            }
            let uid = t.uid.clone();
            ctx.put_task(t).map_err(anyerr)?;
            println!("added task {uid}");
            Ok(())
        }
        TaskCmd::Edit {
            id,
            title,
            status,
            priority,
            project,
            clear_project,
            due,
            clear_due,
            scheduled,
            clear_scheduled,
            area,
            tags,
            reminders,
            clear_reminders,
            body,
        } => {
            let uid = resolve_task(ctx, &id)?;
            let mut t = ctx.task(&uid).cloned().unwrap();
            if let Some(s) = title {
                t.title = s;
            }
            if let Some(s) = status {
                t.status = parse_task_status(&s)?;
            }
            if let Some(p) = priority {
                t.priority = parse_priority(&p)?;
            }
            if clear_project {
                t.project = None;
            } else if let Some(p) = project {
                ctx.add_project(p.clone()).map_err(anyerr)?;
                t.project = Some(p);
            }
            if clear_due {
                t.due = None;
            } else if let Some(d) = due {
                t.due = Some(crate::datetime::parse_when(&d)?);
            }
            if clear_scheduled {
                t.scheduled = None;
            } else if let Some(d) = scheduled {
                t.scheduled = Some(crate::datetime::parse_when(&d)?);
            }
            if let Some(a) = area {
                t.area = Some(a);
            }
            if let Some(tg) = tags {
                t.tags = split_tags(&tg);
            }
            if clear_reminders {
                t.reminders.clear();
            } else if !reminders.is_empty() {
                t.reminders = parse_reminders(&reminders)?;
            }
            if let Some(b) = body {
                t.body = b;
            }
            ctx.put_task(t).map_err(anyerr)?;
            println!("updated task {uid}");
            Ok(())
        }
        TaskCmd::Rm { id } => {
            let uid = resolve_task(ctx, &id)?;
            ctx.delete_task(&uid).map_err(anyerr)?;
            println!("deleted task {uid}");
            Ok(())
        }
        TaskCmd::Show { id, json } => {
            let uid = resolve_task(ctx, &id)?;
            let t = ctx.task(&uid).unwrap();
            if json {
                println!("{}", task_json(t));
            } else {
                println!("{}", task_line(t));
            }
            Ok(())
        }
        TaskCmd::List {
            status,
            project,
            json,
        } => {
            let want_status = status.map(|s| parse_task_status(&s)).transpose()?;
            let mut tasks: Vec<Task> = ctx
                .tasks()
                .iter()
                .filter(|t| want_status.as_deref().map(|s| t.status == s).unwrap_or(true))
                .filter(|t| project.as_deref().map(|p| t.project.as_deref() == Some(p)).unwrap_or(true))
                .cloned()
                .collect();
            tasks.sort_by(|a, b| a.calendar_date().cmp(&b.calendar_date()).then(a.title.cmp(&b.title)));
            print_list(&tasks, json, task_json, task_line);
            Ok(())
        }
    }
}

// ---- UID resolution ----------------------------------------------------------------

fn resolve_event(ctx: &MgmtContext, prefix: &str) -> Result<Uid> {
    resolve(prefix, ctx.events().iter().map(|e| (&e.uid, e.summary.as_str())))
}

fn resolve_task(ctx: &MgmtContext, prefix: &str) -> Result<Uid> {
    resolve(prefix, ctx.tasks().iter().map(|t| (&t.uid, t.title.as_str())))
}

/// Resolve a UID prefix to exactly one item. Errors list the ambiguous/absent candidates so
/// the caller (often an agent) can recover.
fn resolve<'a>(prefix: &str, items: impl Iterator<Item = (&'a Uid, &'a str)>) -> Result<Uid> {
    let matches: Vec<(Uid, String)> = items
        .filter(|(uid, _)| uid.as_str().starts_with(prefix))
        .map(|(uid, label)| (uid.clone(), label.to_string()))
        .collect();
    match matches.len() {
        1 => Ok(matches.into_iter().next().unwrap().0),
        0 => Err(anyhow!("no item with UID starting {prefix:?}")),
        _ => {
            let listed: Vec<String> = matches.iter().take(8).map(|(u, l)| format!("  {u}  {l}")).collect();
            Err(anyhow!(
                "{} items match UID prefix {prefix:?}; be more specific:\n{}",
                matches.len(),
                listed.join("\n")
            ))
        }
    }
}

// ---- parsing helpers ---------------------------------------------------------------

fn resolve_end(
    start: chrono::DateTime<Utc>,
    end: Option<&str>,
    duration: Option<i64>,
    all_day: bool,
) -> Result<chrono::DateTime<Utc>> {
    if let Some(e) = end {
        return crate::datetime::parse_when(e);
    }
    if let Some(mins) = duration {
        return Ok(start + Duration::minutes(mins));
    }
    Ok(start + if all_day { Duration::days(1) } else { Duration::minutes(60) })
}

fn parse_event_status(s: &str) -> Result<EventStatus> {
    Ok(match s.to_ascii_lowercase().as_str() {
        "confirmed" => EventStatus::Confirmed,
        "tentative" => EventStatus::Tentative,
        "cancelled" | "canceled" => EventStatus::Cancelled,
        other => bail!("unknown event status {other:?} (confirmed|tentative|cancelled)"),
    })
}

/// Statuses are configurable, so any non-empty id is accepted (lowercased/trimmed). A couple of
/// common aliases fold to the built-in canonical ids.
fn parse_task_status(s: &str) -> Result<String> {
    let s = s.trim().to_ascii_lowercase();
    if s.is_empty() {
        bail!("task status is empty");
    }
    Ok(match s.as_str() {
        "in-progress" | "in_progress" => "doing".into(),
        "completed" => "done".into(),
        "canceled" => "cancelled".into(),
        _ => s,
    })
}

fn parse_priority(s: &str) -> Result<Priority> {
    Ok(match s.to_ascii_lowercase().as_str() {
        "none" => Priority::None,
        "low" => Priority::Low,
        "medium" | "med" => Priority::Medium,
        "high" => Priority::High,
        other => bail!("unknown priority {other:?} (none|low|medium|high)"),
    })
}

fn alarm_minutes_before(mins: i64) -> Alarm {
    Alarm {
        trigger: AlarmTrigger::MinutesBefore(mins),
        description: None,
    }
}

fn split_tags(s: &str) -> Vec<String> {
    s.split(',').map(|t| t.trim().to_string()).filter(|t| !t.is_empty()).collect()
}

/// Parse a list of reminder offset strings (`1d`, `2h`, `30m`, or bare minutes).
fn parse_reminders(items: &[String]) -> Result<Vec<ReminderOffset>> {
    items
        .iter()
        .map(|s| ReminderOffset::parse(s).ok_or_else(|| anyhow!("invalid reminder offset {s:?} (try 1d, 2h, 30m)")))
        .collect()
}

fn anyerr(e: mgmt_core::Error) -> anyhow::Error {
    anyhow!(e.to_string())
}

// ---- output ------------------------------------------------------------------------

fn print_list<T>(items: &[T], json: bool, to_json: fn(&T) -> String, to_line: fn(&T) -> String) {
    if json {
        let body: Vec<String> = items.iter().map(to_json).collect();
        println!("[{}]", body.join(","));
    } else if items.is_empty() {
        println!("(none)");
    } else {
        for it in items {
            println!("{}", to_line(it));
        }
    }
}

fn event_line(e: &Event) -> String {
    let span = if e.all_day {
        format!("{} (all-day)", crate::datetime::fmt_when(e.start).split(' ').next().unwrap_or_default())
    } else {
        format!("{} -> {}", crate::datetime::fmt_when(e.start), crate::datetime::fmt_when(e.end))
    };
    let rec = e.rrule.as_ref().map(|r| format!(" [{}]", mgmt_ical::to_rrule(r))).unwrap_or_default();
    let proj = e.project.as_deref().map(|p| format!(" #{p}")).unwrap_or_default();
    format!("{}  {span}  {} ({}){proj}{rec}", short(&e.uid), e.summary, e.calendar)
}

fn task_line(t: &Task) -> String {
    let when = t
        .calendar_date()
        .map(|d| format!(" @{}", crate::datetime::fmt_when(d)))
        .unwrap_or_default();
    let proj = t.project.as_deref().map(|p| format!(" #{p}")).unwrap_or_default();
    format!("{}  [{}] {}{when}{proj}", short(&t.uid), t.status, t.title)
}

/// First 8 chars of a UID — enough to copy/paste back as a prefix.
fn short(uid: &Uid) -> String {
    uid.as_str().chars().take(8).collect()
}

fn event_json(e: &Event) -> String {
    let mut f = JsonObj::new();
    f.str("uid", e.uid.as_str());
    f.str("calendar", &e.calendar);
    f.str("summary", &e.summary);
    f.opt_str("description", e.description.as_deref());
    f.opt_str("location", e.location.as_deref());
    f.opt_str("project", e.project.as_deref());
    f.bool("all_day", e.all_day);
    f.str("start", &crate::datetime::fmt_when(e.start));
    f.str("end", &crate::datetime::fmt_when(e.end));
    f.opt_owned("rrule", e.rrule.as_ref().map(mgmt_ical::to_rrule));
    f.str("status", event_status_token(e.status));
    f.finish()
}

fn task_json(t: &Task) -> String {
    let mut f = JsonObj::new();
    f.str("uid", t.uid.as_str());
    f.str("title", &t.title);
    f.str("status", &t.status);
    f.str("priority", priority_token(t.priority));
    f.opt_str("project", t.project.as_deref());
    f.opt_str("area", t.area.as_deref());
    f.opt_owned("due", t.due.map(crate::datetime::fmt_when));
    f.opt_owned("scheduled", t.scheduled.map(crate::datetime::fmt_when));
    f.list("tags", &t.tags);
    let reminders: Vec<String> = t.reminders.iter().map(|r| r.label()).collect();
    f.list("reminders", &reminders);
    f.finish()
}

fn event_status_token(s: EventStatus) -> &'static str {
    match s {
        EventStatus::Confirmed => "confirmed",
        EventStatus::Tentative => "tentative",
        EventStatus::Cancelled => "cancelled",
    }
}

fn priority_token(p: Priority) -> &'static str {
    match p {
        Priority::None => "none",
        Priority::Low => "low",
        Priority::Medium => "medium",
        Priority::High => "high",
    }
}

/// Minimal JSON object builder — avoids pulling serde_json into the offline build for what
/// is only flat, string-keyed output.
struct JsonObj {
    fields: Vec<String>,
}

impl JsonObj {
    fn new() -> Self {
        JsonObj { fields: Vec::new() }
    }
    fn str(&mut self, key: &str, val: &str) {
        self.fields.push(format!("{}:{}", quote(key), quote(val)));
    }
    fn opt_str(&mut self, key: &str, val: Option<&str>) {
        if let Some(v) = val {
            self.str(key, v);
        }
    }
    fn opt_owned(&mut self, key: &str, val: Option<String>) {
        if let Some(v) = val {
            self.str(key, &v);
        }
    }
    fn bool(&mut self, key: &str, val: bool) {
        self.fields.push(format!("{}:{}", quote(key), val));
    }
    fn list(&mut self, key: &str, vals: &[String]) {
        let items: Vec<String> = vals.iter().map(|v| quote(v)).collect();
        self.fields.push(format!("{}:[{}]", quote(key), items.join(",")));
    }
    fn finish(self) -> String {
        format!("{{{}}}", self.fields.join(","))
    }
}

/// JSON-escape and quote a string (handles the control/escape set we can actually emit).
fn quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quote_escapes_specials() {
        assert_eq!(quote("a\"b\\c\n"), "\"a\\\"b\\\\c\\n\"");
    }

    #[test]
    fn split_tags_trims_and_drops_empties() {
        assert_eq!(split_tags("a, b ,,c"), vec!["a", "b", "c"]);
    }

    #[test]
    fn resolve_rejects_ambiguous_and_missing() {
        let a = Uid::from_string("abc111");
        let b = Uid::from_string("abc222");
        let items = [(&a, "one"), (&b, "two")];
        assert!(resolve("abc", items.iter().copied()).is_err()); // ambiguous
        assert!(resolve("zzz", items.iter().copied()).is_err()); // missing
        assert_eq!(resolve("abc1", items.iter().copied()).unwrap(), a); // unique
    }

    #[test]
    fn resolve_end_prefers_explicit_then_duration_then_default() {
        let start = crate::datetime::parse_when("2026-06-18 09:00").unwrap();
        // default timed = +60m
        assert_eq!(resolve_end(start, None, None, false).unwrap(), start + Duration::minutes(60));
        // all-day default = +1d
        assert_eq!(resolve_end(start, None, None, true).unwrap(), start + Duration::days(1));
        // duration wins over default
        assert_eq!(resolve_end(start, None, Some(30), false).unwrap(), start + Duration::minutes(30));
    }
}
