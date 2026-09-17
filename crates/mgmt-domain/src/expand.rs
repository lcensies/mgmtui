//! Recurrence expansion — turning a recurring [`Event`] into its concrete occurrences within
//! a time window. RFC 5545 set generation: each period (day/week/month/year) contributes a
//! candidate set built from `BYMONTH` → `BYMONTHDAY` → `BYDAY` → `BYSETPOS`, then `DTSTART`,
//! `COUNT` and `UNTIL` trim it.
//!
//! ponytail: the BY* parts we do not model (`BYHOUR`, `BYMINUTE`, `BYYEARDAY`, `BYWEEKNO`) are
//! carried verbatim in `RecurrenceRule::extra` for round-tripping but ignored when expanding.
//! Upgrade path: extend `candidates`/`period_starts` — the set-generation shape already fits.

use chrono::{DateTime, Datelike, Days, Months, NaiveDate, NaiveTime, Utc};

use crate::{ByDay, Event, Frequency, RecurrenceRule, Weekday};

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

        let max = rule.count.unwrap_or(u32::MAX);
        let time = self.start.time();
        let dtstart = self.start.date_naive();

        let mut out = Vec::new();
        let mut produced = 0u32;
        let mut period = period_start(dtstart, rule);
        let mut guard = 0u32;

        'outer: while produced < max && guard < MAX_OCCURRENCES && period <= to.date_naive() {
            guard += 1;
            for start in set_for_period(period, rule, dtstart, time) {
                if start < self.start {
                    continue; // before DTSTART
                }
                if rule.until.is_some_and(|u| start > u) {
                    break 'outer;
                }
                if produced >= max {
                    break 'outer;
                }
                produced += 1;
                let end = start + duration;
                if start < to && end > from {
                    let mut e = self.clone();
                    e.start = start;
                    e.end = end;
                    out.push(e);
                }
            }
            period = match next_period(period, rule) {
                Some(next) if next > period => next,
                _ => break,
            };
        }
        out
    }
}

/// The start of the period containing `date`: the day, the WKST-aligned week, the 1st of the
/// month, or Jan 1.
fn period_start(date: NaiveDate, rule: &RecurrenceRule) -> NaiveDate {
    match rule.freq {
        Frequency::Daily => date,
        Frequency::Weekly => {
            let back = (weekday_index(from_chrono(date.weekday())) + 7 - weekday_index(rule.wkst)) % 7;
            date - Days::new(back as u64)
        }
        Frequency::Monthly => date.with_day(1).expect("day 1 exists"),
        Frequency::Yearly => NaiveDate::from_ymd_opt(date.year(), 1, 1).expect("Jan 1 exists"),
    }
}

fn next_period(period: NaiveDate, rule: &RecurrenceRule) -> Option<NaiveDate> {
    let n = rule.interval.max(1);
    match rule.freq {
        Frequency::Daily => period.checked_add_days(Days::new(n as u64)),
        Frequency::Weekly => period.checked_add_days(Days::new(n as u64 * 7)),
        Frequency::Monthly => period.checked_add_months(Months::new(n)),
        Frequency::Yearly => period.checked_add_months(Months::new(n * 12)),
    }
}

/// The occurrence starts contributed by one period, sorted, after `BYSETPOS`.
fn set_for_period(
    period: NaiveDate,
    rule: &RecurrenceRule,
    dtstart: NaiveDate,
    time: NaiveTime,
) -> Vec<DateTime<Utc>> {
    let mut days = candidates(period, rule, dtstart);
    days.sort_unstable();
    days.dedup();
    if !rule.by_setpos.is_empty() {
        days = rule
            .by_setpos
            .iter()
            .filter_map(|p| pick(&days, *p))
            .collect();
        days.sort_unstable();
        days.dedup();
    }
    days.into_iter().map(|d| d.and_time(time).and_utc()).collect()
}

