//! Status surfaces for external "status bar" widgets: a persisted pomodoro/flowtime session and
//! the next upcoming calendar event. Two consumers share this:
//!
//! - `mgmt focus …` mutates the [`PomodoroState`] file and `mgmt status` prints a snapshot;
//! - the `mgmt daemon` polls both, renders them, and pushes them to the configured bar.
//!
//! The pomodoro session lives in a JSON file (not process memory) so the daemon, the CLI, and a
//! future TUI all drive the *same* session across processes and restarts. Timing is wall-clock
//! (`DateTime<Utc>`), never `Instant`, precisely because it must survive a process boundary.
//!
//! Two serialisations exist on purpose:
//! - [`StatusSnapshot`] is *resolved at `now`* (it carries `remaining_secs`) — friendly for
//!   `mgmt status --json` and the plain-text [`StatusSnapshot::render`] used by dumb bars.
//! - [`wire_payload`] is *tick-stable* (it carries absolute `ends_at`/`count_from` instants, not a
//!   live countdown) so a smart bar like the GNOME extension can animate the seconds itself and
//!   the daemon only has to push when the underlying state actually changes.

use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use mgmt_domain::Event;

use crate::time::{Engine, Flowtime, Phase, Pomodoro, Technique};

/// On-disk path of the shared pomodoro session, under the data root's `.state/` dir (alongside
/// the reminder de-dup state).
pub fn pomodoro_path(root: &Path) -> PathBuf {
    root.join(".state").join("pomodoro.json")
}

/// A persisted focus session: which engine drives it, the current phase, and the wall-clock
/// bookkeeping needed to compute elapsed/remaining without an `Instant`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PomodoroState {
    engine: Engine,
    phase: Phase,
    /// Time accumulated in the current phase *before* the latest resume.
    elapsed_before: Duration,
    /// When the current running segment began; `None` while paused.
    running_since: Option<DateTime<Utc>>,
}

impl PomodoroState {
    /// Start a fresh session on its engine's initial phase, already running.
    pub fn start(engine: Engine, now: DateTime<Utc>) -> Self {
        let phase = engine.initial();
        PomodoroState { engine, phase, elapsed_before: Duration::ZERO, running_since: Some(now) }
    }

    /// Start a standard 25/5/15 pomodoro.
    pub fn start_pomodoro(now: DateTime<Utc>) -> Self {
        Self::start(Engine::Pomodoro(Pomodoro::standard()), now)
    }

    /// Start an open-ended flowtime session (break = focus / 5).
    pub fn start_flowtime(now: DateTime<Utc>) -> Self {
        Self::start(Engine::Flowtime(Flowtime::new(5)), now)
    }

    pub fn is_running(&self) -> bool {
        self.running_since.is_some()
    }

    pub fn is_focus(&self) -> bool {
        self.phase.is_focus()
    }

    /// An open-ended phase (no target) — i.e. a flowtime focus that counts up.
    pub fn is_open(&self) -> bool {
        self.phase.target().is_none()
    }

    /// `"focus"` or `"break"` — the stable label used across the wire and the renderer.
    pub fn phase_label(&self) -> &'static str {
        if self.phase.is_focus() { "focus" } else { "break" }
    }

    /// Time spent in the current phase as of `now`.
    pub fn elapsed(&self, now: DateTime<Utc>) -> Duration {
        let live = self.running_since.map(|s| (now - s).to_std().unwrap_or(Duration::ZERO)).unwrap_or(Duration::ZERO);
        self.elapsed_before + live
    }

    /// Time left in the current phase, or `None` for an open-ended (flowtime) focus.
    pub fn remaining(&self, now: DateTime<Utc>) -> Option<Duration> {
        self.phase.target().map(|t| t.saturating_sub(self.elapsed(now)))
    }

    /// Pause a running session or resume a paused one.
    pub fn toggle(&mut self, now: DateTime<Utc>) {
        match self.running_since.take() {
            Some(since) => self.elapsed_before += (now - since).to_std().unwrap_or(Duration::ZERO),
            None => self.running_since = Some(now),
        }
    }

    /// Advance to the next phase now, preserving the running/paused state.
    pub fn skip(&mut self, now: DateTime<Utc>) {
        let elapsed = self.elapsed(now);
        self.phase = self.engine.next(self.phase, elapsed);
        self.elapsed_before = Duration::ZERO;
        if self.running_since.is_some() {
            self.running_since = Some(now);
        }
    }

    /// Auto-advance a *running, targeted* phase that has run out. Returns the new phase iff it
    /// advanced — the caller uses that to fire a one-shot "phase done" notification. Open-ended
    /// (flowtime) focus never auto-advances; the user ends it explicitly.
    pub fn tick(&mut self, now: DateTime<Utc>) -> Option<Phase> {
        if self.is_running() && self.remaining(now) == Some(Duration::ZERO) {
            self.skip(now);
            Some(self.phase)
        } else {
            None
        }
    }

    /// The stable wall-clock instant a running, targeted phase ends — `None` while paused or
    /// open-ended. Stable because it derives only from `running_since`/`elapsed_before`, not
    /// `now`, so a self-ticking bar can animate the countdown without the daemon re-pushing.
    pub fn ends_at(&self) -> Option<DateTime<Utc>> {
        let since = self.running_since?;
        let target = self.phase.target()?;
        let left = target.saturating_sub(self.elapsed_before);
        Some(since + chrono::Duration::from_std(left).ok()?)
    }

    /// The stable virtual start of a running, open-ended phase (for count-up display) — `None`
    /// while paused or targeted.
    pub fn count_from(&self) -> Option<DateTime<Utc>> {
        let since = self.running_since?;
        if self.phase.target().is_some() {
            return None;
        }
        let before = chrono::Duration::from_std(self.elapsed_before).unwrap_or_else(|_| chrono::Duration::zero());
        Some(since - before)
    }

    /// Load the session, or `None` if absent/corrupt (treated as "no session").
    pub fn load(path: &Path) -> Option<Self> {
        let text = std::fs::read_to_string(path).ok()?;
        serde_json::from_str(&text).ok()
    }

    /// Persist atomically (write-temp-then-rename) so a concurrent reader never sees a torn file.
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_string(self).unwrap_or_default())?;
        std::fs::rename(&tmp, path)
    }
}

