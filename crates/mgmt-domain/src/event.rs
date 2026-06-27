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

/// When an alarm fires, relative to the event start.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AlarmTrigger {
    /// Minutes before the event start (e.g. 15 => "15 minutes before").
    MinutesBefore(i64),
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

    /// The minutes-before-start offset of this alarm's trigger.
    pub fn minutes(&self) -> i64 {
        match self.trigger {
            AlarmTrigger::MinutesBefore(m) => m,
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
    #[serde(default)]
    pub all_day: bool,
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rrule: Option<RecurrenceRule>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub alarms: Vec<Alarm>,
    #[serde(default)]
    pub status: EventStatus,
    #[serde(default)]
    pub sync: SyncMeta,
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
            all_day: false,
            start,
            end,
            rrule: None,
            alarms: Vec::new(),
            status: EventStatus::default(),
            sync: SyncMeta::default(),
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
    fn overlaps_is_half_open() {
        let e = Event::new("work", "standup", at(9, 0), at(10, 0));
        assert!(e.overlaps(at(9, 30), at(9, 45)));
        assert!(!e.overlaps(at(10, 0), at(11, 0))); // touching end does not overlap
        assert!(!e.overlaps(at(8, 0), at(9, 0))); // touching start does not overlap
    }
}
