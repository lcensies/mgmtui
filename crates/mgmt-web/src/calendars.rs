//! Local calendar management: CRUD over the collection directories under `<vault>/calendars`,
//! plus ICS upload/download.
//!
//! The directory name is authoritative for an event's `calendar` (see `mgmt_store::VdirStore`);
//! `config.yaml`'s `calendars:` block only carries presentation metadata (display name, color),
//! so a calendar can exist on disk without a config entry.

use std::collections::BTreeMap;
use std::path::PathBuf;

use axum::extract::{Path, Query, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};

use mgmt_config::{CalendarEntry, Config};
use mgmt_store::VdirStore;

use crate::error::{bad_request, not_found, ApiError};
use crate::state::{is_safe_user_id, AppState};

/// Calendar events land here when their calendar is force-deleted.
const DEFAULT_CALENDAR: &str = "default";

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/calendars", get(list).post(create))
        .route("/calendars/:id", axum::routing::put(update).delete(remove))
        .route("/calendars/:id/import", post(import))
        .route("/calendars/:id/export.ics", get(export))
}

fn vdir(st: &AppState) -> VdirStore {
    VdirStore::new(mgmt_store::calendars_dir(st.root()))
}

/// `config.yaml` sits next to the web-managed `caldav.yaml`, the only config path the server is
/// handed. ponytail: derived rather than threaded through `WebOptions`/`AppState`.
fn config_path(st: &AppState) -> Result<PathBuf, ApiError> {
    let caldav = st.caldav_file();
    if caldav.as_os_str().is_empty() {
        return Err(bad_request("no config file configured"));
    }
    Ok(caldav.with_file_name("config.yaml"))
}

fn entries(st: &AppState) -> Result<Vec<CalendarEntry>, ApiError> {
    Ok(Config::load(&config_path(st)?)?.calendars)
}

/// A calendar id doubles as a directory name, so it must be traversal-safe.
fn check_id(id: &str) -> Result<(), ApiError> {
    if is_safe_user_id(id) {
        Ok(())
    } else {
        Err(bad_request(format!("invalid calendar id '{id}'")))
    }
}

/// `GET /api/calendars` — every collection on disk (plus any config-only entry), with its
/// display name, color, and event count.
async fn list(State(st): State<AppState>) -> Result<Json<Value>, ApiError> {
    let meta: BTreeMap<String, CalendarEntry> = entries(&st)?.into_iter().map(|e| (e.id.clone(), e)).collect();
    let ctx = st.read().await;
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for id in vdir(&st).collections()? {
        counts.entry(id).or_default();
    }
    for ev in ctx.events() {
        *counts.entry(ev.calendar.clone()).or_default() += 1;
    }
    for id in meta.keys() {
        counts.entry(id.clone()).or_default();
    }
    let out: Vec<Value> = counts
        .into_iter()
        .map(|(id, events)| {
            let e = meta.get(&id);
            json!({
                "id": id,
                "display_name": e.and_then(|e| e.display_name.clone()).unwrap_or_else(|| id.clone()),
                "color": e.and_then(|e| e.color.clone()),
                "events": events,
            })
        })
        .collect();
    Ok(Json(json!(out)))
}

#[derive(Deserialize)]
struct NewCalendar {
    id: String,
    #[serde(default)]
    display_name: Option<String>,
    #[serde(default)]
    color: Option<String>,
}

/// `POST /api/calendars` — create the collection directory and record its metadata.
async fn create(State(st): State<AppState>, Json(body): Json<NewCalendar>) -> Result<Json<Value>, ApiError> {
    let id = body.id.trim().to_string();
    check_id(&id)?;
    let store = vdir(&st);
    if store.collections()?.iter().any(|c| c == &id) {
        return Err(ApiError::new(StatusCode::CONFLICT, format!("calendar '{id}' already exists")));
    }
    store.ensure_collection(&id)?;
    let mut list = entries(&st)?;
    list.retain(|e| e.id != id);
    list.push(CalendarEntry { id: id.clone(), display_name: body.display_name, color: body.color });
    mgmt_config::save_calendars(&config_path(&st)?, &list)?;
    Ok(Json(json!({ "id": id })))
}

#[derive(Deserialize)]
struct CalendarEdit {
    /// New id — renames the collection directory (and re-homes its events with it).
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    display_name: Option<String>,
    #[serde(default)]
    color: Option<String>,
}