/// A resolved-at-`now` view of the status surfaces. Serialised by `mgmt status --json` and
/// rendered to text by [`StatusSnapshot::render`].
#[derive(Debug, Clone, Default, Serialize)]
pub struct StatusSnapshot {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pomodoro: Option<PomodoroSnapshot>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_event: Option<EventSnapshot>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PomodoroSnapshot {
    pub phase: &'static str,
    pub running: bool,
    pub open: bool,
    /// Seconds left; `None` for an open-ended focus.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remaining_secs: Option<i64>,
    pub elapsed_secs: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct EventSnapshot {
    pub summary: String,
    /// Event start as a Unix timestamp (UTC).
    pub start: i64,
    pub all_day: bool,
    /// Seconds until start (0 once it has begun).
    pub in_secs: i64,
}

impl StatusSnapshot {
    /// Resolve the snapshot at `now` from the live session and the next event.
    pub fn compute(pomo: Option<&PomodoroState>, next: Option<&Event>, now: DateTime<Utc>) -> Self {
        let pomodoro = pomo.map(|p| PomodoroSnapshot {
            phase: p.phase_label(),
            running: p.is_running(),
            open: p.is_open(),
            remaining_secs: p.remaining(now).map(|d| d.as_secs() as i64),
            elapsed_secs: p.elapsed(now).as_secs() as i64,
        });
        let next_event = next.map(|e| EventSnapshot {
            summary: e.summary.clone(),
            start: e.start.timestamp(),
            all_day: e.all_day,
            in_secs: (e.start - now).num_seconds().max(0),
        });
        StatusSnapshot { pomodoro, next_event }
    }

    pub fn is_empty(&self) -> bool {
        self.pomodoro.is_none() && self.next_event.is_none()
    }

