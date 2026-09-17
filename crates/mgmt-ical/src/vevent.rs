//! [`Event`] <-> `VEVENT` mapping.

use mgmt_core::{Error, Result, Uid};
use mgmt_domain::{Alarm, AlarmAction, AlarmTrigger, Attendee, Classification, Event, EventStatus, Transparency};

use crate::parser::{self, Component, Prop};
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
    series_to_ics(std::slice::from_ref(ev), include_sync)
}

/// Serialize a whole series — the master plus its `RECURRENCE-ID` overrides — as one
/// `VCALENDAR` document (the standard multi-`VEVENT` layout Google/Radicale produce).
pub fn series_to_ics(comps: &[Event], include_sync: bool) -> String {
    let mut out = String::new();
    value::write_folded(&mut out, "BEGIN:VCALENDAR");
    value::write_folded(&mut out, "VERSION:2.0");
    value::write_folded(&mut out, "PRODID:-//mgmt//mgmt-ical//EN");
    for ev in comps {
        write_vevent(&mut out, ev, include_sync);
    }
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
    write_fields(out, ev);
    // Project binding is real user data (not sync bookkeeping), so it rides to the server too.
    if let Some(project) = &ev.project {
        value::write_folded(out, &format!("X-MGMT-PROJECT:{}", value::escape_text(project)));
    }
    if let Some(r) = &ev.rrule {
        value::write_folded(out, &format!("RRULE:{}", rrule::to_rrule(r)));
    }
    if !ev.exdates.is_empty() {
        let list = ev.exdates.iter().map(|d| occ_time(*d, ev.all_day)).collect::<Vec<_>>().join(",");
        value::write_folded(out, &format!("EXDATE{}:{}", occ_param(ev.all_day), list));
    }
    if let Some(rid) = ev.recurrence_id {
        value::write_folded(out, &format!("RECURRENCE-ID{}:{}", occ_param(ev.all_day), occ_time(rid, ev.all_day)));
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
    value::write_folded(out, &trigger_line(alarm.trigger));
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

/// The `TRIGGER` line for an alarm. Start-relative offsets are plain durations; end-relative
/// ones carry `RELATED=END` and absolute ones `VALUE=DATE-TIME`.
fn trigger_line(t: AlarmTrigger) -> String {
    /// `-PT15M` / `PT15M` for a signed minute offset (negative = earlier).
    fn dur(minutes: i64) -> String {
        format!("{}PT{}M", if minutes >= 0 { "-" } else { "" }, minutes.abs())
    }
    match t {
        AlarmTrigger::MinutesBefore(m) => format!("TRIGGER:{}", dur(m)),
        AlarmTrigger::MinutesAfterStart(m) => format!("TRIGGER:{}", dur(-m)),
        AlarmTrigger::MinutesBeforeEnd(m) => format!("TRIGGER;RELATED=END:{}", dur(m)),
        AlarmTrigger::At(dt) => format!("TRIGGER;VALUE=DATE-TIME:{}", value::format_datetime(dt)),
    }
}

/// Write the RFC 5545/7986 properties beyond the core VEVENT set: busy/free, visibility, color,
/// URL, categories and participants. One block so it stays a single merge unit.
fn write_fields(out: &mut String, ev: &Event) {
    // Defaults (OPAQUE / PUBLIC) are omitted — an absent property means exactly that.
    if ev.transp == Transparency::Transparent {
        value::write_folded(out, "TRANSP:TRANSPARENT");
    }
    match ev.class {
        Classification::Public => {}
        Classification::Private => value::write_folded(out, "CLASS:PRIVATE"),
        Classification::Confidential => value::write_folded(out, "CLASS:CONFIDENTIAL"),
    }
    if let Some(c) = &ev.color {
        value::write_folded(out, &format!("COLOR:{}", value::escape_text(c)));
    }
    if let Some(u) = &ev.url {
        value::write_folded(out, &format!("URL:{u}")); // URI value: not text-escaped
    }
    if !ev.categories.is_empty() {
        let list: Vec<String> = ev.categories.iter().map(|c| value::escape_text(c)).collect();
        value::write_folded(out, &format!("CATEGORIES:{}", list.join(",")));
    }
    if let Some(o) = &ev.organizer {
        value::write_folded(out, &format!("ORGANIZER{}", cal_address(o)));
    }
    for a in &ev.attendees {
        value::write_folded(out, &format!("ATTENDEE{}", cal_address(a)));
    }
}

/// The `;PARAM=…:mailto:…` tail shared by `ORGANIZER` and `ATTENDEE`.
fn cal_address(a: &Attendee) -> String {
    let mut s = String::new();
    if let Some(n) = &a.name {
        s.push_str(&format!(";CN={}", quote_param(n)));
    }
    if let Some(r) = &a.role {
        s.push_str(&format!(";ROLE={}", quote_param(r)));
    }
    if let Some(p) = &a.partstat {
        s.push_str(&format!(";PARTSTAT={}", quote_param(p)));
    }
    if a.rsvp {
        s.push_str(";RSVP=TRUE");
    }
    s.push_str(&format!(":mailto:{}", a.email));
    s
}

/// Quote a parameter value that carries characters a bare param may not hold (RFC 5545 §3.2).
/// A quoted-string cannot contain `"` at all, so an embedded quote degrades to an apostrophe.
fn quote_param(v: &str) -> String {
    if v.contains([';', ':', ',', '"']) {
        format!("\"{}\"", v.replace('"', "'"))
    } else {
        v.to_string()
    }
}

/// Read the properties written by [`write_fields`].
fn read_fields(ev: &mut Event, ve: &Component) {
    ev.transp = match ve.value("TRANSP") {
        Some(v) if v.eq_ignore_ascii_case("TRANSPARENT") => Transparency::Transparent,
        _ => Transparency::Opaque,
    };
    ev.class = match ve.value("CLASS") {
        Some(v) if v.eq_ignore_ascii_case("PRIVATE") => Classification::Private,
        Some(v) if v.eq_ignore_ascii_case("CONFIDENTIAL") => Classification::Confidential,
        _ => Classification::Public,
    };
    ev.color = ve.value("COLOR").map(value::unescape_text);
    ev.url = ve.value("URL").map(|v| v.to_string());
    // CATEGORIES may appear more than once, each holding a comma-separated list.
    ev.categories = props(ve, "CATEGORIES").flat_map(|p| split_list(&p.value)).collect();
    ev.organizer = ve.prop("ORGANIZER").map(attendee_from);
    ev.attendees = props(ve, "ATTENDEE").map(attendee_from).collect();
}

fn props<'a>(c: &'a Component, name: &'a str) -> impl Iterator<Item = &'a Prop> {
    c.props.iter().filter(move |p| p.name.eq_ignore_ascii_case(name))
}

/// Split a comma-separated iCalendar list value, honouring `\,` escapes.
fn split_list(v: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut esc = false;
    for ch in v.chars() {
        match ch {
            _ if esc => {
                cur.push('\\');
                cur.push(ch);
                esc = false;
            }
            '\\' => esc = true,
            ',' => out.push(std::mem::take(&mut cur)),
            _ => cur.push(ch),
        }
    }
    out.push(cur);
    out.iter().map(|s| value::unescape_text(s)).filter(|s| !s.is_empty()).collect()
}

fn attendee_from(p: &Prop) -> Attendee {
    let v = p.value.trim();
    let email = v.get(..7).filter(|s| s.eq_ignore_ascii_case("mailto:")).map_or(v, |_| &v[7..]);
    Attendee {
        email: email.to_string(),
        name: p.param("CN").map(value::unescape_text),
        role: p.param("ROLE").map(|s| s.to_string()),
        partstat: p.param("PARTSTAT").map(|s| s.to_string()),
        rsvp: p.param("RSVP").map(|s| s.eq_ignore_ascii_case("TRUE")).unwrap_or(false),
    }
}

/// Parse the first `VEVENT` found in `input` into an [`Event`] under `calendar`. When the
/// document holds a whole series, this is its master (see [`series_from_ics`]).
pub fn from_ics(input: &str, calendar: &str) -> Result<Event> {
    series_from_ics(input, calendar)?
        .into_iter()
        .next()
        .ok_or_else(|| Error::Parse("no VEVENT in document".into()))
}

/// Parse every `VEVENT` in `input`: the series master first, then its `RECURRENCE-ID` overrides
/// (all sharing the master's `UID`).
pub fn series_from_ics(input: &str, calendar: &str) -> Result<Vec<Event>> {
    let root = parser::parse(input)?;
    let mut comps = Vec::new();
    collect_vevents(&root, &mut comps);
    let mut out: Vec<Event> = comps.iter().map(|c| from_component(c, calendar)).collect::<Result<_>>()?;
    out.sort_by_key(|e| e.recurrence_id); // `None` (the master) first
    Ok(out)
}

fn collect_vevents<'a>(c: &'a Component, out: &mut Vec<&'a Component>) {
    if c.name.eq_ignore_ascii_case("VEVENT") {
        out.push(c);
        return;
    }
    for child in &c.children {
        collect_vevents(child, out);
    }
}

/// `;VALUE=DATE` for all-day events, nothing for timed ones.
fn occ_param(all_day: bool) -> &'static str {
    if all_day { ";VALUE=DATE" } else { "" }
}

