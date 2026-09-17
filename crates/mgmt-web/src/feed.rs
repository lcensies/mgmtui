//! Calendar subscriptions (read-only ICS/webcal mirrors) and tokenised feed URLs.
//!
//! Two surfaces live here:
//!   * `GET /api/feed/<token>.ics` — **unauthenticated** (exempted in [`crate::middleware`]).
//!     The token is the only credential, so it is 32 random bytes, compared in constant time,
//!     and an unknown token is a plain 404 — never a 401 and never a hint that some other
//!     calendar exists.
//!   * admin-only management of subscriptions and feed tokens, persisted in the vault's
//!     `calendars.yaml` sidecar (`mgmt_store::{load,save}_calendars`).
//!
//! ponytail: feeds resolve against the admin vault only (the web UI's vault). Per-user feeds
//! would need the token → user mapping to live in the credential DB instead.

use axum::extract::{Extension, Path, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};
use subtle::ConstantTimeEq;

use mgmt_domain::{normalize_feed_url, Collection, CollectionKind, RemoteSource};
use mgmt_store::{load_calendars, save_calendars, VdirStore};

use crate::auth::new_api_token;
use crate::error::{bad_request, not_found, ApiError};
use crate::middleware::Principal;
use crate::state::{is_safe_user_id as is_safe_id, AppState};

/// Subscription + feed routes, merged into the `/api` router.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/feed/:token", get(feed))
        .route("/calendar-feeds", get(list_calendars))
        .route("/calendar-feeds/:id", post(mint_feed).delete(revoke_feed))
        .route("/subscriptions", post(subscribe))
        .route("/subscriptions/:id", axum::routing::delete(unsubscribe))
        .route("/subscriptions/:id/refresh", post(refresh_now))
}

// ---- the public feed --------------------------------------------------------------------

/// `GET /api/feed/<token>.ics` — the calendar as one `VCALENDAR`, no authentication.
async fn feed(State(st): State<AppState>, Path(token): Path<String>) -> Result<Response, ApiError> {
    let token = token.strip_suffix(".ics").unwrap_or(&token).to_string();
    let cols = load_calendars(st.root())?;
    // Constant-time over every candidate: no early exit, no distinction between "no feeds at
    // all" and "wrong token", and an unknown token looks exactly like a missing page.
    let mut found: Option<&Collection> = None;
    for c in &cols {
        if let Some(t) = &c.feed_token {
            if bool::from(t.as_bytes().ct_eq(token.as_bytes())) {
                found = Some(c);
            }
        }
    }
    let coll = found.ok_or_else(|| not_found("not found"))?;
    let ctx = st.read().await;
    let body = to_vcalendar(&coll.display_name, ctx.events().iter().filter(|e| e.calendar == coll.id));
    Ok((
        [(header::CONTENT_TYPE, "text/calendar; charset=utf-8")],
        body,
    )
        .into_response())
}

/// Join events into a single `VCALENDAR` document (each `event_to_ics` is a document of its own).
fn to_vcalendar<'a>(name: &str, events: impl Iterator<Item = &'a mgmt_domain::Event>) -> String {
    let mut out = String::from("BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//mgmt//mgmt-ical//EN\r\n");
    out.push_str(&format!("X-WR-CALNAME:{}\r\n", name.replace(['\r', '\n'], " ")));
    for ev in events {
        let doc = mgmt_ical::event_to_ics(ev);
        for line in doc.lines().filter(|l| {
            !(l.starts_with("BEGIN:VCALENDAR") || l.starts_with("END:VCALENDAR") || l.starts_with("VERSION:") || l.starts_with("PRODID:"))
        }) {
            out.push_str(line);
            out.push_str("\r\n");
        }
    }
    out.push_str("END:VCALENDAR\r\n");
    out
}

// ---- management (admin only) ------------------------------------------------------------

fn require_admin(p: &Principal) -> Result<(), ApiError> {
    if p.0 == mgmt_store::ADMIN_USER {
        Ok(())
    } else {
        Err(ApiError::new(StatusCode::FORBIDDEN, "admin only"))
    }
}

/// Absolute feed URL when a public origin is configured, else the site-relative path.
fn feed_url(st: &AppState, token: &str) -> String {
    let path = format!("/api/feed/{token}.ics");
    match st.public_origin() {
        Some(o) => format!("{}{path}", o.trim_end_matches('/')),
        None => path,
    }
}

fn entry(st: &AppState, c: &Collection) -> Value {
    json!({
        "id": c.id,
        "display_name": c.display_name,
        "read_only": c.is_read_only(),
        "subscription": c.subscription().map(|(url, mins)| json!({ "url": url, "refresh_minutes": mins })),
        "feed_url": c.feed_token.as_deref().map(|t| feed_url(st, t)),
    })
}

