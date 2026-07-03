//! Value formatting: text escaping, date/time conversion, and folded line emission.

use chrono::{DateTime, NaiveDate, NaiveDateTime, Utc};
use mgmt_core::{Error, Result};

/// Escape a TEXT value per RFC 5545 §3.3.11.
pub fn escape_text(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            ';' => out.push_str("\\;"),
            ',' => out.push_str("\\,"),
            '\n' => out.push_str("\\n"),
            _ => out.push(ch),
        }
    }
    out
}

/// Reverse [`escape_text`].
pub fn unescape_text(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(ch) = chars.next() {
        if ch == '\\' {
            match chars.next() {
                Some('n') | Some('N') => out.push('\n'),
                Some('\\') => out.push('\\'),
                Some(';') => out.push(';'),
                Some(',') => out.push(','),
                Some(other) => out.push(other),
                None => out.push('\\'),
            }
        } else {
            out.push(ch);
        }
    }
    out
}

/// Format a UTC instant as an iCalendar UTC date-time (`20260618T090000Z`).
pub fn format_datetime(dt: DateTime<Utc>) -> String {
    dt.format("%Y%m%dT%H%M%SZ").to_string()
}

/// Format a date as an iCalendar DATE value (`20260618`).
pub fn format_date(dt: DateTime<Utc>) -> String {
    dt.format("%Y%m%d").to_string()
}

/// Parse an iCalendar DATE-TIME. Accepts trailing `Z` (UTC); naive values are treated as UTC.
pub fn parse_datetime(s: &str) -> Result<DateTime<Utc>> {
    let s = s.trim();
    let naive = if let Some(stripped) = s.strip_suffix('Z') {
        NaiveDateTime::parse_from_str(stripped, "%Y%m%dT%H%M%S")
    } else {
        NaiveDateTime::parse_from_str(s, "%Y%m%dT%H%M%S")
    }
    .map_err(|e| Error::Parse(format!("bad datetime {s:?}: {e}")))?;
    Ok(naive.and_utc())
}

/// Parse an iCalendar DATE value into a UTC instant at midnight.
pub fn parse_date(s: &str) -> Result<DateTime<Utc>> {
    let d = NaiveDate::parse_from_str(s.trim(), "%Y%m%d")
        .map_err(|e| Error::Parse(format!("bad date {s:?}: {e}")))?;
    Ok(d.and_hms_opt(0, 0, 0).unwrap().and_utc())
}

/// Append a property line to `out`, folding at 75 octets per RFC 5545 §3.1.
pub fn write_folded(out: &mut String, line: &str) {
    const LIMIT: usize = 75;
    let bytes = line.as_bytes();
    if bytes.len() <= LIMIT {
        out.push_str(line);
        out.push_str("\r\n");
        return;
    }
    let mut start = 0;
    let mut first = true;
    while start < bytes.len() {
        // Keep chunks on char boundaries.
        let budget = if first { LIMIT } else { LIMIT - 1 };
        let mut end = (start + budget).min(bytes.len());
        while end > start && !line.is_char_boundary(end) {
            end -= 1;
        }
        if !first {
            out.push(' ');
        }
        out.push_str(&line[start..end]);
        out.push_str("\r\n");
        start = end;
        first = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn text_escape_round_trips() {
        let s = "a,b; c\\d\nend";
        assert_eq!(unescape_text(&escape_text(s)), s);
    }

    #[test]
    fn datetime_round_trips() {
        let dt = Utc.with_ymd_and_hms(2026, 6, 18, 9, 30, 0).unwrap();
        assert_eq!(parse_datetime(&format_datetime(dt)).unwrap(), dt);
    }

    #[test]
    fn folding_wraps_long_lines_with_leading_space() {
        let mut out = String::new();
        let long = format!("DESCRIPTION:{}", "x".repeat(200));
        write_folded(&mut out, &long);
        for (i, l) in out.split("\r\n").filter(|l| !l.is_empty()).enumerate() {
            assert!(l.len() <= 75, "line too long: {}", l.len());
            if i > 0 {
                assert!(l.starts_with(' '));
            }
        }
        // unfolding it back yields the original
        let unfolded: String = out
            .split("\r\n")
            .filter(|l| !l.is_empty())
            .enumerate()
            .map(|(i, l)| if i == 0 { l.to_string() } else { l[1..].to_string() })
            .collect();
        assert_eq!(unfolded, long);
    }
}
