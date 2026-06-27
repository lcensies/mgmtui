//! Background reminder daemon: polls due task/event reminders and fires each one's action —
//! a desktop notification, focusing mgmt on the event, or running a user hook. It runs headless
//! so reminders fire even when the TUI is closed, and persists a de-dup set across polls and
//! restarts so a reminder never re-fires within its window.
//!
//! Focusing is intentionally compositor-agnostic: a `navigate` reminder first tries to *raise*
//! an existing mgmt window via a strategy (`gnome` window-calls D-Bus, `wlroots` wdotool, or a
//! custom `command`), and otherwise spawns a fresh terminal running mgmt navigated to the event.
//! The spawn fallback needs nothing beyond a terminal emulator, so navigation always works.
//!
//! It also drives the optional status-bar widgets (pomodoro + next event) on a faster cadence:
//! it renders them and pushes to the configured backend (see [`crate::statusbar`]).

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::Duration;

use anyhow::Result;
use chrono::Utc;
use notify_rust::Notification;

use mgmt_config::{Config, FocusCfg};
use mgmt_core::Uid;
use mgmt_domain::Event;
use mgmt_service::{
    pomodoro_path, wire_payload, HitAction, MgmtContext, Phase, PomodoroState, ReminderHit, StatusSnapshot,
};

use crate::statusbar::{self, Update};

/// Run the daemon loop forever, interleaving two cadences: reminder checks at the poll interval,
/// and a status-bar refresh at the (faster) status interval so the pomodoro countdown stays live
/// and phase transitions fire on time. `poll_override` (the `--poll` flag) wins for the former.
pub fn run(root: &Path, cfg: Config, mut ctx: MgmtContext, poll_override: Option<u64>) -> Result<()> {
    let poll = poll_override.unwrap_or(cfg.daemon().poll_seconds).max(1);
    let sb = &cfg.daemon().status_bar;
    let interval = sb.interval_seconds.max(1);
    let horizon = chrono::Duration::hours(sb.next_event_horizon_hours.max(1) as i64);
    let pomo_path = pomodoro_path(root);
    // The bar execs this on click (e.g. `mgmt focus toggle`); the daemon knows its own path.
    let bin = std::env::current_exe().ok().map(|p| p.to_string_lossy().into_owned());

    let mut state = FiredState::load(state_path(root));
    let mut bar = statusbar::select(sb);
    let mut last_push: Option<String> = None;
    let mut next_ev: Option<Event> = None;
    let mut since_reload = u64::MAX; // force a reminder pass on the first tick
    let mut recalc_event = true;

    eprintln!(
        "mgmt daemon: started (reminders every {poll}s, status every {interval}s, bar: {})",
        bar.as_ref().map(|b| b.name()).unwrap_or("off")
    );

    loop {
        let now = Utc::now();

        // Reminder checks + context reload at the (slower) poll cadence.
        if since_reload >= poll {
            if let Err(e) = ctx.reload() {
                eprintln!("mgmt daemon: reload failed: {e}");
            }
            let fired = state.keys();
            for hit in ctx.pending_reminders(now, &fired) {
                fire(&hit, &cfg);
                state.mark(hit.key, now.timestamp());
            }
            // Forget reminders older than two days so the state file stays small.
            state.prune(now.timestamp() - 2 * 86_400);
            if let Err(e) = state.save() {
                eprintln!("mgmt daemon: could not persist state: {e}");
            }
            since_reload = 0;
            recalc_event = true; // cache changed → re-find the next event
        }

        // Status-bar refresh at the (faster) status cadence.
        if let Some(bar) = bar.as_mut() {
            // The shared pomodoro session is the source of truth. Auto-advance a finished phase
            // (firing a one-shot notification) and persist it so every reader agrees.
            let mut pomo = sb.show_pomodoro.then(|| PomodoroState::load(&pomo_path)).flatten();
            if let Some(p) = pomo.as_mut() {
                if let Some(phase) = p.tick(now) {
                    let _ = p.save(&pomo_path);
                    notify_phase(phase);
                }
            }
            // Re-find the next event after a reload, or once the cached one has started.
            if sb.show_next_event && (recalc_event || next_ev.as_ref().map(|e| e.start <= now).unwrap_or(false)) {
                next_ev = ctx.next_event(now, horizon);
                recalc_event = false;
            }
            let next_ref = sb.show_next_event.then_some(next_ev.as_ref()).flatten();

            let snap = StatusSnapshot::compute(pomo.as_ref(), next_ref, now);
            if snap.is_empty() {
                if last_push.as_deref() != Some("") {
                    bar.clear();
                    last_push = Some(String::new());
                }
            } else {
                let mut json = wire_payload(pomo.as_ref(), next_ref, now);
                if let (Some(obj), Some(bin)) = (json.as_object_mut(), bin.as_ref()) {
                    obj.insert("bin".into(), serde_json::Value::String(bin.clone()));
                }
                let text = snap.render();
                let update = Update { json: &json, text: &text };
                let key = bar.change_key(&update);
                if last_push.as_deref() != Some(key.as_str()) {
                    bar.set(&update);
                    last_push = Some(key);
                }
            }
        }

        thread::sleep(Duration::from_secs(interval));
        since_reload = since_reload.saturating_add(interval);
    }
}

