//! RRULE <-> [`RecurrenceRule`] conversion (RFC 5545 §3.3.10, the common subset).

use mgmt_core::{Error, Result};
use mgmt_domain::{Frequency, RecurrenceRule, Weekday};

use crate::value;

pub fn to_rrule(rule: &RecurrenceRule) -> String {
    let mut parts = vec![format!("FREQ={}", freq_token(rule.freq))];
    if rule.interval > 1 {
        parts.push(format!("INTERVAL={}", rule.interval));
    }
    if let Some(count) = rule.count {
        parts.push(format!("COUNT={count}"));
    }
    if let Some(until) = rule.until {
        // UNTIL is emitted as a UTC date-time at midnight.
        parts.push(format!("UNTIL={}T000000Z", until.format("%Y%m%d")));
    }
    if !rule.by_weekday.is_empty() {
        let days: Vec<&str> = rule.by_weekday.iter().map(|w| w.ical_token()).collect();
        parts.push(format!("BYDAY={}", days.join(",")));
    }
    if !rule.by_monthday.is_empty() {
        let days: Vec<String> = rule.by_monthday.iter().map(|d| d.to_string()).collect();
        parts.push(format!("BYMONTHDAY={}", days.join(",")));
    }
    if !rule.by_month.is_empty() {
        let months: Vec<String> = rule.by_month.iter().map(|m| m.to_string()).collect();
        parts.push(format!("BYMONTH={}", months.join(",")));
    }
    parts.join(";")
}

pub fn from_rrule(s: &str) -> Result<RecurrenceRule> {
    let mut freq = None;
    let mut interval = 1u32;
    let mut count = None;
    let mut until = None;
    let mut by_weekday = Vec::new();
    let mut by_monthday = Vec::new();
    let mut by_month = Vec::new();

    for part in s.split(';') {
        let (key, val) = part
            .split_once('=')
            .ok_or_else(|| Error::Parse(format!("bad RRULE part {part:?}")))?;
        match key.to_ascii_uppercase().as_str() {
            "FREQ" => freq = Some(parse_freq(val)?),
            "INTERVAL" => interval = val.parse().map_err(|_| Error::Parse(format!("bad INTERVAL {val:?}")))?,
            "COUNT" => count = Some(val.parse().map_err(|_| Error::Parse(format!("bad COUNT {val:?}")))?),
            "UNTIL" => until = Some(value::parse_datetime(val)?.date_naive()),
            "BYDAY" => {
                for tok in val.split(',') {
                    // strip any ordinal prefix (e.g. +2MO) — we keep the weekday only
                    let day = tok.trim_start_matches(|c: char| c == '+' || c == '-' || c.is_ascii_digit());
                    by_weekday
                        .push(Weekday::from_ical_token(day).ok_or_else(|| Error::Parse(format!("bad BYDAY {tok:?}")))?);
                }
            }
            "BYMONTHDAY" => {
                for tok in val.split(',') {
                    by_monthday.push(tok.parse().map_err(|_| Error::Parse(format!("bad BYMONTHDAY {tok:?}")))?);
                }
            }
            "BYMONTH" => {
                for tok in val.split(',') {
                    by_month.push(tok.parse().map_err(|_| Error::Parse(format!("bad BYMONTH {tok:?}")))?);
                }
            }
            _ => {} // ignore unsupported parts (BYSETPOS, WKST, ...)
        }
    }

    Ok(RecurrenceRule {
        freq: freq.ok_or_else(|| Error::Parse("RRULE missing FREQ".into()))?,
        interval,
        count,
        until,
        by_weekday,
        by_monthday,
        by_month,
    })
}

fn freq_token(f: Frequency) -> &'static str {
    match f {
        Frequency::Daily => "DAILY",
        Frequency::Weekly => "WEEKLY",
        Frequency::Monthly => "MONTHLY",
        Frequency::Yearly => "YEARLY",
    }
}

fn parse_freq(s: &str) -> Result<Frequency> {
    Ok(match s.to_ascii_uppercase().as_str() {
        "DAILY" => Frequency::Daily,
        "WEEKLY" => Frequency::Weekly,
        "MONTHLY" => Frequency::Monthly,
        "YEARLY" => Frequency::Yearly,
        other => return Err(Error::Parse(format!("unsupported FREQ {other:?}"))),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn simple_weekly_round_trips() {
        let mut r = RecurrenceRule::every(Frequency::Weekly, 2);
        r.by_weekday = vec![Weekday::Mon, Weekday::Wed];
        r.count = Some(10);
        let s = to_rrule(&r);
        assert_eq!(from_rrule(&s).unwrap(), r);
    }

    #[test]
    fn parses_ordinal_byday_dropping_prefix() {
        let r = from_rrule("FREQ=MONTHLY;BYDAY=+2MO").unwrap();
        assert_eq!(r.by_weekday, vec![Weekday::Mon]);
    }

    #[test]
    fn missing_freq_is_error() {
        assert!(from_rrule("INTERVAL=2").is_err());
    }
}
