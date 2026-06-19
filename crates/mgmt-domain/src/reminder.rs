//! Reminder offsets for tasks: "fire N before the due date". Stored compactly as a count of
//! minutes but written/parsed as friendly `1d` / `2h` / `30m` strings in frontmatter and on the
//! command line.

use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// A reminder expressed as a positive offset *before* a task's due time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct ReminderOffset {
    pub minutes: i64,
}

impl ReminderOffset {
    pub fn new(minutes: i64) -> Self {
        ReminderOffset { minutes }
    }

    /// Parse `1d`, `2h`, `30m`, or a bare integer (minutes). Returns `None` on garbage.
    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim();
        if s.is_empty() {
            return None;
        }
        let (num, mult) = match s.chars().last().unwrap() {
            'd' | 'D' => (&s[..s.len() - 1], 1440),
            'h' | 'H' => (&s[..s.len() - 1], 60),
            'm' | 'M' => (&s[..s.len() - 1], 1),
            c if c.is_ascii_digit() => (s, 1),
            _ => return None,
        };
        let n: i64 = num.trim().parse().ok()?;
        if n < 0 {
            return None;
        }
        Some(ReminderOffset { minutes: n * mult })
    }

    /// Render back to the most compact friendly unit (`1d`, `2h`, `30m`).
    pub fn label(&self) -> String {
        let m = self.minutes;
        if m != 0 && m % 1440 == 0 {
            format!("{}d", m / 1440)
        } else if m != 0 && m % 60 == 0 {
            format!("{}h", m / 60)
        } else {
            format!("{m}m")
        }
    }
}

impl fmt::Display for ReminderOffset {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.label())
    }
}

impl Serialize for ReminderOffset {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.label())
    }
}

impl<'de> Deserialize<'de> for ReminderOffset {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        // Accept either a friendly string ("1d") or a bare integer count of minutes.
        struct V;
        impl serde::de::Visitor<'_> for V {
            type Value = ReminderOffset;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("a reminder offset like \"1d\", \"2h\", \"30m\", or minutes as an integer")
            }
            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<ReminderOffset, E> {
                ReminderOffset::parse(v).ok_or_else(|| E::custom(format!("invalid reminder offset {v:?}")))
            }
            fn visit_i64<E: serde::de::Error>(self, v: i64) -> Result<ReminderOffset, E> {
                Ok(ReminderOffset::new(v))
            }
            fn visit_u64<E: serde::de::Error>(self, v: u64) -> Result<ReminderOffset, E> {
                Ok(ReminderOffset::new(v as i64))
            }
        }
        d.deserialize_any(V)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_units_and_bare_minutes() {
        assert_eq!(ReminderOffset::parse("1d"), Some(ReminderOffset::new(1440)));
        assert_eq!(ReminderOffset::parse("2h"), Some(ReminderOffset::new(120)));
        assert_eq!(ReminderOffset::parse("30m"), Some(ReminderOffset::new(30)));
        assert_eq!(ReminderOffset::parse("45"), Some(ReminderOffset::new(45)));
        assert_eq!(ReminderOffset::parse("garbage"), None);
        assert_eq!(ReminderOffset::parse("-5m"), None);
    }

    #[test]
    fn label_round_trips_to_compact_unit() {
        assert_eq!(ReminderOffset::new(1440).label(), "1d");
        assert_eq!(ReminderOffset::new(120).label(), "2h");
        assert_eq!(ReminderOffset::new(30).label(), "30m");
        assert_eq!(ReminderOffset::new(90).label(), "90m");
    }
}