fn fire(hit: &ReminderHit, cfg: &Config) {
    match &hit.action {
        HitAction::Notify => notify(&hit.title, &hit.body),
        HitAction::Navigate { event } => {
            notify(&hit.title, &hit.body);
            focus_event(event, cfg);
        }
        HitAction::Run { command, args } => run_hook(command, args),
    }
}

fn notify(summary: &str, body: &str) {
    if let Err(e) = Notification::new().summary(summary).body(body).appname("mgmt").show() {
        eprintln!("mgmt daemon: notification failed: {e}");
    }
}

/// Fire the one-shot "phase done" notification when the daemon auto-advances the pomodoro. The
/// argument is the *new* phase: entering a break means focus just ended, and vice versa.
fn notify_phase(phase: Phase) {
    let (title, body) = match phase {
        Phase::Focus { .. } => ("Break over", "Back to focus."),
        Phase::Break { .. } => ("Focus done", "Time for a break."),
    };
    notify(title, body);
}

/// Spawn a user hook (the `run` alarm action). Placeholders are already expanded; we detach the
/// process and never wait, so a slow or hanging hook can't stall the poll loop.
fn run_hook(command: &str, args: &[String]) {
    if command.is_empty() {
        return;
    }
    if let Err(e) = Command::new(command)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    {
        eprintln!("mgmt daemon: hook '{command}' failed to start: {e}");
    }
}

/// Bring mgmt to `uid`: raise an existing window if a strategy succeeds, else open a fresh one.
fn focus_event(uid: &Uid, cfg: &Config) {
    let focus = &cfg.daemon().focus;
    if raise_window(focus) {
        return;
    }
    spawn_mgmt(uid, cfg);
}

fn raise_window(focus: &FocusCfg) -> bool {
    let strategy = if focus.strategy == "auto" { detect_strategy() } else { focus.strategy.as_str() };
    match strategy {
        "gnome" => raise_gnome(&focus.window_title),
        "wlroots" => raise_wlroots(&focus.window_title),
        "command" => raise_command(&focus.command, &focus.window_title),
        _ => false, // "spawn" / "none" / unknown → never raise; always open fresh
    }
}

/// Best-effort detection of a window-raising strategy from the session environment.
fn detect_strategy() -> &'static str {
    let env = |k: &str| std::env::var(k).unwrap_or_default();
    if !env("SWAYSOCK").is_empty() || !env("HYPRLAND_INSTANCE_SIGNATURE").is_empty() {
        return "wlroots";
    }
    let desktop = env("XDG_CURRENT_DESKTOP").to_lowercase();
    if ["sway", "hyprland", "wlroots", "river", "wayfire"].iter().any(|d| desktop.contains(d)) {
        return "wlroots";
    }
    if desktop.contains("gnome") {
        return "gnome";
    }
    "spawn"
}

/// GNOME: list windows via the `window-calls` extension's D-Bus API, find the mgmt window by
/// title or wm_class, and activate it.
fn raise_gnome(title: &str) -> bool {
    const OBJ: &str = "/org/gnome/Shell/Extensions/Windows";
    const IFACE: &str = "org.gnome.Shell.Extensions.Windows";
    let list = run_capture(
        "gdbus",
        &["call", "--session", "--dest", "org.gnome.Shell", "--object-path", OBJ, "--method", &format!("{IFACE}.List")],
    );
    let Some(out) = list else { return false };
    let Some(json) = extract_json_array(&out) else { return false };
    let Ok(wins) = serde_json::from_str::<Vec<serde_json::Value>>(json) else { return false };
    let needle = title.to_lowercase();
    for w in &wins {
        let t = w.get("title").and_then(|v| v.as_str()).unwrap_or("").to_lowercase();
        let c = w.get("wm_class").and_then(|v| v.as_str()).unwrap_or("").to_lowercase();
        if !t.contains(&needle) && !c.contains(&needle) {
            continue;
        }
        let id = match w.get("id") {
            Some(serde_json::Value::Number(n)) => n.to_string(),
            Some(serde_json::Value::String(s)) => s.clone(),
            _ => continue,
        };
        return run_status(
            "gdbus",
            &["call", "--session", "--dest", "org.gnome.Shell", "--object-path", OBJ, "--method", &format!("{IFACE}.Activate"), &id],
        );
    }
    false
}

/// wlroots compositors (Sway/Hyprland/…): use wdotool's foreign-toplevel window management.
fn raise_wlroots(title: &str) -> bool {
    let Some(out) = run_capture("wdotool", &["search", "--name", title]) else { return false };
    let Some(id) = out.split_whitespace().next() else { return false };
    if id.is_empty() {
        return false;
    }
    run_status("wdotool", &["windowactivate", id])
}

