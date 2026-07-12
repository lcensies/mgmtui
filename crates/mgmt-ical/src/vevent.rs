//! [`Event`] <-> `VEVENT` mapping.

use mgmt_core::{Error, Result, Uid};
use mgmt_domain::{Alarm, AlarmAction, AlarmTrigger, Event, EventStatus};

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
    // DTSTAMP must be *deterministic* for unchanged events: the native sync detects local edits by
    // hashing the clean serialization, so a `Utc::now()` here would make every event look dirty on
    // every pass. Anchor it to the last-modified stamp (falling back to the start when unset).
    let stamp = ev.modified.unwrap_or(ev.start);
    value::write_folded(out, &format!("DTSTAMP:{}", value::format_datetime(stamp)));
    if let Some(modified) = ev.modified {
        value::write_folded(out, &format!("LAST-MODIFIED:{}", value::format_datetime(modified)));
    }
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
    if let Some(url) = &ev.conference_url {
        // RFC 7986 CONFERENCE is a URI value (no text escaping). Advertise VIDEO so clients that
        // group conference links (and Google) recognize it.
        value::write_folded(out, &format!("CONFERENCE;VALUE=URI;FEATURE=VIDEO:{url}"));
    }
    value::write_folded(out, &format!("STATUS:{}", status_token(ev.status)));
    // Project binding is real user data (not sync bookkeeping), so it rides to the server too.
    if let Some(project) = &ev.project {
        value::write_folded(out, &format!("X-MGMT-PROJECT:{}", value::escape_text(project)));
    }
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
    // mgmt-specific action. `notify` is the default and writes nothing, leaving a plain portable
    // DISPLAY alarm; `navigate`/`run` ride along as X-properties other clients ignore.
    match &alarm.action {
        AlarmAction::Notify => {}
        AlarmAction::Navigate => {
            value::write_folded(out, "X-MGMT-ALARM-ACTION:navigate");
        }
        AlarmAction::Run { command, args } => {
            value::write_folded(out, "X-MGMT-ALARM-ACTION:run");
            value::write_folded(out, &format!("X-MGMT-ALARM-CMD:{}", value::escape_text(command)));
            for a in args {
                value::write_folded(out, &format!("X-MGMT-ALARM-ARG:{}", value::escape_text(a)));
            }
        }
    }
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
        value::parse_datetime_tz(&dtstart.value, dtstart.param("TZID"))?
    };
    let end = match ve.prop("DTEND") {
        Some(p) if all_day => value::parse_date(&p.value)?,
        Some(p) => value::parse_datetime_tz(&p.value, p.param("TZID"))?,
        None => start, // zero-length if unspecified
    };

    let mut ev = Event::new(calendar, summary, start, end);
    ev.uid = uid;
    ev.all_day = all_day;
    ev.project = ve.value("X-MGMT-PROJECT").map(value::unescape_text);
    ev.description = ve.value("DESCRIPTION").map(value::unescape_text);
    ev.location = ve.value("LOCATION").map(value::unescape_text);
    ev.status = ve.value("STATUS").map(parse_status).unwrap_or_default();
    ev.modified = ve.value("LAST-MODIFIED").and_then(|v| value::parse_datetime(v).ok());
    // Conference join URL: prefer RFC 7986 CONFERENCE, then Google's X-GOOGLE-CONFERENCE, then a
    // URL sniffed from the description (Telemost/Meet/Zoom/Jitsi links pasted by other clients).
    ev.conference_url = ve
        .value("CONFERENCE")
        .map(|v| v.to_string())
        .or_else(|| ve.value("X-GOOGLE-CONFERENCE").map(|v| v.to_string()))
        .or_else(|| ev.description.as_deref().and_then(sniff_conference_url));
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

