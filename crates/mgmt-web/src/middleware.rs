//! Request middleware: resolve the request principal (admin session *or* a per-user sync token),
//! enforce authentication + first-run setup gating, and stamp security headers.
//!
//! Session/cookie management is handled upstream by `axum-login` + `tower-sessions`; this guard just
//! reads the resolved [`AuthSession`] and layers on the bits those libraries don't cover: the
//! bearer-token → user-vault mapping for the native sync protocol, open/setup modes, and the
//! `Principal` extension that per-user handlers consume. CSRF is covered by the session cookie's
//! `SameSite=Lax` attribute (a cross-site mutation never carries the cookie).

use axum::extract::{Request, State};
use axum::http::header::{
    AUTHORIZATION, CACHE_CONTROL, CONTENT_SECURITY_POLICY, REFERRER_POLICY, X_CONTENT_TYPE_OPTIONS,
};
use axum::http::{HeaderMap, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;

use crate::auth::CredStore;
use crate::auth_backend::AuthSession;
use crate::state::AppState;

/// The authenticated principal for a request: the id of the user whose vault this request operates
/// on. Stashed in the request extensions by [`guard`] so per-user handlers (the sync protocol) can
/// pick the right context. A session cookie resolves to the admin; a bearer token to its owner.
#[derive(Debug, Clone)]
pub struct Principal(pub String);

/// Paths reachable without authentication (login, session probe, liveness). Note: the guard runs
/// inside the nested `/api` router, so it sees paths with the `/api` prefix already stripped.
fn is_public(path: &str) -> bool {
    matches!(path, "/auth/login" | "/auth/session" | "/health")
}

/// Paths reachable while the server is in first-run *setup* mode (nothing else is served).
fn is_setup_public(path: &str) -> bool {
    matches!(path, "/auth/setup" | "/auth/session" | "/health")
}

/// The main auth/guard middleware.
pub async fn guard(
    State(st): State<AppState>,
    auth_session: AuthSession,
    mut req: Request,
    next: Next,
) -> Response {
    let creds = st.creds();
    let path = req.uri().path().to_string();

    // First-run setup: no admin is configured and open mode is off. Lock everything except the
    // setup/probe endpoints so a passwordless public server can't be read or written until claimed.
    if creds.needs_setup() {
        if !is_setup_public(&path) {
            return setup_required();
        }
        return finish(next, req).await;
    }

    let principal = resolve_principal(creds, &auth_session, req.headers());
    if principal.is_none() && !is_public(&path) {
        return unauthorized();
    }
    if let Some(user) = principal {
        req.extensions_mut().insert(Principal(user));
    }
    finish(next, req).await
}

/// Resolve the request principal: a valid bearer token wins (native sync → its user's vault),
/// otherwise a logged-in admin session, otherwise the admin when running in open mode.
fn resolve_principal(creds: &CredStore, auth_session: &AuthSession, headers: &HeaderMap) -> Option<String> {
    if let Some(token) = bearer_token(headers) {
        if let Some(uid) = creds.resolve_bearer(&token) {
            return Some(uid);
        }
    }
    if let Some(user) = &auth_session.user {
        return Some(user.id.clone());
    }
    if creds.is_open() {
        return Some(mgmt_store::ADMIN_USER.to_string());
    }
    None
}

async fn finish(next: Next, req: Request) -> Response {
    let mut resp = next.run(req).await;
    stamp_security_headers(&mut resp);
    resp
}

fn bearer_token(headers: &HeaderMap) -> Option<String> {
    let v = headers.get(AUTHORIZATION)?.to_str().ok()?;
    v.strip_prefix("Bearer ").map(|s| s.trim().to_string())
}

fn stamp_security_headers(resp: &mut Response) {
    let h = resp.headers_mut();
    h.insert(
        CONTENT_SECURITY_POLICY,
        "default-src 'self'; img-src 'self' data:; connect-src 'self'".parse().unwrap(),
    );
    h.insert(X_CONTENT_TYPE_OPTIONS, "nosniff".parse().unwrap());
    h.insert(REFERRER_POLICY, "no-referrer".parse().unwrap());
    h.insert(CACHE_CONTROL, "no-store".parse().unwrap());
}

fn unauthorized() -> Response {
    (StatusCode::UNAUTHORIZED, Json(json!({ "error": "unauthorized" }))).into_response()
}

fn setup_required() -> Response {
    (StatusCode::FORBIDDEN, Json(json!({ "error": "setup_required" }))).into_response()
}

/// Extract the client IP for rate limiting, honoring `X-Forwarded-For` (first hop) when present.
pub fn client_ip(headers: &HeaderMap) -> std::net::IpAddr {
    headers
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(',').next())
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or_else(|| "127.0.0.1".parse().unwrap())
}
