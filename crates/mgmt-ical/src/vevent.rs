//! [`Event`] <-> `VEVENT` mapping.

use chrono::Utc;
use mgmt_core::{Error, Result, Uid};
use mgmt_domain::{Alarm, AlarmTrigger, Event, EventStatus};

use crate::parser::{self, Component};
use crate::{rrule, value};

/// Serialize an event as a complete `VCALENDAR` document for sending to a remote server
/// (no mgmt-private properties).
pub fn to_ics(ev: &Event) -> String {
    render(ev, false)
}

/// Serialize for the local vdir store, embedding sync metadata (`href`/`etag`) as
/// `X-MGMT-*` properties so it survives a round-trip through the `.ics` file. These are
/// stripped before the event is sent to a server.
pub fn to_ics_local(ev: &Event) -> String {
    render(ev, true)
}

fn render(ev: &Event, include_sync: bool) -> String {
    let mut out = String::new();
    value::write_folded(&mut out, "BEGIN:VCALENDAR");
    value::write_folded(&mut out, "VERSION:2.0");
    value::write_folded(&mut out, "PRODID:-//mgmt//mgmt-ical//EN");
    write_vevent(&mut out, ev, include_sync);
    value::write_folded(&mut out, "END:VCALENDAR");
    out
}

/// Serialize just the `VEVENT` block. `include_sync` embeds `X-MGMT-HREF`/`X-MGMT-ETAG`.
pub fn write_vevent(out: &mut String, ev: &Event, include_sync: bool) {
    value::write_folded(out, "BEGIN:VEVENT");
    value::write_folded(out, &format!("UID:{}", ev.uid));
    value::write_folded(out, &format!("DTSTAMP:{}", value::format_datetime(Utc::now())));
    if ev.all_day {
        value::write_folded(out, &format!("DTSTART;VALUE=DATE:{}", value::format_date(ev.start)));
        value::write_folded(out, &format!("DTEND;VALUE=DATE:{}", value::format_date(ev.end)));
    } else {
        value::write_folded(out, &format!("DTSTART:{}", value::format_datetime(ev.start)));
        value::write_folded(out, &format!("DTEND:{}", value::format_datetime(ev.end)));
    }
    value::write_folded(out, &format!("SUMMARY:{}", value::escape_text(&ev.summary)));
    if let Some(d) = &ev.description {
        value::write_folded(out, &format!("DESCRIPTION:{}", value::escape_text(d)));
    }
    if let Some(l) = &ev.location {
        value::write_folded(out, &format!("LOCATION:{}", value::escape_text(l)));
    }
    value::write_folded(out, &format!("STATUS:{}", status_token(ev.status)));
    if let Some(r) = &ev.rrule {
        value::write_folded(out, &format!("RRULE:{}", rrule::to_rrule(r)));
    }
    for alarm in &ev.alarms {
        write_valarm(out, alarm);
    }
    if include_sync {
        if let Some(href) = &ev.sync.href {
            value::write_folded(out, &format!("X-MGMT-HREF:{}", value::escape_text(href)));
        }
        if let Some(etag) = &ev.sync.etag {
            value::write_folded(out, &format!("X-MGMT-ETAG:{}", value::escape_text(etag)));
        }
    }
    value::write_folded(out, "END:VEVENT");
}

fn write_valarm(out: &mut String, alarm: &Alarm) {
    value::write_folded(out, "BEGIN:VALARM");
    value::write_folded(out, "ACTION:DISPLAY");
    match alarm.trigger {
        AlarmTrigger::MinutesBefore(m) => {
            value::write_folded(out, &format!("TRIGGER:-PT{m}M"));
        }
    }
    let desc = alarm.description.as_deref().unwrap_or("Reminder");
    value::write_folded(out, &format!("DESCRIPTION:{}", value::escape_text(desc)));
    value::write_folded(out, "END:VALARM");
}

/// Parse the first `VEVENT` found in `input` into an [`Event`] under `calendar`.
pub fn from_ics(input: &str, calendar: &str) -> Result<Event> {
    let root = parser::parse(input)?;
    let ve = root
        .find("VEVENT")
        .ok_or_else(|| Error::Parse("no VEVENT in document".into()))?;
    from_component(ve, calendar)
}

