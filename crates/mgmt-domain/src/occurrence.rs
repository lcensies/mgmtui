//! Occurrence exceptions: `EXDATE` filtering and `RECURRENCE-ID` override substitution on top
//! of plain recurrence expansion ([`Event::occurrences_in`]).

use chrono::{DateTime, Utc};

use crate::Event;

impl Event {
    /// Like [`Event::occurrences_in`], but with this series' exceptions applied: occurrences
    /// listed in `exdates` are dropped, and an occurrence whose start matches an override's
    /// `recurrence_id` is replaced by that override (which may move it in time, even out of —
    /// or into — the `[from, to)` window).
    ///
    /// `overrides` are the sibling events sharing this event's `uid` that carry a
    /// `recurrence_id`.
    pub fn occurrences_with_overrides(&self, overrides: &[Event], from: DateTime<Utc>, to: DateTime<Utc>) -> Vec<Event> {
        if self.exdates.is_empty() && overrides.is_empty() {
            return self.occurrences_in(from, to);
        }
        let excluded = |t: DateTime<Utc>| self.exdates.contains(&t);
        let mut out = Vec::new();
        let mut taken: Vec<DateTime<Utc>> = Vec::new();
        for occ in self.occurrences_in(from, to) {
            if excluded(occ.start) {
                continue;
            }
            match overrides.iter().find(|o| o.recurrence_id == Some(occ.start)) {
                Some(o) => {
                    taken.push(occ.start);
                    if o.overlaps(from, to) {
                        out.push(o.clone());
                    }
                }
                None => out.push(occ),
            }
        }
        // An override may have been moved into the window from a slot outside it.
        for o in overrides {
            let Some(rid) = o.recurrence_id else { continue };
            if !excluded(rid) && !taken.contains(&rid) && o.overlaps(from, to) {
                out.push(o.clone());
            }
        }
        out.sort_by_key(|e| e.start);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Frequency, RecurrenceRule};
    use chrono::{Datelike, TimeZone};

    fn at(d: u32, h: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 6, d, h, 0, 0).unwrap()
    }

    fn daily() -> Event {
        let mut e = Event::new("c", "standup", at(1, 9), at(1, 10));
        e.rrule = Some(RecurrenceRule::every(Frequency::Daily, 1));
        e
    }

    #[test]
    fn exdate_removes_only_that_occurrence() {
        let mut e = daily();
        e.exdates.push(at(3, 9));
        let days: Vec<u32> = e
            .occurrences_with_overrides(&[], at(1, 0), at(5, 0))
            .iter()
            .map(|o| o.start.day())
            .collect();
        assert_eq!(days, vec![1, 2, 4]);
    }

    #[test]
    fn override_replaces_the_generated_instance() {
        let e = daily();
        let mut o = e.clone();
        o.rrule = None;
        o.recurrence_id = Some(at(3, 9));
        o.start = at(3, 14);
        o.end = at(3, 15);
        let occ = e.occurrences_with_overrides(std::slice::from_ref(&o), at(1, 0), at(5, 0));
        let starts: Vec<DateTime<Utc>> = occ.iter().map(|x| x.start).collect();
        assert_eq!(starts, vec![at(1, 9), at(2, 9), at(3, 14), at(4, 9)]);
    }

    #[test]
    fn override_moved_into_the_window_is_picked_up_and_out_of_it_is_dropped() {
        let e = daily();
        let mut o = e.clone();
        o.rrule = None;
        o.recurrence_id = Some(at(9, 9)); // a slot outside the queried window
        o.start = at(3, 14);
        o.end = at(3, 15);
        let occ = e.occurrences_with_overrides(std::slice::from_ref(&o), at(3, 0), at(4, 0));
        assert_eq!(occ.len(), 2); // the generated 06-03 09:00 plus the moved-in override
        assert!(occ.iter().any(|x| x.start == at(3, 14)));

        // Moved out of the window: the 06-03 slot yields nothing.
        let mut away = o.clone();
        away.recurrence_id = Some(at(3, 9));
        away.start = at(9, 14);
        away.end = at(9, 15);
        let occ = e.occurrences_with_overrides(std::slice::from_ref(&away), at(3, 0), at(4, 0));
        assert!(occ.is_empty());
    }
}
