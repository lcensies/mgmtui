//! mgmt — local-first calendar + markdown tasks + kanban.
//!
//! This binary is the sole owner of the terminal (raw mode, alternate screen, event loop).
//! The `mgmt-tui` library stays terminal-agnostic so it can be embedded in other ratatui
//! hosts (e.g. the wng dashboard).

use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::{Context as _, Result};
use chrono::Utc;
use clap::{Parser, Subcommand};
use crossterm::event::{
    self, Event, KeyEventKind, KeyboardEnhancementFlags, PopKeyboardEnhancementFlags,
    PushKeyboardEnhancementFlags,
};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, SetTitle, disable_raw_mode, enable_raw_mode,
    supports_keyboard_enhancement,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

use mgmt_config::{Account, Config};
use mgmt_service::MgmtContext;
use mgmt_store::{VaultStore, VdirStore};
use mgmt_sync::{
    Auth, CalDavClient, HttpRemote, RusticalConfig, SyncReport, run_hook, sync_events,
    sync_events_http, sync_tasks, sync_tasks_http,
};
use mgmt_tui::{MgmtApp, Outcome};

mod backup;
mod crud;
mod daemon;
mod focus;
mod pair;
mod web;
mod datetime;
mod meta;
mod statusbar;
use crud::{EventCmd, TaskCmd};

#[derive(Parser)]
#[command(name = "mgmt", version, about = "Local-first calendar + markdown tasks + kanban")]
struct Cli {
    /// Override the data root (defaults to $XDG_DATA_HOME/mgmt).
    #[arg(long, global = true)]
    data_dir: Option<PathBuf>,