/// `PUT /api/calendars/:id` — rename (directory), relabel, and/or recolor a calendar.
async fn update(State(st): State<AppState>, Path(id): Path<String>, Json(body): Json<CalendarEdit>) -> Result<Json<Value>, ApiError> {
    let store = vdir(&st);
    if !store.collections()?.iter().any(|c| c == &id) {
        return Err(not_found(format!("calendar {id}")));
    }
    let new_id = body.id.map(|s| s.trim().to_string()).filter(|s| !s.is_empty() && s != &id);
    if let Some(new) = &new_id {
        check_id(new)?;
        if store.collections()?.iter().any(|c| c == new) {
            return Err(ApiError::new(StatusCode::CONFLICT, format!("calendar '{new}' already exists")));
        }
        // The directory is authoritative for `Event.calendar`, so renaming it re-homes the events.
        let mut ctx = st.write().await;
        std::fs::rename(store.root().join(&id), store.root().join(new)).map_err(mgmt_core::Error::Io)?;
        ctx.reload()?;
    }
    let target = new_id.clone().unwrap_or_else(|| id.clone());
    let mut list = entries(&st)?;
    let mut entry = list.iter().find(|e| e.id == id).cloned().unwrap_or_default();
    entry.id = target.clone();
    if let Some(name) = body.display_name {
        entry.display_name = Some(name).filter(|s| !s.trim().is_empty());
    }
    if let Some(color) = body.color {
        entry.color = Some(color).filter(|s| !s.trim().is_empty());
    }
    list.retain(|e| e.id != id && e.id != target);
    list.push(entry);
    mgmt_config::save_calendars(&config_path(&st)?, &list)?;
    Ok(Json(json!({ "id": target })))
}

#[derive(Deserialize)]
struct DeleteQuery {
    #[serde(default)]
    force: Option<String>,
}

/// `DELETE /api/calendars/:id[?force=1]` — delete a calendar. A non-empty calendar needs `force`,
/// which moves its events into `default` instead of destroying them.
async fn remove(State(st): State<AppState>, Path(id): Path<String>, Query(q): Query<DeleteQuery>) -> Result<Json<Value>, ApiError> {
    let store = vdir(&st);
    if !store.collections()?.iter().any(|c| c == &id) {
        return Err(not_found(format!("calendar {id}")));
    }
    if id == DEFAULT_CALENDAR {
        return Err(bad_request("the default calendar cannot be deleted"));
    }
    let force = q.force.map(|f| f != "0" && f != "false").unwrap_or(false);
    let mut moved = 0;
    {
        let mut ctx = st.write().await;
        let events: Vec<_> = ctx.events().iter().filter(|e| e.calendar == id).cloned().collect();
        if !events.is_empty() && !force {
            return Err(ApiError::new(
                StatusCode::CONFLICT,
                format!("calendar '{id}' has {} event(s); pass force to move them to {DEFAULT_CALENDAR}", events.len()),
            ));
        }
        store.ensure_collection(DEFAULT_CALENDAR)?;
        for mut ev in events {
            ev.calendar = DEFAULT_CALENDAR.to_string();
            ctx.put_event(ev)?; // re-homes the .ics file, removing the old copy
            moved += 1;
        }
        std::fs::remove_dir_all(store.root().join(&id)).map_err(mgmt_core::Error::Io)?;
        ctx.reload()?;
    }
    let mut list = entries(&st)?;
    list.retain(|e| e.id != id);
    mgmt_config::save_calendars(&config_path(&st)?, &list)?;
    Ok(Json(json!({ "ok": true, "moved": moved })))
}

/// `POST /api/calendars/:id/import` — body is the raw `.ics` document (`text/calendar`).
///
/// ponytail: raw body instead of `multipart/form-data` — axum's `multipart` feature pulls `multer`,
/// which is not in the offline crate cache. The browser reads the file with `File.text()`.
async fn import(State(st): State<AppState>, Path(id): Path<String>, text: String) -> Result<Json<Value>, ApiError> {
    check_id(&id)?;
    vdir(&st).ensure_collection(&id)?;
    let mut ctx = st.write().await;
    let imported = mgmt_service::import_ics(&mut ctx, &text, &id)?;
    Ok(Json(json!({ "imported": imported })))
}

