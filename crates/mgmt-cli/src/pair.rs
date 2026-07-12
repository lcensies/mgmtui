//! `mgmt pair` — manage persistent sync pairings with remote `mgmt web` users. Importing a
//! `mgmt://pair/...` URL clones the remote vault and saves a pairing; the daemon (or `mgmt sync`)
//! then keeps it in sync bidirectionally on an interval.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
use clap::Subcommand;

use mgmt_config::Config;
use mgmt_sync::{run_pairing, Pairing, Pairings};

#[derive(Subcommand)]
pub enum PairCmd {
    /// Import a `mgmt://pair/...` URL: clone the remote vault and save a persistent pairing.
    Import {
        url: String,
        /// This node should NOT poll — the remote polls this node instead (requires `mgmt web`).
        #[arg(long)]
        no_poll: bool,
        /// Override the local pairing name (defaults to the remote user id).
        #[arg(long)]
        name: Option<String>,
    },
    /// List saved pairings.
    List,
    /// Remove a saved pairing by name.
    Remove { name: String },
}

fn anyerr(e: impl std::fmt::Display) -> anyhow::Error {
    anyhow::anyhow!(e.to_string())
}

/// `~/.config/mgmt/sync-pairings.yaml` (next to `config.yaml`).
pub fn pairings_path() -> Result<PathBuf> {
    let cfg = Config::default_path().map_err(anyerr)?;
    Ok(cfg.parent().map(|p| p.to_path_buf()).unwrap_or_default().join("sync-pairings.yaml"))
}

pub fn run_pair(root: &Path, cmd: PairCmd) -> Result<()> {
    let path = pairings_path()?;
    match cmd {
        PairCmd::Import { url, no_poll, name } => {
            let (host, token, user) = decode_pair_url(&url)?;
            let name = name.unwrap_or(user);
            let remote = format!("{}/api/sync", host.trim_end_matches('/'));
            let mut p = Pairing::new(name.clone(), remote, token);
            p.poll = !no_poll;

            let mut pairings = Pairings::load(&path).map_err(anyerr)?;
            pairings.upsert(p.clone());
            pairings.save(&path).map_err(anyerr)?;
            println!("saved pairing '{name}' (this node polls: {})", p.poll);

            // Initial clone + first bidirectional pass.
            let r = run_pairing(root, &p).map_err(anyerr)?;
            println!("initial sync: {} pushed, {} pulled, {} deleted", r.pushed, r.pulled, r.deleted);
            Ok(())
        }
        PairCmd::List => {
            let pairings = Pairings::load(&path).map_err(anyerr)?;
            if pairings.pairings.is_empty() {
                println!("no pairings ({})", path.display());
            }
            for p in &pairings.pairings {
                println!(
                    "{}\t{}\tpoll={} every {}s\ttasks={} calendars={:?}",
                    p.name, p.remote, p.poll, p.interval_secs, p.tasks, p.calendars
                );
            }
            Ok(())
        }
        PairCmd::Remove { name } => {
            let mut pairings = Pairings::load(&path).map_err(anyerr)?;
            if pairings.remove(&name) {
                pairings.save(&path).map_err(anyerr)?;
                println!("removed pairing '{name}'");
            } else {
                println!("no pairing named '{name}'");
            }
            Ok(())
        }
    }
}

/// Run every saved pairing once (used by `mgmt sync`). When `poll_only`, skip pairings this node
/// doesn't poll. Returns the number of pairings run.
pub fn run_pairings(root: &Path, poll_only: bool) -> Result<usize> {
    let pairings = Pairings::load(&pairings_path()?).map_err(anyerr)?;
    let mut n = 0;
    for p in &pairings.pairings {
        if poll_only && !p.poll {
            continue;
        }
        match run_pairing(root, p) {
            Ok(r) => {
                println!("paired '{}': {} pushed, {} pulled, {} deleted", p.name, r.pushed, r.pulled, r.deleted);
                n += 1;
            }
            Err(e) => eprintln!("pairing '{}' failed: {e}", p.name),
        }
    }
    Ok(n)
}

/// Run the single pairing named `name` (used by `mgmt sync <target>`). Returns whether a pairing
/// with that name exists.
pub fn run_named(root: &Path, name: &str) -> Result<bool> {
    let pairings = Pairings::load(&pairings_path()?).map_err(anyerr)?;
    let Some(p) = pairings.pairings.iter().find(|p| p.name == name) else {
        return Ok(false);
    };
    let r = run_pairing(root, p).map_err(anyerr)?;
    println!("paired '{}': {} pushed, {} pulled, {} deleted", p.name, r.pushed, r.pulled, r.deleted);
    Ok(true)
}

/// Daemon tick: run each `poll: true` pairing whose interval has elapsed. `since` accumulates
/// seconds per pairing across ticks; `elapsed_secs` is this tick's duration. Reloads the pairings
/// file each tick so newly-imported pairings are picked up without a daemon restart.
pub fn poll_due(root: &Path, since: &mut HashMap<String, u64>, elapsed_secs: u64) {
    let Ok(path) = pairings_path() else { return };
    let Ok(pairings) = Pairings::load(&path) else { return };
    for p in &pairings.pairings {
        if !p.poll {
            continue;
        }
        // Start at MAX so a freshly-started daemon syncs on its first tick.
        let counter = since.entry(p.name.clone()).or_insert(u64::MAX);
        *counter = counter.saturating_add(elapsed_secs);
        if *counter >= p.interval_secs {
            *counter = 0;
            match run_pairing(root, p) {
                Ok(r) if r.pushed + r.pulled + r.deleted > 0 => {
                    eprintln!("mgmt daemon: paired '{}': {} pushed, {} pulled, {} deleted", p.name, r.pushed, r.pulled, r.deleted);
                }
                Ok(_) => {}
                Err(e) => eprintln!("mgmt daemon: pairing '{}' failed: {e}", p.name),
            }
        }
    }
}

/// Decode a `mgmt://pair/<base64url({host,token,user})>` URL.
fn decode_pair_url(url: &str) -> Result<(String, String, String)> {
    let blob = url.trim().strip_prefix("mgmt://pair/").context("not a mgmt://pair/ URL")?;
    let bytes = data_encoding::BASE64URL_NOPAD.decode(blob.as_bytes()).context("bad base64 in pair URL")?;
    let v: serde_json::Value = serde_json::from_slice(&bytes).context("bad JSON in pair URL")?;
    let field = |k: &str| v[k].as_str().map(str::to_string).with_context(|| format!("pair URL missing '{k}'"));
    Ok((field("host")?, field("token")?, field("user")?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_a_pair_url() {
        let payload = serde_json::json!({ "host": "https://ex.com", "token": "tok", "user": "alice" });
        let blob = data_encoding::BASE64URL_NOPAD.encode(serde_json::to_vec(&payload).unwrap().as_slice());
        let (host, token, user) = decode_pair_url(&format!("mgmt://pair/{blob}")).unwrap();
        assert_eq!((host.as_str(), token.as_str(), user.as_str()), ("https://ex.com", "tok", "alice"));
    }
}
