//! mgmt — local-first calendar + markdown tasks + kanban.
//!
//! This binary is the sole owner of the terminal (raw mode, alternate screen, event loop).
//! The `mgmt-tui` library stays terminal-agnostic so it can be embedded in other ratatui
//! hosts (e.g. the wng dashboard).

use std::io;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context as _, Result};
use clap::{Parser, Subcommand};
use crossterm::event::{self, Event, KeyEventKind};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, SetTitle, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

use mgmt_config::{Account, Config};
use mgmt_service::MgmtContext;
use mgmt_store::{VaultStore, VdirStore};
use mgmt_sync::{Auth, CalDavClient, RusticalConfig, run_hook, sync_events, sync_tasks};
use mgmt_tui::{MgmtApp, Outcome};

mod crud;
mod daemon;
mod datetime;
mod meta;
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
    /// Sync collections with their remote CalDAV servers.
    Sync {
        /// Only sync this collection.
        target: Option<String>,
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
    /// Emit the task-metadata schema as JSON (used by the editor/nvim completion plugin).
    Meta {
        #[arg(long)]
        json: bool,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    // One YAML config drives everything; an absent file yields built-in defaults.
    let cfg = Config::load(&Config::default_path().map_err(anyerr)?).map_err(anyerr)?;
    // Data root precedence: --data-dir flag, then config `data_dir`, then XDG default.
    let root = match &cli.data_dir {
        Some(p) => p.clone(),
        None => match &cfg.data_dir {
            Some(p) => p.clone(),
            None => mgmt_store::data_root().map_err(anyerr)?,
        },
    };

    match cli.cmd.unwrap_or(Cmd::Tui { event: None }) {
        Cmd::Tui { event } => run_tui(&root, cfg, event),
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
        Cmd::Serve => cmd_serve(&root),
        Cmd::Daemon { poll } => cmd_daemon(&root, cfg, poll),
        Cmd::Meta { json: _ } => {
            let ctx = open_context(&root, &cfg)?;
            println!("{}", meta::schema_json(&ctx, &root));
            Ok(())
        }
    }
}

fn open_context(root: &PathBuf, cfg: &Config) -> Result<MgmtContext> {
    let vault = VaultStore::new(mgmt_store::tasks_dir(root));
    let vdir = VdirStore::new(mgmt_store::calendars_dir(root));
    MgmtContext::open_with(vault, vdir, cfg.clone()).map_err(|e| anyhow::anyhow!(e.to_string()))
}

/// Map a config account block to a `mgmt_sync::Auth`.
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
        other => anyhow::bail!("unknown auth kind: {other}"),
    })
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
    let root_comp = mgmt_ical::parse(&text).map_err(|e| anyhow::anyhow!(e.to_string()))?;
    let mut vevents = Vec::new();
    collect_named(&root_comp, "VEVENT", &mut vevents);
    let mut n = 0;
    for ve in vevents {
        let ev = mgmt_ical::event_from_component(ve, calendar).map_err(|e| anyhow::anyhow!(e.to_string()))?;
        ctx.put_event(ev).map_err(|e| anyhow::anyhow!(e.to_string()))?;
        n += 1;
    }
    println!("imported {n} event(s) into '{calendar}'");
    Ok(())
}

fn cmd_export(root: &PathBuf, cfg: &Config, calendar: Option<&str>) -> Result<()> {
    let ctx = open_context(root, cfg)?;
    for ev in ctx.events() {
        if calendar.map(|c| c == ev.calendar).unwrap_or(true) {
            print!("{}", mgmt_ical::event_to_ics(ev));
        }
    }
    Ok(())
}

fn cmd_sync(root: &PathBuf, cfg: &Config, target: Option<&str>) -> Result<()> {
    let cfg_path = Config::default_path().map_err(anyerr)?;
    if cfg.collections.is_empty() {
        println!(
            "no collections configured — add `accounts:` and `collections:` to {}",
            cfg_path.display()
        );
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
        let client = CalDavClient::new(&coll.url, account_auth(account)?).map_err(anyerr)?;

        let report = match coll.kind.as_str() {
            "events" => {
                let mut store = VdirStore::new(mgmt_store::calendars_dir(root));
                sync_events(&client, &coll.url, &mut store, &coll.name).map_err(anyerr)?
            }
            "tasks" => {
                let mut store = VaultStore::new(mgmt_store::tasks_dir(root));
                sync_tasks(&client, &coll.url, &mut store).map_err(anyerr)?
            }
            other => anyhow::bail!("collection '{}' has unknown kind '{other}'", coll.name),
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
fn collect_named<'a>(c: &'a mgmt_ical::Component, name: &str, out: &mut Vec<&'a mgmt_ical::Component>) {
    if c.name.eq_ignore_ascii_case(name) {
        out.push(c);
    }
    for child in &c.children {
        collect_named(child, name, out);
    }
}

fn cmd_daemon(root: &PathBuf, cfg: Config, poll: Option<u64>) -> Result<()> {
    let ctx = open_context(root, &cfg)?;
    daemon::run(root, cfg, ctx, poll)
}

fn run_tui(root: &PathBuf, cfg: Config, event: Option<String>) -> Result<()> {
    let ctx = open_context(root, &cfg)?;
    // The window title lets `navigate` reminders (and the daemon's raise strategies) find us.
    let title = cfg.daemon().focus.window_title.clone();
    let mut app = MgmtApp::new(ctx);
    if let Some(arg) = event.as_deref() {
        if !app.focus_event_arg(arg) {
            eprintln!("mgmt: no event matching {arg:?}");
        }
    }

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, SetTitle(&title))?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let result = run_loop(&mut terminal, &mut app);

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    result
}

fn run_loop(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>, app: &mut MgmtApp) -> Result<()> {
    loop {
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
