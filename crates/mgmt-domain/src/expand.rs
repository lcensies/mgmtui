//! Recurrence expansion — turning a recurring [`Event`] into its concrete occurrences within
//! a time window. Supports the common RFC 5545 subset: `FREQ` (daily/weekly/monthly/yearly),
//! `INTERVAL`, `COUNT`, `UNTIL`, and `BYDAY` for weekly rules.

use chrono::{DateTime, Datelike, Days, Months, Utc};

use crate::{Event, Frequency, Weekday};

/// A safety cap so a malformed/endless rule can never loop forever.
const MAX_OCCURRENCES: u32 = 10_000;

impl Event {
    /// Concrete occurrences of this event overlapping the half-open window `[from, to)`.
    ///
    /// Non-recurring events yield themselves (when they overlap). Recurring events yield one
    /// clone per occurrence, each with `start`/`end` shifted and the original `uid` retained
    /// (so editing/deleting an occurrence acts on the series).
    pub fn occurrences_in(&self, from: DateTime<Utc>, to: DateTime<Utc>) -> Vec<Event> {
        let duration = self.end - self.start;
        let Some(rule) = &self.rrule else {
            return if self.overlaps(from, to) { vec![self.clone()] } else { vec![] };
        };

        let until = rule.until.and_then(|d| d.and_hms_opt(23, 59, 59)).map(|d| d.and_utc());
        let max = rule.count.unwrap_or(u32::MAX);
        let interval = rule.interval.max(1);

        let mut out = Vec::new();
        let mut produced = 0u32;

        // Weekly with BYDAY expands to several weekdays per stepped week; everything else
        // advances one anchor at a time.
        let weekdays: Vec<Weekday> = if rule.freq == Frequency::Weekly && !rule.by_weekday.is_empty() {
            let mut w = rule.by_weekday.clone();
            w.sort_by_key(|d| weekday_index(*d));
            w
        } else {
            Vec::new()
        };

        let mut anchor = self.start;
        let mut guard = 0u32;
        loop {
            guard += 1;
            if guard > MAX_OCCURRENCES || produced >= max {
                break;
            }

            // The set of occurrence-starts contributed by this step.
            let starts: Vec<DateTime<Utc>> = if weekdays.is_empty() {
                vec![anchor]
            } else {
                let monday = week_monday(anchor);
                weekdays.iter().map(|wd| monday + Days::new(weekday_index(*wd) as u64)).collect()
            };

            let mut past_window = false;
            for start in starts {
                if start < self.start {
                    continue; // before DTSTART (can happen for the first BYDAY week)
                }
                if let Some(u) = until {
                    if start > u {
                        past_window = true;
                        break;
                    }
                }
                if produced >= max {
                    break;
                }
                produced += 1;
                let end = start + duration;
                if start < to && end > from {
                    let mut e = self.clone();
                    e.start = start;
                    e.end = end;
                    out.push(e);
                }
                if start >= to {
                    past_window = true;
                }
            }
            if past_window {
                break;
            }

            anchor = match advance(anchor, rule.freq, interval) {
                Some(next) if next > anchor => next,
                _ => break,
            };
        }
        out
    }
}

fn advance(dt: DateTime<Utc>, freq: Frequency, interval: u32) -> Option<DateTime<Utc>> {
    match freq {
        Frequency::Daily => Some(dt + Days::new(interval as u64)),
        Frequency::Weekly => Some(dt + Days::new(interval as u64 * 7)),
        Frequency::Monthly => dt.checked_add_months(Months::new(interval)),
        Frequency::Yearly => dt.checked_add_months(Months::new(interval * 12)),
    }
}

fn week_monday(dt: DateTime<Utc>) -> DateTime<Utc> {
    dt - Days::new(dt.weekday().num_days_from_monday() as u64)
}

fn weekday_index(w: Weekday) -> u32 {
    match w {
        Weekday::Mon => 0,
        Weekday::Tue => 1,
        Weekday::Wed => 2,
        Weekday::Thu => 3,
        Weekday::Fri => 4,
        Weekday::Sat => 5,
        Weekday::Sun => 6,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Frequency, RecurrenceRule};
    use chrono::TimeZone;

    fn at(y: i32, mo: u32, d: u32, h: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(y, mo, d, h, 0, 0).unwrap()
    }

    fn daily_event() -> Event {
        let mut e = Event::new("c", "standup", at(2026, 6, 1, 9), at(2026, 6, 1, 9) + chrono::Duration::minutes(30));
        e.rrule = Some(RecurrenceRule::every(Frequency::Daily, 1));
        e
    }

    #[test]
    fn non_recurring_yields_itself_when_overlapping() {
        let e = Event::new("c", "x", at(2026, 6, 10, 9), at(2026, 6, 10, 10));
        assert_eq!(e.occurrences_in(at(2026, 6, 10, 0), at(2026, 6, 11, 0)).len(), 1);
        assert_eq!(e.occurrences_in(at(2026, 6, 11, 0), at(2026, 6, 12, 0)).len(), 0);
    }

    #[test]
    fn daily_expands_across_window() {
        let e = daily_event();
        let occ = e.occurrences_in(at(2026, 6, 1, 0), at(2026, 6, 8, 0));
        assert_eq!(occ.len(), 7);
        assert_eq!(occ[0].start, at(2026, 6, 1, 9));
        assert_eq!(occ[6].start, at(2026, 6, 7, 9));
    }

    #[test]
    fn count_limits_total_occurrences() {
        let mut e = daily_event();
        e.rrule.as_mut().unwrap().count = Some(3);
        let occ = e.occurrences_in(at(2026, 6, 1, 0), at(2026, 7, 1, 0));
        assert_eq!(occ.len(), 3);
    }

    #[test]
    fn until_limits_by_date() {
        let mut e = daily_event();
        e.rrule.as_mut().unwrap().until = Some(chrono::NaiveDate::from_ymd_opt(2026, 6, 3).unwrap());
        let occ = e.occurrences_in(at(2026, 6, 1, 0), at(2026, 7, 1, 0));
        assert_eq!(occ.len(), 3); // 1st, 2nd, 3rd
    }

    #[test]
    fn weekly_byday_expands_multiple_weekdays() {
        // Mon 2026-06-01; recur weekly on Mon + Wed
        let mut e = Event::new("c", "class", at(2026, 6, 1, 9), at(2026, 6, 1, 10));
        let mut r = RecurrenceRule::every(Frequency::Weekly, 1);
        r.by_weekday = vec![Weekday::Mon, Weekday::Wed];
        e.rrule = Some(r);
        let occ = e.occurrences_in(at(2026, 6, 1, 0), at(2026, 6, 15, 0));
        // Mon 1, Wed 3, Mon 8, Wed 10 => 4 within two weeks
        let days: Vec<u32> = occ.iter().map(|o| o.start.day()).collect();
        assert_eq!(days, vec![1, 3, 8, 10]);
    }

    #[test]
    fn monthly_keeps_day_of_month() {
        let mut e = Event::new("c", "rent", at(2026, 1, 15, 9), at(2026, 1, 15, 10));
        e.rrule = Some(RecurrenceRule::every(Frequency::Monthly, 1));
        let occ = e.occurrences_in(at(2026, 1, 1, 0), at(2026, 4, 1, 0));
        let months: Vec<u32> = occ.iter().map(|o| o.start.month()).collect();
        assert_eq!(months, vec![1, 2, 3]);
        assert!(occ.iter().all(|o| o.start.day() == 15));
    }
}