fn candidates(period: NaiveDate, rule: &RecurrenceRule, dtstart: NaiveDate) -> Vec<NaiveDate> {
    match rule.freq {
        Frequency::Daily => {
            let d = period;
            let ok = month_ok(d, rule)
                && (rule.by_monthday.is_empty() || monthdays(d.year(), d.month(), &rule.by_monthday).contains(&d))
                && (rule.by_weekday.is_empty() || weekday_listed(d, &rule.by_weekday));
            if ok { vec![d] } else { vec![] }
        }
        Frequency::Weekly => {
            let wanted: Vec<Weekday> = if rule.by_weekday.is_empty() {
                vec![from_chrono(dtstart.weekday())]
            } else {
                rule.by_weekday.iter().map(|b| b.weekday).collect()
            };
            (0..7)
                .filter_map(|i| period.checked_add_days(Days::new(i)))
                .filter(|d| wanted.contains(&from_chrono(d.weekday())) && month_ok(*d, rule))
                .collect()
        }
        Frequency::Monthly => {
            if !month_ok(period, rule) {
                return vec![];
            }
            month_set(period.year(), period.month(), rule, dtstart)
        }
        Frequency::Yearly => {
            let year = period.year();
            let months: Vec<u32> = if rule.by_month.is_empty() {
                vec![dtstart.month()]
            } else {
                rule.by_month.iter().map(|m| *m as u32).collect()
            };
            // BYDAY ordinals count within the year when no BYMONTH narrows the rule.
            if rule.by_monthday.is_empty() && !rule.by_weekday.is_empty() && rule.by_month.is_empty() {
                let (first, last) = (
                    NaiveDate::from_ymd_opt(year, 1, 1).expect("Jan 1"),
                    NaiveDate::from_ymd_opt(year, 12, 31).expect("Dec 31"),
                );
                return byday_set(first, last, &rule.by_weekday);
            }
            months
                .into_iter()
                .flat_map(|m| month_set(year, m, rule, dtstart))
                .collect()
        }
    }
}

/// Candidate days inside one month. When no BY* part applies the fallback is the DTSTART
/// day-of-month, which makes a day 29–31 rule skip short months.
fn month_set(year: i32, month: u32, rule: &RecurrenceRule, dtstart: NaiveDate) -> Vec<NaiveDate> {
    let Some(first) = NaiveDate::from_ymd_opt(year, month, 1) else {
        return vec![];
    };
    let last = first.checked_add_months(Months::new(1)).and_then(|d| d.pred_opt()).expect("month end");

    if !rule.by_monthday.is_empty() {
        let days = monthdays(year, month, &rule.by_monthday);
        return if rule.by_weekday.is_empty() {
            days
        } else {
            days.into_iter().filter(|d| weekday_listed(*d, &rule.by_weekday)).collect()
        };
    }
    if !rule.by_weekday.is_empty() {
        return byday_set(first, last, &rule.by_weekday);
    }
    monthdays(year, month, &[dtstart.day() as i8])
}

/// Resolve `BYDAY` entries (ordinals counted inside `[first, last]`) to concrete dates.
fn byday_set(first: NaiveDate, last: NaiveDate, by_weekday: &[ByDay]) -> Vec<NaiveDate> {
    by_weekday
        .iter()
        .flat_map(|bd| {
            let all = weekday_dates(first, last, bd.weekday);
            match bd.ordinal {
                Some(n) => pick(&all, n as i16).into_iter().collect::<Vec<_>>(),
                None => all,
            }
        })
        .collect()
}

/// Resolve `BYMONTHDAY` values (negatives count back from the month's end); out-of-range days
/// are skipped, never shifted.
fn monthdays(year: i32, month: u32, days: &[i8]) -> Vec<NaiveDate> {
    let len = NaiveDate::from_ymd_opt(year, month, 1)
        .and_then(|d| d.checked_add_months(Months::new(1)))
        .and_then(|d| d.pred_opt())
        .map(|d| d.day())
        .unwrap_or(0) as i32;
    days.iter()
        .filter_map(|d| {
            let day = if *d < 0 { len + 1 + *d as i32 } else { *d as i32 };
            u32::try_from(day).ok().and_then(|day| NaiveDate::from_ymd_opt(year, month, day))
        })
        .collect()
}

fn weekday_dates(first: NaiveDate, last: NaiveDate, wd: Weekday) -> Vec<NaiveDate> {
    let shift = (weekday_index(wd) + 7 - weekday_index(from_chrono(first.weekday()))) % 7;
    let mut d = first + Days::new(shift as u64);
    let mut out = Vec::new();
    while d <= last {
        out.push(d);
        d = d + Days::new(7);
    }
    out
}

