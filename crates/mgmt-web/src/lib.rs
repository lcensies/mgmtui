//! `mgmt-web` — an axum HTTP/JSON API over [`mgmt_service::MgmtContext`], plus a host for the PWA.
//!
//! The whole app is a thin, UI-free wrapper around the same service layer the TUI and CLI use, so
//! there is one source of truth for board grouping, agenda expansion, smart-view filtering, and
//! mutations. [`run`] owns a tokio runtime internally (the `mgmt-dav` pattern) so `mgmt-cli` stays
//! synchronous.
//!
//! This milestone exposes the read-only surface (tasks/events/board/meta/status) and serves the
//! SPA; authentication, mutations, and the sync protocol land in later milestones.

mod assets;
pub mod auth;
mod dto;
mod error;
mod meta;
mod middleware;
mod routes;
mod state;
mod sync;

use std::net::SocketAddr;
use std::path::PathBuf;

use axum::Router;
use tower_http::trace::TraceLayer;

use mgmt_config::Config;
use mgmt_core::{Error, Result};

use crate::auth::AuthState;
pub use state::AppState;

/// Runtime options for the web server.
#[derive(Debug, Clone)]
pub struct WebOptions {
    pub bind: SocketAddr,
    /// Serve a built SPA from this directory (SPA fallback to `index.html`). `None` → placeholder.
    pub assets_dir: Option<PathBuf>,
    /// Path to `web-auth.yaml`. When it has no password, the server runs unauthenticated.
    pub auth_file: PathBuf,
    /// Configured public origin (for cookie `Secure` + the mutation Origin check).
    pub public_origin: Option<String>,
    /// Rolling session lifetime in days.
    pub session_ttl_days: u64,
    /// Allow running without a password even on a non-loopback bind (dangerous; explicit opt-in).
    pub no_auth: bool,
}

/// Build the full application router (API under `/api`, SPA/placeholder as the fallback). Exposed
/// for tests to drive via `tower::ServiceExt::oneshot`.
pub fn build_router(state: AppState, assets_dir: Option<PathBuf>) -> Router {
    let app = Router::new()
        .nest("/api", routes::api_router(state.clone()))
        .nest("/api/sync", sync::router(state));
    assets::attach(app, assets_dir).layer(TraceLayer::new_for_http())
}

/// Open the vault at `root` and serve until interrupted. Owns its own tokio runtime.
pub fn run(root: PathBuf, cfg: Config, opts: WebOptions) -> Result<()> {
    let sessions_path = root.join(".state").join("web-sessions.json");
    let auth = AuthState::load(
        opts.auth_file.clone(),
        sessions_path,
        opts.public_origin.clone(),
        opts.session_ttl_days,
    )?;
    if !auth.enabled() && !opts.bind.ip().is_loopback() && !opts.no_auth {
        return Err(Error::Invalid(format!(
            "refusing to bind {} with no password set — run `mgmt web setpass` first, or pass \
             --no-auth to override (only sane behind a trusted network)",
            opts.bind
        )));
    }
    if !auth.enabled() {
        eprintln!("warning: no web password set — the API is unauthenticated. Run `mgmt web setpass`.");
    }
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(Error::Io)?;
    rt.block_on(serve(root, cfg, auth, opts))
}

async fn serve(root: PathBuf, cfg: Config, auth: AuthState, opts: WebOptions) -> Result<()> {
    let state = AppState::new(root, cfg, auth)?;
    let app = build_router(state, opts.assets_dir.clone());
    let listener = tokio::net::TcpListener::bind(opts.bind).await.map_err(Error::Io)?;
    let addr = listener.local_addr().map_err(Error::Io)?;
    tracing::info!("mgmt web listening on http://{addr}");
    println!("mgmt web listening on http://{addr}");
    axum::serve(listener, app.into_make_service())
        .await
        .map_err(Error::Io)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use mgmt_service::MgmtContext;
    use mgmt_store::{VaultStore, VdirStore};
    use tower::ServiceExt; // for `oneshot`

    fn test_state() -> (AppState, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        // Seed one task via the real context so the store files exist.
        let vault = VaultStore::new(mgmt_store::tasks_dir(&root));
        let vdir = VdirStore::new(mgmt_store::calendars_dir(&root));
        let mut ctx = MgmtContext::open(vault, vdir).unwrap();
        ctx.quick_add("Buy milk", Some("home".into())).unwrap();
        drop(ctx);
        let state = AppState::new(root, Config::default(), AuthState::disabled()).unwrap();
        (state, dir)
    }

    async fn get(state: &AppState, uri: &str) -> (StatusCode, serde_json::Value) {
        let app = build_router(state.clone(), None);
        let resp = app
            .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let value = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
        (status, value)
    }

    #[tokio::test]
    async fn health_ok() {
        let (state, _d) = test_state();
        let (status, body) = get(&state, "/api/health").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["ok"], true);
    }

    #[tokio::test]
    async fn tasks_and_board_reflect_seed() {
        let (state, _d) = test_state();
        let (status, tasks) = get(&state, "/api/tasks").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(tasks.as_array().unwrap().len(), 1);
        assert_eq!(tasks[0]["title"], "Buy milk");

        let (_s, board) = get(&state, "/api/board").await;
        let todo = board.as_array().unwrap().iter().find(|c| c["status"] == "todo").unwrap();
        assert_eq!(todo["tasks"].as_array().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn meta_lists_statuses_views_sorts() {
        let (state, _d) = test_state();
        let (status, meta) = get(&state, "/api/meta").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(meta["statuses"][0]["id"], "todo");
        assert_eq!(meta["views"].as_array().unwrap().len(), 4);
        assert!(meta["sorts"].as_array().unwrap().iter().any(|s| s["id"] == "priority"));
    }

    #[tokio::test]
    async fn unknown_task_is_404() {
        let (state, _d) = test_state();
        let (status, body) = get(&state, "/api/tasks/does-not-exist").await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert!(body["error"].as_str().unwrap().contains("does-not-exist"));
    }

    #[tokio::test]
    async fn events_require_range() {
        let (state, _d) = test_state();
        let (status, _b) = get(&state, "/api/events").await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        let (ok, list) = get(&state, "/api/events?from=2026-01-01T00:00:00Z&to=2027-01-01T00:00:00Z").await;
        assert_eq!(ok, StatusCode::OK);
        assert!(list.is_array());
    }
}