/// `GET /api/calendars/:id/export.ics` — the calendar as a downloadable ICS document.
async fn export(State(st): State<AppState>, Path(id): Path<String>) -> Result<Response, ApiError> {
    let ctx = st.read().await;
    let body = mgmt_service::export_ics(&ctx, Some(&id));
    Ok((
        [
            (header::CONTENT_TYPE, "text/calendar; charset=utf-8".to_string()),
            (header::CONTENT_DISPOSITION, format!("attachment; filename=\"{id}.ics\"")),
        ],
        body,
    )
        .into_response())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use mgmt_service::MgmtContext;
    use mgmt_store::VaultStore;
    use tower::ServiceExt;
    use tower_sessions::{MemoryStore, SessionManagerLayer};

    async fn test_state() -> (AppState, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        let state = AppState::configure(
            root,
            Config::default(),
            crate::auth::CredStore::disabled().await.unwrap(),
            None,
            dir.path().join("cfg").join("caldav.yaml"),
        )
        .unwrap();
        (state, dir)
    }

    async fn send(st: &AppState, method: &str, uri: &str, body: &str) -> (StatusCode, Value) {
        let app = crate::build_router(st.clone(), None, SessionManagerLayer::new(MemoryStore::default()));
        let resp = app
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(uri)
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let value = serde_json::from_slice(&bytes).unwrap_or(Value::String(String::from_utf8_lossy(&bytes).into()));
        (status, value)
    }

    fn ics(n: usize) -> String {
        let events: String = (1..=n)
            .map(|i| format!("BEGIN:VEVENT\r\nUID:up{i}\r\nSUMMARY:Ev {i}\r\nDTSTART:20260618T090000Z\r\nDTEND:20260618T100000Z\r\nEND:VEVENT\r\n"))
            .collect();
        format!("BEGIN:VCALENDAR\r\nVERSION:2.0\r\n{events}END:VCALENDAR\r\n")
    }

    #[tokio::test]
    async fn crud_round_trip() {
        let (st, _d) = test_state().await;
        assert_eq!(send(&st, "POST", "/api/calendars", r##"{"id":"work","color":"#ff8800"}"##).await.0, StatusCode::OK);
        assert_eq!(send(&st, "POST", "/api/calendars", r#"{"id":"work"}"#).await.1["error"].is_string(), true);
        assert_eq!(send(&st, "POST", "/api/calendars", r#"{"id":"../etc"}"#).await.0, StatusCode::BAD_REQUEST);

        let (status, body) = send(&st, "GET", "/api/calendars", "").await;
        assert_eq!(status, StatusCode::OK);
        let work = body.as_array().unwrap().iter().find(|c| c["id"] == "work").unwrap().clone();
        assert_eq!(work["color"], "#ff8800");
        assert_eq!(work["display_name"], "work");
        assert_eq!(work["events"], 0);

        // Recolor + relabel persist to config.yaml.
        assert_eq!(send(&st, "PUT", "/api/calendars/work", r##"{"display_name":"Work","color":"#00ff00"}"##).await.0, StatusCode::OK);
        let cfg = Config::load(&st.caldav_file().with_file_name("config.yaml")).unwrap();
        assert_eq!(cfg.calendars[0].color.as_deref(), Some("#00ff00"));
        assert_eq!(cfg.calendars[0].display_name.as_deref(), Some("Work"));

        // Rename moves the directory (and with it the events' calendar).
        assert_eq!(send(&st, "PUT", "/api/calendars/work", r#"{"id":"job"}"#).await.0, StatusCode::OK);
        let (_s, body) = send(&st, "GET", "/api/calendars", "").await;
        assert!(body.as_array().unwrap().iter().any(|c| c["id"] == "job"));
        assert!(!body.as_array().unwrap().iter().any(|c| c["id"] == "work"));

        assert_eq!(send(&st, "DELETE", "/api/calendars/job", "").await.0, StatusCode::OK);
        let (_s, body) = send(&st, "GET", "/api/calendars", "").await;
        assert!(!body.as_array().unwrap().iter().any(|c| c["id"] == "job"));
    }

    #[tokio::test]
    async fn upload_reports_count_and_export_returns_them() {
        let (st, _d) = test_state().await;
        let (status, body) = send(&st, "POST", "/api/calendars/work/import", &ics(5)).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["imported"], 5);

        let (_s, list) = send(&st, "GET", "/api/calendars", "").await;
        let work = list.as_array().unwrap().iter().find(|c| c["id"] == "work").unwrap().clone();
        assert_eq!(work["events"], 5);

        let (status, exported) = send(&st, "GET", "/api/calendars/work/export.ics", "").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(exported.as_str().unwrap().matches("BEGIN:VEVENT").count(), 5);
    }

    #[tokio::test]
    async fn non_empty_delete_needs_force_and_moves_events_to_default() {
        let (st, _d) = test_state().await;
        send(&st, "POST", "/api/calendars/work/import", &ics(2)).await;

        assert_eq!(send(&st, "DELETE", "/api/calendars/work", "").await.0, StatusCode::CONFLICT);

        let (status, body) = send(&st, "DELETE", "/api/calendars/work?force=1", "").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["moved"], 2);

        let vault = VaultStore::new(mgmt_store::tasks_dir(st.root()));
        let events = MgmtContext::open(vault, vdir(&st)).unwrap();
        assert_eq!(events.events().len(), 2);
        assert!(events.events().iter().all(|e| e.calendar == DEFAULT_CALENDAR));
    }
}