/// 1-based positive / -1-based negative index into a set (`BYSETPOS`, `BYDAY` ordinals).
fn pick<T: Copy>(set: &[T], pos: i16) -> Option<T> {
    let i = if pos > 0 {
        (pos as isize) - 1
    } else if pos < 0 {
        set.len() as isize + pos as isize
    } else {
        return None;
    };
    usize::try_from(i).ok().and_then(|i| set.get(i)).copied()
}

fn month_ok(d: NaiveDate, rule: &RecurrenceRule) -> bool {
    rule.by_month.is_empty() || rule.by_month.contains(&(d.month() as u8))
}

fn weekday_listed(d: NaiveDate, by_weekday: &[ByDay]) -> bool {
    by_weekday.iter().any(|b| b.weekday == from_chrono(d.weekday()))
}

fn from_chrono(w: chrono::Weekday) -> Weekday {
    match w {
        chrono::Weekday::Mon => Weekday::Mon,
        chrono::Weekday::Tue => Weekday::Tue,
        chrono::Weekday::Wed => Weekday::Wed,
        chrono::Weekday::Thu => Weekday::Thu,
        chrono::Weekday::Fri => Weekday::Fri,
        chrono::Weekday::Sat => Weekday::Sat,
        chrono::Weekday::Sun => Weekday::Sun,
    }
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
    use crate::{end_of_day, Frequency, RecurrenceRule};
    use chrono::TimeZone;

    fn at(y: i32, mo: u32, d: u32, h: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(y, mo, d, h, 0, 0).unwrap()
    }

    fn daily_event() -> Event {
        let mut e = Event::new("c", "standup", at(2026, 6, 1, 9), at(2026, 6, 1, 9) + chrono::Duration::minutes(30));
        e.rrule = Some(RecurrenceRule::every(Frequency::Daily, 1));
        e
    }

    /// An event at `start` with `rule`, expanded over `[from, to)`, as `(month, day)` pairs.
    fn days(rule: RecurrenceRule, start: DateTime<Utc>, from: DateTime<Utc>, to: DateTime<Utc>) -> Vec<(u32, u32)> {
        let mut e = Event::new("c", "x", start, start + chrono::Duration::hours(1));
        e.rrule = Some(rule);
        e.occurrences_in(from, to).iter().map(|o| (o.start.month(), o.start.day())).collect()
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
        assert_eq!(e.occurrences_in(at(2026, 6, 1, 0), at(2026, 7, 1, 0)).len(), 3);
    }

    #[test]
    fn weekly_byday_expands_multiple_weekdays() {
        let mut r = RecurrenceRule::every(Frequency::Weekly, 1);
        r.by_weekday = vec![Weekday::Mon.into(), Weekday::Wed.into()];
        assert_eq!(
            days(r, at(2026, 6, 1, 9), at(2026, 6, 1, 0), at(2026, 6, 15, 0)),
            vec![(6, 1), (6, 3), (6, 8), (6, 10)]
        );
    }

    #[test]
    fn monthly_keeps_day_of_month() {
        let r = RecurrenceRule::every(Frequency::Monthly, 1);
        assert_eq!(
            days(r, at(2026, 1, 15, 9), at(2026, 1, 1, 0), at(2026, 4, 1, 0)),
            vec![(1, 15), (2, 15), (3, 15)]
        );
    }

    // --- spec: recurrence-engine ---

    /// Ordinal BYDAY in monthly rules: second Monday of each month.
    #[test]
    fn second_monday_of_each_month() {
        let mut r = RecurrenceRule::every(Frequency::Monthly, 1);
        r.by_weekday = vec![ByDay { weekday: Weekday::Mon, ordinal: Some(2) }];
        assert_eq!(
            days(r, at(2026, 6, 8, 9), at(2026, 6, 1, 0), at(2026, 9, 1, 0)),
            vec![(6, 8), (7, 13), (8, 10)]
        );
    }

    /// BYMONTHDAY=-1 is the last day of every month, February included.
    #[test]
    fn last_day_of_month() {
        let mut r = RecurrenceRule::every(Frequency::Monthly, 1);
        r.by_monthday = vec![-1];
        assert_eq!(
            days(r, at(2026, 1, 31, 9), at(2026, 1, 1, 0), at(2026, 5, 1, 0)),
            vec![(1, 31), (2, 28), (3, 31), (4, 30)]
        );
        // leap year
        let mut r = RecurrenceRule::every(Frequency::Monthly, 1);
        r.by_monthday = vec![-1];
        assert_eq!(days(r, at(2028, 2, 29, 9), at(2028, 2, 1, 0), at(2028, 3, 1, 0)), vec![(2, 29)]);
    }

    /// BYSETPOS=-1 over the weekdays picks exactly one day per month: its last weekday.
    #[test]
    fn last_weekday_of_month_via_setpos() {
        let mut r = RecurrenceRule::every(Frequency::Monthly, 1);
        r.by_weekday = [Weekday::Mon, Weekday::Tue, Weekday::Wed, Weekday::Thu, Weekday::Fri]
            .map(ByDay::from)
            .to_vec();
        r.by_setpos = vec![-1];
        // May 2026 ends Sun 31 -> Fri 29; June ends Tue 30; July ends Fri 31.
        assert_eq!(
            days(r, at(2026, 5, 29, 9), at(2026, 5, 1, 0), at(2026, 8, 1, 0)),
            vec![(5, 29), (6, 30), (7, 31)]
        );
    }

    /// Yearly rules restricted to given months.
    #[test]
    fn yearly_on_specific_months() {
        let mut r = RecurrenceRule::every(Frequency::Yearly, 1);
        r.by_month = vec![3, 9];
        r.by_monthday = vec![1];
        assert_eq!(
            days(r, at(2026, 3, 1, 9), at(2026, 1, 1, 0), at(2027, 4, 1, 0)),
            vec![(3, 1), (9, 1), (3, 1)]
        );
    }

    /// A monthly rule anchored on the 31st skips months without a 31st.
    #[test]
    fn thirty_first_skips_february() {
        let r = RecurrenceRule::every(Frequency::Monthly, 1);
        assert_eq!(
            days(r, at(2026, 1, 31, 9), at(2026, 1, 1, 0), at(2026, 4, 1, 0)),
            vec![(1, 31), (3, 31)]
        );
    }

    /// A date-only UNTIL covers the whole last day.
    #[test]
    fn until_includes_the_last_day() {
        let mut e = daily_event();
        e.rrule.as_mut().unwrap().until = Some(end_of_day(NaiveDate::from_ymd_opt(2026, 6, 3).unwrap()));
        let occ = e.occurrences_in(at(2026, 6, 1, 0), at(2026, 7, 1, 0));
        assert_eq!(occ.len(), 3);
        assert_eq!(occ.last().unwrap().start, at(2026, 6, 3, 9));
    }

    /// WKST shifts which days a multi-week interval groups together: with INTERVAL=2 and a
    /// Sunday start, Sun 2026-06-07 and the following Mon belong to the same (skipped) week
    /// under WKST=SU but to different weeks under WKST=MO.
    #[test]
    fn wkst_changes_biweekly_grouping() {
        let mut r = RecurrenceRule::every(Frequency::Weekly, 2);
        r.by_weekday = vec![Weekday::Sun.into(), Weekday::Mon.into()];
        r.wkst = Weekday::Sun;
        assert_eq!(
            days(r.clone(), at(2026, 6, 7, 9), at(2026, 6, 1, 0), at(2026, 6, 30, 0)),
            vec![(6, 7), (6, 8), (6, 21), (6, 22)]
        );
        r.wkst = Weekday::Mon;
        assert_eq!(
            days(r, at(2026, 6, 7, 9), at(2026, 6, 1, 0), at(2026, 6, 30, 0)),
            vec![(6, 7), (6, 15), (6, 21), (6, 29)]
        );
    }

    /// Unmodelled parts never change expansion (they only ride along for export).
    #[test]
    fn extra_parts_do_not_affect_expansion() {
        let mut r = RecurrenceRule::every(Frequency::Daily, 1);
        r.extra = vec![("BYHOUR".into(), "9".into())];
        assert_eq!(days(r, at(2026, 6, 1, 9), at(2026, 6, 1, 0), at(2026, 6, 4, 0)).len(), 3);
    }

    /// An endless rule can never spin past the safety cap.
    #[test]
    fn endless_rule_is_bounded() {
        let e = daily_event();
        let occ = e.occurrences_in(at(2099, 1, 1, 0), at(2099, 2, 1, 0));
        assert!(occ.is_empty(), "guard stops before the far window: {}", occ.len());
    }
}
