//! Human-editable YAML configuration for mgmt, loaded from `$XDG_CONFIG_HOME/mgmt/config.yaml`.
//!
//! One file drives the app's taxonomy: the task [`Workflow`] (statuses), project colors, default
//! reminders, theme palette overrides, the Tasks-view smart lists, and CalDAV accounts/
//! collections. Everything is optional — an absent file yields [`Config::default`], which behaves
//! exactly like the original hard-coded build.
//!
//! Colors are kept as plain strings here (matching `mgmt_domain::Collection.color`); turning a
//! string into a concrete `ratatui::Color` stays in `mgmt-tui` so this crate has no UI deps.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use mgmt_core::{Error, Result};
use mgmt_domain::{auto_color, Alarm, AlarmAction, ReminderOffset, SmartView, StatusDef, Workflow};

/// Calendar view display settings.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct CalendarCfg {
    /// Show `HH:MM–HH:MM` end time in the day agenda panel (default: true).
    pub show_end_time: bool,
    /// How many event-label rows to render below each date row in the month grid (0 = dots only).
    /// Values above 3 are clamped to 3. Enabling this widens the month panel automatically.
    pub month_event_lines: u8,
    /// Style of the right-hand panel in month view.
    ///   "grid" (default) — Google-Calendar-style time-block grid for the selected day.
    ///   "list"           — compact agenda list (the old behaviour).
    pub month_panel_style: String,
    /// Color palette for events that have no explicit project color. Each entry is a color string
    /// (name, `#rrggbb`, or ANSI index). Events are assigned colors by a stable hash of their
    /// calendar name, so every event from the same calendar shares a color. Empty → theme.event.
    pub event_palette: Vec<String>,
}

impl Default for CalendarCfg {
    fn default() -> Self {
        CalendarCfg {
            show_end_time: true,
            month_event_lines: 0,
            month_panel_style: "grid".into(),
            event_palette: vec![],
        }
    }
}

/// The whole config tree. All sections default to empty.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Override the data root (where tasks/calendars live). Falls back to `$XDG_DATA_HOME/mgmt`.
    pub data_dir: Option<PathBuf>,
    /// Ordered task statuses / kanban columns. Empty → the built-in five.
    statuses: Vec<StatusDef>,
    /// Per-project overrides keyed by project name (currently just color).
    projects: BTreeMap<String, ProjectCfg>,
    reminders: RemindersCfg,
    /// Palette overrides keyed by theme slot ("accent", "today", …); values are color strings.
    theme: BTreeMap<String, String>,
    /// Tasks-view smart lists, by id. Empty → all built-in views.
    views: Vec<ViewCfg>,
    /// Calendar display settings.
    calendar: CalendarCfg,
    /// CalDAV credentials blocks (consumed by `mgmt sync`).
    pub accounts: Vec<Account>,
    /// Local collections mirrored to remote CalDAV.
    pub collections: Vec<Collection>,
    /// Background reminder daemon (`mgmt daemon`) settings.
    daemon: DaemonCfg,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
struct ProjectCfg {
    color: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
struct RemindersCfg {
    /// Reminders applied to a task when it gets a due date but no explicit reminders.
    defaults: Vec<ReminderOffset>,
    /// Alarms applied to a new event that specifies none. Empty → a single notification 15
    /// minutes before start.
    event_defaults: Vec<EventAlarmCfg>,
}

/// A configurable default alarm for new events.
#[derive(Debug, Clone, Deserialize)]
struct EventAlarmCfg {
    /// Minutes before the event start.
    minutes: i64,
    /// `notify` (default), `navigate`, or `run`.
    #[serde(default)]
    action: AlarmActionKind,
    /// For `action: run` — the binary to execute.
    #[serde(default)]
    command: Option<String>,
    /// For `action: run` — its arguments (event-field placeholders allowed).
    #[serde(default)]
    args: Vec<String>,
}

#[derive(Debug, Clone, Copy, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
enum AlarmActionKind {
    #[default]
    Notify,
    Navigate,
    Run,
}

impl EventAlarmCfg {
    fn to_alarm(&self) -> Alarm {
        let action = match self.action {
            AlarmActionKind::Notify => AlarmAction::Notify,
            AlarmActionKind::Navigate => AlarmAction::Navigate,
            AlarmActionKind::Run => AlarmAction::Run {
                command: self.command.clone().unwrap_or_default(),
                args: self.args.clone(),
            },
        };
        Alarm::with_action(self.minutes, action)
    }
}

/// Settings for the background reminder daemon spawned by `mgmt daemon`.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct DaemonCfg {
    /// Seconds between reminder checks.
    pub poll_seconds: u64,
    /// Command prefix used to open mgmt in a terminal for `navigate` reminders; the mgmt
    /// invocation is appended (e.g. `["kitty", "-e"]` → `kitty -e mgmt tui --event <uid>`).
    /// Empty → auto-detect a terminal emulator at runtime.
    pub terminal: Vec<String>,
    /// How to raise an already-running mgmt window before spawning a fresh one.
    pub focus: FocusCfg,
    /// Status-bar widgets (pomodoro + next event) pushed to a desktop bar.
    pub status_bar: StatusBarCfg,
}

