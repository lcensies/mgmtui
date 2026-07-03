//! Shared application state: the single `MgmtContext` behind an async `RwLock`, plus a filesystem
//! watcher that flags the context stale when the vault changes underneath us (a CLI edit, a cron
//! `mgmt` mutation, a sync). Reads reload lazily when the flag is set.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use tokio::sync::{RwLock, RwLockReadGuard, RwLockWriteGuard};

use mgmt_config::Config;
use mgmt_core::Result;
use mgmt_service::MgmtContext;
use mgmt_store::{VaultStore, VdirStore};

use crate::auth::AuthState;

#[derive(Clone)]
pub struct AppState {
    inner: Arc<Inner>,
}

struct Inner {
    ctx: RwLock<MgmtContext>,
    root: PathBuf,
    auth: AuthState,
    /// Set by the fs watcher when something changed on disk; cleared by the next read after a reload.
    stale: Arc<AtomicBool>,
    /// Kept alive for the process lifetime (dropping it stops watching). Behind a std Mutex so the
    /// state stays `Sync`.
    _watcher: Mutex<Option<notify::RecommendedWatcher>>,
}

impl AppState {
    /// Open the vault at `root` with `cfg` + `auth` and start watching it for external changes.
    pub fn new(root: PathBuf, cfg: Config, auth: AuthState) -> Result<Self> {
        let vault = VaultStore::new(mgmt_store::tasks_dir(&root));
        let vdir = VdirStore::new(mgmt_store::calendars_dir(&root));
        let ctx = MgmtContext::open_with(vault, vdir, cfg)?;
        let stale = Arc::new(AtomicBool::new(false));
        let watcher = build_watcher(&root, stale.clone());
        Ok(AppState {
            inner: Arc::new(Inner {
                ctx: RwLock::new(ctx),
                root,
                auth,
                stale,
                _watcher: Mutex::new(watcher),
            }),
        })
    }

    pub fn root(&self) -> &Path {
        &self.inner.root
    }

    pub fn auth(&self) -> &AuthState {
        &self.inner.auth
    }

    /// Force the next read to reload from disk (used after sync writes).
    pub fn mark_stale(&self) {
        self.inner.stale.store(true, Ordering::SeqCst);
    }

    /// Acquire a read guard, first reloading from disk if an external change was observed. The
    /// reload is idempotent, so an occasional spurious reload (e.g. triggered by our own write) is
    /// harmless.
    pub async fn read(&self) -> RwLockReadGuard<'_, MgmtContext> {
        if self.inner.stale.swap(false, Ordering::SeqCst) {
            let mut w = self.inner.ctx.write().await;
            let _ = w.reload();
        }
        self.inner.ctx.read().await
    }

    /// Acquire a write guard. Writers go through the same lock; the stale flag is cleared so our
    /// own write doesn't force a redundant reload on the next read.
    pub async fn write(&self) -> RwLockWriteGuard<'_, MgmtContext> {
        self.inner.stale.store(false, Ordering::SeqCst);
        self.inner.ctx.write().await
    }
}

/// Watch `root` recursively; any event flips the stale flag. Best-effort — a failure to set up the
/// watcher just means external edits aren't auto-detected (a manual `POST /api/reload` still works).
fn build_watcher(root: &Path, stale: Arc<AtomicBool>) -> Option<notify::RecommendedWatcher> {
    use notify::{RecursiveMode, Watcher};
    let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        if res.is_ok() {
            stale.store(true, Ordering::SeqCst);
        }
    })
    .ok()?;
    // The data root may not fully exist yet; watch what we can.
    let _ = watcher.watch(root, RecursiveMode::Recursive);
    Some(watcher)
}
