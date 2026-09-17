//! RRULE <-> [`RecurrenceRule`] conversion (RFC 5545 §3.3.10, the common subset).

use mgmt_core::{Error, Result};
use mgmt_domain::{end_of_day, ByDay, Frequency, RecurrenceRule, Weekday};

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
        parts.push(format!("UNTIL={}", value::format_datetime(until)));
    }
    if !rule.by_weekday.is_empty() {
        let days: Vec<String> = rule.by_weekday.iter().map(byday_token).collect();
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
    if !rule.by_setpos.is_empty() {
        let pos: Vec<String> = rule.by_setpos.iter().map(|p| p.to_string()).collect();
        parts.push(format!("BYSETPOS={}", pos.join(",")));
    }
    if rule.wkst != Weekday::Mon {
        parts.push(format!("WKST={}", rule.wkst.ical_token()));
    }
    parts.extend(rule.extra.iter().map(|(k, v)| format!("{k}={v}")));
    parts.join(";")
}

pub fn from_rrule(s: &str) -> Result<RecurrenceRule> {
    let mut rule = RecurrenceRule::every(Frequency::Daily, 1);
    let mut freq = None;

    for part in s.split(';') {
        let (key, val) = part
            .split_once('=')
            .ok_or_else(|| Error::Parse(format!("bad RRULE part {part:?}")))?;
        let key = key.to_ascii_uppercase();
        match key.as_str() {
            "FREQ" => freq = Some(parse_freq(val)?),
            "INTERVAL" => {
                rule.interval = val.parse().map_err(|_| Error::Parse(format!("bad INTERVAL {val:?}")))?
            }
            "COUNT" => rule.count = Some(val.parse().map_err(|_| Error::Parse(format!("bad COUNT {val:?}")))?),
            "UNTIL" => rule.until = Some(parse_until(val)?),
            "BYDAY" => {
                for tok in val.split(',') {
                    rule.by_weekday
                        .push(parse_byday(tok).ok_or_else(|| Error::Parse(format!("bad BYDAY {tok:?}")))?);
                }
            }
            "BYMONTHDAY" => {
                for tok in val.split(',') {
                    rule.by_monthday
                        .push(tok.parse().map_err(|_| Error::Parse(format!("bad BYMONTHDAY {tok:?}")))?);
                }
            }
            "BYMONTH" => {
                for tok in val.split(',') {
                    rule.by_month.push(tok.parse().map_err(|_| Error::Parse(format!("bad BYMONTH {tok:?}")))?);
                }
            }
            "BYSETPOS" => {
                for tok in val.split(',') {
                    rule.by_setpos
                        .push(tok.parse().map_err(|_| Error::Parse(format!("bad BYSETPOS {tok:?}")))?);
                }
            }
            "WKST" => {
                rule.wkst =
                    Weekday::from_ical_token(&val.to_ascii_uppercase()).ok_or_else(|| Error::Parse(format!("bad WKST {val:?}")))?
            }
            // Parts we do not model ride along verbatim so foreign rules survive a round-trip.
            _ => rule.extra.push((key, val.to_string())),
        }
    }

    rule.freq = freq.ok_or_else(|| Error::Parse("RRULE missing FREQ".into()))?;
    Ok(rule)
}

/// `UNTIL` is a UTC date-time; a date-only value means the end of that day.
fn parse_until(val: &str) -> Result<chrono::DateTime<chrono::Utc>> {
    match value::parse_datetime(val) {
        Ok(dt) => Ok(dt),
        Err(e) => value::parse_date(val).map(|d| end_of_day(d.date_naive())).map_err(|_| e),
    }
}

fn byday_token(bd: &ByDay) -> String {
    match bd.ordinal {
        Some(n) => format!("{n}{}", bd.weekday.ical_token()),
        None => bd.weekday.ical_token().to_string(),
    }
}

fn parse_byday(tok: &str) -> Option<ByDay> {
    let tok = tok.trim().to_ascii_uppercase();
    let split = tok.len().checked_sub(2)?;
    let (prefix, day) = tok.split_at(split);
    let weekday = Weekday::from_ical_token(day)?;
    let ordinal = match prefix {
        "" => None,
        p => Some(p.trim_start_matches('+').parse().ok()?),
    };
    Some(ByDay { weekday, ordinal })
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
    use chrono::{TimeZone, Utc};

    #[test]
    fn simple_weekly_round_trips() {
        let mut r = RecurrenceRule::every(Frequency::Weekly, 2);
        r.by_weekday = vec![Weekday::Mon.into(), Weekday::Wed.into()];
        r.count = Some(10);
        let s = to_rrule(&r);
        assert_eq!(s, "FREQ=WEEKLY;INTERVAL=2;COUNT=10;BYDAY=MO,WE");
        assert_eq!(from_rrule(&s).unwrap(), r);
    }

    /// Google/Yandex samples keep their ordinals, set positions and week start.
    #[test]
    fn ordinals_setpos_and_wkst_round_trip() {
        for s in [
            "FREQ=MONTHLY;BYDAY=2MO",
            "FREQ=MONTHLY;BYDAY=-1FR",
            "FREQ=MONTHLY;BYDAY=MO,TU,WE,TH,FR;BYSETPOS=-1",
            "FREQ=YEARLY;BYMONTHDAY=1;BYMONTH=3,9",
            "FREQ=WEEKLY;INTERVAL=2;BYDAY=SU,MO;WKST=SU",
        ] {
            assert_eq!(to_rrule(&from_rrule(s).unwrap()), s, "round-trip of {s:?}");
        }
        // Parts are re-emitted in RFC 5545 recur-rule-part order, so an input ordered otherwise
        // normalises instead of round-tripping byte-for-byte.
        assert_eq!(
            to_rrule(&from_rrule("FREQ=YEARLY;BYMONTH=3,9;BYMONTHDAY=1").unwrap()),
            "FREQ=YEARLY;BYMONTHDAY=1;BYMONTH=3,9"
        );
        assert_eq!(from_rrule("FREQ=MONTHLY;BYDAY=+2MO").unwrap().by_weekday[0].ordinal, Some(2));
    }

    /// UNTIL carries the full time; a date-only value means the end of that day.
    #[test]
    fn until_is_a_full_instant() {
        let r = from_rrule("FREQ=DAILY;UNTIL=20260603").unwrap();
        assert_eq!(r.until, Some(Utc.with_ymd_and_hms(2026, 6, 3, 23, 59, 59).unwrap()));
        assert_eq!(to_rrule(&r), "FREQ=DAILY;UNTIL=20260603T235959Z");
        assert_eq!(
            from_rrule("FREQ=DAILY;UNTIL=20260603T170000Z").unwrap().until,
            Some(Utc.with_ymd_and_hms(2026, 6, 3, 17, 0, 0).unwrap())
        );
    }

    /// Parts we do not model survive the round-trip verbatim.
    #[test]
    fn unknown_parts_pass_through() {
        let r = from_rrule("FREQ=DAILY;BYHOUR=9").unwrap();
        assert_eq!(r.extra, vec![("BYHOUR".to_string(), "9".to_string())]);
        assert_eq!(to_rrule(&r), "FREQ=DAILY;BYHOUR=9");
    }

    #[test]
    fn missing_freq_is_error() {
        assert!(from_rrule("INTERVAL=2").is_err());
        assert!(from_rrule("FREQ=DAILY;BYDAY=XX").is_err());
    }
}