    #[command(subcommand)]
    cmd: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Launch the interactive TUI (default).
    Tui {
        /// Open focused on the event with this UID (a unique prefix is accepted).
        #[arg(long, value_name = "UID")]
        event: Option<String>,
        /// Open the calendar in a specific view: day, week, or month.
        #[arg(long, value_name = "VIEW")]
        view: Option<String>,
    },
    /// Quick-add a task to the vault.
    Add {
        /// Task title.
        title: Vec<String>,
        /// Project to file it under.
        #[arg(short, long)]
        project: Option<String>,
    },
    /// Create/edit/list/delete calendar events (full fields, incl. recurrence).
    Event {
        #[command(subcommand)]
        action: EventCmd,
    },
    /// Create/edit/list/delete tasks (full fields).
    Task {
        #[command(subcommand)]
        action: TaskCmd,
    },
    /// Import events from an iCalendar (.ics) file into a calendar collection.
    Import {
        path: PathBuf,
        #[arg(short, long, default_value = "default")]
        calendar: String,
    },
    /// Export a calendar collection (or all) as iCalendar to stdout.
    Export {
        #[arg(short, long)]
        calendar: Option<String>,
    },
    /// Sync collections with their remote CalDAV servers (and run all native pairings).
    Sync {
        /// Only sync this collection.
        target: Option<String>,
    },
    /// Manage persistent bidirectional sync pairings with remote `mgmt web` users.
    Pair {
        #[command(subcommand)]
        action: pair::PairCmd,
    },
    /// Google Calendar via OAuth: log in, and create Google Meet links on events.
    Google {
        #[command(subcommand)]
        action: GoogleCmd,
    },
    /// Run the bundled rustical CalDAV server (serves the vault to your phone).
    Serve,
    /// Run the background reminder daemon: fires notifications, focuses mgmt on events, and runs
    /// hooks even when the TUI is closed. Meant to be supervised (e.g. a systemd user service).
    Daemon {
        /// Seconds between reminder checks (overrides the config `daemon.poll_seconds`).
        #[arg(long)]
        poll: Option<u64>,
    },
    /// Control the daemon-managed pomodoro/flowtime session shown on the status bar.
    Focus {
        #[command(subcommand)]
        action: focus::FocusCmd,
    },
    /// Print the status line (pomodoro + next event) for a status bar (`--json` for scripting).
    Status {
        #[arg(long)]
        json: bool,
    },
    /// Emit the task-metadata schema as JSON (used by the editor/nvim completion plugin).
    Meta {
        #[arg(long)]
        json: bool,
    },
    /// Create/list/prune/verify encrypted vault snapshots on a remote (rclone crypt).
    Backup {
        #[command(subcommand)]
        action: backup::BackupCmd,
    },
    /// Restore the vault (or extract elsewhere) from a snapshot on the remote.
    Restore {
        /// Snapshot filename, unique prefix, or `latest`.
        name: String,
        /// Confirm overwriting the live data root (kept aside as `<data_root>.pre-restore-<ts>`).
        #[arg(long)]
        yes: bool,
        /// Extract to this directory for inspection instead of swapping into the data root.
        #[arg(long, value_name = "DIR")]
        to: Option<PathBuf>,
    },
    /// Serve the HTTP/JSON API + PWA, or manage its credentials. Bare `mgmt web` serves.
    Web {
        #[command(subcommand)]
        action: Option<web::WebCmd>,
    },
    /// One-time migration from the legacy "UTC wall-clock" convention: reinterpret every timed
    /// event and task due/scheduled stamp as *local* wall-clock and store the true UTC instant.
    /// Run once per vault after upgrading (all-day events are untouched).
    MigrateTz {
        /// Show what would change without writing anything.
        #[arg(long)]
        dry_run: bool,
        /// Apply without the confirmation prompt.
        #[arg(short, long)]
        yes: bool,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    // One YAML config drives everything; an absent file yields built-in defaults.
    let cfg = Config::load(&Config::default_path().map_err(anyerr)?).map_err(anyerr)?;
    // Data root precedence: --data-dir flag, then config `data_dir`, then XDG default.
    let data_root = match &cli.data_dir {
        Some(p) => p.clone(),
        None => match &cfg.data_dir {
            Some(p) => p.clone(),
            None => mgmt_store::data_root().map_err(anyerr)?,
        },
    };
    // Local commands operate on the effective vault root: `users/admin` once the data root has been
    // migrated to the multi-user layout, else the legacy root. `mgmt web` (below) keeps the raw data
    // root — it owns the whole `users/` tree.
    let root = mgmt_store::local_vault_root(&data_root);

    match cli.cmd.unwrap_or(Cmd::Tui { event: None, view: None }) {
        Cmd::Tui { event, view } => run_tui(&root, cfg, event, view),
        Cmd::Add { title, project } => cmd_add(&root, &cfg, title.join(" "), project),
        Cmd::Event { action } => {
            let mut ctx = open_context(&root, &cfg)?;
            crud::run_event(&mut ctx, action)
        }
        Cmd::Task { action } => {
            let mut ctx = open_context(&root, &cfg)?;
            crud::run_task(&mut ctx, action)
        }
        Cmd::Import { path, calendar } => cmd_import(&root, &cfg, &path, &calendar),
        Cmd::Export { calendar } => cmd_export(&root, &cfg, calendar.as_deref()),
        Cmd::Sync { target } => cmd_sync(&root, &cfg, target.as_deref()),
        Cmd::Pair { action } => pair::run_pair(&root, action),
        Cmd::Google { action } => cmd_google(&root, &cfg, action),
        Cmd::Serve => cmd_serve(&root),
        Cmd::Daemon { poll } => cmd_daemon(&root, cfg, poll),
        Cmd::Focus { action } => focus::run_focus(&root, action),
        Cmd::Status { json } => {
            let ctx = open_context(&root, &cfg)?;
            focus::cmd_status(&ctx, &cfg, &root, json)
        }
        Cmd::Meta { json: _ } => {
            let ctx = open_context(&root, &cfg)?;
            println!("{}", meta::schema_json(&ctx, &root));
            Ok(())
        }
        Cmd::Backup { action } => backup::run_backup(&data_root, &cfg, action),
        Cmd::Restore { name, yes, to } => backup::run_restore(&data_root, &cfg, name, yes, to),
        Cmd::Web { action } => web::run_web(&data_root, cfg, action),
        Cmd::MigrateTz { dry_run, yes } => cmd_migrate_tz(&root, &cfg, dry_run, yes),
    }
}

/// Reinterpret stored wall-clock-as-UTC stamps as local wall-clock (see `Cmd::MigrateTz`). The
/// shift equals the local UTC offset at each stamp, so nothing changes when TZ=UTC or the vault
/// was already migrated (running twice WOULD double-shift — hence the confirmation).
fn cmd_migrate_tz(root: &PathBuf, cfg: &Config, dry_run: bool, yes: bool) -> Result<()> {
    use chrono::TimeZone;
    // A persistent marker makes re-running a true no-op: the shift is unconditional (it always
    // finds "work"), so without this a second run would silently double-shift every time.
    let marker = root.join(".state").join("tz-migrated");
    if marker.exists() {
        println!("already migrated ({}). Delete that marker to force a re-run.", marker.display());
        return Ok(());
    }
    let reinterpret = |dt: chrono::DateTime<Utc>| -> chrono::DateTime<Utc> {
        let naive = dt.naive_utc();
        chrono::Local
            .from_local_datetime(&naive)
            .earliest()
            .map(|d| d.with_timezone(&Utc))
            .unwrap_or(dt)
    };

    let mut ctx = open_context(root, cfg)?;
    let mut events = Vec::new();
    for ev in ctx.events() {
        if ev.all_day {
            continue; // pure dates — nothing to shift
        }
        let (s, e) = (reinterpret(ev.start), reinterpret(ev.end));
        if s != ev.start || e != ev.end {
            events.push((ev.uid.clone(), ev.summary.clone(), ev.start, s, e));
        }
    }
    let mut tasks = Vec::new();
    for t in ctx.tasks() {
        let due = t.due.map(reinterpret);
        let sched = t.scheduled.map(reinterpret);
        if due != t.due || sched != t.scheduled {
            tasks.push((t.uid.clone(), t.title.clone(), due, sched));
        }
    }

    if events.is_empty() && tasks.is_empty() {
        println!("nothing to migrate (already local, or TZ=UTC).");
        if !dry_run {
            write_tz_marker(&marker)?; // record it so we don't re-scan / re-prompt next time
        }
        return Ok(());
    }
    println!(
        "{} timed event(s) and {} task stamp(s) will be reinterpreted as local wall-clock:",
        events.len(),
        tasks.len()
    );
    for (uid, summary, old, new, _) in events.iter().take(20) {
        println!(
            "  {}  {summary}: {} -> {} UTC",
            &uid.to_string()[..8.min(uid.to_string().len())],
            old.format("%Y-%m-%d %H:%M"),
            new.format("%Y-%m-%d %H:%M")
        );
    }
    if events.len() > 20 {
        println!("  … and {} more", events.len() - 20);
    }
    if dry_run {
        println!("dry run — nothing written.");
        return Ok(());
    }
    if !yes {
        println!(
            "note: events pulled from a CalDAV server with TZID/UTC times were already correct \
             and would be shifted too — review the list above first. Re-running would double-shift."
        );
        print!("apply? [y/N] ");
        use std::io::Write;
        std::io::stdout().flush().ok();
        let mut line = String::new();
        std::io::stdin().read_line(&mut line)?;
        if !matches!(line.trim(), "y" | "Y" | "yes") {
            anyhow::bail!("aborted");
        }
    }

    let now = Utc::now();
    let (mut ne, mut nt) = (0usize, 0usize);
    for (uid, _, _, s, e) in &events {
        if let Some(mut ev) = ctx.event(uid).cloned() {
            ev.start = *s;
            ev.end = *e;
            ev.modified = Some(now);
            ctx.put_event(ev).map_err(anyerr)?;
            ne += 1;
        }
    }
    for (uid, _, due, sched) in &tasks {
        if let Some(mut t) = ctx.task(uid).cloned() {
            t.due = *due;
            t.scheduled = *sched;
            t.modified = Some(now);
            ctx.put_task(t).map_err(anyerr)?;
            nt += 1;
        }
    }
    write_tz_marker(&marker)?;
    println!("migrated {ne} event(s) and {nt} task(s). Run this once on every node that holds a copy of the vault (or let sync propagate the changes).");
    Ok(())
}

/// Drop the "timezone migration done" sentinel so `migrate-tz` never double-shifts a vault.
fn write_tz_marker(marker: &Path) -> Result<()> {
    if let Some(parent) = marker.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(marker, "migrated\n")?;
    Ok(())
}

fn open_context(root: &PathBuf, cfg: &Config) -> Result<MgmtContext> {
    let vault = VaultStore::new(mgmt_store::tasks_dir(root));
    let vdir = VdirStore::new(mgmt_store::calendars_dir(root));
    MgmtContext::open_with(vault, vdir, cfg.clone()).map_err(|e| anyhow::anyhow!(e.to_string()))
}

/// Map a config account block to a `mgmt_sync::Auth`. A `google` account holds no static secret —
/// it refreshes an OAuth access token (shared with Meet creation) and uses it as a bearer.
fn account_auth(a: &Account) -> Result<Auth> {
    Ok(match a.auth.as_str() {
        "none" => Auth::None,
        "bearer" => Auth::Bearer {
            token: a.token.clone().context("account.token required for bearer auth")?,
        },
        "basic" => Auth::Basic {
            user: a.username.clone().context("account.username required for basic auth")?,
            password: a.password.clone().context("account.password required for basic auth")?,
        },
        "google" => Auth::Bearer { token: mgmt_google::access_token(&google_token_path(&a.name)?).map_err(anyerr)? },
        other => anyhow::bail!("unknown auth kind: {other}"),
    })
}

/// Where an account's persisted Google OAuth token store lives (`<config>/google/<account>-token.json`).
/// The same store is written by the CLI loopback login and by the web Connect flow, so sync works
/// regardless of how the account was authorized.
fn google_token_path(account: &str) -> Result<PathBuf> {
    let dir = Config::default_path().map_err(anyerr)?.parent().unwrap_or(Path::new(".")).join("google");
    Ok(dir.join(format!("{}-token.json", mgmt_store::safe_stem(account))))
}

#[derive(Subcommand)]
enum GoogleCmd {
    /// Authorize an account (opens a browser for consent). Put the OAuth client-secret JSON from
    /// Google Cloud at `<config>/google/<account>-client.json` first.
    Login {
        /// Account name (matches the `account:` on your Google collections).
        #[arg(default_value = "google")]
        account: String,
    },
    /// Create a Google Meet on an event and store its join URL (pushed on the next sync).
    Meet {
        /// Event UID.
        uid: String,
    },
}

fn cmd_google(root: &PathBuf, cfg: &Config, cmd: GoogleCmd) -> Result<()> {
    match cmd {
        GoogleCmd::Login { account } => {
            let store = google_token_path(&account)?;
            let client_json = store.with_file_name(format!("{}-client.json", mgmt_store::safe_stem(&account)));
            if !client_json.exists() {
                anyhow::bail!(
                    "missing OAuth client secret at {} — create an OAuth client (Desktop app) in \
                     Google Cloud, enable the Calendar API, and save the downloaded JSON there",
                    client_json.display()
                );
            }
            let text = std::fs::read_to_string(&client_json)?;
            let (id, secret) = mgmt_google::parse_client_secret(&text).map_err(anyerr)?;
            // Loopback redirect on a fixed local port (Google auto-allows it for Desktop clients).
            mgmt_google::login_loopback(&id, &secret, &store, 9321).map_err(anyerr)?;
            println!("authorized Google account '{account}' (token cached at {})", store.display());
            Ok(())
        }
        GoogleCmd::Meet { uid } => cmd_google_meet(root, cfg, &uid),
    }
}

/// Create a Google Meet for a local event: resolve its Google collection → account + calendar id,
/// mint the Meet via the REST API (shared OAuth), store the URL on the event, and let the next
/// `mgmt sync` push it (as the iCalendar CONFERENCE property).
fn cmd_google_meet(root: &PathBuf, cfg: &Config, uid: &str) -> Result<()> {
    let mut ctx = open_context(root, cfg)?;
    let uid_owned = mgmt_core::Uid::from_string(uid.to_string());
    let event = ctx.event(&uid_owned).cloned().context("no event with that uid")?;

    // The event's local calendar maps to a Google collection (protocol mgmt/caldav, auth google).
    let coll = cfg
        .collections
        .iter()
        .find(|c| c.name == event.calendar && c.kind == "events")
        .with_context(|| format!("no collection for calendar '{}'", event.calendar))?;
    let account = cfg.account(&coll.account).with_context(|| format!("unknown account '{}'", coll.account))?;
    if account.auth != "google" {
        anyhow::bail!("account '{}' is not a Google (OAuth) account", account.name);
    }
    let cal_id = mgmt_google::calendar_id_from_caldav_url(&coll.url)
        .with_context(|| format!("could not derive the Google calendar id from '{}'", coll.url))?;

    let token = mgmt_google::access_token(&google_token_path(&account.name)?).map_err(anyerr)?;
    let url = mgmt_google::create_meet(&token, &cal_id, uid).map_err(anyerr)?;

    let mut updated = event;
    updated.conference_url = Some(url.clone());
    ctx.put_event(updated).map_err(anyerr)?;
    println!("attached Google Meet: {url}");
    println!("run `mgmt sync` to push it to Google.");
    Ok(())
}

fn cmd_add(root: &PathBuf, cfg: &Config, title: String, project: Option<String>) -> Result<()> {
    if title.trim().is_empty() {
        anyhow::bail!("task title is empty");
    }
    let mut ctx = open_context(root, cfg)?;
    let uid = ctx
        .quick_add(title.clone(), project)
        .map_err(|e| anyhow::anyhow!(e.to_string()))?;
    println!("added task {uid}: {title}");
    Ok(())
}

fn cmd_import(root: &PathBuf, cfg: &Config, path: &PathBuf, calendar: &str) -> Result<()> {
    let text = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let mut ctx = open_context(root, cfg)?;
    let n = mgmt_service::import_ics(&mut ctx, &text, calendar).map_err(|e| anyhow::anyhow!(e.to_string()))?;
    println!("imported {n} event(s) into '{calendar}'");
    Ok(())
}

fn cmd_export(root: &PathBuf, cfg: &Config, calendar: Option<&str>) -> Result<()> {
    let ctx = open_context(root, cfg)?;
    print!("{}", mgmt_service::export_ics(&ctx, calendar));
    Ok(())
}

fn cmd_sync(root: &PathBuf, cfg: &Config, target: Option<&str>) -> Result<()> {
    let cfg_path = Config::default_path().map_err(anyerr)?;
    // Native pairings run on every manual sync (regardless of their poll flag). A target may
    // name a pairing instead of a collection.
    let paired = match target {
        None => pair::run_pairings(root, false)?,
        Some(t) => usize::from(pair::run_named(root, t)?),
    };
    // A target that names neither a collection nor a pairing must error, not silently no-op.
    if let Some(t) = target {
        if paired == 0 && !cfg.collections.iter().any(|c| c.name == t) {
            anyhow::bail!(
                "no collection or pairing named '{t}' (collections: {}; see `mgmt pair list`)",
                if cfg.collections.is_empty() {
                    "none".to_string()
                } else {
                    cfg.collections.iter().map(|c| c.name.as_str()).collect::<Vec<_>>().join(", ")
                }
            );
        }
    }
    if cfg.collections.is_empty() {
        if paired == 0 {
            println!(
                "nothing to sync — add `accounts:`/`collections:` to {} or import a pairing with `mgmt pair import`",
                cfg_path.display()
            );
        }
        return Ok(());
    }
    let hooks_dir = cfg_path.parent().unwrap_or(root).join("hooks");

    if run_hook(&hooks_dir, "pre-sync").map_err(anyerr)? {
        println!("ran pre-sync hook");
    }

    for coll in &cfg.collections {
        if let Some(t) = target {
            if t != coll.name {
                continue;
            }
        }
        let account = cfg
            .account(&coll.account)
            .with_context(|| format!("collection '{}' references unknown account '{}'", coll.name, coll.account))?;

        let report = match coll.protocol.as_str() {
            "mgmt" => sync_collection_http(root, coll, account)?,
            "caldav" | "" => {
                let client = CalDavClient::new(&coll.url, account_auth(account)?).map_err(anyerr)?;
                match coll.kind.as_str() {
                    "events" => {
                        let mut store = VdirStore::new(mgmt_store::calendars_dir(root));
                        sync_events(&client, &coll.url, &mut store, &coll.name).map_err(anyerr)?
                    }
                    "tasks" => {
                        let mut store = VaultStore::new(mgmt_store::tasks_dir(root));
                        sync_tasks(&client, &coll.url, &mut store).map_err(anyerr)?
                    }
                    other => anyhow::bail!("collection '{}' has unknown kind '{other}'", coll.name),
                }
            }
            other => anyhow::bail!("collection '{}' has unknown protocol '{other}'", coll.name),
        };
        println!(
            "synced '{}': {} pushed, {} pulled, {} deleted",
            coll.name, report.pushed, report.pulled, report.deleted
        );
    }

    if run_hook(&hooks_dir, "post-sync").map_err(anyerr)? {
        println!("ran post-sync hook");
    }
    Ok(())
}

/// Sync one collection against a native `mgmt web` server (`protocol: mgmt`).
fn sync_collection_http(root: &PathBuf, coll: &mgmt_config::Collection, account: &Account) -> Result<SyncReport> {
    // Native sync authenticates with a bearer token (the account's `token`).
    let token = match account.auth.as_str() {
        "bearer" => Some(account.token.clone().context("account.token required for bearer auth")?),
        "none" => None,
        other => anyhow::bail!("native (mgmt) sync needs auth: bearer (or none), got '{other}'"),
    };
    let remote = HttpRemote::new(coll.url.clone(), token).map_err(anyerr)?;
    // The three-way merge base for this collection, keyed by account + collection so distinct
    // remotes never share a snapshot.
    let base_path = root
        .join(".state")
        .join("sync")
        .join(mgmt_store::safe_stem(&account.name))
        .join(format!("{}-{}.json", mgmt_store::safe_stem(&coll.name), coll.kind));
    match coll.kind.as_str() {
        "events" => {
            let mut store = VdirStore::new(mgmt_store::calendars_dir(root));
            sync_events_http(&remote, &mut store, &coll.name, &base_path).map_err(anyerr)
        }
        "tasks" => {
            let mut store = VaultStore::new(mgmt_store::tasks_dir(root));
            sync_tasks_http(&remote, &mut store, &base_path).map_err(anyerr)
        }
        other => anyhow::bail!("collection '{}' has unknown kind '{other}'", coll.name),
    }
}

fn cmd_serve(root: &PathBuf) -> Result<()> {
    let cfg_dir = Config::default_path().map_err(anyerr)?.parent().unwrap_or(root).to_path_buf();
    let rustical_cfg = cfg_dir.join("rustical.toml");
    let rc = RusticalConfig::new(root);
    rc.write(&rustical_cfg).map_err(anyerr)?;
    println!("wrote rustical config to {}", rustical_cfg.display());
    match rc.spawn(&rustical_cfg) {
        Ok(mut child) => {
            println!("rustical running on {}:{} (Ctrl-C to stop)", rc.host, rc.port);
            child.wait()?;
            Ok(())
        }
        Err(e) => {
            println!("could not start rustical: {e}");
            println!("install rustical and re-run, or point any CalDAV server at {}", root.display());
            Ok(())
        }
    }
}

fn anyerr(e: mgmt_core::Error) -> anyhow::Error {
    anyhow::anyhow!(e.to_string())
}

/// Depth-first collect references to components named `name`.
fn cmd_daemon(root: &PathBuf, cfg: Config, poll: Option<u64>) -> Result<()> {
    let ctx = open_context(root, &cfg)?;
    daemon::run(root, cfg, ctx, poll)
}

fn run_tui(root: &PathBuf, cfg: Config, event: Option<String>, view: Option<String>) -> Result<()> {
    let ctx = open_context(root, &cfg)?;
    // The window title lets `navigate` reminders (and the daemon's raise strategies) find us.
    let title = cfg.daemon().focus.window_title.clone();
    let mut app = MgmtApp::new(ctx);
    if let Some(v) = view.as_deref() {
        app.set_view(v);
    }
    if let Some(arg) = event.as_deref() {
        if !app.focus_event_arg(arg) {
            eprintln!("mgmt: no event matching {arg:?}");
        }
    }

    // Watch the vault so edits landing underneath us (the web UI, a sync run, another mgmt
    // instance) show up live instead of only after a restart.
    let vault_stale = Arc::new(AtomicBool::new(false));
    let _watcher = watch_vault(root, vault_stale.clone());

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, SetTitle(&title))?;
    // Opt into the kitty keyboard protocol when the terminal supports it, so the event form can
    // distinguish Shift+Enter (create now) from a bare Enter. Terminals without it just see Enter.
    let kbd_enhanced = supports_keyboard_enhancement().unwrap_or(false);
    if kbd_enhanced {
        execute!(stdout, PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES))?;
    }
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let result = run_loop(&mut terminal, &mut app, &vault_stale);

    if kbd_enhanced {
        let _ = execute!(terminal.backend_mut(), PopKeyboardEnhancementFlags);
    }
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    result
}

