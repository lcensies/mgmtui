//! Reminder computation: which task/event reminders are due to fire *now*, and what each one
//! should do. Pure and testable — the side effects (notifications, focusing mgmt, running hooks)
//! and the de-dup set live in the host (the TUI or the `mgmt daemon`), which calls [`pending`]
//! each tick with the keys it has already fired.

use std::collections::HashSet;

use chrono::{DateTime, Duration, Local, Timelike, Utc};

use mgmt_core::Uid;
use mgmt_domain::{AlarmAction, Event, Task};

/// What firing a reminder should actually do. Task reminders are always [`HitAction::Notify`];
/// event alarms carry the action configured on the alarm.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HitAction {
    /// Show the desktop notification only.
    Notify,
    /// Show the notification, then bring mgmt to the event with this UID.
    Navigate { event: Uid },
    /// Run a command. Placeholders are already expanded against the event.
    Run { command: String, args: Vec<String> },
}

/// A reminder that should fire now. `key` is a stable de-dup id (so a given reminder fires once
/// per fire window — for events the occurrence start is folded in so each occurrence of a
/// recurring series dedups independently).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReminderHit {
    pub key: String,
    pub title: String,
    pub body: String,
    pub action: HitAction,
}

/// Task reminders (offsets before `due`) and event alarms (minutes before `start`) whose fire
/// window `[fire_at, target)` contains `now` and that aren't already in `fired`. Recurring events
/// must be pre-expanded into occurrences by the caller (see `MgmtContext::pending_reminders`).
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
                        action: HitAction::Notify,
                    });
                }
            }
        }
    }

    for e in events {
        for a in &e.alarms {
            let m = a.minutes();
            let fire_at = e.start - Duration::minutes(m);
            if now >= fire_at && now < e.start {
                let key = format!("event:{}:{}:{}", e.uid, e.start.timestamp(), m);
                if !fired.contains(&key) {
                    let action = match &a.action {
                        AlarmAction::Notify => HitAction::Notify,
                        AlarmAction::Navigate => HitAction::Navigate { event: e.uid.clone() },
                        AlarmAction::Run { command, args } => HitAction::Run {
                            command: expand(command, e, m),
                            args: args.iter().map(|x| expand(x, e, m)).collect(),
                        },
                    };
                    let local_start = e.start.with_timezone(&Local);
                    let mins_away = m;
                    let time_str = format!("{:02}:{:02}", local_start.hour(), local_start.minute());
                    let in_str = if mins_away < 60 {
                        format!("in {mins_away}m")
                    } else {
                        let h = mins_away / 60;
                        let rm = mins_away % 60;
                        if rm == 0 { format!("in {h}h") } else { format!("in {h}h {rm}m") }
                    };
                    out.push(ReminderHit {
                        key,
                        title: e.summary.clone(),
                        body: format!("starts at {time_str} ({in_str})"),
                        action,
                    });
                }
            }
        }
    }

    out
}

/// Expand event-field placeholders in a `Run` command/argument template.
fn expand(tpl: &str, e: &Event, minutes: i64) -> String {
    tpl.replace("{uid}", &e.uid.to_string())
        .replace("{summary}", &e.summary)
        .replace("{start}", &e.start.to_rfc3339())
        .replace("{end}", &e.end.to_rfc3339())
        .replace("{location}", e.location.as_deref().unwrap_or(""))
        .replace("{calendar}", &e.calendar)
        .replace("{minutes}", &minutes.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use mgmt_domain::{Alarm, AlarmAction, ReminderOffset};

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
        e.alarms = vec![Alarm::minutes_before(15)];
        let hits = pending(&[], std::slice::from_ref(&e), at(8, 50), &HashSet::new());
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].title, "Standup");
        assert!(hits[0].body.starts_with("starts at "), "body was: {}", hits[0].body);
        assert!(hits[0].body.contains("in 15m"), "body was: {}", hits[0].body);
        assert_eq!(hits[0].action, HitAction::Notify);
    }

    #[test]
    fn navigate_and_run_actions_are_carried_and_expanded() {
        let mut e = Event::new("work", "Standup", at(9, 0), at(9, 30));
        e.location = Some("Room 1".into());
        e.alarms = vec![
            Alarm::with_action(15, AlarmAction::Navigate),
            Alarm::with_action(
                10,
                AlarmAction::Run {
                    command: "hook".into(),
                    args: vec!["{summary}@{location}".into(), "in {minutes}m".into()],
                },
            ),
        ];
        let hits = pending(&[], std::slice::from_ref(&e), at(8, 50), &HashSet::new());
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].action, HitAction::Navigate { event: e.uid.clone() });
        assert_eq!(
            hits[1].action,
            HitAction::Run { command: "hook".into(), args: vec!["Standup@Room 1".into(), "in 10m".into()] }
        );
    }

    #[test]
    fn task_without_due_never_fires() {
        let mut t = Task::new("someday");
        t.reminders = vec![ReminderOffset::new(60)];
        assert!(pending(std::slice::from_ref(&t), &[], at(16, 30), &HashSet::new()).is_empty());
    }
}