/// Parse an RFC 5545 relative TRIGGER duration into "minutes before start". Handles the full
/// duration grammar (`-P0DT0H10M0S`, `-PT1H`, `-P1W`…), not just `-PT<n>M` — foreign alarms
/// (Google emits day+time forms) must not vanish on import. Unparseable/absolute triggers
/// degrade to 0 minutes rather than dropping the alarm.
fn trigger_minutes(trigger: &str) -> i64 {
    fn parse(s: &str) -> Option<i64> {
        let (neg, s) = match s.trim().strip_prefix('-') {
            Some(rest) => (true, rest),
            None => (false, s.trim().strip_prefix('+').unwrap_or(s.trim())),
        };
        let s = s.strip_prefix('P')?;
        let (mut mins, mut num) = (0i64, String::new());
        for ch in s.chars() {
            match ch {
                'T' | 't' => {} // date/time separator
                d if d.is_ascii_digit() => num.push(d),
                unit => {
                    let n: i64 = num.parse().ok()?;
                    num.clear();
                    mins += match unit.to_ascii_uppercase() {
                        'W' => n * 7 * 24 * 60,
                        'D' => n * 24 * 60,
                        'H' => n * 60,
                        'M' => n,
                        'S' => n / 60,
                        _ => return None,
                    };
                }
            }
        }
        num.is_empty().then_some(if neg { mins } else { -mins })
    }
    parse(trigger).unwrap_or(0)
}