pub fn from_component(ve: &Component, calendar: &str) -> Result<Event> {
    let uid = ve
        .value("UID")
        .map(Uid::from_string)
        .unwrap_or_default();
    let summary = ve.value("SUMMARY").map(value::unescape_text).unwrap_or_default();

    let dtstart = ve
        .prop("DTSTART")
        .ok_or_else(|| Error::Parse("VEVENT missing DTSTART".into()))?;
    let all_day = dtstart.param("VALUE").map(|v| v.eq_ignore_ascii_case("DATE")).unwrap_or(false);
    let start = if all_day {
        value::parse_date(&dtstart.value)?
    } else {
        value::parse_datetime(&dtstart.value)?
    };
    let end = match ve.prop("DTEND") {
        Some(p) if all_day => value::parse_date(&p.value)?,
        Some(p) => value::parse_datetime(&p.value)?,
        None => start, // zero-length if unspecified
    };

    let mut ev = Event::new(calendar, summary, start, end);
    ev.uid = uid;
    ev.all_day = all_day;
    ev.description = ve.value("DESCRIPTION").map(value::unescape_text);
    ev.location = ve.value("LOCATION").map(value::unescape_text);
    ev.status = ve.value("STATUS").map(parse_status).unwrap_or_default();
    if let Some(r) = ve.value("RRULE") {
        ev.rrule = Some(rrule::from_rrule(r)?);
    }
    for child in &ve.children {
        if child.name.eq_ignore_ascii_case("VALARM") {
            if let Some(a) = parse_valarm(child) {
                ev.alarms.push(a);
            }
        }
    }
    // mgmt-private sync metadata, if this came from the local vdir store.
    ev.sync.href = ve.value("X-MGMT-HREF").map(value::unescape_text);
    ev.sync.etag = ve.value("X-MGMT-ETAG").map(value::unescape_text);
    Ok(ev)
}

fn parse_valarm(c: &Component) -> Option<Alarm> {
    let trigger = c.value("TRIGGER")?;
    // Parse "-PT15M" style relative triggers; positive/other forms fall back to 0.
    let minutes = trigger
        .trim_start_matches('-')
        .trim_start_matches("PT")
        .trim_end_matches('M')
        .parse::<i64>()
        .ok()?;
    Some(Alarm {
        trigger: AlarmTrigger::MinutesBefore(minutes),
        description: c.value("DESCRIPTION").map(value::unescape_text),
    })
}

fn status_token(s: EventStatus) -> &'static str {
    match s {
        EventStatus::Confirmed => "CONFIRMED",
        EventStatus::Tentative => "TENTATIVE",
        EventStatus::Cancelled => "CANCELLED",
    }
}

fn parse_status(s: &str) -> EventStatus {
    match s.to_ascii_uppercase().as_str() {
        "TENTATIVE" => EventStatus::Tentative,
        "CANCELLED" => EventStatus::Cancelled,
        _ => EventStatus::Confirmed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use mgmt_domain::{Frequency, RecurrenceRule};

    #[test]
    fn timed_event_round_trips() {
        let mut ev = Event::new(
            "work",
            "Standup, daily; sync",
            Utc.with_ymd_and_hms(2026, 6, 18, 9, 0, 0).unwrap(),
            Utc.with_ymd_and_hms(2026, 6, 18, 9, 30, 0).unwrap(),
        );
        ev.uid = Uid::from_string("fixed-uid");
        ev.description = Some("line1\nline2".into());
        ev.location = Some("Room 1".into());
        ev.rrule = Some(RecurrenceRule::every(Frequency::Daily, 1));
        ev.alarms.push(Alarm {
            trigger: AlarmTrigger::MinutesBefore(15),
            description: None,
        });

        let parsed = from_ics(&to_ics(&ev), "work").unwrap();
        assert_eq!(parsed.uid, ev.uid);
        assert_eq!(parsed.summary, ev.summary);
        assert_eq!(parsed.start, ev.start);
        assert_eq!(parsed.end, ev.end);
        assert_eq!(parsed.description, ev.description);
        assert_eq!(parsed.location, ev.location);
        assert_eq!(parsed.rrule, ev.rrule);
        assert_eq!(parsed.alarms.len(), 1);
    }

    #[test]
    fn all_day_event_uses_date_value() {
        let mut ev = Event::new(
            "personal",
            "Holiday",
            Utc.with_ymd_and_hms(2026, 12, 25, 0, 0, 0).unwrap(),
            Utc.with_ymd_and_hms(2026, 12, 26, 0, 0, 0).unwrap(),
        );
        ev.all_day = true;
        let ics = to_ics(&ev);
        assert!(ics.contains("DTSTART;VALUE=DATE:20261225"));
        let parsed = from_ics(&ics, "personal").unwrap();
        assert!(parsed.all_day);
        assert_eq!(parsed.start, ev.start);
    }

    #[test]
    fn local_serialization_persists_sync_meta_remote_omits_it() {
        let mut ev = Event::new(
            "work",
            "synced",
            Utc.with_ymd_and_hms(2026, 6, 18, 9, 0, 0).unwrap(),
            Utc.with_ymd_and_hms(2026, 6, 18, 9, 30, 0).unwrap(),
        );
        ev.sync.href = Some("/cal/work/synced.ics".into());
        ev.sync.etag = Some("\"abc\"".into());

        let local = to_ics_local(&ev);
        assert!(local.contains("X-MGMT-HREF:/cal/work/synced.ics"));
        let reloaded = from_ics(&local, "work").unwrap();
        assert_eq!(reloaded.sync.href.as_deref(), Some("/cal/work/synced.ics"));
        assert_eq!(reloaded.sync.etag.as_deref(), Some("\"abc\""));

        // The body we send to a server must not leak mgmt-private props.
        let remote = to_ics(&ev);
        assert!(!remote.contains("X-MGMT"));
    }
}
