//! Human-friendly datetime parsing for the CLI. The domain stores everything in UTC, so
//! we normalize every accepted form to `DateTime<Utc>`.
//!
//! Accepted forms (most specific first):
//! - RFC 3339 with offset — `2026-06-18T09:00:00+02:00` (converted to UTC)
//! - `YYYY-MM-DDTHH:MM[:SS]` or `YYYY-MM-DD HH:MM[:SS]` — interpreted as UTC
//! - `YYYY-MM-DD` — date only, anchored at UTC midnight (use for `--all-day`)

use anyhow::{Result, anyhow};
use chrono::{DateTime, NaiveDate, NaiveDateTime, Utc};

/// Parse a user-supplied date/time into a UTC instant. Naive (offset-less) values are
/// treated as UTC; values carrying an offset are converted to UTC.
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
            return Ok(naive.and_utc());
        }
    }

    // 3. Date only -> UTC midnight.
    if let Ok(date) = NaiveDate::parse_from_str(s, "%Y-%m-%d") {
        return Ok(date.and_hms_opt(0, 0, 0).unwrap().and_utc());
    }

    Err(anyhow!(
        "could not parse datetime {s:?}; use 'YYYY-MM-DD', 'YYYY-MM-DD HH:MM', or RFC3339 (e.g. 2026-06-18T09:00:00+02:00)"
    ))
}

/// Render a UTC instant back in the compact `YYYY-MM-DD HH:MM` form used in CLI output.
pub fn fmt_when(dt: DateTime<Utc>) -> String {
    dt.format("%Y-%m-%d %H:%M").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Timelike;

    #[test]
    fn parses_date_only_as_utc_midnight() {
        let dt = parse_when("2026-06-18").unwrap();
        assert_eq!(fmt_when(dt), "2026-06-18 00:00");
        assert_eq!(dt.hour(), 0);
    }

    #[test]
    fn parses_space_and_t_separators() {
        assert_eq!(parse_when("2026-06-18 09:30").unwrap(), parse_when("2026-06-18T09:30").unwrap());
    }

    #[test]
    fn rfc3339_offset_converts_to_utc() {
        // 09:00 at +02:00 is 07:00 UTC.
        let dt = parse_when("2026-06-18T09:00:00+02:00").unwrap();
        assert_eq!(fmt_when(dt), "2026-06-18 07:00");
    }

    #[test]
    fn garbage_is_rejected() {
        assert!(parse_when("next tuesday").is_err());
    }
}
