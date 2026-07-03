//! [`Task`] <-> `VTODO` projection. The markdown file is the local source of truth; this
//! mapping is what rides CalDAV. The markdown body projects to `DESCRIPTION` (lossy on a
//! remote round-trip, by design — see the plan's risk note).

use chrono::Utc;
use mgmt_core::{Error, Result, Uid};
use mgmt_domain::{Priority, Task};

use crate::parser::{self, Component};
use crate::value;

pub fn to_ics(task: &Task) -> String {
    let mut out = String::new();
    value::write_folded(&mut out, "BEGIN:VCALENDAR");
    value::write_folded(&mut out, "VERSION:2.0");
    value::write_folded(&mut out, "PRODID:-//mgmt//mgmt-ical//EN");
    write_vtodo(&mut out, task);
    value::write_folded(&mut out, "END:VCALENDAR");
    out
}

pub fn write_vtodo(out: &mut String, task: &Task) {
    value::write_folded(out, "BEGIN:VTODO");
    value::write_folded(out, &format!("UID:{}", task.uid));
    value::write_folded(out, &format!("DTSTAMP:{}", value::format_datetime(Utc::now())));
    value::write_folded(out, &format!("SUMMARY:{}", value::escape_text(&task.title)));
    if !task.body.is_empty() {
        value::write_folded(out, &format!("DESCRIPTION:{}", value::escape_text(&task.body)));
    }
    if let Some(due) = task.due {
        value::write_folded(out, &format!("DUE:{}", value::format_datetime(due)));
    }
    value::write_folded(out, &format!("STATUS:{}", status_token(&task.status)));
    value::write_folded(out, &format!("PRIORITY:{}", priority_value(task.priority)));
    if let Some(pct) = task.completion {
        value::write_folded(out, &format!("PERCENT-COMPLETE:{pct}"));
    }
    if !task.tags.is_empty() {
        let cats: Vec<String> = task.tags.iter().map(|t| value::escape_text(t)).collect();
        value::write_folded(out, &format!("CATEGORIES:{}", cats.join(",")));
    }
    value::write_folded(out, "END:VTODO");
}

pub fn from_ics(input: &str) -> Result<Task> {
    let root = parser::parse(input)?;
    let vt = root
        .find("VTODO")
        .ok_or_else(|| Error::Parse("no VTODO in document".into()))?;
    from_component(vt)
}

pub fn from_component(vt: &Component) -> Result<Task> {
    let mut task = Task::new(vt.value("SUMMARY").map(value::unescape_text).unwrap_or_default());
    if let Some(uid) = vt.value("UID") {
        task.uid = Uid::from_string(uid);
    }
    task.body = vt.value("DESCRIPTION").map(value::unescape_text).unwrap_or_default();
    if let Some(due) = vt.value("DUE") {
        task.due = Some(value::parse_datetime(due)?);
    }
    task.status = vt.value("STATUS").map(parse_status).unwrap_or_default();
    if let Some(p) = vt.value("PRIORITY") {
        task.priority = parse_priority(p);
    }
    if let Some(pct) = vt.value("PERCENT-COMPLETE") {
        task.completion = pct.parse().ok();
    }
    if let Some(cats) = vt.value("CATEGORIES") {
        task.tags = cats.split(',').map(value::unescape_text).filter(|s| !s.is_empty()).collect();
    }
    Ok(task)
}

/// Map a status id to a coarse iCalendar VTODO `STATUS`. Tasks are markdown-sourced (the exact
/// id round-trips there); this projection is interop-only, so custom/unknown ids that aren't one
/// of the built-in done/cancelled/active ids fall back to `NEEDS-ACTION`.
fn status_token(id: &str) -> &'static str {
    match id {
        "doing" | "incomplete" | "in-progress" | "in_progress" => "IN-PROCESS",
        "done" | "completed" => "COMPLETED",
        "cancelled" | "canceled" => "CANCELLED",
        _ => "NEEDS-ACTION",
    }
}

/// Map an iCalendar VTODO `STATUS` back to a built-in status id.
fn parse_status(s: &str) -> String {
    match s.to_ascii_uppercase().as_str() {
        "IN-PROCESS" => "doing".into(),
        "COMPLETED" => "done".into(),
        "CANCELLED" => "cancelled".into(),
        _ => "todo".into(),
    }
}

/// Map our four-level priority onto the iCalendar 0-9 scale (1-4 high, 5 med, 6-9 low).
fn priority_value(p: Priority) -> u8 {
    match p {
        Priority::None => 0,
        Priority::High => 1,
        Priority::Medium => 5,
        Priority::Low => 9,
    }
}

fn parse_priority(s: &str) -> Priority {
    match s.parse::<u8>().unwrap_or(0) {
        0 => Priority::None,
        1..=4 => Priority::High,
        5 => Priority::Medium,
        _ => Priority::Low,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Timelike;

    #[test]
    fn task_round_trips_core_fields() {
        let mut t = Task::new("Write the plan");
        t.uid = Uid::from_string("task-1");
        t.body = "notes\nmore notes".into();
        t.status = "doing".into();
        t.priority = Priority::High;
        t.completion = Some(40);
        t.tags = vec!["work".into(), "urgent".into()];
        t.due = Some(Utc::now().with_nanosecond(0).unwrap());

        let parsed = from_ics(&to_ics(&t)).unwrap();
        assert_eq!(parsed.uid, t.uid);
        assert_eq!(parsed.title, t.title);
        assert_eq!(parsed.body, t.body);
        assert_eq!(parsed.status, t.status);
        assert_eq!(parsed.priority, t.priority);
        assert_eq!(parsed.completion, t.completion);
        assert_eq!(parsed.tags, t.tags);
        assert_eq!(parsed.due, t.due);
    }

    #[test]
    fn priority_bands_map_back() {
        assert_eq!(parse_priority(&priority_value(Priority::High).to_string()), Priority::High);
        assert_eq!(parse_priority(&priority_value(Priority::Medium).to_string()), Priority::Medium);
        assert_eq!(parse_priority(&priority_value(Priority::Low).to_string()), Priority::Low);
        assert_eq!(parse_priority(&priority_value(Priority::None).to_string()), Priority::None);
    }
}