impl Default for DaemonCfg {
    fn default() -> Self {
        DaemonCfg {
            poll_seconds: 30,
            terminal: Vec::new(),
            focus: FocusCfg::default(),
            status_bar: StatusBarCfg::default(),
        }
    }
}

/// Status-bar widgets driven by the daemon: a compact line carrying the pomodoro session and the
/// next event, pushed to a desktop bar. `auto` shows it on GNOME (via the bundled shell
/// extension) and is otherwise off unless you pick the `command`/`file` backend for another bar.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct StatusBarCfg {
    /// Master switch for the status-bar push.
    pub enabled: bool,
    /// `auto` (GNOME if a GNOME session, else off), `gnome`, `command`, `file`, or `none`.
    pub backend: String,
    /// Seconds between refreshes. The GNOME extension animates the countdown itself, so this only
    /// bounds how fast a phase / next-event change is reflected there; for the `command`/`file`
    /// backends it is the text-refresh cadence.
    pub interval_seconds: u64,
    /// Include the pomodoro widget.
    pub show_pomodoro: bool,
    /// Include the next-event widget.
    pub show_next_event: bool,
    /// How far ahead to look for the next event.
    pub next_event_horizon_hours: u64,
    /// `command` backend: argv with the rendered line appended as the final argument
    /// (e.g. `[my-bar]` → `my-bar "Focus 23:14 · Standup in 25m"`).
    pub command: Vec<String>,
    /// `file` backend: write the rendered line here (a bar can watch/tail it).
    pub file: Option<PathBuf>,
}

impl Default for StatusBarCfg {
    fn default() -> Self {
        StatusBarCfg {
            enabled: true,
            backend: "auto".into(),
            interval_seconds: 1,
            show_pomodoro: true,
            show_next_event: true,
            next_event_horizon_hours: 168,
            command: Vec::new(),
            file: None,
        }
    }
}

/// How a `navigate` reminder raises an existing mgmt window. The universal fallback (used when a
/// strategy finds no window or none is configured) is to spawn a fresh terminal navigated to the
/// event, so this never has to work for navigation to function.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct FocusCfg {
    /// `auto` (detect from the session), `gnome` (window-calls D-Bus), `wlroots` (wdotool),
    /// `command` (run `command`), or `spawn` (never raise; always open a fresh window).
    pub strategy: String,
    /// The terminal-window title mgmt sets and the raise strategy matches against.
    pub window_title: String,
    /// For `strategy: command` — argv used to raise the window; `{title}` is substituted and a
    /// zero exit status means a window was raised.
    pub command: Vec<String>,
}

impl Default for FocusCfg {
    fn default() -> Self {
        FocusCfg { strategy: "auto".into(), window_title: "mgmt".into(), command: Vec::new() }
    }
}

#[derive(Debug, Clone, Deserialize)]
struct ViewCfg {
    id: String,
}

/// A CalDAV credentials block. Mapping to a concrete auth scheme lives in `mgmt-cli` so this
/// crate need not depend on `mgmt-sync`.
#[derive(Debug, Clone, Deserialize)]
pub struct Account {
    pub name: String,
    /// `basic`, `bearer`, or `none`.
    #[serde(default = "default_auth_kind")]
    pub auth: String,
    #[serde(default)]
    pub username: Option<String>,
    #[serde(default)]
    pub password: Option<String>,
    #[serde(default)]
    pub token: Option<String>,
}

fn default_auth_kind() -> String {
    "basic".into()
}

/// A local collection mirrored to a remote CalDAV collection.
#[derive(Debug, Clone, Deserialize)]
pub struct Collection {
    /// Local collection / vault-project name.
    pub name: String,
    /// `events` or `tasks`.
    pub kind: String,
    /// Remote CalDAV collection URL.
    pub url: String,
    /// Name of the account block to authenticate with.
    pub account: String,
}

impl Config {
    /// Default config path: `$XDG_CONFIG_HOME/mgmt/config.yaml`.
    pub fn default_path() -> Result<PathBuf> {
        let dirs = directories::ProjectDirs::from("", "", "mgmt")
            .ok_or_else(|| Error::Other("cannot resolve config dir".into()))?;
        Ok(dirs.config_dir().join("config.yaml"))
    }

