//! `mgmt-web` — an axum HTTP/JSON API over [`mgmt_service::MgmtContext`], plus a host for the PWA.
//!
//! The whole app is a thin, UI-free wrapper around the same service layer the TUI and CLI use, so
//! there is one source of truth for board grouping, agenda expansion, smart-view filtering, and
//! mutations. [`run`] owns a tokio runtime internally (the `mgmt-dav` pattern) so `mgmt-cli` stays
//! synchronous.
//!
//! This milestone exposes the read-only surface (tasks/events/board/meta/status) and serves the
//! SPA; authentication, mutations, and the sync protocol land in later milestones.

mod admin;
mod assets;
pub mod auth;
mod auth_backend;
mod calendars;
mod db;
mod dto;
mod error;
mod feed;
mod meta;
mod middleware;
mod routes;
mod session_store;
mod state;
mod sync;

use std::net::SocketAddr;
use std::path::PathBuf;

use axum::Router;
use axum_login::AuthManagerLayerBuilder;
use tower_http::trace::TraceLayer;
use tower_sessions::cookie::SameSite;
use tower_sessions::{Expiry, SessionManagerLayer, SessionStore};

use mgmt_config::Config;
use mgmt_core::{Error, Result};

use crate::auth::CredStore;
use crate::auth_backend::Backend;
use crate::session_store::FileSessionStore;
pub use state::AppState;

/// Runtime options for the web server.
#[derive(Debug, Clone)]
pub struct WebOptions {
    pub bind: SocketAddr,
    /// Serve a built SPA from this directory (SPA fallback to `index.html`). `None` → placeholder.
    pub assets_dir: Option<PathBuf>,
    /// Path to `web-auth.yaml`. When it has no password, the server runs unauthenticated.
    pub auth_file: PathBuf,
    /// Path to the web-managed `caldav.yaml` (admin-editable CalDAV accounts/collections).
    pub caldav_file: PathBuf,
    /// Configured public origin; an `https://` origin makes the session cookie `Secure`.
    pub public_origin: Option<String>,
    /// Rolling session lifetime in days.
    pub session_ttl_days: u64,
    /// Allow running without a password even on a non-loopback bind (dangerous; explicit opt-in).
    pub no_auth: bool,
}

/// Build the full application router (API under `/api`, SPA/placeholder as the fallback). The
/// `session_layer` carries the (pluggable) session store; the axum-login auth layer is stacked on
/// top so `AuthSession` is available to the guard and handlers. Exposed for tests to drive via
/// `tower::ServiceExt::oneshot`.
pub fn build_router<S>(state: AppState, assets_dir: Option<PathBuf>, session_layer: SessionManagerLayer<S>) -> Router
where
    S: SessionStore + Clone,
{
    let backend = Backend::new(state.creds().clone());
    let auth_layer = AuthManagerLayerBuilder::new(backend, session_layer).build();
    let app = Router::new()
        .nest("/api", routes::api_router(state.clone()))
        .nest("/api/sync", sync::router(state));
    assets::attach(app, assets_dir)
        .layer(auth_layer)
        .layer(TraceLayer::new_for_http())
}