    /// One compact line for "dumb" bars (the `command`/`file` backends) and `mgmt status`.
    pub fn render(&self) -> String {
        let mut parts: Vec<String> = Vec::new();
        if let Some(p) = &self.pomodoro {
            let label = if p.phase == "focus" { "Focus" } else { "Break" };
            let secs = if p.open { p.elapsed_secs } else { p.remaining_secs.unwrap_or(0) };
            let mut s = format!("{label} {}", clock(secs));
            if !p.running {
                s.push_str(" paused");
            }
            parts.push(s);
        }
        if let Some(e) = &self.next_event {
            parts.push(render_event(e));
        }
        parts.join(" · ")
    }
}

/// Build the tick-stable wire payload a smart bar consumes. Carries absolute instants
/// (`ends_at`/`count_from`) for a running session so the bar animates locally and the daemon
/// pushes only on real state changes. `bin` is added by the daemon (the bar execs it on click).
pub fn wire_payload(pomo: Option<&PomodoroState>, next: Option<&Event>, now: DateTime<Utc>) -> Value {
    let mut obj = serde_json::Map::new();
    if let Some(p) = pomo {
        let mut po = serde_json::Map::new();
        po.insert("phase".into(), json!(p.phase_label()));
        po.insert("running".into(), json!(p.is_running()));
        po.insert("open".into(), json!(p.is_open()));
        if p.is_running() {
            if let Some(end) = p.ends_at() {
                po.insert("ends_at".into(), json!(end.timestamp()));
            }
            if let Some(from) = p.count_from() {
                po.insert("count_from".into(), json!(from.timestamp()));
            }
        } else {
            // Paused: freeze the displayed figure so the bar shows a static value.
            if let Some(r) = p.remaining(now) {
                po.insert("remaining".into(), json!(r.as_secs() as i64));
            }
            po.insert("elapsed".into(), json!(p.elapsed(now).as_secs() as i64));
        }
        obj.insert("pomodoro".into(), Value::Object(po));
    }
    if let Some(e) = next {
        obj.insert(
            "next_event".into(),
            json!({ "summary": e.summary, "start": e.start.timestamp(), "all_day": e.all_day }),
        );
    }
    Value::Object(obj)
}

/// `MM:SS`, or `H:MM:SS` past an hour.
fn clock(secs: i64) -> String {
    let s = secs.max(0);
    let (h, m, sec) = (s / 3600, (s % 3600) / 60, s % 60);
    if h > 0 {
        format!("{h}:{m:02}:{sec:02}")
    } else {
        format!("{m:02}:{sec:02}")
    }
}

fn render_event(e: &EventSnapshot) -> String {
    if e.all_day {
        format!("{} (all day)", e.summary)
    } else {
        format!("{} {}", e.summary, humanize_in(e.in_secs))
    }
}

/// "now" / "in 25m" / "in 2h05m" / "in 3d".
fn humanize_in(secs: i64) -> String {
    if secs <= 0 {
        return "now".into();
    }
    let mins = secs / 60;
    if mins < 60 {
        format!("in {mins}m")
    } else if mins < 60 * 24 {
        let (h, m) = (mins / 60, mins % 60);
        if m == 0 { format!("in {h}h") } else { format!("in {h}h{m:02}m") }
    } else {
        format!("in {}d", mins / (60 * 24))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t0() -> DateTime<Utc> {
        DateTime::from_timestamp(1_700_000_000, 0).unwrap()
    }

    #[test]
    fn pomodoro_counts_down_and_freezes_when_paused() {
        let now = t0();
        let mut p = PomodoroState::start_pomodoro(now);
        assert!(p.is_running() && p.is_focus());
        // 25-minute focus: 60s in → 24:00 left.
        let at = now + chrono::Duration::seconds(60);
        assert_eq!(p.remaining(at), Some(Duration::from_secs(24 * 60)));
        // ends_at is now-independent: start + 25m.
        assert_eq!(p.ends_at(), Some(now + chrono::Duration::minutes(25)));
        // Pause at +60s; remaining must freeze even as wall-clock advances.
        p.toggle(at);
        assert!(!p.is_running());
        assert_eq!(p.ends_at(), None);
        let later = at + chrono::Duration::seconds(120);
        assert_eq!(p.remaining(later), Some(Duration::from_secs(24 * 60)));
        // Resume: countdown continues from where it froze.
        p.toggle(later);
        assert_eq!(p.remaining(later + chrono::Duration::seconds(60)), Some(Duration::from_secs(23 * 60)));
    }

    #[test]
    fn tick_auto_advances_focus_to_break_exactly_once() {
        let now = t0();
        let mut p = PomodoroState::start_pomodoro(now);
        // Not yet due.
        assert_eq!(p.tick(now + chrono::Duration::minutes(24)), None);
        // Due → advances to a 5-minute break, once.
        let due = now + chrono::Duration::minutes(25);
        let advanced = p.tick(due).expect("focus should auto-advance");
        assert!(!advanced.is_focus());
        assert_eq!(p.phase_label(), "break");
        assert_eq!(p.remaining(due), Some(Duration::from_secs(5 * 60)));
        // A second tick right after must NOT advance again.
        assert_eq!(p.tick(due), None);
    }

    #[test]
    fn flowtime_focus_is_open_and_never_auto_advances() {
        let now = t0();
        let mut p = PomodoroState::start_flowtime(now);
        assert!(p.is_open());
        assert_eq!(p.remaining(now + chrono::Duration::hours(3)), None);
        assert_eq!(p.tick(now + chrono::Duration::hours(3)), None);
        // count_from is the stable virtual start for count-up.
        assert_eq!(p.count_from(), Some(now));
    }

    #[test]
    fn wire_payload_is_tick_stable_while_running() {
        let now = t0();
        let p = PomodoroState::start_pomodoro(now);
        let a = wire_payload(Some(&p), None, now);
        let b = wire_payload(Some(&p), None, now + chrono::Duration::seconds(37));
        assert_eq!(a, b, "running payload must not change second-to-second");
        assert_eq!(a["pomodoro"]["ends_at"], json!((now + chrono::Duration::minutes(25)).timestamp()));
    }

    #[test]
    fn render_is_compact_and_human() {
        let now = t0();
        let p = PomodoroState::start_pomodoro(now);
        let ev = Event::new("work", "Standup", now + chrono::Duration::minutes(25), now + chrono::Duration::minutes(40));
        let snap = StatusSnapshot::compute(Some(&p), Some(&ev), now);
        assert_eq!(snap.render(), "Focus 25:00 · Standup in 25m");
    }
}
