//! Request middleware: enforce authentication, guard mutations with an Origin check, and stamp
//! security headers on every response.

use axum::extract::{Request, State};
use axum::http::header::{
    AUTHORIZATION, CACHE_CONTROL, CONTENT_SECURITY_POLICY, COOKIE, ORIGIN, REFERRER_POLICY,
    X_CONTENT_TYPE_OPTIONS,
};
use axum::http::{HeaderMap, Method, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::Json;
use chrono::Utc;
use serde_json::json;

use crate::auth::AuthState;
use crate::state::AppState;

/// How a request authenticated (or didn't).
enum AuthKind {
    None,
    Bearer,
    Session,
}

/// Paths reachable without authentication (login, session probe, liveness). Note: the guard runs
/// inside the nested `/api` router, so it sees paths with the `/api` prefix already stripped.
fn is_public(path: &str) -> bool {
    matches!(path, "/auth/login" | "/auth/session" | "/health")
}

/// Whether the request carries a valid session or bearer credential (for the session probe).
pub fn is_authenticated(headers: &HeaderMap, auth: &AuthState, now: chrono::DateTime<Utc>) -> bool {
    !matches!(classify(headers, auth, now), AuthKind::None)
}

fn is_mutation(method: &Method) -> bool {
    !matches!(*method, Method::GET | Method::HEAD | Method::OPTIONS)
}

/// The main auth/guard middleware.
pub async fn guard(State(st): State<AppState>, req: Request, next: Next) -> Response {
    let auth = st.auth();
    let path = req.uri().path().to_string();

    if auth.enabled() && !is_public(&path) {
        let now = Utc::now();
        let kind = classify(req.headers(), auth, now);
        if matches!(kind, AuthKind::None) {
            return unauthorized();
        }
        // Cookie-authenticated mutations must carry a matching Origin (CSRF defense). Bearer clients
        // (the sync tool) send no Origin and are exempt.
        if matches!(kind, AuthKind::Session) && is_mutation(req.method()) {
            if let Some(expected) = auth.public_origin() {
                if !origin_ok(req.headers(), expected) {
                    return forbidden("bad origin");
                }
            }
        }
    }

    let mut resp = next.run(req).await;
    stamp_security_headers(&mut resp);
    resp
}

fn classify(headers: &HeaderMap, auth: &AuthState, now: chrono::DateTime<Utc>) -> AuthKind {
    if let Some(token) = bearer_token(headers) {
        if auth.validate_bearer(&token) {
            return AuthKind::Bearer;
        }
    }
    if let Some(token) = session_cookie(headers, auth.cookie_name()) {
        if auth.validate_session(&token, now) {
            return AuthKind::Session;
        }
    }
    AuthKind::None
}

fn bearer_token(headers: &HeaderMap) -> Option<String> {
    let v = headers.get(AUTHORIZATION)?.to_str().ok()?;
    v.strip_prefix("Bearer ").map(|s| s.trim().to_string())
}

fn session_cookie(headers: &HeaderMap, name: &str) -> Option<String> {
    let raw = headers.get(COOKIE)?.to_str().ok()?;
    for pair in raw.split(';') {
        let pair = pair.trim();
        if let Some((k, v)) = pair.split_once('=') {
            if k == name {
                return Some(v.to_string());
            }
        }
    }
    None
}

fn origin_ok(headers: &HeaderMap, expected: &str) -> bool {
    match headers.get(ORIGIN).and_then(|v| v.to_str().ok()) {
        // A same-origin fetch sends Origin; it must match. A missing Origin (some non-browser
        // clients) is allowed — those aren't the CSRF threat model.
        Some(origin) => origin == expected,
        None => true,
    }
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

fn forbidden(msg: &str) -> Response {
    (StatusCode::FORBIDDEN, Json(json!({ "error": msg }))).into_response()
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
