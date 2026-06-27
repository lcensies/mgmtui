//! Background reminder daemon: polls due task/event reminders and fires each one's action —
//! a desktop notification, focusing mgmt on the event, or running a user hook. It runs headless
//! so reminders fire even when the TUI is closed, and persists a de-dup set across polls and
//! restarts so a reminder never re-fires within its window.
//!
//! Focusing is intentionally compositor-agnostic: a `navigate` reminder first tries to *raise*
//! an existing mgmt window via a strategy (`gnome` window-calls D-Bus, `wlroots` wdotool, or a
//! custom `command`), and otherwise spawns a fresh terminal running mgmt navigated to the event.
//! The spawn fallback needs nothing beyond a terminal emulator, so navigation always works.

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
use mgmt_service::{HitAction, MgmtContext, ReminderHit};

/// Run the reminder loop forever. `poll_override` (the `--poll` flag) wins over the config.
pub fn run(root: &Path, cfg: Config, mut ctx: MgmtContext, poll_override: Option<u64>) -> Result<()> {
    let poll = poll_override.unwrap_or(cfg.daemon().poll_seconds).max(1);
    let mut state = FiredState::load(state_path(root));
    eprintln!("mgmt daemon: started, polling every {poll}s");
    loop {
        // Pick up edits made elsewhere (TUI, CLI, sync) since the previous tick.
        if let Err(e) = ctx.reload() {
            eprintln!("mgmt daemon: reload failed: {e}");
        }
        let now = Utc::now();
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
        thread::sleep(Duration::from_secs(poll));
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
