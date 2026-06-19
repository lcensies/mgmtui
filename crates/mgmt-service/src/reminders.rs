//! Reminder computation: which task/event reminders are due to fire *now*. Pure and testable —
//! the firing (desktop notifications) and the session de-dup set live in the TUI host, which
//! calls [`pending`] each tick with the keys it has already shown.

use std::collections::HashSet;

use chrono::{DateTime, Duration, Utc};

use mgmt_domain::{AlarmTrigger, Event, Task};

/// A reminder that should fire now. `key` is a stable de-dup id (so a given reminder fires once
/// per session).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReminderHit {
    pub key: String,
    pub title: String,
    pub body: String,
}

/// Task reminders (offsets before `due`) and event alarms (minutes before `start`) whose fire
/// window `[fire_at, target)` contains `now` and that aren't already in `fired`.
pub fn pending(tasks: &[Task], events: &[Event], now: DateTime<Utc>, fired: &HashSet<String>) -> Vec<ReminderHit> {
    let mut out = Vec::new();

    for t in tasks {
        let Some(due) = t.due else { continue };
        for r in &t.reminders {
            let fire_at = due - Duration::minutes(r.minutes);
            if now >= fire_at && now < due {
                let key = format!("task:{}:{}", t.uid, r.minutes);
                if !fired.contains(&key) {
                    out.push(ReminderHit {
                        key,
                        title: format!("Task due in {}", r.label()),
                        body: t.title.clone(),
                    });
                }
            }
        }
    }

    for e in events {
        for a in &e.alarms {
            let AlarmTrigger::MinutesBefore(m) = a.trigger;
            let fire_at = e.start - Duration::minutes(m);
            if now >= fire_at && now < e.start {
                let key = format!("event:{}:{}", e.uid, m);
                if !fired.contains(&key) {
                    out.push(ReminderHit {
                        key,
                        title: a.description.clone().unwrap_or_else(|| "Upcoming event".into()),
                        body: e.summary.clone(),
                    });
                }
            }
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use mgmt_domain::{Alarm, ReminderOffset};

    fn at(h: u32, m: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 6, 18, h, m, 0).unwrap()
    }

    #[test]
    fn task_reminder_fires_inside_window_only() {
        let mut t = Task::new("Pay rent");
        t.due = Some(at(17, 0));
        t.reminders = vec![ReminderOffset::new(60)]; // 1h before -> 16:00
        let empty = HashSet::new();
        // before the window
        assert!(pending(std::slice::from_ref(&t), &[], at(15, 0), &empty).is_empty());
        // inside the window
        assert_eq!(pending(std::slice::from_ref(&t), &[], at(16, 30), &empty).len(), 1);
        // after due
        assert!(pending(std::slice::from_ref(&t), &[], at(17, 30), &empty).is_empty());
    }

    #[test]
    fn fired_keys_are_suppressed() {
        let mut t = Task::new("x");
        t.due = Some(at(17, 0));
        t.reminders = vec![ReminderOffset::new(60)];
        let hits = pending(std::slice::from_ref(&t), &[], at(16, 30), &HashSet::new());
        let fired: HashSet<String> = hits.iter().map(|h| h.key.clone()).collect();
        assert!(pending(std::slice::from_ref(&t), &[], at(16, 31), &fired).is_empty());
    }

    #[test]
    fn event_alarm_fires() {
        let mut e = Event::new("work", "Standup", at(9, 0), at(9, 30));
        e.alarms = vec![Alarm { trigger: AlarmTrigger::MinutesBefore(15), description: None }];
        let hits = pending(&[], std::slice::from_ref(&e), at(8, 50), &HashSet::new());
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].body, "Standup");
    }

    #[test]
    fn task_without_due_never_fires() {
        let mut t = Task::new("someday");
        t.reminders = vec![ReminderOffset::new(60)];
        assert!(pending(std::slice::from_ref(&t), &[], at(16, 30), &HashSet::new()).is_empty());
    }
}
