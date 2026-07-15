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
    AUTHORIZATION, CACHE_CONTROL, CONTENT_SECURITY_POLICY, HOST, ORIGIN, REFERRER_POLICY,
    X_CONTENT_TYPE_OPTIONS,
};
use axum::http::{HeaderMap, Method, StatusCode};
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
    matches!(path, "/auth/login" | "/auth/session" | "/auth/invite" | "/auth/invite/accept" | "/health")
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
    if creds.needs_setup().await {
        if !is_setup_public(&path) {
            return setup_required();
        }
        return finish(next, req).await;
    }

    let principal = resolve_principal(creds, &auth_session, req.headers()).await;
    if principal.is_none() && !is_public(&path) {
        return unauthorized();
    }
    // CSRF defense in depth for cookie-authenticated mutations (incl. login): a browser-supplied
    // Origin must match the request Host or the configured public origin. Bearer clients don't
    // ride ambient credentials and non-browser clients send no Origin — both pass; open mode
    // (unauthenticated local dev, often behind the vite proxy) has no cookie to protect.
    let via_bearer = match bearer_token(req.headers()) {
        Some(t) => creds.resolve_bearer(&t).await.is_some(),
        None => false,
    };
    if is_mutation(req.method()) && !via_bearer && !creds.is_open().await && !origin_ok(req.headers(), st.public_origin()) {
        return forbidden_origin();
    }
    if let Some(user) = principal {
        req.extensions_mut().insert(Principal(user));
    }
    finish(next, req).await
}

fn is_mutation(m: &Method) -> bool {
    !matches!(*m, Method::GET | Method::HEAD | Method::OPTIONS)
}

/// Whether a request's Origin (if any) is our own: the configured public origin, or the same
/// host:port the request was addressed to.
fn origin_ok(headers: &HeaderMap, public_origin: Option<&str>) -> bool {
    let Some(origin) = headers.get(ORIGIN).and_then(|v| v.to_str().ok()) else {
        return true; // non-browser client
    };
    if let Some(po) = public_origin {
        if origin.trim_end_matches('/') == po.trim_end_matches('/') {
            return true;
        }
    }
    let host = headers.get(HOST).and_then(|v| v.to_str().ok()).unwrap_or("");
    !host.is_empty()
        && origin
            .strip_prefix("https://")
            .or_else(|| origin.strip_prefix("http://"))
            .map(|h| h == host)
            .unwrap_or(false)
}

/// Resolve the request principal: a valid bearer token wins (native sync → its user's vault),
/// otherwise a logged-in admin session, otherwise the admin when running in open mode.
async fn resolve_principal(creds: &CredStore, auth_session: &AuthSession, headers: &HeaderMap) -> Option<String> {
    if let Some(token) = bearer_token(headers) {
        if let Some(uid) = creds.resolve_bearer(&token).await {
            return Some(uid);
        }
    }
    if let Some(user) = &auth_session.user {
        return Some(user.id.clone());
    }
    if creds.is_open().await {
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

fn forbidden_origin() -> Response {
    (StatusCode::FORBIDDEN, Json(json!({ "error": "cross-origin request rejected" }))).into_response()
}

/// Extract the client IP for rate limiting. The TCP peer address is authoritative;
/// `X-Forwarded-For` (first hop) is honored only when the peer is loopback — i.e. a reverse proxy
/// on this host. Trusting the header from arbitrary peers would hand every attacker a fresh
/// rate-limit bucket per request (and let them lock out other clients by spoofing their IP).
pub fn client_ip(peer: Option<std::net::SocketAddr>, headers: &HeaderMap) -> std::net::IpAddr {
    let forwarded = headers
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(',').next())
        .and_then(|s| s.trim().parse().ok());
    match peer {
        Some(p) if p.ip().is_loopback() => forwarded.unwrap_or_else(|| p.ip()),
        Some(p) => p.ip(),
        // No connect info (in-process test harness) — fall back to the header, then loopback.
        None => forwarded.unwrap_or_else(|| "127.0.0.1".parse().unwrap()),
    }
}
