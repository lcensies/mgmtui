//! The native sync protocol: serves the vault's raw `.md`/`.ics` files with content-hash ETags so
//! a desktop `mgmt sync` can reconcile against this server using the same href/etag, remote-wins
//! algorithm it uses for CalDAV — but with full markdown fidelity (no lossy VTODO round-trip).
//!
//! Listings return `[{ href, etag }]`; item GET/PUT/DELETE carry `ETag` and honor
//! `If-Match`/`If-None-Match` (mismatch → 412). Writes go straight to disk (byte-fidelity) and then
//! reload the shared context so the read API reflects them.

use std::path::{Path, PathBuf};

use axum::extract::{Path as UrlPath, State};
use axum::http::header::{CONTENT_TYPE, ETAG, IF_MATCH, IF_NONE_MATCH};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde_json::json;
use sha2::{Digest, Sha256};

use mgmt_core::Error;
use mgmt_store::atomic_write;

use crate::error::{bad_request, not_found, ApiError};
use crate::state::AppState;

/// The sync sub-router, mounted under `/api/sync`. Every route requires authentication (typically a
/// bearer token from the desktop client), enforced by the shared guard.
pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/tasks", get(list_tasks))
        .route("/tasks/:file", get(get_task).put(put_task).delete(delete_task))
        .route("/calendars/:coll", get(list_events))
        .route("/calendars/:coll/:file", get(get_event).put(put_event).delete(delete_event))
        .layer(axum::middleware::from_fn_with_state(state.clone(), crate::middleware::guard))
        .with_state(state)
}

// ---- tasks ------------------------------------------------------------------------

async fn list_tasks(State(st): State<AppState>) -> Json<serde_json::Value> {
    let dir = mgmt_store::tasks_dir(st.root());
    Json(json!(listing(&dir, "md")))
}

async fn get_task(State(st): State<AppState>, UrlPath(file): UrlPath<String>) -> Result<Response, ApiError> {
    let path = task_path(&st, &file)?;
    serve_file(&path, "text/markdown")
}

async fn put_task(State(st): State<AppState>, UrlPath(file): UrlPath<String>, headers: HeaderMap, body: String) -> Result<Response, ApiError> {
    let path = task_path(&st, &file)?;
    write_item(&st, &path, &headers, body).await
}

async fn delete_task(State(st): State<AppState>, UrlPath(file): UrlPath<String>, headers: HeaderMap) -> Result<Response, ApiError> {
    let path = task_path(&st, &file)?;
    remove_item(&st, &path, &headers).await
}

// ---- events ------------------------------------------------------------------------

async fn list_events(State(st): State<AppState>, UrlPath(coll): UrlPath<String>) -> Result<Json<serde_json::Value>, ApiError> {
    let dir = collection_dir(&st, &coll)?;
    Ok(Json(json!(listing(&dir, "ics"))))
}

async fn get_event(State(st): State<AppState>, UrlPath((coll, file)): UrlPath<(String, String)>) -> Result<Response, ApiError> {
    let path = event_path(&st, &coll, &file)?;
    serve_file(&path, "text/calendar")
}

async fn put_event(State(st): State<AppState>, UrlPath((coll, file)): UrlPath<(String, String)>, headers: HeaderMap, body: String) -> Result<Response, ApiError> {
    let path = event_path(&st, &coll, &file)?;
    write_item(&st, &path, &headers, body).await
}

async fn delete_event(State(st): State<AppState>, UrlPath((coll, file)): UrlPath<(String, String)>, headers: HeaderMap) -> Result<Response, ApiError> {
    let path = event_path(&st, &coll, &file)?;
    remove_item(&st, &path, &headers).await
}

// ---- shared helpers ---------------------------------------------------------------

/// A directory listing of `{ href, etag }` for files with `ext`.
fn listing(dir: &Path, ext: &str) -> Vec<serde_json::Value> {
    let mut out = Vec::new();
    let files = mgmt_store::collect_files(dir, ext).unwrap_or_default();
    for path in files {
        if let (Some(name), Ok(bytes)) = (path.file_name().and_then(|n| n.to_str()), std::fs::read(&path)) {
            out.push(json!({ "href": name, "etag": etag(&bytes) }));
        }
    }
    out
}

