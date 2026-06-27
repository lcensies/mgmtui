//! `mgmt focus …` and `mgmt status` — the control + read surface for the daemon-managed status
//! widgets. Focus mutations write the shared `.state/pomodoro.json` session; the daemon picks the
//! change up on its next tick and pushes it to the bar (so a `mgmt focus toggle` bound to a key,
//! or the GNOME widget's click, updates the top bar within a second). No IPC: the file *is* the
//! contract between this command, the daemon, and any other reader.

use std::path::Path;

use anyhow::Result;
use chrono::{DateTime, Utc};
use clap::Subcommand;

use mgmt_config::Config;
use mgmt_service::{pomodoro_path, MgmtContext, PomodoroState, StatusSnapshot};

#[derive(Subcommand)]
pub enum FocusCmd {
    /// Start a focus session — a standard 25/5/15 pomodoro, or open-ended flowtime.
    Start {
        /// Open-ended focus; the break is sized to the focus length when you skip.
        #[arg(long)]
        flowtime: bool,
    },
    /// Pause a running session, or resume a paused one.
    Toggle,
    /// Skip to the next phase (focus ↔ break).
    Skip,
    /// Stop and clear the session.
    Stop,
}

pub fn run_focus(root: &Path, action: FocusCmd) -> Result<()> {
    let path = pomodoro_path(root);
    let now = Utc::now();
    match action {
        FocusCmd::Start { flowtime } => {
            let s = if flowtime { PomodoroState::start_flowtime(now) } else { PomodoroState::start_pomodoro(now) };
            s.save(&path)?;
            print_focus(&s, now);
        }
        FocusCmd::Toggle => mutate(&path, now, PomodoroState::toggle)?,
        FocusCmd::Skip => mutate(&path, now, PomodoroState::skip)?,
        FocusCmd::Stop => match std::fs::remove_file(&path) {
            Ok(()) => println!("focus: stopped"),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => println!("focus: no active session"),
            Err(e) => return Err(e.into()),
        },
    }
    Ok(())
}

/// Load → mutate → persist → echo. A missing session is a no-op with a hint (never an error).
fn mutate(path: &Path, now: DateTime<Utc>, f: impl FnOnce(&mut PomodoroState, DateTime<Utc>)) -> Result<()> {
    match PomodoroState::load(path) {
        Some(mut s) => {
            f(&mut s, now);
            s.save(path)?;
            print_focus(&s, now);
        }
        None => println!("focus: no active session (run `mgmt focus start`)"),
    }
    Ok(())
}

fn print_focus(s: &PomodoroState, now: DateTime<Utc>) {
    println!("{}", StatusSnapshot::compute(Some(s), None, now).render());
}

/// `mgmt status [--json]` — the combined status line (pomodoro + next event). The pull-based
/// counterpart to the daemon's push: a bar that polls can call this directly.
pub fn cmd_status(ctx: &MgmtContext, cfg: &Config, root: &Path, json: bool) -> Result<()> {
    let now = Utc::now();
    let pomo = PomodoroState::load(&pomodoro_path(root));
    let horizon = chrono::Duration::hours(cfg.daemon().status_bar.next_event_horizon_hours.max(1) as i64);
    let next = ctx.next_event(now, horizon);
    let snap = StatusSnapshot::compute(pomo.as_ref(), next.as_ref(), now);
    if json {
        println!("{}", serde_json::to_string(&snap)?);
    } else {
        println!("{}", snap.render());
    }
    Ok(())
}
