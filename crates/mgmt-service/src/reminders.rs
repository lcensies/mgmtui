//! Reminder computation: which task/event reminders are due to fire *now*, and what each one
//! should do. Pure and testable — the side effects (notifications, focusing mgmt, running hooks)
//! and the de-dup set live in the host (the TUI or the `mgmt daemon`), which calls [`pending`]
//! each tick with the keys it has already fired.

use std::collections::HashSet;

use chrono::{DateTime, Duration, Local, Timelike, Utc};

use mgmt_core::Uid;
use mgmt_domain::{AlarmAction, Event, EventStatus, Task};

/// How long an alarm that fires at or after the event start stays eligible, so a daemon that was
/// asleep across the tick still delivers it. Lead-time alarms keep their natural window (up to the
/// event start) instead.
const LATE_GRACE_MIN: i64 = 15;

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
    /// When this reminder's fire window closes (the task due / event start). Hosts prune their
    /// de-dup sets on this instant — never earlier, or a long-offset reminder re-fires mid-window.
    pub target: DateTime<Utc>,
}

/// Task reminders (offsets before `due`) and event alarms (see [`mgmt_domain::Alarm::fire_at`])
/// whose fire window contains `now` and that aren't already in `fired`. Recurring events must be
/// pre-expanded into occurrences by the caller (see `MgmtContext::pending_reminders`).
pub fn pending(tasks: &[Task], events: &[Event], now: DateTime<Utc>, fired: &HashSet<String>) -> Vec<ReminderHit> {
    let mut out = Vec::new();

    for t in tasks {
        let Some(due) = t.due else { continue };
        for r in &t.reminders {
            let fire_at = due - Duration::minutes(r.minutes);
            if now >= fire_at && now < due {
                // The due timestamp is folded in so rescheduling re-arms the reminder.
                let key = format!("task:{}:{}:{}", t.uid, due.timestamp(), r.minutes);
                if !fired.contains(&key) {
                    out.push(ReminderHit {
                        key,
                        title: format!("Task due in {}", r.label()),
                        body: t.title.clone(),
                        action: HitAction::Notify,
                        target: due,
                    });
                }
            }
        }
    }

    for e in events {
        // A cancelled event has nothing to remind about.
        if e.status == EventStatus::Cancelled {
            continue;
        }
        for a in &e.alarms {
            let fire_at = a.fire_at(e);
            // The window closes at the start for a lead-time alarm (a missed one may still fire
            // late, as before); alarms that fire at/after the start get a fixed catch-up grace.
            let target = e.start.max(fire_at + Duration::minutes(LATE_GRACE_MIN));
            if now >= fire_at && now < target {
                let key = format!("event:{}:{}:{}", e.uid, e.start.timestamp(), fire_at.timestamp());
                if !fired.contains(&key) {
                    let mins_away = (e.start - fire_at).num_minutes();
                    let action = match &a.action {
                        AlarmAction::Notify => HitAction::Notify,
                        AlarmAction::Navigate => HitAction::Navigate { event: e.uid.clone() },
                        AlarmAction::Run { command, args } => HitAction::Run {
                            command: expand(command, e, mins_away),
                            args: args.iter().map(|x| expand(x, e, mins_away)).collect(),
                        },
                    };
                    let local_start = e.start.with_timezone(&Local);
                    let time_str = format!("{:02}:{:02}", local_start.hour(), local_start.minute());
                    let body = match mins_away {
                        m if m <= 0 => format!("starts at {time_str}"),
                        m if m < 60 => format!("starts at {time_str} (in {m}m)"),
                        m if m % 60 == 0 => format!("starts at {time_str} (in {}h)", m / 60),
                        m => format!("starts at {time_str} (in {}h {}m)", m / 60, m % 60),
                    };
                    out.push(ReminderHit {
                        key,
                        title: e.summary.clone(),
                        body,
                        action,
                        target,
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
    fn end_relative_and_absolute_alarms_fire_and_cancelled_events_do_not() {
        use mgmt_domain::AlarmTrigger;
        let mut e = Event::new("work", "Review", at(9, 0), at(10, 0));
        e.alarms = vec![
            Alarm { trigger: AlarmTrigger::MinutesBeforeEnd(5), action: AlarmAction::Notify, description: None },
            Alarm { trigger: AlarmTrigger::At(at(7, 0)), action: AlarmAction::Notify, description: None },
        ];
        // 09:55 = end-5m; the absolute 07:00 one has long since closed.
        let hits = pending(&[], std::slice::from_ref(&e), at(9, 55), &HashSet::new());
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].title, "Review");
        // Fires at/after the start, so the body drops the misleading "in Nm" countdown.
        assert!(hits[0].body.starts_with("starts at ") && !hits[0].body.contains("(in"));
        assert_eq!(pending(&[], std::slice::from_ref(&e), at(7, 0), &HashSet::new()).len(), 1);

        e.status = mgmt_domain::EventStatus::Cancelled;
        assert!(pending(&[], std::slice::from_ref(&e), at(9, 55), &HashSet::new()).is_empty());
    }

    #[test]
    fn task_without_due_never_fires() {
        let mut t = Task::new("someday");
        t.reminders = vec![ReminderOffset::new(60)];
        assert!(pending(std::slice::from_ref(&t), &[], at(16, 30), &HashSet::new()).is_empty());
    }
}
