//! Shared application state.
//!
//! The web UI (session = admin) always operates on the admin's own vault, exposed via
//! [`AppState::read`]/[`AppState::write`] exactly as before. The native sync protocol is per-user:
//! a bearer token resolves to a user id and [`AppState::user_ctx`] hands back that user's isolated
//! [`UserCtx`] (opened lazily under `<data_root>/users/<id>`). Each [`UserCtx`] carries its own
//! filesystem watcher, so an external edit (a sync write, a cron `mgmt` mutation) flags just that
//! user's context stale and reads reload it lazily.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use tokio::sync::{RwLock, RwLockReadGuard, RwLockWriteGuard};

use mgmt_config::Config;
use mgmt_core::{Error, Result};
use mgmt_service::MgmtContext;
use mgmt_store::{VaultStore, VdirStore};

use crate::auth::CredStore;

#[derive(Clone)]
pub struct AppState {
    inner: Arc<Inner>,
}

struct Inner {
    /// The admin's vault (`users/admin`, or the legacy root pre-migration). Backs the web UI.
    admin: Arc<UserCtx>,
    /// `<data_root>/users` — where per-user vaults live.
    users_base: PathBuf,
    /// Shared config used to open each user's context.
    cfg: Config,
    /// Lazily-opened non-admin user contexts, keyed by user id.
    users: Mutex<HashMap<String, Arc<UserCtx>>>,
    creds: CredStore,
    /// Configured public origin (e.g. `https://mgmt.example.com`), used to build export/pair URLs.
    public_origin: Option<String>,
    /// Path to the web-managed `caldav.yaml` (accounts/collections editable from the admin UI).
    caldav_file: PathBuf,
    /// In-flight OAuth authorizations: CSRF `state` → (account being connected, PKCE verifier). The
    /// callback consumes it to finish the exchange and provision the account.
    oauth_pending: Mutex<HashMap<String, (String, String)>>,
}

/// One user's isolated context: an `MgmtContext` behind an async `RwLock`, plus a filesystem watcher
/// that flags it stale when the vault changes underneath us.
pub struct UserCtx {
    ctx: RwLock<MgmtContext>,
    root: PathBuf,
    stale: Arc<AtomicBool>,
    _watcher: Mutex<Option<notify::RecommendedWatcher>>,
}

impl UserCtx {
    /// Open the vault rooted at `root` and start watching it.
    fn open(root: PathBuf, cfg: Config) -> Result<Arc<Self>> {
        let vault = VaultStore::new(mgmt_store::tasks_dir(&root));
        let vdir = VdirStore::new(mgmt_store::calendars_dir(&root));
        let ctx = MgmtContext::open_with(vault, vdir, cfg)?;
        let stale = Arc::new(AtomicBool::new(false));
        let watcher = build_watcher(&root, stale.clone());
        Ok(Arc::new(UserCtx {
            ctx: RwLock::new(ctx),
            root,
            stale,
            _watcher: Mutex::new(watcher),
        }))
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Force the next read to reload from disk (used after sync writes).
    pub fn mark_stale(&self) {
        self.stale.store(true, Ordering::SeqCst);
    }

    /// Acquire a read guard, first reloading from disk if an external change was observed.
    pub async fn read(&self) -> RwLockReadGuard<'_, MgmtContext> {
        if self.stale.swap(false, Ordering::SeqCst) {
            let mut w = self.ctx.write().await;
            let _ = w.reload();
        }
        self.ctx.read().await
    }

    /// Acquire a write guard. Clears the stale flag so our own write doesn't force a redundant
    /// reload on the next read.
    pub async fn write(&self) -> RwLockWriteGuard<'_, MgmtContext> {
        self.stale.store(false, Ordering::SeqCst);
        self.ctx.write().await
    }
}