    /// Load config from `path`, returning [`Config::default`] if it does not exist.
    pub fn load(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Config::default());
        }
        let text = std::fs::read_to_string(path)?;
        serde_yaml::from_str(&text).map_err(|e| Error::Parse(format!("parsing {}: {e}", path.display())))
    }

    /// The configured task workflow (statuses + kanban columns), or the built-in default.
    pub fn workflow(&self) -> Workflow {
        Workflow::new(self.statuses.clone())
    }

    /// An explicit color override for `project` from config, if any (no auto fallback).
    pub fn project_color_override(&self, project: &str) -> Option<String> {
        self.projects.get(project).and_then(|p| p.color.clone())
    }

    /// The color for `project`: an explicit override if present, else a stable auto-assigned one.
    pub fn project_color(&self, project: &str) -> String {
        self.project_color_override(project).unwrap_or_else(|| auto_color(project).to_string())
    }

    /// Reminder offsets applied to a task that gains a due date with none of its own.
    pub fn reminder_defaults(&self) -> &[ReminderOffset] {
        &self.reminders.defaults
    }

    /// Alarms applied to a newly-created event that specifies none of its own. An empty config
    /// section yields a single notification 15 minutes before start.
    pub fn event_alarm_defaults(&self) -> Vec<Alarm> {
        if self.reminders.event_defaults.is_empty() {
            vec![Alarm::minutes_before(15)]
        } else {
            self.reminders.event_defaults.iter().map(|c| c.to_alarm()).collect()
        }
    }

    /// Background reminder daemon settings.
    pub fn daemon(&self) -> &DaemonCfg {
        &self.daemon
    }

    /// Theme palette overrides (slot name → color string).
    pub fn theme_overrides(&self) -> &BTreeMap<String, String> {
        &self.theme
    }

    /// The Tasks-view smart lists, in order. Unknown ids are dropped; an empty/invalid config
    /// yields every built-in view.
    pub fn views(&self) -> Vec<SmartView> {
        let mut out: Vec<SmartView> = self.views.iter().filter_map(|v| SmartView::from_id(&v.id)).collect();
        out.dedup();
        if out.is_empty() {
            SmartView::ALL.to_vec()
        } else {
            out
        }
    }

    pub fn account(&self, name: &str) -> Option<&Account> {
        self.accounts.iter().find(|a| a.name == name)
    }

    pub fn calendar(&self) -> &CalendarCfg {
        &self.calendar
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_file_is_default_config() {
        let cfg = Config::load(Path::new("/nonexistent/mgmt/config.yaml")).unwrap();
        assert!(cfg.accounts.is_empty());
        assert_eq!(cfg.workflow(), Workflow::builtin());
        assert_eq!(cfg.views(), SmartView::ALL.to_vec());
    }

    #[test]
    fn parses_full_config() {
        let yaml = r#"
statuses:
  - { id: todo, label: Inbox }
  - { id: doing, color: cyan, kind: active }
  - { id: done, kind: done }
projects:
  wng: { color: blue }
reminders:
  defaults: [1d, 2h]
  event_defaults:
    - { minutes: 30 }
    - { minutes: 10, action: navigate }
    - { minutes: 5, action: run, command: notify-send, args: ["hi", "{summary}"] }
daemon:
  poll_seconds: 15
  terminal: [kitty, -e]
  focus: { strategy: gnome, window_title: mgmt }
theme:
  accent: magenta
views:
  - { id: today }
  - { id: inbox }
accounts:
  - { name: home, auth: basic, username: u, password: p }
collections:
  - { name: work, kind: events, url: "http://localhost/dav/", account: home }
"#;
        let cfg: Config = serde_yaml::from_str(yaml).unwrap();
        let wf = cfg.workflow();
        assert_eq!(wf.order().len(), 3);
        assert_eq!(wf.label("todo"), "Inbox");
        assert!(wf.is_done("done"));
        assert_eq!(cfg.project_color("wng"), "blue");
        // unconfigured project gets a stable auto color
        assert_eq!(cfg.project_color("home"), cfg.project_color("home"));
        assert_eq!(cfg.reminder_defaults().len(), 2);
        assert_eq!(cfg.theme_overrides().get("accent").unwrap(), "magenta");
        assert_eq!(cfg.views(), vec![SmartView::Today, SmartView::Inbox]);
        assert_eq!(cfg.accounts.len(), 1);
        assert_eq!(cfg.collections[0].kind, "events");

        let alarms = cfg.event_alarm_defaults();
        assert_eq!(alarms.len(), 3);
        assert_eq!(alarms[0].action, AlarmAction::Notify);
        assert_eq!(alarms[0].minutes(), 30);
        assert_eq!(alarms[1].action, AlarmAction::Navigate);
        assert_eq!(
            alarms[2].action,
            AlarmAction::Run { command: "notify-send".into(), args: vec!["hi".into(), "{summary}".into()] }
        );
        assert_eq!(cfg.daemon().poll_seconds, 15);
        assert_eq!(cfg.daemon().terminal, vec!["kitty".to_string(), "-e".to_string()]);
        assert_eq!(cfg.daemon().focus.strategy, "gnome");

        // An empty config yields the built-in 15-minute notify default.
        let default_alarms = Config::default().event_alarm_defaults();
        assert_eq!(default_alarms.len(), 1);
        assert_eq!(default_alarms[0].minutes(), 15);
        assert_eq!(default_alarms[0].action, AlarmAction::Notify);
    }
}