/// Watch the vault under `root` for task/event/project file changes, flagging `stale` so the TUI
/// loop reloads. Best-effort: if the watcher can't start (e.g. inotify exhaustion), the TUI just
/// won't live-reload external edits.
fn watch_vault(root: &Path, stale: Arc<AtomicBool>) -> Option<notify::RecommendedWatcher> {
    use notify::{RecursiveMode, Watcher};
    let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        if let Ok(ev) = res {
            // Any event counts — including bare directory creates, whose first file write can be
            // missed while the recursive watch attaches to the new dir. Only known noise is
            // ignored: `.state/` churn (the shared pomodoro session) and atomic-write temp files.
            let relevant = ev.paths.iter().any(|p| {
                let noise = p.components().any(|c| c.as_os_str() == ".state")
                    || p.extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case("tmp"));
                !noise
            });
            if relevant {
                stale.store(true, Ordering::SeqCst);
            }
        }
    })
    .ok()?;
    watcher.watch(root, RecursiveMode::Recursive).ok()?;
    Some(watcher)
}

fn run_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut MgmtApp,
    vault_stale: &AtomicBool,
) -> Result<()> {
    loop {
        // Pick up external edits (web UI, sync, $EDITOR) when it's safe — not under an open modal,
        // where a reload would yank state out from under the form.
        if !app.has_modal() && vault_stale.swap(false, Ordering::SeqCst) {
            app.reload();
        }
        app.tick(); // advance the pomodoro timer / fire notifications
        terminal.draw(|f| app.draw(f, f.area()))?;
        // Poll with a timeout so the focus-timer display ticks even without input.
        if event::poll(Duration::from_millis(250))? {
            if let Event::Key(key) = event::read()? {
                if key.kind != KeyEventKind::Press {
                    continue;
                }
                match app.handle_key(key) {
                    Outcome::Quit => return Ok(()),
                    Outcome::Continue => {}
                    Outcome::OpenEditor(path) => {
                        edit_in_external_editor(terminal, &path)?;
                        app.reload();
                    }
                }
            }
        }
    }
}

/// Suspend the TUI, run `$EDITOR` (or `$VISUAL`, else `vi`) on `path`, then restore the TUI.
fn edit_in_external_editor(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>, path: &std::path::Path) -> Result<()> {
    let editor = std::env::var("VISUAL")
        .or_else(|_| std::env::var("EDITOR"))
        .unwrap_or_else(|_| "vi".to_string());

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;

    let status = std::process::Command::new(&editor).arg(path).status();

    enable_raw_mode()?;
    execute!(terminal.backend_mut(), EnterAlternateScreen)?;
    // Best-effort full repaint. `clear()` can fail on terminals that don't answer a
    // cursor-position query (e.g. some emulators / test PTYs); that's non-fatal — the next
    // draw repaints anyway, so we don't propagate it.
    let _ = terminal.clear();

    if let Err(e) = status {
        anyhow::bail!("failed to launch editor '{editor}': {e}");
    }
    Ok(())
}
