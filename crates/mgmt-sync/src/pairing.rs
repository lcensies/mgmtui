//! Persistent sync *pairings*: a simple, durable description of "keep this local vault in sync with
//! that remote `mgmt web` user, bidirectionally, on an interval". One node in a pair runs the poll
//! loop (`poll: true`); the other is the passive server. A pairing is created by importing a
//! `mgmt://pair/...` URL (see `mgmt-cli`).

use std::path::Path;

use serde::{Deserialize, Serialize};

use mgmt_core::{Error, Result};
use mgmt_store::{calendars_dir, safe_stem, tasks_dir, VaultStore, VdirStore};

use crate::engine::SyncReport;
use crate::http::{sync_events_http, sync_tasks_http, HttpRemote};

fn default_true() -> bool {
    true
}
fn default_interval() -> u64 {
    60
}

/// One persistent bidirectional link to a remote `mgmt web` user's `/api/sync`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Pairing {
    /// Local identifier for this link (also names its base-snapshot directory).
    pub name: String,
    /// The remote `/api/sync` base URL, e.g. `https://mgmt.example.com/api/sync`.
    pub remote: String,
    /// Scoped bearer token that authorizes this pairing (grants access to one remote user's vault).
    pub token: String,
    /// Whether *this* node polls the remote. `true` → this node runs the periodic loop; `false` →
    /// this node is passive and expects the remote to poll it (it must run `mgmt web`).
    #[serde(default = "default_true")]
    pub poll: bool,
    /// Poll interval in seconds (only meaningful when `poll` is true).
    #[serde(default = "default_interval")]
    pub interval_secs: u64,
    /// Sync the markdown task vault.
    #[serde(default = "default_true")]
    pub tasks: bool,
    /// Calendars (vdir collections) to sync. Empty means "tasks only".
    #[serde(default)]
    pub calendars: Vec<String>,
}

impl Pairing {
    /// A sensible default pairing built from an imported URL: bidirectional, this node polls, tasks
    /// plus the `default` calendar.
    pub fn new(name: impl Into<String>, remote: impl Into<String>, token: impl Into<String>) -> Self {
        Pairing {
            name: name.into(),
            remote: remote.into(),
            token: token.into(),
            poll: true,
            interval_secs: default_interval(),
            tasks: true,
            calendars: vec!["default".to_string()],
        }
    }
}

/// The `sync-pairings.yaml` document.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Pairings {
    pub pairings: Vec<Pairing>,
}

impl Pairings {
    /// Load pairings from `path` (an absent file yields an empty set).
    pub fn load(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Pairings::default());
        }
        let text = std::fs::read_to_string(path)?;
        serde_yaml::from_str(&text).map_err(|e| Error::Parse(format!("parsing {}: {e}", path.display())))
    }

    /// Persist pairings to `path` (creates the parent directory). The token is a secret, so the
    /// file is chmod 0600 on unix.
    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let text = serde_yaml::to_string(self).map_err(|e| Error::Other(format!("serializing pairings: {e}")))?;
        std::fs::write(path, text)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
        }
        Ok(())
    }

    pub fn find(&self, name: &str) -> Option<&Pairing> {
        self.pairings.iter().find(|p| p.name == name)
    }

    /// Add or replace a pairing by name.
    pub fn upsert(&mut self, p: Pairing) {
        match self.pairings.iter_mut().find(|x| x.name == p.name) {
            Some(slot) => *slot = p,
            None => self.pairings.push(p),
        }
    }

    /// Remove a pairing by name; returns true if one was removed.
    pub fn remove(&mut self, name: &str) -> bool {
        let before = self.pairings.len();
        self.pairings.retain(|p| p.name != name);
        before != self.pairings.len()
    }
}

/// Run one full bidirectional sync pass for a pairing against the local vault at `root`. This is the
/// same operation whether invoked once (import / `mgmt sync`) or on the daemon's interval. On the
/// first pass the base snapshot is empty, so it performs the initial clone (pull everything remote,
/// push everything local).
pub fn run_pairing(root: &Path, p: &Pairing) -> Result<SyncReport> {
    let remote = HttpRemote::new(p.remote.clone(), Some(p.token.clone()))?;
    let base_dir = root.join(".state").join("sync").join(safe_stem(&p.name));
    let mut total = SyncReport::default();

    if p.tasks {
        let mut store = VaultStore::new(tasks_dir(root));
        let r = sync_tasks_http(&remote, &mut store, &base_dir.join("tasks.json"))?;
        total.pushed += r.pushed;
        total.pulled += r.pulled;
        total.deleted += r.deleted;
    }
    for cal in &p.calendars {
        let mut store = VdirStore::new(calendars_dir(root));
        let r = sync_events_http(&remote, &mut store, cal, &base_dir.join(format!("cal-{}.json", safe_stem(cal))))?;
        total.pushed += r.pushed;
        total.pulled += r.pulled;
        total.deleted += r.deleted;
    }
    Ok(total)
}