/// `GET /api/calendar-feeds` — every calendar on disk, plus its subscription/feed metadata.
async fn list_calendars(State(st): State<AppState>, Extension(p): Extension<Principal>) -> Result<Json<Value>, ApiError> {
    require_admin(&p)?;
    let mut cols = load_calendars(st.root())?;
    for id in VdirStore::new(mgmt_store::calendars_dir(st.root())).collections()? {
        if !cols.iter().any(|c| c.id == id) {
            cols.push(Collection::local(id, CollectionKind::Events));
        }
    }
    cols.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(Json(json!({ "calendars": cols.iter().map(|c| entry(&st, c)).collect::<Vec<_>>() })))
}

/// Read the sidecar and apply `f` to the entry for `id` (creating it if absent), then save.
fn update_collection(st: &AppState, id: &str, f: impl FnOnce(&mut Collection)) -> Result<Collection, ApiError> {
    if !is_safe_id(id) {
        return Err(bad_request("invalid calendar id (use a-z, 0-9, - or _)"));
    }
    let mut cols = load_calendars(st.root())?;
    let idx = match cols.iter().position(|c| c.id == id) {
        Some(i) => i,
        None => {
            cols.push(Collection::local(id, CollectionKind::Events));
            cols.len() - 1
        }
    };
    f(&mut cols[idx]);
    save_calendars(st.root(), &cols)?;
    Ok(cols.swap_remove(idx))
}

/// `POST /api/calendar-feeds/:id` — mint (or rotate) the calendar's feed token.
async fn mint_feed(
    State(st): State<AppState>,
    Extension(p): Extension<Principal>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    require_admin(&p)?;
    let token = new_api_token().0;
    let c = update_collection(&st, &id, |c| c.feed_token = Some(token.clone()))?;
    Ok(Json(entry(&st, &c)))
}

/// `DELETE /api/calendar-feeds/:id` — revoke the token; the old URL 404s immediately.
async fn revoke_feed(
    State(st): State<AppState>,
    Extension(p): Extension<Principal>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    require_admin(&p)?;
    let c = update_collection(&st, &id, |c| c.feed_token = None)?;
    Ok(Json(entry(&st, &c)))
}

#[derive(Deserialize)]
struct SubscribeBody {
    /// Calendar id (directory name). Defaults to a slug of `name`.
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    name: Option<String>,
    url: String,
    #[serde(default)]
    refresh_minutes: Option<u32>,
}

/// `POST /api/subscriptions` — subscribe a (new) calendar to an ICS/webcal URL and fetch it once.
async fn subscribe(
    State(st): State<AppState>,
    Extension(p): Extension<Principal>,
    Json(body): Json<SubscribeBody>,
) -> Result<Json<Value>, ApiError> {
    require_admin(&p)?;
    let url = normalize_feed_url(&body.url)
        .ok_or_else(|| bad_request("URL must be a non-local http(s):// or webcal:// address"))?;
    let name = body.name.clone().unwrap_or_else(|| body.id.clone().unwrap_or_default());
    let id = body.id.clone().unwrap_or_else(|| slug(&name));
    let refresh = body.refresh_minutes.unwrap_or(60).clamp(5, 24 * 60);
    let display = if name.is_empty() { id.clone() } else { name };

    let c = update_collection(&st, &id, |c| {
        c.display_name = display;
        c.remote = Some(RemoteSource::Ics { url, refresh_minutes: refresh });
    })?;
    let fetched = refresh_blocking(&st, &c).await;
    let mut out = entry(&st, &c);
    out["events"] = json!(fetched?);
    Ok(Json(out))
}

/// `POST /api/subscriptions/:id/refresh` — fetch now instead of waiting for the daemon.
async fn refresh_now(
    State(st): State<AppState>,
    Extension(p): Extension<Principal>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    require_admin(&p)?;
    let c = load_calendars(st.root())?
        .into_iter()
        .find(|c| c.id == id && c.is_read_only())
        .ok_or_else(|| not_found(format!("no subscription '{id}'")))?;
    let n = refresh_blocking(&st, &c).await?;
    Ok(Json(json!({ "id": c.id, "events": n })))
}

/// `DELETE /api/subscriptions/:id` — unsubscribe and drop the mirrored copy (it is remote data).
async fn unsubscribe(
    State(st): State<AppState>,
    Extension(p): Extension<Principal>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    require_admin(&p)?;
    if !is_safe_id(&id) {
        return Err(bad_request("invalid calendar id"));
    }
    let mut cols = load_calendars(st.root())?;
    let Some(i) = cols.iter().position(|c| c.id == id && c.is_read_only()) else {
        return Err(not_found(format!("no subscription '{id}'")));
    };
    cols.remove(i);
    save_calendars(st.root(), &cols)?;
    let dir = mgmt_store::calendars_dir(st.root()).join(&id);
    std::fs::remove_dir_all(&dir).ok();
    st.mark_stale();
    st.admin().notify_changed();
    Ok(Json(json!({ "ok": true })))
}

