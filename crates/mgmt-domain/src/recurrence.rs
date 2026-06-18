//! Recurrence rules — a domain-level subset of RFC 5545 RRULE. `mgmt-ical` converts
//! between this and the textual `RRULE:` representation.

use chrono::NaiveDate;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Frequency {
    Daily,
    Weekly,
    Monthly,
    Yearly,
}

/// Days of the week, ordered Monday-first to match ISO and most calendars.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Weekday {
    Mon,
    Tue,
    Wed,
    Thu,
    Fri,
    Sat,
    Sun,
}

impl Weekday {
    /// The two-letter iCalendar token (`MO`, `TU`, ...).
    pub fn ical_token(self) -> &'static str {
        match self {
            Weekday::Mon => "MO",
            Weekday::Tue => "TU",
            Weekday::Wed => "WE",
            Weekday::Thu => "TH",
            Weekday::Fri => "FR",
            Weekday::Sat => "SA",
            Weekday::Sun => "SU",
        }
    }

    pub fn from_ical_token(s: &str) -> Option<Self> {
        Some(match s {
            "MO" => Weekday::Mon,
            "TU" => Weekday::Tue,
            "WE" => Weekday::Wed,
            "TH" => Weekday::Thu,
            "FR" => Weekday::Fri,
            "SA" => Weekday::Sat,
            "SU" => Weekday::Sun,
            _ => return None,
        })
    }
}

/// A recurrence rule. `count` and `until` are mutually exclusive ends; both `None` means
/// the rule repeats forever.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecurrenceRule {
    pub freq: Frequency,
    pub interval: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub count: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub until: Option<NaiveDate>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub by_weekday: Vec<Weekday>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub by_monthday: Vec<i8>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub by_month: Vec<u8>,
}

impl RecurrenceRule {
    /// A simple "every N <freq>" rule with no BY* parts.
    pub fn every(freq: Frequency, interval: u32) -> Self {
        RecurrenceRule {
            freq,
            interval: interval.max(1),
            count: None,
            until: None,
            by_weekday: Vec::new(),
            by_monthday: Vec::new(),
            by_month: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn weekday_token_round_trips() {
        for wd in [
            Weekday::Mon,
            Weekday::Tue,
            Weekday::Wed,
            Weekday::Thu,
            Weekday::Fri,
            Weekday::Sat,
            Weekday::Sun,
        ] {
            assert_eq!(Weekday::from_ical_token(wd.ical_token()), Some(wd));
        }
    }

    #[test]
    fn every_clamps_zero_interval_to_one() {
        assert_eq!(RecurrenceRule::every(Frequency::Daily, 0).interval, 1);
    }
}
