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

use serde::{Deserialize, Serialize};

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
    /// Local calendar metadata (display name + color), managed from the web UI.
    pub calendars: Vec<CalendarEntry>,
    /// Background reminder daemon (`mgmt daemon`) settings.
    daemon: DaemonCfg,
    /// Encrypted remote backups (`mgmt backup`). Absent → backups disabled.
    backup: Option<BackupCfg>,
    /// Web server (`mgmt web`) settings.
    web: WebCfg,
}

/// Settings for the HTTP/JSON API + PWA server (`mgmt web`).
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct WebCfg {
    /// Address to bind. Loopback by default; put a TLS reverse proxy (Caddy/nginx) in front.
    pub bind: String,
    /// Public origin the app is served from, e.g. `https://mgmt.example.com`. Enables the cookie
    /// `Secure` attribute and the mutation Origin (CSRF) check. Empty → not enforced.
    pub public_origin: String,
    /// Rolling browser-session lifetime, in days.
    pub session_ttl_days: u64,
}

impl Default for WebCfg {
    fn default() -> Self {
        WebCfg { bind: "127.0.0.1:8321".into(), public_origin: String::new(), session_ttl_days: 30 }
    }
}

/// Encrypted, provider-agnostic backup settings. Backups are `tar.zst` snapshots pushed to an
/// rclone remote; point `remote` at an `rclone crypt` wrapper and encryption is rclone's job (mgmt
/// stores no passphrase). Absent config section → `mgmt backup` reports that backups are disabled.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct BackupCfg {
    /// rclone remote and path, e.g. `crypt-b2:mgmt-backups`. Required to enable backups.
    pub remote: String,
    /// Always keep at least this many most-recent snapshots.
    pub keep_last: usize,
    /// Age cap: snapshots older than this many days are pruned even within `keep_last`
    /// (0 = no age cap). The newest snapshot is always kept regardless.
    pub keep_days: u32,
    /// Path/name of the rclone binary.
    pub rclone_binary: String,
    /// Bundle `config.yaml` (and `web-auth.yaml`) into each snapshot.
    pub include_config: bool,
}

impl Default for BackupCfg {
    fn default() -> Self {
        BackupCfg {
            remote: String::new(),
            keep_last: 14,
            keep_days: 180,
            rclone_binary: "rclone".into(),
            include_config: true,
        }
    }
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
#[derive(Debug, Clone, Serialize, Deserialize)]
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

/// A local collection mirrored to a remote server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Collection {
    /// Local collection / vault-project name.
    pub name: String,
    /// `events` or `tasks`.
    pub kind: String,
    /// Remote URL. For `caldav` this is the CalDAV collection; for `mgmt` it is the server's
    /// `/api/sync` base (e.g. `https://mgmt.example.com/api/sync`).
    pub url: String,
    /// Name of the account block to authenticate with.
    pub account: String,
    /// Sync protocol: `caldav` (default) or `mgmt` (the native `mgmt web` sync endpoint).
    #[serde(default = "default_protocol")]
    pub protocol: String,
}

fn default_protocol() -> String {
    "caldav".into()
}

/// Metadata for a local calendar (a directory under `<data_root>/calendars`). The directory is
/// authoritative for which events belong to it; this only carries presentation fields.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CalendarEntry {
    /// Collection id = directory name.
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
}

/// Rewrite just the `calendars:` block of `path`, keeping every other key.
///
/// ponytail: serde_yaml round-trips the document, so comments and key order in `config.yaml` are
/// lost on the first web-side calendar edit. Upgrade path: an edit-preserving YAML parser
/// (`yaml-rust2` / `serde_yaml::Value` spans) if that becomes a complaint.
pub fn save_calendars(path: &Path, calendars: &[CalendarEntry]) -> Result<()> {
    let mut doc: serde_yaml::Value = if path.exists() {
        let text = std::fs::read_to_string(path)?;
        serde_yaml::from_str(&text).map_err(|e| Error::Parse(format!("parsing {}: {e}", path.display())))?
    } else {
        serde_yaml::Value::Mapping(Default::default())
    };
    if !doc.is_mapping() {
        doc = serde_yaml::Value::Mapping(Default::default());
    }
    let value = serde_yaml::to_value(calendars).map_err(|e| Error::Other(format!("serializing calendars: {e}")))?;
    doc.as_mapping_mut()
        .expect("mapping")
        .insert(serde_yaml::Value::from("calendars"), value);
    let text = serde_yaml::to_string(&doc).map_err(|e| Error::Other(format!("serializing {}: {e}", path.display())))?;
    write_private(path, &text) // config.yaml may hold CalDAV secrets → owner-only, atomic
}

/// Web-managed CalDAV config, kept in a separate `caldav.yaml` so the web UI can add accounts and
/// collections without rewriting (and clobbering the comments in) the hand-edited `config.yaml`.
/// It is merged into [`Config`] at load; entries in `config.yaml` win on a name clash.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct CalDavFile {
    pub accounts: Vec<Account>,
    pub collections: Vec<Collection>,
}

impl CalDavFile {
    /// Default path: `caldav.yaml` alongside `config.yaml`.
    pub fn default_path() -> Result<PathBuf> {
        Ok(Config::default_path()?.with_file_name("caldav.yaml"))
    }

    /// The `caldav.yaml` path in a given config directory (the merge sibling of `config.yaml`).
    pub fn default_path_in(config_dir: &Path) -> PathBuf {
        config_dir.join("caldav.yaml")
    }

