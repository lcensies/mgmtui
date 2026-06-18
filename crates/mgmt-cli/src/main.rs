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
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

use mgmt_service::MgmtContext;
use mgmt_store::{VaultStore, VdirStore};
use mgmt_sync::{CalDavClient, RusticalConfig, run_hook, sync_events, sync_tasks};
use mgmt_tui::{MgmtApp, Outcome};

mod config;
use config::Config;

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
    Tui,
    /// Quick-add a task to the vault.
    Add {
        /// Task title.
        title: Vec<String>,
        /// Project to file it under.
        #[arg(short, long)]
        project: Option<String>,
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
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let root = match &cli.data_dir {
        Some(p) => p.clone(),
        None => mgmt_store::data_root().map_err(|e| anyhow::anyhow!(e.to_string()))?,
    };

    match cli.cmd.unwrap_or(Cmd::Tui) {
        Cmd::Tui => run_tui(&root),
        Cmd::Add { title, project } => cmd_add(&root, title.join(" "), project),
        Cmd::Import { path, calendar } => cmd_import(&root, &path, &calendar),
        Cmd::Export { calendar } => cmd_export(&root, calendar.as_deref()),
        Cmd::Sync { target } => cmd_sync(&root, target.as_deref()),
        Cmd::Serve => cmd_serve(&root),
    }
}

fn open_context(root: &PathBuf) -> Result<MgmtContext> {
    let vault = VaultStore::new(mgmt_store::tasks_dir(root));
    let vdir = VdirStore::new(mgmt_store::calendars_dir(root));
    MgmtContext::open(vault, vdir).map_err(|e| anyhow::anyhow!(e.to_string()))
}

fn cmd_add(root: &PathBuf, title: String, project: Option<String>) -> Result<()> {
    if title.trim().is_empty() {
        anyhow::bail!("task title is empty");
    }
    let mut ctx = open_context(root)?;
    let uid = ctx
        .quick_add(title.clone(), project)
        .map_err(|e| anyhow::anyhow!(e.to_string()))?;
    println!("added task {uid}: {title}");
    Ok(())
}

fn cmd_import(root: &PathBuf, path: &PathBuf, calendar: &str) -> Result<()> {
    let text = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let mut ctx = open_context(root)?;
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

fn cmd_export(root: &PathBuf, calendar: Option<&str>) -> Result<()> {
    let ctx = open_context(root)?;
    for ev in ctx.events() {
        if calendar.map(|c| c == ev.calendar).unwrap_or(true) {
            print!("{}", mgmt_ical::event_to_ics(ev));
        }
    }
    Ok(())
}

fn cmd_sync(root: &PathBuf, target: Option<&str>) -> Result<()> {
    let cfg_path = Config::default_path()?;
    let cfg = Config::load(&cfg_path)?;
    if cfg.collections.is_empty() {
        println!(
            "no collections configured — add [[account]] and [[collection]] blocks to {}",
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
        let client = CalDavClient::new(&coll.url, account.to_auth()?).map_err(anyerr)?;

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
    let cfg_dir = Config::default_path()?.parent().unwrap_or(root).to_path_buf();
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

fn run_tui(root: &PathBuf) -> Result<()> {
    let ctx = open_context(root)?;
    let mut app = MgmtApp::new(ctx);

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
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
        terminal.draw(|f| app.draw(f, f.area()))?;
        // Poll with a timeout so the focus-timer display ticks even without input.
        if event::poll(Duration::from_millis(250))? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press && app.handle_key(key) == Outcome::Quit {
                    return Ok(());
                }
            }
        }
    }
}