/// Fetch + replace on the blocking pool (the sync path is synchronous by design).
async fn refresh_blocking(st: &AppState, coll: &Collection) -> Result<usize, ApiError> {
    let root = st.root().to_path_buf();
    let coll = coll.clone();
    let n = tokio::task::spawn_blocking(move || {
        let mut store = VdirStore::new(mgmt_store::calendars_dir(&root));
        mgmt_sync::refresh_subscription(&mut store, &coll)
    })
    .await
    .map_err(|e| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    // A remote feed that is down is the client's problem to see, not a 500.
    .map_err(|e| ApiError::new(StatusCode::BAD_GATEWAY, e.to_string()))?;
    st.mark_stale();
    st.admin().notify_changed();
    Ok(n)
}

/// Directory-safe id from a display name.
fn slug(name: &str) -> String {
    let s: String = name
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '-' })
        .collect();
    let s = s.trim_matches('-').to_string();
    if s.is_empty() { "subscription".into() } else { s.chars().take(64).collect() }
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use chrono::{TimeZone, Utc};
    use mgmt_config::Config;
    use mgmt_service::MgmtContext;
    use mgmt_store::{VaultStore, VdirStore};
    use tower::ServiceExt;
    use tower_sessions::{MemoryStore, SessionManagerLayer};

    use crate::auth::CredStore;
    use crate::{build_router, AppState};

    async fn state_with_event() -> (AppState, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        let vault = VaultStore::new(mgmt_store::tasks_dir(&root));
        let vdir = VdirStore::new(mgmt_store::calendars_dir(&root));
        let mut ctx = MgmtContext::open(vault, vdir).unwrap();
        let start = Utc.with_ymd_and_hms(2026, 6, 18, 9, 0, 0).unwrap();
        ctx.put_event(mgmt_domain::Event::new("work", "Standup", start, start + chrono::Duration::hours(1)))
            .unwrap();
        drop(ctx);
        let state = AppState::new(root, Config::default(), CredStore::disabled().await.unwrap()).unwrap();
        (state, dir)
    }

    async fn send(st: &AppState, method: &str, uri: &str) -> (StatusCode, String) {
        let app = build_router(st.clone(), None, SessionManagerLayer::new(MemoryStore::default()));
        let resp = app
            .oneshot(Request::builder().method(method).uri(uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        (status, String::from_utf8_lossy(&bytes).into_owned())
    }

    #[tokio::test]
    async fn feed_token_serves_the_calendar_and_404s_once_revoked() {
        let (st, _d) = state_with_event().await;

        let (status, body) = send(&st, "POST", "/api/calendar-feeds/work").await;
        assert_eq!(status, StatusCode::OK);
        let minted: serde_json::Value = serde_json::from_str(&body).unwrap();
        let url = minted["feed_url"].as_str().unwrap().to_string();
        let token = url.trim_start_matches("/api/feed/").trim_end_matches(".ics").to_string();
        assert!(token.len() >= 32, "feed token must carry >= 32 bytes of entropy");

        let (status, ics) = send(&st, "GET", &url).await;
        assert_eq!(status, StatusCode::OK);
        assert!(ics.contains("BEGIN:VCALENDAR") && ics.contains("SUMMARY:Standup"));

        // An unknown token is indistinguishable from a missing page (404, not 401).
        let (status, _b) = send(&st, "GET", "/api/feed/definitely-not-a-token.ics").await;
        assert_eq!(status, StatusCode::NOT_FOUND);

        let (status, _b) = send(&st, "DELETE", "/api/calendar-feeds/work").await;
        assert_eq!(status, StatusCode::OK);
        let (status, _b) = send(&st, "GET", &url).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "a revoked feed URL must stop working");
    }

    #[tokio::test]
    async fn subscription_events_are_read_only_and_unsubscribe_removes_them() {
        let (st, _d) = state_with_event().await;
        let root = st.root().to_path_buf();

        // Simulate what a refresh produces: the sidecar entry plus mirrored events.
        let mut coll = mgmt_domain::Collection::local("holidays", mgmt_domain::CollectionKind::Events);
        coll.remote = Some(mgmt_domain::RemoteSource::Ics {
            url: "https://ex.org/h.ics".into(),
            refresh_minutes: 60,
        });
        mgmt_store::save_calendars(&root, &[coll]).unwrap();
        let mut store = VdirStore::new(mgmt_store::calendars_dir(&root));
        mgmt_sync::replace_collection(
            &mut store,
            "holidays",
            "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:h1\r\nDTSTART:20260101T000000Z\r\nDTEND:20260101T010000Z\r\nSUMMARY:NY\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n",
        )
        .unwrap();

        let (status, body) = send(&st, "GET", "/api/calendar-feeds").await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.contains("\"read_only\":true"));

        let (status, _b) = send(&st, "DELETE", "/api/subscriptions/holidays").await;
        assert_eq!(status, StatusCode::OK);
        assert!(!mgmt_store::calendars_dir(&root).join("holidays").exists());
        assert!(mgmt_store::load_calendars(&root).unwrap().is_empty());
    }
}