/// Open the vault at `root` and serve until interrupted. Owns its own tokio runtime.
///
/// `root` is the server's *data root*; the vault is migrated in-place to the multi-user layout
/// (`users/admin` + `users/<id>`) on first start. The admin password can be provisioned three ways:
/// `mgmt web setpass` (CLI), the `MGMT_WEB_PASSWORD`/`MGMT_WEB_PASSWORD_HASH` env vars (below), or
/// the first-run setup flow in the web UI (when neither is present the server locks itself to the
/// setup endpoints until claimed).
pub fn run(root: PathBuf, cfg: Config, opts: WebOptions) -> Result<()> {
    // Establish the multi-user layout, moving any legacy single vault under users/admin.
    if mgmt_store::migrate_to_multiuser(&root)? {
        println!("migrated existing vault into the multi-user layout (users/admin)");
    }

    // A session cookie without `Secure` on a non-loopback bind transits in cleartext unless a
    // TLS proxy fronts us — make sure the operator knows which mode they're in.
    let https = opts.public_origin.as_deref().map(|o| o.starts_with("https://")).unwrap_or(false);
    if !opts.bind.ip().is_loopback() && !https {
        eprintln!(
            "warning: serving on a non-loopback address without an https public_origin — the \
             session cookie is not marked Secure and logins transit in cleartext unless a TLS \
             proxy fronts this server. Set web.public_origin to your https:// URL."
        );
    }

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(Error::Io)?;
    rt.block_on(async move {
        // The credential DB lives under the data root (so `--data-dir` fully isolates an
        // instance); a legacy `web-auth.yaml` beside `config.yaml` is imported once.
        let db_path = auth_db_path(&root);
        let creds = CredStore::open(&db_path, Some(&opts.auth_file)).await?;

        // Bootstrap the admin from the environment when no password is set yet.
        if !creds.enabled().await {
            if let Ok(hash) = std::env::var("MGMT_WEB_PASSWORD_HASH") {
                if creds.set_admin_password_hash(&hash).await? {
                    println!("admin password set from MGMT_WEB_PASSWORD_HASH");
                }
            } else if let Ok(pw) = std::env::var("MGMT_WEB_PASSWORD") {
                creds.set_admin_password(&pw, None).await?;
                println!("admin password set from MGMT_WEB_PASSWORD");
            }
        }

        // Open mode = unauthenticated (loopback dev or explicit --no-auth). Otherwise a
        // passwordless server enters first-run setup mode rather than refusing to start.
        let open = opts.bind.ip().is_loopback() || opts.no_auth;
        creds.set_open_mode(open);
        if !creds.enabled().await {
            if open {
                eprintln!("warning: no web password set — the API is unauthenticated. Run `mgmt web setpass`.");
            } else {
                println!("no admin configured — open the web UI to create the admin account (setup mode).");
            }
        }

        serve(root, cfg, creds, opts).await
    })
}

/// The credential DB path for a data root (`<root>/.state/web-auth.db`).
pub fn auth_db_path(root: &std::path::Path) -> PathBuf {
    root.join(".state").join("web-auth.db")
}

