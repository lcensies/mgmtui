//! Recurrence rules — a domain-level subset of RFC 5545 RRULE. `mgmt-ical` converts
//! between this and the textual `RRULE:` representation.

use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Deserializer, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Frequency {
    Daily,
    Weekly,
    Monthly,
    Yearly,
}

/// Days of the week, ordered Monday-first to match ISO and most calendars.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Weekday {
    #[default]
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

/// One `BYDAY` entry: a weekday with an optional ordinal (`+2MO`, `-1FR`). The ordinal is
/// only meaningful for `MONTHLY`/`YEARLY` rules.
///
/// Serde: written as `{"weekday":"Mon","ordinal":2}`; a bare `"Mon"` string still
/// deserialises (pre-change vaults and web payloads).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ByDay {
    pub weekday: Weekday,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ordinal: Option<i8>,
}

impl From<Weekday> for ByDay {
    fn from(weekday: Weekday) -> Self {
        ByDay { weekday, ordinal: None }
    }
}

impl<'de> Deserialize<'de> for ByDay {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Repr {
            Plain(Weekday),
            Full {
                weekday: Weekday,
                #[serde(default)]
                ordinal: Option<i8>,
            },
        }
        Ok(match Repr::deserialize(d)? {
            Repr::Plain(weekday) => ByDay { weekday, ordinal: None },
            Repr::Full { weekday, ordinal } => ByDay { weekday, ordinal },
        })
    }
}

/// Deserialize `until` from either a full date-time or a bare `YYYY-MM-DD` date, which is
/// taken as the end of that day in UTC (the pre-change JSON shape).
fn de_until<'de, D: Deserializer<'de>>(d: D) -> std::result::Result<Option<DateTime<Utc>>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Repr {
        Dt(DateTime<Utc>),
        Date(NaiveDate),
    }
    Ok(match Option::<Repr>::deserialize(d)? {
        None => None,
        Some(Repr::Dt(dt)) => Some(dt),
        Some(Repr::Date(d)) => Some(end_of_day(d)),
    })
}

/// 23:59:59 UTC on `date` — how a date-only `UNTIL` is interpreted (RFC 5545 §3.3.10).
pub fn end_of_day(date: NaiveDate) -> DateTime<Utc> {
    date.and_hms_opt(23, 59, 59).expect("valid time").and_utc()
}

/// A recurrence rule. `count` and `until` are mutually exclusive ends; both `None` means
/// the rule repeats forever.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecurrenceRule {
    pub freq: Frequency,
    pub interval: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub count: Option<u32>,
    #[serde(default, deserialize_with = "de_until", skip_serializing_if = "Option::is_none")]
    pub until: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub by_weekday: Vec<ByDay>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub by_monthday: Vec<i8>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub by_month: Vec<u8>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub by_setpos: Vec<i16>,
    /// Week start for `WEEKLY` expansion (RFC default `MO`).
    #[serde(default)]
    pub wkst: Weekday,
    /// RRULE parts we do not model (`BYHOUR`, `BYYEARDAY`, ...), kept verbatim so a foreign
    /// rule survives a sync round-trip.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub extra: Vec<(String, String)>,
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
            by_setpos: Vec::new(),
            wkst: Weekday::Mon,
            extra: Vec::new(),
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

    /// Pre-change payloads (`by_weekday` as bare strings, `until` as a bare date) must keep
    /// deserialising.
    #[test]
    fn legacy_json_shape_still_deserialises() {
        let r: RecurrenceRule = serde_json::from_str(
            r#"{"freq":"Weekly","interval":1,"until":"2026-06-03","by_weekday":["Mon",{"weekday":"Fri","ordinal":-1}]}"#,
        )
        .unwrap();
        assert_eq!(r.until, Some(end_of_day(NaiveDate::from_ymd_opt(2026, 6, 3).unwrap())));
        assert_eq!(r.by_weekday[0], ByDay::from(Weekday::Mon));
        assert_eq!(r.by_weekday[1].ordinal, Some(-1));
        assert_eq!(r.wkst, Weekday::Mon);
        // and the new shape round-trips
        let back: RecurrenceRule = serde_json::from_str(&serde_json::to_string(&r).unwrap()).unwrap();
        assert_eq!(back, r);
    }
}