/// A fully custom raise command; `{title}` is substituted. A zero exit status means it raised one.
fn raise_command(cmd: &[String], title: &str) -> bool {
    let Some((head, tail)) = cmd.split_first() else { return false };
    let args: Vec<String> = tail.iter().map(|a| a.replace("{title}", title)).collect();
    let argv: Vec<&str> = args.iter().map(String::as_str).collect();
    run_status(head, &argv)
}

/// Open a fresh terminal running `mgmt tui --event <uid>`. The freshly mapped window legitimately
/// takes focus on every compositor, so this is the universal navigation path.
fn spawn_mgmt(uid: &Uid, cfg: &Config) {
    let Some(mut argv) = resolve_terminal(&cfg.daemon().terminal) else {
        eprintln!("mgmt daemon: no terminal found; set daemon.terminal in config to enable navigate");
        return;
    };
    let mgmt = std::env::current_exe()
        .ok()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|| "mgmt".into());
    argv.push(mgmt);
    argv.extend(["tui".to_string(), "--event".to_string(), uid.to_string()]);
    let (head, tail) = argv.split_first().expect("terminal argv is non-empty");
    if let Err(e) = Command::new(head)
        .args(tail)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    {
        eprintln!("mgmt daemon: failed to open terminal ({head}): {e}");
    }
}

/// The terminal-command prefix to run mgmt under: the configured value if any, else the first
/// known emulator found on `PATH` with its program-passing flag.
fn resolve_terminal(configured: &[String]) -> Option<Vec<String>> {
    if !configured.is_empty() {
        return Some(configured.to_vec());
    }
    const TABLE: &[(&str, &[&str])] = &[
        ("kitty", &["-e"]),
        ("alacritty", &["-e"]),
        ("foot", &[]),
        ("wezterm", &["start", "--"]),
        ("gnome-terminal", &["--"]),
        ("konsole", &["-e"]),
        ("xterm", &["-e"]),
        ("x-terminal-emulator", &["-e"]),
    ];
    for (bin, sep) in TABLE {
        if which(bin) {
            let mut v = vec![(*bin).to_string()];
            v.extend(sep.iter().map(|s| (*s).to_string()));
            return Some(v);
        }
    }
    None
}

fn which(bin: &str) -> bool {
    let Ok(path) = std::env::var("PATH") else { return false };
    std::env::split_paths(&path).any(|d| d.join(bin).is_file())
}

fn run_capture(bin: &str, args: &[&str]) -> Option<String> {
    let out = Command::new(bin).args(args).stdin(Stdio::null()).stderr(Stdio::null()).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

fn run_status(bin: &str, args: &[&str]) -> bool {
    Command::new(bin)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Slice out the first `[ … ]` JSON array from gdbus's `('…',)` GVariant wrapper.
fn extract_json_array(s: &str) -> Option<&str> {
    let start = s.find('[')?;
    let end = s.rfind(']')?;
    (end > start).then(|| &s[start..=end])
}

fn state_path(root: &Path) -> PathBuf {
    root.join(".state").join("reminders.json")
}

/// Persistent de-dup set: reminder key → epoch seconds it fired. Survives daemon restarts.
struct FiredState {
    path: PathBuf,
    fired: HashMap<String, i64>,
}

impl FiredState {
    fn load(path: PathBuf) -> Self {
        let fired = std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str::<HashMap<String, i64>>(&s).ok())
            .unwrap_or_default();
        FiredState { path, fired }
    }

    fn keys(&self) -> HashSet<String> {
        self.fired.keys().cloned().collect()
    }

    fn mark(&mut self, key: String, ts: i64) {
        self.fired.insert(key, ts);
    }

    fn prune(&mut self, before: i64) {
        self.fired.retain(|_, ts| *ts >= before);
    }

    fn save(&self) -> std::io::Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string(&self.fired).unwrap_or_else(|_| "{}".into());
        std::fs::write(&self.path, json)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_json_array_from_gdbus_wrapper() {
        let raw = "('[{\"id\": 42, \"wm_class\": \"kitty\", \"title\": \"mgmt\"}]',)\n";
        let json = extract_json_array(raw).unwrap();
        let wins: Vec<serde_json::Value> = serde_json::from_str(json).unwrap();
        assert_eq!(wins[0].get("id").unwrap().as_i64(), Some(42));
        assert_eq!(wins[0].get("title").unwrap().as_str(), Some("mgmt"));
    }

    #[test]
    fn resolve_terminal_prefers_configured() {
        let cfg = vec!["myterm".to_string(), "-x".to_string()];
        assert_eq!(resolve_terminal(&cfg), Some(cfg));
    }

    #[test]
    fn fired_state_round_trips_and_prunes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("reminders.json");
        let mut s = FiredState::load(path.clone());
        s.mark("event:a:1".into(), 1000);
        s.mark("event:b:2".into(), 5000);
        s.save().unwrap();

        let mut reloaded = FiredState::load(path);
        assert!(reloaded.keys().contains("event:a:1"));
        assert!(reloaded.keys().contains("event:b:2"));
        reloaded.prune(2000); // drops the ts=1000 entry
        assert!(!reloaded.keys().contains("event:a:1"));
        assert!(reloaded.keys().contains("event:b:2"));
    }
}