fn serve_file(path: &Path, content_type: &str) -> Result<Response, ApiError> {
    let bytes = std::fs::read(path).map_err(|_| not_found("no such item"))?;
    let tag = etag(&bytes);
    Ok(([(ETAG, tag), (CONTENT_TYPE, content_type.to_string())], bytes).into_response())
}

/// Write (create or update) an item, honoring `If-Match` / `If-None-Match`, then reload the context.
async fn write_item(st: &AppState, path: &Path, headers: &HeaderMap, body: String) -> Result<Response, ApiError> {
    // Serialize with the read/write path by holding the write lock across the fs op + reload.
    let mut ctx = st.write().await;

    let exists = path.exists();
    let current = if exists { std::fs::read(path).ok().map(|b| etag(&b)) } else { None };

    if header_has(headers, IF_NONE_MATCH, "*") && exists {
        return Err(precondition_failed("item already exists"));
    }
    if let Some(want) = if_match(headers) {
        match &current {
            Some(cur) if etags_equal(cur, &want) => {}
            _ => return Err(precondition_failed("etag mismatch")),
        }
    }

    atomic_write(path, &body)?;
    ctx.reload()?;

    let tag = etag(body.as_bytes());
    let status = if exists { StatusCode::OK } else { StatusCode::CREATED };
    Ok((status, [(ETAG, tag)], Json(json!({ "ok": true }))).into_response())
}

/// Delete an item, honoring `If-Match`, then reload the context.
async fn remove_item(st: &AppState, path: &Path, headers: &HeaderMap) -> Result<Response, ApiError> {
    let mut ctx = st.write().await;
    if !path.exists() {
        return Err(not_found("no such item"));
    }
    if let Some(want) = if_match(headers) {
        let cur = std::fs::read(path).ok().map(|b| etag(&b));
        match cur {
            Some(cur) if etags_equal(&cur, &want) => {}
            _ => return Err(precondition_failed("etag mismatch")),
        }
    }
    std::fs::remove_file(path)?;
    ctx.reload()?;
    Ok(Json(json!({ "ok": true })).into_response())
}

// ---- path resolution (with traversal guards) --------------------------------------

fn safe_component(name: &str) -> bool {
    !name.is_empty() && !name.contains('/') && !name.contains('\\') && !name.contains("..") && !name.starts_with('.')
}

fn task_path(st: &AppState, file: &str) -> Result<PathBuf, ApiError> {
    if !safe_component(file) || !file.ends_with(".md") {
        return Err(bad_request("invalid task href"));
    }
    Ok(mgmt_store::tasks_dir(st.root()).join(file))
}

fn collection_dir(st: &AppState, coll: &str) -> Result<PathBuf, ApiError> {
    if !safe_component(coll) {
        return Err(bad_request("invalid collection"));
    }
    Ok(mgmt_store::calendars_dir(st.root()).join(coll))
}

fn event_path(st: &AppState, coll: &str, file: &str) -> Result<PathBuf, ApiError> {
    if !safe_component(file) || !file.ends_with(".ics") {
        return Err(bad_request("invalid event href"));
    }
    Ok(collection_dir(st, coll)?.join(file))
}

// ---- etag / header helpers --------------------------------------------------------

/// Quoted hex SHA-256 of the bytes (an HTTP strong ETag).
fn etag(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut hex = String::with_capacity(digest.len() * 2);
    for b in digest {
        hex.push_str(&format!("{b:02x}"));
    }
    format!("\"{hex}\"")
}

fn etags_equal(a: &str, b: &str) -> bool {
    a.trim_matches('"') == b.trim_matches('"')
}

fn if_match(headers: &HeaderMap) -> Option<String> {
    headers.get(IF_MATCH).and_then(|v| v.to_str().ok()).map(|s| s.to_string())
}

fn header_has(headers: &HeaderMap, name: axum::http::HeaderName, value: &str) -> bool {
    headers.get(name).and_then(|v| v.to_str().ok()).map(|v| v.trim() == value).unwrap_or(false)
}

fn precondition_failed(msg: &str) -> ApiError {
    ApiError::new(StatusCode::PRECONDITION_FAILED, msg)
}

impl From<std::io::Error> for ApiError {
    fn from(e: std::io::Error) -> Self {
        ApiError::from(Error::Io(e))
    }
}