fn occ_time(dt: chrono::DateTime<chrono::Utc>, all_day: bool) -> String {
    if all_day { value::format_date(dt) } else { value::format_datetime(dt) }
}

/// Parse one `EXDATE`/`RECURRENCE-ID` value, honouring its `VALUE=DATE`/`TZID` params.
fn parse_occ_time(p: &parser::Prop, raw: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    if p.param("VALUE").map(|v| v.eq_ignore_ascii_case("DATE")).unwrap_or(false) {
        value::parse_date(raw).ok()
    } else {
        value::parse_datetime_tz(raw, p.param("TZID")).ok()
    }
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
    read_fields(&mut ev, ve);
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
    ev.exdates = ve
        .props
        .iter()
        .filter(|p| p.name.eq_ignore_ascii_case("EXDATE"))
        .flat_map(|p| p.value.split(',').filter_map(|raw| parse_occ_time(p, raw)))
        .collect();
    ev.recurrence_id = ve.prop("RECURRENCE-ID").and_then(|p| parse_occ_time(p, &p.value));
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

/// Parse an RFC 5545 relative TRIGGER duration into "minutes before" the related instant. Handles
/// the full duration grammar (`-P0DT0H10M0S`, `-PT1H`, `-P1W`…), not just `-PT<n>M` — foreign
/// alarms (Google emits day+time forms) must not vanish on import. A positive result is *before*,
/// negative *after*; unparseable durations degrade to 0 rather than dropping the alarm.
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

/// Read a `TRIGGER` property. Absolute (`VALUE=DATE-TIME`, or any value that isn't a duration)
/// keeps its instant; `RELATED=END` binds the offset to the event end; otherwise a negative
/// duration is before the start and a positive one after it.
fn parse_trigger(p: &Prop) -> AlarmTrigger {
    let looks_absolute = p.param("VALUE").map(|v| v.eq_ignore_ascii_case("DATE-TIME")).unwrap_or(false)
        || !p.value.starts_with(['-', '+', 'P', 'p']);
    if looks_absolute {
        if let Ok(t) = value::parse_datetime(&p.value) {
            return AlarmTrigger::At(t);
        }
    }
    let m = trigger_minutes(&p.value);
    if p.param("RELATED").map(|v| v.eq_ignore_ascii_case("END")).unwrap_or(false) {
        AlarmTrigger::MinutesBeforeEnd(m)
    } else if m >= 0 {
        AlarmTrigger::MinutesBefore(m)
    } else {
        AlarmTrigger::MinutesAfterStart(-m)
    }
}

fn parse_valarm(c: &Component) -> Option<Alarm> {
    let trigger = parse_trigger(c.prop("TRIGGER")?);
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
        trigger,
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
    fn event_fields_round_trip() {
        let mut ev = Event::new(
            "work",
            "Review",
            Utc.with_ymd_and_hms(2026, 6, 18, 9, 0, 0).unwrap(),
            Utc.with_ymd_and_hms(2026, 6, 18, 10, 0, 0).unwrap(),
        );
        ev.status = EventStatus::Tentative;
        ev.transp = Transparency::Transparent;
        ev.class = Classification::Confidential;
        ev.color = Some("tomato".into());
        ev.url = Some("https://example.com/a?b=1&c=2".into());
        ev.categories = vec!["Work".into(), "Q3, planning".into()];
        ev.organizer = Some(Attendee { name: Some("Boss; Big".into()), ..Attendee::new("boss@example.com") });
        ev.attendees = vec![Attendee {
            name: Some("Ann".into()),
            role: Some("REQ-PARTICIPANT".into()),
            partstat: Some("ACCEPTED".into()),
            rsvp: true,
            ..Attendee::new("ann@example.com")
        }];

        let ics = to_ics(&ev);
        assert!(ics.contains("TRANSP:TRANSPARENT"));
        assert!(ics.contains("CLASS:CONFIDENTIAL"));
        assert!(ics.contains("CATEGORIES:Work,Q3\\, planning"));
        assert!(ics.contains("ORGANIZER;CN=\"Boss; Big\":mailto:boss@example.com"));
        // The ATTENDEE line exceeds 75 octets and is folded, so assert on the reparse below.
        assert!(ics.contains("ATTENDEE;CN=Ann;ROLE=REQ-PARTICIPANT;PARTSTAT=ACCEPT"));

        let p = from_ics(&ics, "work").unwrap();
        assert_eq!(p.status, ev.status);
        assert_eq!(p.transp, ev.transp);
        assert_eq!(p.class, ev.class);
        assert_eq!(p.color, ev.color);
        assert_eq!(p.url, ev.url);
        assert_eq!(p.categories, ev.categories);
        assert_eq!(p.organizer, ev.organizer);
        assert_eq!(p.attendees, ev.attendees);

        // Defaults stay off the wire, so an unremarkable event's ICS is unchanged.
        let plain = to_ics(&Event::new("work", "x", ev.start, ev.end));
        assert!(!plain.contains("TRANSP") && !plain.contains("CLASS") && !plain.contains("ATTENDEE"));
    }

    #[test]
    fn google_invite_attendees_are_imported() {
        let ics = "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:g1\r\nDTSTART:20260618T090000Z\r\nDTEND:20260618T093000Z\r\n\
                   SUMMARY:Sync\r\nORGANIZER;CN=Chair:mailto:chair@example.com\r\n\
                   ATTENDEE;CUTYPE=INDIVIDUAL;ROLE=REQ-PARTICIPANT;PARTSTAT=ACCEPTED;CN=A A;X-NUM-GUESTS=0:mailto:a@example.com\r\n\
                   ATTENDEE;ROLE=REQ-PARTICIPANT;PARTSTAT=ACCEPTED;CN=B B:MAILTO:b@example.com\r\n\
                   ATTENDEE;ROLE=OPT-PARTICIPANT;PARTSTAT=ACCEPTED;RSVP=TRUE:mailto:c@example.com\r\n\
                   CATEGORIES:meeting\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
        let ev = from_ics(ics, "work").unwrap();
        assert_eq!(ev.attendees.len(), 3);
        assert!(ev.attendees.iter().all(|a| a.partstat.as_deref() == Some("ACCEPTED")));
        assert_eq!(ev.attendees[1].email, "b@example.com"); // uppercase MAILTO:
        assert_eq!(ev.attendees[2].rsvp, true);
        assert_eq!(ev.organizer.unwrap().email, "chair@example.com");
        assert_eq!(ev.categories, vec!["meeting".to_string()]);
    }

    #[test]
    fn alarm_triggers_round_trip_and_absolute_imports() {
        let mut ev = Event::new(
            "work",
            "Review",
            Utc.with_ymd_and_hms(2026, 6, 18, 9, 0, 0).unwrap(),
            Utc.with_ymd_and_hms(2026, 6, 18, 10, 0, 0).unwrap(),
        );
        let abs = Utc.with_ymd_and_hms(2026, 6, 18, 8, 0, 0).unwrap();
        for t in [
            AlarmTrigger::MinutesBefore(15),
            AlarmTrigger::MinutesAfterStart(10),
            AlarmTrigger::MinutesBeforeEnd(5),
            AlarmTrigger::At(abs),
        ] {
            ev.alarms.push(Alarm { trigger: t, action: AlarmAction::Notify, description: None });
        }
        let ics = to_ics(&ev);
        assert!(ics.contains("TRIGGER:-PT15M"));
        assert!(ics.contains("TRIGGER:PT10M"));
        assert!(ics.contains("TRIGGER;RELATED=END:-PT5M"));
        assert!(ics.contains("TRIGGER;VALUE=DATE-TIME:20260618T080000Z"));
        let p = from_ics(&ics, "work").unwrap();
        assert_eq!(p.alarms.iter().map(|a| a.trigger).collect::<Vec<_>>(), ev.alarms.iter().map(|a| a.trigger).collect::<Vec<_>>());
        // An absolute trigger without VALUE=DATE-TIME must not degrade to "0 minutes before".
        let bare = "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:a\r\nDTSTART:20260618T090000Z\r\nDTEND:20260618T093000Z\r\n\
                    SUMMARY:x\r\nBEGIN:VALARM\r\nACTION:DISPLAY\r\nTRIGGER:20260618T080000Z\r\nEND:VALARM\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
        assert_eq!(from_ics(bare, "work").unwrap().alarms[0].trigger, AlarmTrigger::At(abs));
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
    fn exdates_and_overrides_round_trip_in_one_document() {
        let mut master = Event::new(
            "work",
            "Standup",
            Utc.with_ymd_and_hms(2026, 6, 1, 9, 0, 0).unwrap(),
            Utc.with_ymd_and_hms(2026, 6, 1, 9, 30, 0).unwrap(),
        );
        master.uid = Uid::from_string("series");
        master.rrule = Some(RecurrenceRule::every(Frequency::Daily, 1));
        master.exdates = vec![Utc.with_ymd_and_hms(2026, 6, 3, 9, 0, 0).unwrap()];
        let mut over = master.clone();
        over.rrule = None;
        over.exdates.clear();
        over.recurrence_id = Some(Utc.with_ymd_and_hms(2026, 6, 4, 9, 0, 0).unwrap());
        over.start = Utc.with_ymd_and_hms(2026, 6, 4, 14, 0, 0).unwrap();
        over.end = Utc.with_ymd_and_hms(2026, 6, 4, 14, 30, 0).unwrap();

        let doc = series_to_ics(&[master.clone(), over.clone()], false);
        assert!(doc.contains("EXDATE:20260603T090000Z"));
        assert!(doc.contains("RECURRENCE-ID:20260604T090000Z"));
        assert_eq!(doc.matches("BEGIN:VEVENT").count(), 2);

        let parsed = series_from_ics(&doc, "work").unwrap();
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].exdates, master.exdates);
        assert!(parsed[0].recurrence_id.is_none()); // master first
        assert_eq!(parsed[1].recurrence_id, over.recurrence_id);
        assert_eq!(parsed[1].start, over.start);
        // A Google-style override re-exports unchanged.
        assert_eq!(series_to_ics(&parsed, false), doc);
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
