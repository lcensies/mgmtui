//! Calendar events (the iCalendar `VEVENT` side). Times are held in UTC; all-day events
//! are anchored at UTC midnight and flagged via `all_day`.

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};

use mgmt_core::Uid;

use crate::{RecurrenceRule, SyncMeta};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EventStatus {
    Confirmed,
    Tentative,
    Cancelled,
}

impl Default for EventStatus {
    fn default() -> Self {
        EventStatus::Confirmed
    }
}

/// Busy/free transparency (iCalendar `TRANSP`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Transparency {
    /// Blocks time (busy) — the default.
    #[default]
    Opaque,
    /// Does not block time (free).
    Transparent,
}

/// Access classification (iCalendar `CLASS`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Classification {
    #[default]
    Public,
    Private,
    Confidential,
}

/// A calendar user on an event (`ORGANIZER`/`ATTENDEE`). Stored and synced only — mgmt never
/// sends invitations or RSVPs.
///
/// ponytail: `role`/`partstat` stay raw iCalendar tokens (`REQ-PARTICIPANT`, `ACCEPTED`) rather
/// than enums. RFC 5545 allows x-name values there, so a String round-trips foreign servers
/// losslessly with no mapping table; upgrade to enums only once a surface must reason about them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Attendee {
    pub email: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub partstat: Option<String>,
    #[serde(default)]
    pub rsvp: bool,
}

impl Attendee {
    /// An attendee with just an address (no name, default role/partstat).
    pub fn new(email: impl Into<String>) -> Self {
        Attendee { email: email.into(), ..Default::default() }
    }
}

/// When an alarm fires, relative to the event (or at an absolute instant).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AlarmTrigger {
    /// Minutes before the event start (e.g. 15 => "15 minutes before").
    MinutesBefore(i64),
    /// Minutes after the event start.
    MinutesAfterStart(i64),
    /// Minutes before the event end (`TRIGGER;RELATED=END`). Negative means after the end.
    MinutesBeforeEnd(i64),
    /// An absolute instant (`TRIGGER;VALUE=DATE-TIME`).
    At(DateTime<Utc>),
}

