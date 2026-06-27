//! Time-management engine: pomodoro and flowtime. Pure session-state logic over
//! `std::time::Duration`; the TUI drives it with a wall clock and renders the result.

use std::time::Duration;

use serde::{Deserialize, Serialize};

/// The phase of a focus/break cycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Phase {
    /// Working. `target` is the planned length, or `None` for an open-ended focus (flowtime).
    Focus { target: Option<Duration> },
    /// Resting. `target` is the planned break length.
    Break { target: Option<Duration> },
}

impl Phase {
    pub fn is_focus(self) -> bool {
        matches!(self, Phase::Focus { .. })
    }

    pub fn target(self) -> Option<Duration> {
        match self {
            Phase::Focus { target } | Phase::Break { target } => target,
        }
    }
}

/// A technique decides what phase comes after the current one, given how long the current
/// phase actually ran.
pub trait Technique {
    /// The phase to start from a cold stop.
    fn initial(&self) -> Phase;
    /// Advance from `current`, which ran for `elapsed`, to the next phase.
    fn next(&mut self, current: Phase, elapsed: Duration) -> Phase;
}

/// Classic pomodoro: fixed focus, short breaks, and a long break every N focuses.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Pomodoro {
    pub focus: Duration,
    pub short_break: Duration,
    pub long_break: Duration,
    pub focuses_before_long_break: u32,
    completed_focuses: u32,
}

impl Pomodoro {
    pub fn new(focus: Duration, short_break: Duration, long_break: Duration, focuses_before_long_break: u32) -> Self {
        Pomodoro {
            focus,
            short_break,
            long_break,
            focuses_before_long_break: focuses_before_long_break.max(1),
            completed_focuses: 0,
        }
    }

    /// Default 25/5/15, long break every 4.
    pub fn standard() -> Self {
        Pomodoro::new(
            Duration::from_secs(25 * 60),
            Duration::from_secs(5 * 60),
            Duration::from_secs(15 * 60),
            4,
        )
    }
}

impl Technique for Pomodoro {
    fn initial(&self) -> Phase {
        Phase::Focus { target: Some(self.focus) }
    }

    fn next(&mut self, current: Phase, _elapsed: Duration) -> Phase {
        match current {
            Phase::Focus { .. } => {
                self.completed_focuses += 1;
                if self.completed_focuses % self.focuses_before_long_break == 0 {
                    Phase::Break { target: Some(self.long_break) }
                } else {
                    Phase::Break { target: Some(self.short_break) }
                }
            }
            Phase::Break { .. } => Phase::Focus { target: Some(self.focus) },
        }
    }
}

/// Flowtime: focus as long as you like, then take a break proportional to the focus length.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Flowtime {
    /// Break = focus_elapsed / `break_divisor`.
    pub break_divisor: u32,
}

impl Flowtime {
    pub fn new(break_divisor: u32) -> Self {
        Flowtime {
            break_divisor: break_divisor.max(1),
        }
    }
}

impl Technique for Flowtime {
    fn initial(&self) -> Phase {
        Phase::Focus { target: None }
    }

    fn next(&mut self, current: Phase, elapsed: Duration) -> Phase {
        match current {
            Phase::Focus { .. } => Phase::Break {
                target: Some(elapsed / self.break_divisor),
            },
            Phase::Break { .. } => Phase::Focus { target: None },
        }
    }
}

/// A serialisable technique — classic [`Pomodoro`] or [`Flowtime`]. A persisted focus session
/// stores this so the daemon and every `mgmt focus` invocation drive one shared engine across
/// processes (a `dyn Technique` can't round-trip through JSON; this enum can).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Engine {
    Pomodoro(Pomodoro),
    Flowtime(Flowtime),
}

impl Technique for Engine {
    fn initial(&self) -> Phase {
        match self {
            Engine::Pomodoro(p) => p.initial(),
            Engine::Flowtime(f) => f.initial(),
        }
    }

    fn next(&mut self, current: Phase, elapsed: Duration) -> Phase {
        match self {
            Engine::Pomodoro(p) => p.next(current, elapsed),
            Engine::Flowtime(f) => f.next(current, elapsed),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pomodoro_inserts_long_break_every_fourth_focus() {
        let mut p = Pomodoro::new(
            Duration::from_secs(1500),
            Duration::from_secs(300),
            Duration::from_secs(900),
            4,
        );
        let mut phase = p.initial();
        assert!(phase.is_focus());
        let mut breaks = Vec::new();
        for _ in 0..4 {
            phase = p.next(phase, Duration::from_secs(1500)); // a break
            breaks.push(phase.target().unwrap());
            phase = p.next(phase, phase.target().unwrap()); // back to focus
        }
        assert_eq!(breaks[0], Duration::from_secs(300));
        assert_eq!(breaks[3], Duration::from_secs(900)); // 4th is long
    }

    #[test]
    fn flowtime_break_is_proportional_to_focus() {
        let mut f = Flowtime::new(5);
        let focus = f.initial();
        let brk = f.next(focus, Duration::from_secs(50 * 60));
        assert_eq!(brk.target().unwrap(), Duration::from_secs(10 * 60));
    }
}