impl AppState {
    /// Open the server at `data_root` with `cfg` + `auth`. The admin vault is `users/admin` once the
    /// data root has been migrated to the multi-user layout, else the legacy root (so existing
    /// single-vault setups and tests keep working unchanged).
    pub fn new(data_root: PathBuf, cfg: Config, creds: CredStore) -> Result<Self> {
        Self::configure(data_root, cfg, creds, None, PathBuf::new())
    }

    pub fn configure(
        data_root: PathBuf,
        cfg: Config,
        creds: CredStore,
        public_origin: Option<String>,
        caldav_file: PathBuf,
    ) -> Result<Self> {
        let admin_root = mgmt_store::local_vault_root(&data_root);
        let admin = UserCtx::open(admin_root, cfg.clone())?;
        Ok(AppState {
            inner: Arc::new(Inner {
                admin,
                users_base: mgmt_store::users_dir(&data_root),
                cfg,
                users: Mutex::new(HashMap::new()),
                creds,
                public_origin,
                caldav_file,
                oauth_pending: Mutex::new(HashMap::new()),
            }),
        })
    }

    /// Begin an OAuth authorization: register `state` → (account, PKCE verifier).
    pub fn oauth_begin(&self, state: String, account: String, pkce_verifier: String) {
        self.inner.oauth_pending.lock().unwrap().insert(state, (account, pkce_verifier));
    }

    /// Consume a pending OAuth `state`, returning (account, PKCE verifier) (single-use).
    pub fn oauth_take(&self, state: &str) -> Option<(String, String)> {
        self.inner.oauth_pending.lock().unwrap().remove(state)
    }

    /// Path to the web-managed `caldav.yaml`.
    pub fn caldav_file(&self) -> &Path {
        &self.inner.caldav_file
    }

    /// Base directory for per-user vaults (`<data_root>/users`).
    pub fn users_base(&self) -> &Path {
        &self.inner.users_base
    }

    /// The configured public origin, used when building export/pair URLs.
    pub fn public_origin(&self) -> Option<&str> {
        self.inner.public_origin.as_deref()
    }

    /// The admin vault root (used by the UI routes for `.state` paths and sync file paths).
    pub fn root(&self) -> &Path {
        self.inner.admin.root()
    }

    pub fn creds(&self) -> &CredStore {
        &self.inner.creds
    }

    /// The admin `UserCtx` (the web UI's vault).
    pub fn admin(&self) -> &Arc<UserCtx> {
        &self.inner.admin
    }

    /// Resolve a user's isolated context, opening (and caching) it on first access. `admin` returns
    /// the shared admin context; any other id maps to `<data_root>/users/<id>`.
    pub fn user_ctx(&self, id: &str) -> Result<Arc<UserCtx>> {
        if id == mgmt_store::ADMIN_USER {
            return Ok(self.inner.admin.clone());
        }
        if !is_safe_user_id(id) {
            return Err(Error::Invalid(format!("invalid user id '{id}'")));
        }
        let mut map = self.inner.users.lock().unwrap();
        if let Some(uc) = map.get(id) {
            return Ok(uc.clone());
        }
        let uc = UserCtx::open(self.inner.users_base.join(id), self.inner.cfg.clone())?;
        map.insert(id.to_string(), uc.clone());
        Ok(uc)
    }

    /// Drop a cached user context (after the admin deletes the user).
    pub fn forget_user(&self, id: &str) {
        self.inner.users.lock().unwrap().remove(id);
    }

    // ---- admin-vault convenience (the UI routes call these, unchanged) -----------------

    pub fn mark_stale(&self) {
        self.inner.admin.mark_stale();
    }

    pub async fn read(&self) -> RwLockReadGuard<'_, MgmtContext> {
        self.inner.admin.read().await
    }

    pub async fn write(&self) -> RwLockWriteGuard<'_, MgmtContext> {
        self.inner.admin.write().await
    }
}

/// A user id safe to use as a directory name (no traversal, no separators, no leading dot).
pub fn is_safe_user_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && !id.starts_with('.')
        && id.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
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