fn parse_valarm(c: &Component) -> Option<Alarm> {
    let trigger = c.value("TRIGGER")?;
    let minutes = trigger_minutes(trigger);
    let action = match c.value("X-MGMT-ALARM-ACTION") {
        Some(a) if a.eq_ignore_ascii_case("navigate") => AlarmAction::Navigate,
        Some(a) if a.eq_ignore_ascii_case("run") => {
            let command = c.value("X-MGMT-ALARM-CMD").map(value::unescape_text).unwrap_or_default();
            let args = c
                .props
                .iter()
                .filter(|p| p.name.eq_ignore_ascii_case("X-MGMT-ALARM-ARG"))
                .map(|p| value::unescape_text(&p.value))
                .collect();
            AlarmAction::Run { command, args }
        }
        _ => AlarmAction::Notify,
    };
    Some(Alarm {
        trigger: AlarmTrigger::MinutesBefore(minutes),
        action,
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

/// Find the first known video-conference URL in free text (event description). Recognizes the
/// common hosted services so a Telemost/Meet link pasted into the notes still surfaces as the
/// event's join URL.
fn sniff_conference_url(text: &str) -> Option<String> {
    const HOSTS: [&str; 6] = [
        "telemost.yandex.",
        "meet.google.com",
        "zoom.us",
        "meet.jit.si",
        "teams.microsoft.com",
        "whereby.com",
    ];
    for token in text.split(|c: char| c.is_whitespace() || c == '<' || c == '>' || c == '"') {
        let t = token.trim_end_matches(|c: char| matches!(c, '.' | ',' | ')' | ']' | '。'));
        if (t.starts_with("https://") || t.starts_with("http://")) && HOSTS.iter().any(|h| t.contains(h)) {
            return Some(t.to_string());
        }
    }
    None
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
    use chrono::{TimeZone, Utc};
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
        ev.project = Some("wng".into());
        ev.description = Some("line1\nline2".into());
        ev.location = Some("Room 1".into());
        ev.rrule = Some(RecurrenceRule::every(Frequency::Daily, 1));
        ev.alarms.push(Alarm::minutes_before(15));

        let parsed = from_ics(&to_ics(&ev), "work").unwrap();
        assert_eq!(parsed.uid, ev.uid);
        assert_eq!(parsed.summary, ev.summary);
        assert_eq!(parsed.start, ev.start);
        assert_eq!(parsed.end, ev.end);
        assert_eq!(parsed.description, ev.description);
        assert_eq!(parsed.location, ev.location);
        assert_eq!(parsed.project, ev.project);
        assert_eq!(parsed.rrule, ev.rrule);
        assert_eq!(parsed.alarms.len(), 1);
    }

    #[test]
    fn trigger_durations_beyond_pt_minutes_are_parsed() {
        assert_eq!(trigger_minutes("-PT15M"), 15);
        assert_eq!(trigger_minutes("-PT1H"), 60);
        assert_eq!(trigger_minutes("-P0DT0H10M0S"), 10); // Google's form
        assert_eq!(trigger_minutes("-P1W"), 7 * 24 * 60);
        assert_eq!(trigger_minutes("PT5M"), -5); // after start
        assert_eq!(trigger_minutes("PT0S"), 0);
        assert_eq!(trigger_minutes("garbage"), 0); // degrade, don't drop the alarm
    }

    #[test]
    fn tzid_events_convert_to_utc() {
        let ics = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:tz1\r\n\
                   DTSTART;TZID=Europe/Moscow:20260618T090000\r\nDTEND;TZID=Europe/Moscow:20260618T100000\r\n\
                   SUMMARY:msk\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
        let ev = from_ics(ics, "work").unwrap();
        assert_eq!(ev.start, Utc.with_ymd_and_hms(2026, 6, 18, 6, 0, 0).unwrap());
        assert_eq!(ev.end, Utc.with_ymd_and_hms(2026, 6, 18, 7, 0, 0).unwrap());
    }

    #[test]
    fn last_modified_round_trips_and_serialization_is_deterministic() {
        let mut ev = Event::new(
            "work",
            "synced",
            Utc.with_ymd_and_hms(2026, 6, 18, 9, 0, 0).unwrap(),
            Utc.with_ymd_and_hms(2026, 6, 18, 9, 30, 0).unwrap(),
        );
        ev.uid = Uid::from_string("u");
        ev.modified = Some(Utc.with_ymd_and_hms(2026, 7, 4, 12, 0, 0).unwrap());

        let ics = to_ics(&ev);
        assert!(ics.contains("LAST-MODIFIED:20260704T120000Z"));
        // DTSTAMP is anchored to `modified`, not wall-clock, so an unchanged event serializes
        // byte-identically every pass (the native-sync hash depends on this).
        assert_eq!(ics, to_ics(&ev));

        let parsed = from_ics(&ics, "work").unwrap();
        assert_eq!(parsed.modified, ev.modified);
    }

    #[test]
    fn conference_url_round_trips_and_is_sniffed() {
        let mut ev = Event::new(
            "work",
            "Standup",
            Utc.with_ymd_and_hms(2026, 6, 18, 9, 0, 0).unwrap(),
            Utc.with_ymd_and_hms(2026, 6, 18, 9, 30, 0).unwrap(),
        );
        ev.conference_url = Some("https://telemost.yandex.ru/j/1234567890".into());
        let ics = to_ics(&ev);
        assert!(ics.contains("CONFERENCE;VALUE=URI;FEATURE=VIDEO:https://telemost.yandex.ru/j/1234567890"));
        let parsed = from_ics(&ics, "work").unwrap();
        assert_eq!(parsed.conference_url, ev.conference_url);

        // A Google Meet link only present in the description is sniffed out.
        let doc = "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:x\r\nDTSTART:20260618T090000Z\r\nDTEND:20260618T093000Z\r\nSUMMARY:Sync\r\nDESCRIPTION:Join at https://meet.google.com/abc-defg-hij please\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
        let sniffed = from_ics(doc, "work").unwrap();
        assert_eq!(sniffed.conference_url.as_deref(), Some("https://meet.google.com/abc-defg-hij"));
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

    #[test]
    fn alarm_actions_round_trip() {
        let mut ev = Event::new(
            "work",
            "review",
            Utc.with_ymd_and_hms(2026, 6, 18, 9, 0, 0).unwrap(),
            Utc.with_ymd_and_hms(2026, 6, 18, 9, 30, 0).unwrap(),
        );
        ev.alarms.push(Alarm::minutes_before(10)); // notify (default)
        ev.alarms.push(Alarm::with_action(15, AlarmAction::Navigate));
        ev.alarms.push(Alarm::with_action(
            5,
            AlarmAction::Run { command: "notify-send".into(), args: vec!["hi there".into(), "go".into()] },
        ));

        let parsed = from_ics(&to_ics(&ev), "work").unwrap();
        assert_eq!(parsed.alarms.len(), 3);
        assert_eq!(parsed.alarms[0].action, AlarmAction::Notify);
        assert_eq!(parsed.alarms[1].action, AlarmAction::Navigate);
        assert_eq!(
            parsed.alarms[2].action,
            AlarmAction::Run { command: "notify-send".into(), args: vec!["hi there".into(), "go".into()] }
        );
        // A plain notify alarm must stay a clean portable DISPLAY alarm.
        assert!(!to_ics(&ev).contains("X-MGMT-ALARM-ACTION:notify"));
    }
}
