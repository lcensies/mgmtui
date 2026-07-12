//! Human-friendly datetime parsing for the CLI. The domain stores true UTC instants; naive
//! (offset-less) input is what a human types, so it is interpreted in the *local* timezone
//! and converted — a calendar that shifts your 09:00 meeting to 09:00 UTC is wrong everywhere
//! that matters (reminders, CalDAV/Google interop, the now line).
//!
//! Accepted forms (most specific first):
//! - RFC 3339 with offset — `2026-06-18T09:00:00+02:00` (converted to UTC)
//! - `YYYY-MM-DDTHH:MM[:SS]` or `YYYY-MM-DD HH:MM[:SS]` — interpreted as local time
//! - `YYYY-MM-DD` — date only, anchored at local midnight (use for `--all-day`)

use anyhow::{Result, anyhow};
use chrono::{DateTime, Local, NaiveDate, NaiveDateTime, TimeZone, Utc};

/// Resolve a naive local datetime to UTC. Ambiguous times (DST fold) take the earlier
/// instant; times inside a DST gap shift forward to the next valid instant.
pub fn local_to_utc(naive: NaiveDateTime) -> DateTime<Utc> {
    Local
        .from_local_datetime(&naive)
        .earliest()
        .map(|d| d.with_timezone(&Utc))
        .unwrap_or_else(|| naive.and_utc())
}

/// Re-anchor an instant to UTC midnight of its *local* calendar date. All-day events carry pure
/// date semantics (serialized as an iCalendar DATE), so they are anchored at UTC midnight — not
/// at the local-midnight instant a timed parse produces.
pub fn date_anchor_utc(dt: DateTime<Utc>) -> DateTime<Utc> {
    dt.with_timezone(&Local).date_naive().and_hms_opt(0, 0, 0).unwrap().and_utc()
}

/// Parse a user-supplied date/time into a UTC instant. Naive (offset-less) values are
/// interpreted as local time; values carrying an offset are converted to UTC.
pub fn parse_when(s: &str) -> Result<DateTime<Utc>> {
    let s = s.trim();

    // 1. RFC 3339 / ISO 8601 with an explicit offset or trailing Z.
    if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
        return Ok(dt.with_timezone(&Utc));
    }

    // 2. Naive date-time, allowing either 'T' or a space separator and optional seconds.
    let normalized = s.replacen(' ', "T", 1);
    for fmt in ["%Y-%m-%dT%H:%M:%S", "%Y-%m-%dT%H:%M"] {
        if let Ok(naive) = NaiveDateTime::parse_from_str(&normalized, fmt) {
            return Ok(local_to_utc(naive));
        }
    }

    // 3. Date only -> local midnight.
    if let Ok(date) = NaiveDate::parse_from_str(s, "%Y-%m-%d") {
        return Ok(local_to_utc(date.and_hms_opt(0, 0, 0).unwrap()));
    }

    Err(anyhow!(
        "could not parse datetime {s:?}; use 'YYYY-MM-DD', 'YYYY-MM-DD HH:MM' (local time), or RFC3339 (e.g. 2026-06-18T09:00:00+02:00)"
    ))
}

/// Render a UTC instant in *local* time, in the compact `YYYY-MM-DD HH:MM` form used in CLI
/// output — the mirror of [`parse_when`], so create → list round-trips what you typed.
pub fn fmt_when(dt: DateTime<Utc>) -> String {
    dt.with_timezone(&Local).format("%Y-%m-%d %H:%M").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    // Tests are written timezone-agnostically (parse↔format round-trips) so they pass under
    // any TZ the host happens to run in.

    #[test]
    fn naive_input_round_trips_through_local_display() {
        assert_eq!(fmt_when(parse_when("2026-06-18 09:30").unwrap()), "2026-06-18 09:30");
        assert_eq!(fmt_when(parse_when("2026-06-18").unwrap()), "2026-06-18 00:00");
    }

    #[test]
    fn naive_input_is_local_not_utc() {
        let dt = parse_when("2026-06-18 09:30").unwrap();
        let expected = local_to_utc(
            NaiveDate::from_ymd_opt(2026, 6, 18).unwrap().and_hms_opt(9, 30, 0).unwrap(),
        );
        assert_eq!(dt, expected);
    }

    #[test]
    fn parses_space_and_t_separators() {
        assert_eq!(parse_when("2026-06-18 09:30").unwrap(), parse_when("2026-06-18T09:30").unwrap());
    }

    #[test]
    fn rfc3339_offset_converts_to_utc_instant() {
        // 09:00 at +02:00 is 07:00 UTC, whatever the local zone displays it as.
        let dt = parse_when("2026-06-18T09:00:00+02:00").unwrap();
        assert_eq!(dt, chrono::Utc.with_ymd_and_hms(2026, 6, 18, 7, 0, 0).unwrap());
    }

    #[test]
    fn garbage_is_rejected() {
        assert!(parse_when("next tuesday").is_err());
    }
}