async fn serve(root: PathBuf, cfg: Config, creds: CredStore, opts: WebOptions) -> Result<()> {
    let secure = opts.public_origin.as_deref().map(|o| o.starts_with("https://")).unwrap_or(false);
    let session_store = FileSessionStore::new(root.join(".state").join("web-sessions.json"));
    let session_layer = SessionManagerLayer::new(session_store)
        .with_name("mgmt_session")
        .with_http_only(true)
        .with_same_site(SameSite::Lax)
        .with_secure(secure)
        .with_expiry(Expiry::OnInactivity(time::Duration::days(opts.session_ttl_days.max(1) as i64)));
    let state = AppState::configure(root, cfg, creds, opts.public_origin.clone(), opts.caldav_file.clone())?;
    let app = build_router(state, opts.assets_dir.clone(), session_layer);
    let listener = tokio::net::TcpListener::bind(opts.bind).await.map_err(Error::Io)?;
    let addr = listener.local_addr().map_err(Error::Io)?;
    tracing::info!("mgmt web listening on http://{addr}");
    println!("mgmt web listening on http://{addr}");
    // Connect info feeds the login rate limiter the real TCP peer address (see
    // `middleware::client_ip` — X-Forwarded-For alone is attacker-controlled).
    axum::serve(listener, app.into_make_service_with_connect_info::<std::net::SocketAddr>())
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
    use tower_sessions::MemoryStore;

    async fn test_state() -> (AppState, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        // Seed one task via the real context so the store files exist.
        let vault = VaultStore::new(mgmt_store::tasks_dir(&root));
        let vdir = VdirStore::new(mgmt_store::calendars_dir(&root));
        let mut ctx = MgmtContext::open(vault, vdir).unwrap();
        ctx.quick_add("Buy milk", Some("home".into())).unwrap();
        drop(ctx);
        let state = AppState::new(root, Config::default(), CredStore::disabled().await.unwrap()).unwrap();
        (state, dir)
    }

    fn test_router(state: &AppState) -> Router {
        build_router(state.clone(), None, SessionManagerLayer::new(MemoryStore::default()))
    }

    async fn get(state: &AppState, uri: &str) -> (StatusCode, serde_json::Value) {
        let app = test_router(state);
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
        let (state, _d) = test_state().await;
        let (status, body) = get(&state, "/api/health").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["ok"], true);
    }

    #[tokio::test]
    async fn tasks_and_board_reflect_seed() {
        let (state, _d) = test_state().await;
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
        let (state, _d) = test_state().await;
        let (status, meta) = get(&state, "/api/meta").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(meta["statuses"][0]["id"], "todo");
        assert_eq!(meta["views"].as_array().unwrap().len(), 4);
        assert!(meta["sorts"].as_array().unwrap().iter().any(|s| s["id"] == "priority"));
    }

    #[tokio::test]
    async fn unknown_task_is_404() {
        let (state, _d) = test_state().await;
        let (status, body) = get(&state, "/api/tasks/does-not-exist").await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert!(body["error"].as_str().unwrap().contains("does-not-exist"));
    }

    #[tokio::test]
    async fn login_required_then_cookie_grants_access() {
        use axum::http::header::{COOKIE, SET_COOKIE};

        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        {
            let vault = VaultStore::new(mgmt_store::tasks_dir(&root));
            let vdir = VdirStore::new(mgmt_store::calendars_dir(&root));
            let mut ctx = MgmtContext::open(vault, vdir).unwrap();
            ctx.quick_add("Buy milk", None).unwrap();
        }
        let creds = CredStore::open(&root.join("web-auth.db"), None).await.unwrap();
        creds.set_admin_password("supersecret", None).await.unwrap();
        creds.set_open_mode(false); // enforce auth
        let state = AppState::new(root, Config::default(), creds).unwrap();
        // One router instance so the in-memory session store persists across requests.
        let app = build_router(state, None, SessionManagerLayer::new(MemoryStore::default()));

        let send = |app: Router, req: Request<Body>| async move { app.oneshot(req).await.unwrap() };

        // Unauthenticated read is rejected.
        let resp = send(app.clone(), Request::builder().uri("/api/tasks").body(Body::empty()).unwrap()).await;
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

        // Wrong password is rejected.
        let bad = Request::builder()
            .method("POST").uri("/api/auth/login")
            .header("content-type", "application/json")
            .body(Body::from(r#"{"password":"nope"}"#)).unwrap();
        assert_eq!(send(app.clone(), bad).await.status(), StatusCode::UNAUTHORIZED);

        // Correct password issues a session cookie.
        let ok = Request::builder()
            .method("POST").uri("/api/auth/login")
            .header("content-type", "application/json")
            .body(Body::from(r#"{"password":"supersecret"}"#)).unwrap();
        let resp = send(app.clone(), ok).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let cookie = resp.headers().get(SET_COOKIE).unwrap().to_str().unwrap().split(';').next().unwrap().to_string();

        // The cookie grants access to the protected read.
        let with_cookie = Request::builder().uri("/api/tasks").header(COOKIE, &cookie).body(Body::empty()).unwrap();
        assert_eq!(send(app.clone(), with_cookie).await.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn setup_locks_everything_and_is_once_per_deployment() {
        use axum::http::header::SET_COOKIE;

        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        let creds = CredStore::open(&root.join("web-auth.db"), None).await.unwrap();
        creds.set_open_mode(false); // non-loopback deployment, no password yet → setup mode
        let state = AppState::new(root, Config::default(), creds.clone()).unwrap();
        let app = build_router(state, None, SessionManagerLayer::new(MemoryStore::default()));

        // In setup mode every data/auth route except setup/session/health is refused.
        for uri in ["/api/tasks", "/api/board", "/api/meta", "/api/admin/users", "/api/settings"] {
            let resp = app.clone().oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap()).await.unwrap();
            assert_eq!(resp.status(), StatusCode::FORBIDDEN, "{uri} must be locked in setup mode");
        }
        let (_s, session) = get_on(&app, "/api/auth/session").await;
        assert_eq!(session["needs_setup"], true);

        // First setup claims the admin (and logs in).
        let setup = |pw: &str| {
            Request::builder().method("POST").uri("/api/auth/setup")
                .header("content-type", "application/json")
                .body(Body::from(format!(r#"{{"password":"{pw}"}}"#))).unwrap()
        };
        let resp = app.clone().oneshot(setup("first-password")).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert!(resp.headers().get(SET_COOKIE).is_some());

        // A second (unauthenticated) setup attempt is rejected — once per deployment. The guard
        // 401s it before the handler's own 409 backstop; the original password stays in force.
        let resp = app.clone().oneshot(setup("evil-takeover-pw")).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        assert!(creds.verify_credentials("first-password", None, chrono::Utc::now()).await);

        // With the admin claimed, unauthenticated data routes now require login (401, not 403).
        let resp = app.clone().oneshot(Request::builder().uri("/api/tasks").body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

        // 2FA is opt-in: the session probe reports no TOTP unless enrolled.
        let (_s, session) = get_on(&app, "/api/auth/session").await;
        assert_eq!(session["totp"], false);
    }

    async fn get_on(app: &Router, uri: &str) -> (StatusCode, serde_json::Value) {
        let resp = app.clone().oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap()).await.unwrap();
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        (status, serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null))
    }

    #[tokio::test]
    async fn admin_creates_user_and_token_is_isolated_to_their_vault() {
        use axum::http::header::{AUTHORIZATION, COOKIE, SET_COOKIE};

        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        {
            let vault = VaultStore::new(mgmt_store::tasks_dir(&root));
            let vdir = VdirStore::new(mgmt_store::calendars_dir(&root));
            let mut ctx = MgmtContext::open(vault, vdir).unwrap();
            ctx.quick_add("admins secret task", None).unwrap(); // lives in the admin vault
        }
        let creds = CredStore::open(&root.join("web-auth.db"), None).await.unwrap();
        creds.set_admin_password("supersecret", None).await.unwrap();
        creds.set_open_mode(false);
        let state = AppState::new(root, Config::default(), creds).unwrap();
        let app = build_router(state, None, SessionManagerLayer::new(MemoryStore::default()));

        let body = |r: axum::response::Response| async {
            let s = r.status();
            let b = axum::body::to_bytes(r.into_body(), usize::MAX).await.unwrap();
            (s, serde_json::from_slice::<serde_json::Value>(&b).unwrap_or(serde_json::Value::Null))
        };

        // Admin logs in.
        let login = Request::builder().method("POST").uri("/api/auth/login")
            .header("content-type", "application/json")
            .body(Body::from(r#"{"password":"supersecret"}"#)).unwrap();
        let resp = app.clone().oneshot(login).await.unwrap();
        let cookie = resp.headers().get(SET_COOKIE).unwrap().to_str().unwrap().split(';').next().unwrap().to_string();

        // Create user "alice".
        let create = Request::builder().method("POST").uri("/api/admin/users")
            .header(COOKIE, &cookie).header("content-type", "application/json")
            .body(Body::from(r#"{"id":"alice","name":"Alice"}"#)).unwrap();
        assert_eq!(app.clone().oneshot(create).await.unwrap().status(), StatusCode::CREATED);

        // Mint a scoped sync token for alice.
        let mint = Request::builder().method("POST").uri("/api/admin/users/alice/tokens")
            .header(COOKIE, &cookie).header("content-type", "application/json")
            .body(Body::from("{}")).unwrap();
        let (_s, minted) = body(app.clone().oneshot(mint).await.unwrap()).await;
        let token = minted["token"].as_str().unwrap().to_string();

        // Alice's sync listing is her (empty) vault — she cannot see the admin's task.
        let alice_list = Request::builder().uri("/api/sync/tasks")
            .header(AUTHORIZATION, format!("Bearer {token}")).body(Body::empty()).unwrap();
        let (s, list) = body(app.clone().oneshot(alice_list).await.unwrap()).await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(list.as_array().unwrap().len(), 0, "alice's vault is isolated & empty");

        // Alice's token cannot reach admin-only routes.
        let alice_admin = Request::builder().uri("/api/admin/users")
            .header(AUTHORIZATION, format!("Bearer {token}")).body(Body::empty()).unwrap();
        assert_eq!(app.clone().oneshot(alice_admin).await.unwrap().status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn events_require_range() {
        let (state, _d) = test_state().await;
        let (status, _b) = get(&state, "/api/events").await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        let (ok, list) = get(&state, "/api/events?from=2026-01-01T00:00:00Z&to=2027-01-01T00:00:00Z").await;
        assert_eq!(ok, StatusCode::OK);
        assert!(list.is_array());
    }
}