    pub fn load(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(CalDavFile::default());
        }
        let text = std::fs::read_to_string(path)?;
        serde_yaml::from_str(&text).map_err(|e| Error::Parse(format!("parsing {}: {e}", path.display())))
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        let text = serde_yaml::to_string(self).map_err(|e| Error::Other(format!("serializing caldav.yaml: {e}")))?;
        write_private(path, &text) // holds CalDAV passwords/tokens → owner-only, atomic
    }
}

/// Write a secrets file owner-readable-only (`0600`) and atomically: a sibling temp file created
/// with restrictive perms from the start (no world-readable window that `std::fs::write` + a
/// follow-up `chmod` would leave) is renamed over the target. On non-unix, best-effort.
pub fn write_private(path: &Path, contents: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension(format!(
        "{}.{}.tmp",
        path.extension().and_then(|e| e.to_str()).unwrap_or(""),
        std::process::id()
    ));
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&tmp)?;
        f.write_all(contents.as_bytes())?;
        f.sync_all().ok();
    }
    #[cfg(not(unix))]
    {
        std::fs::write(&tmp, contents)?;
    }
    if let Err(e) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e.into());
    }
    Ok(())
}

impl Config {
    /// Default config path: `$XDG_CONFIG_HOME/mgmt/config.yaml`.
    pub fn default_path() -> Result<PathBuf> {
        let dirs = directories::ProjectDirs::from("", "", "mgmt")
            .ok_or_else(|| Error::Other("cannot resolve config dir".into()))?;
        Ok(dirs.config_dir().join("config.yaml"))
    }

    /// Load config from `path`, returning [`Config::default`] if it does not exist. Also merges a
    /// sibling `caldav.yaml` (web-managed CalDAV accounts/collections), so both the CLI and the web
    /// see the same sync targets. On a name clash, `config.yaml` wins (hand-edited is authoritative).
    pub fn load(path: &Path) -> Result<Self> {
        let mut cfg = if path.exists() {
            let text = std::fs::read_to_string(path)?;
            serde_yaml::from_str(&text).map_err(|e| Error::Parse(format!("parsing {}: {e}", path.display())))?
        } else {
            Config::default()
        };
        let caldav = CalDavFile::load(&path.with_file_name("caldav.yaml"))?;
        for a in caldav.accounts {
            if !cfg.accounts.iter().any(|x| x.name == a.name) {
                cfg.accounts.push(a);
            }
        }
        for c in caldav.collections {
            if !cfg.collections.iter().any(|x| x.name == c.name) {
                cfg.collections.push(c);
            }
        }
        Ok(cfg)
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

    /// Backup settings, if a non-empty `remote` is configured (backups disabled otherwise).
    pub fn backup(&self) -> Option<&BackupCfg> {
        self.backup.as_ref().filter(|b| !b.remote.trim().is_empty())
    }

    /// Web server settings.
    pub fn web(&self) -> &WebCfg {
        &self.web
    }

    /// Theme palette overrides (slot name → color string).
    pub fn theme_overrides(&self) -> &BTreeMap<String, String> {
        &self.theme
    }

    /// The Tasks-view smart lists, in order. Unknown ids are dropped; an empty/invalid config
    /// yields every built-in view.
    pub fn views(&self) -> Vec<SmartView> {
        let mut out: Vec<SmartView> = self.views.iter().filter_map(|v| SmartView::from_id(&v.id)).collect();
        // Stable de-dup: `Vec::dedup` only drops *adjacent* repeats ([today, inbox, today]
        // would keep two `today` tabs).
        let mut seen = std::collections::HashSet::new();
        out.retain(|v| seen.insert(*v));
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
    fn caldav_yaml_merges_into_config_without_clobbering() {
        let dir = tempfile::tempdir().unwrap();
        let cfg_path = dir.path().join("config.yaml");
        // config.yaml owns account "hand"; caldav.yaml adds "web" and a duplicate-name "hand".
        std::fs::write(&cfg_path, "accounts:\n  - { name: hand, auth: basic }\n").unwrap();
        let cd = CalDavFile {
            accounts: vec![
                Account { name: "web".into(), auth: "bearer".into(), username: None, password: None, token: Some("t".into()) },
                Account { name: "hand".into(), auth: "none".into(), username: None, password: None, token: None },
            ],
            collections: vec![Collection { name: "cal".into(), kind: "events".into(), url: "https://x/".into(), account: "web".into(), protocol: "caldav".into() }],
        };
        cd.save(&CalDavFile::default_path_in(dir.path())).unwrap();

        let cfg = Config::load(&cfg_path).unwrap();
        // Both accounts present; the hand-edited "hand" wins over caldav.yaml's duplicate.
        assert_eq!(cfg.accounts.len(), 2);
        assert_eq!(cfg.account("hand").unwrap().auth, "basic");
        assert_eq!(cfg.account("web").unwrap().token.as_deref(), Some("t"));
        assert_eq!(cfg.collections.len(), 1);
        assert_eq!(cfg.collections[0].name, "cal");
    }

    #[test]
    fn backup_is_opt_in_and_defaults_fill() {
        // No section -> disabled.
        assert!(Config::default().backup().is_none());
        // Empty remote -> still disabled.
        let cfg: Config = serde_yaml::from_str("backup: { remote: '' }").unwrap();
        assert!(cfg.backup().is_none());
        // Remote set -> enabled, other fields default.
        let cfg: Config = serde_yaml::from_str("backup:\n  remote: 'crypt-b2:mgmt'\n").unwrap();
        let b = cfg.backup().expect("enabled");
        assert_eq!(b.remote, "crypt-b2:mgmt");
        assert_eq!(b.keep_last, 14);
        assert_eq!(b.keep_days, 180);
        assert_eq!(b.rclone_binary, "rclone");
        assert!(b.include_config);
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
