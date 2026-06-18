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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Alarm {
    pub trigger: AlarmTrigger,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Event {
    pub uid: Uid,
    /// Local collection this event belongs to (e.g. "work", "personal").
    pub calendar: String,
    pub summary: String,
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