/// What an alarm *does* when it fires. The default ([`AlarmAction::Notify`]) is a plain desktop
/// notification — the historical behaviour. The other variants let an event drive an arbitrary
/// side effect: bring mgmt to the event, or run a custom binary (a user hook).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum AlarmAction {
    /// Desktop notification only.
    #[default]
    Notify,
    /// Bring mgmt to this event — focus a running instance or open a fresh one — alongside the
    /// desktop notification. The concrete focusing mechanism lives in the firing layer.
    Navigate,
    /// Run an external command when the alarm fires. The firing layer expands `{uid}`,
    /// `{summary}`, `{start}`, `{end}`, `{location}`, `{calendar}`, and `{minutes}` placeholders
    /// in `command`/`args` against the event before spawning it.
    Run { command: String, args: Vec<String> },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Alarm {
    pub trigger: AlarmTrigger,
    /// What happens when the alarm fires. Defaults to a plain notification.
    #[serde(default)]
    pub action: AlarmAction,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

impl Alarm {
    /// A plain desktop-notification alarm firing `minutes` before the event start.
    pub fn minutes_before(minutes: i64) -> Self {
        Alarm { trigger: AlarmTrigger::MinutesBefore(minutes), action: AlarmAction::Notify, description: None }
    }

    /// An alarm firing `minutes` before the start with an explicit [`AlarmAction`].
    pub fn with_action(minutes: i64, action: AlarmAction) -> Self {
        Alarm { trigger: AlarmTrigger::MinutesBefore(minutes), action, description: None }
    }

    /// The minutes-before-start offset of this alarm's trigger. Only meaningful for
    /// [`AlarmTrigger::MinutesBefore`]; every other trigger reports 0 — use [`Alarm::fire_at`],
    /// which is exact for all of them.
    pub fn minutes(&self) -> i64 {
        match self.trigger {
            AlarmTrigger::MinutesBefore(m) => m,
            _ => 0,
        }
    }

    /// The instant this alarm fires for `ev`.
    pub fn fire_at(&self, ev: &Event) -> DateTime<Utc> {
        match self.trigger {
            AlarmTrigger::MinutesBefore(m) => ev.start - Duration::minutes(m),
            AlarmTrigger::MinutesAfterStart(m) => ev.start + Duration::minutes(m),
            AlarmTrigger::MinutesBeforeEnd(m) => ev.end - Duration::minutes(m),
            AlarmTrigger::At(t) => t,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Event {
    pub uid: Uid,
    /// Local collection this event belongs to (e.g. "work", "personal").
    pub calendar: String,
    pub summary: String,
    /// Project this event is bound to (shares the task project registry & colors). Stored in
    /// the `.ics` as `X-MGMT-PROJECT`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location: Option<String>,
    /// Video-conference join URL (Yandex Telemost, Google Meet, Jitsi, …). Maps to the iCalendar
    /// `CONFERENCE` property (RFC 7986); also read from Google's `X-GOOGLE-CONFERENCE` and, as a
    /// last resort, sniffed out of the description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conference_url: Option<String>,
    #[serde(default)]
    pub all_day: bool,
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rrule: Option<RecurrenceRule>,
    /// Occurrence starts excluded from this series (`EXDATE`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exdates: Vec<DateTime<Utc>>,
    /// Set on an *override*: this event replaces the single occurrence of its series that would
    /// start at this instant (`RECURRENCE-ID`). Overrides share the master's `uid`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recurrence_id: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub alarms: Vec<Alarm>,
    #[serde(default)]
    pub status: EventStatus,
    /// Last time this event was mutated (iCalendar `LAST-MODIFIED`). Stamped by the service layer
    /// on every change; the last-write-wins tiebreaker for concurrent edits across synced nodes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub modified: Option<DateTime<Utc>>,
    #[serde(default)]
    pub sync: SyncMeta,
    /// Busy/free (`TRANSP`).
    #[serde(default)]
    pub transp: Transparency,
    /// Visibility (`CLASS`).
    #[serde(default)]
    pub class: Classification,
    /// Per-event color (RFC 7986 `COLOR`); wins over the calendar/project color when set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    /// Associated web page (`URL`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub categories: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub organizer: Option<Attendee>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attendees: Vec<Attendee>,
}

impl Event {
    /// Construct a minimal timed event spanning `start..end`.
    pub fn new(calendar: impl Into<String>, summary: impl Into<String>, start: DateTime<Utc>, end: DateTime<Utc>) -> Self {
        Event {
            uid: Uid::new(),
            calendar: calendar.into(),
            summary: summary.into(),
            project: None,
            description: None,
            location: None,
            conference_url: None,
            all_day: false,
            start,
            end,
            rrule: None,
            exdates: Vec::new(),
            recurrence_id: None,
            alarms: Vec::new(),
            status: EventStatus::default(),
            modified: None,
            sync: SyncMeta::default(),
            transp: Transparency::default(),
            class: Classification::default(),
            color: None,
            url: None,
            categories: Vec::new(),
            organizer: None,
            attendees: Vec::new(),
        }
    }

    /// Event length. Always non-negative for well-formed events.
    pub fn duration(&self) -> Duration {
        self.end - self.start
    }

    /// Move the whole event by `delta`, preserving its duration. This is what the TUI's
    /// "move event up/down" reschedule action calls; positive shifts later in time.
    pub fn shift(&mut self, delta: Duration) {
        self.start += delta;
        self.end += delta;
    }

    /// Whether the event overlaps the half-open instant range `[from, to)`.
    pub fn overlaps(&self, from: DateTime<Utc>, to: DateTime<Utc>) -> bool {
        self.start < to && self.end > from
    }

    /// Case-insensitive substring match against the summary (and location). Drives calendar
    /// search.
    pub fn matches_text(&self, query: &str) -> bool {
        let q = query.to_lowercase();
        self.summary.to_lowercase().contains(&q)
            || self.location.as_deref().map(|l| l.to_lowercase().contains(&q)).unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn at(h: u32, m: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 6, 18, h, m, 0).unwrap()
    }

    #[test]
    fn shift_preserves_duration_and_moves_both_ends() {
        let mut e = Event::new("work", "standup", at(9, 0), at(9, 30));
        let original = e.duration();
        e.shift(Duration::minutes(30));
        assert_eq!(e.start, at(9, 30));
        assert_eq!(e.end, at(10, 0));
        assert_eq!(e.duration(), original);
    }

    #[test]
    fn negative_shift_moves_earlier() {
        let mut e = Event::new("work", "standup", at(9, 0), at(9, 30));
        e.shift(Duration::minutes(-15));
        assert_eq!(e.start, at(8, 45));
    }

    #[test]
    fn fire_at_covers_every_trigger_kind() {
        let e = Event::new("work", "standup", at(9, 0), at(10, 0));
        let f = |tr| Alarm { trigger: tr, action: AlarmAction::Notify, description: None }.fire_at(&e);
        assert_eq!(f(AlarmTrigger::MinutesBefore(15)), at(8, 45));
        assert_eq!(f(AlarmTrigger::MinutesAfterStart(10)), at(9, 10));
        assert_eq!(f(AlarmTrigger::MinutesBeforeEnd(5)), at(9, 55));
        assert_eq!(f(AlarmTrigger::At(at(7, 0))), at(7, 0));
    }

    #[test]
    fn new_event_fields_default_and_are_optional_in_json() {
        // Every added field is `serde(default)`, so pre-change JSON still deserialises.
        let json = r#"{"uid":"u","calendar":"work","summary":"s","all_day":false,
            "start":"2026-06-18T09:00:00Z","end":"2026-06-18T09:30:00Z"}"#;
        let e: Event = serde_json::from_str(json).unwrap();
        assert_eq!(e.transp, Transparency::Opaque);
        assert_eq!(e.class, Classification::Public);
        assert!(e.categories.is_empty() && e.attendees.is_empty() && e.color.is_none());
    }

    #[test]
    fn overlaps_is_half_open() {
        let e = Event::new("work", "standup", at(9, 0), at(10, 0));
        assert!(e.overlaps(at(9, 30), at(9, 45)));
        assert!(!e.overlaps(at(10, 0), at(11, 0))); // touching end does not overlap
        assert!(!e.overlaps(at(8, 0), at(9, 0))); // touching start does not overlap
    }
}
