//! API handlers and the guarded API router: reads, mutations, auth, and focus control — every one
//! a thin wrapper over the shared [`MgmtContext`] (or the pomodoro session / auth state).

use std::collections::HashMap;
use std::net::SocketAddr;

use axum::extract::{ConnectInfo, Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::sse::{Event as SseEvent, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Extension, Json, Router};
use chrono::{Duration, Utc};
use serde::Deserialize;
use serde_json::{json, Value};

use mgmt_core::Uid;
use mgmt_domain::{Event, Filter, Task};
use mgmt_service::{pomodoro_path, wire_payload, MgmtContext, PomodoroState};

use crate::auth_backend::{AuthSession, Credentials};
use crate::dto::{filter_from_query, parse_rfc3339, project_selected, projects_from_query, sort_from_query, NO_PROJECT};
use crate::error::{bad_request, not_found, ApiError};
use crate::meta::meta_json;
use crate::middleware::{client_ip, Principal};
use crate::state::AppState;

/// How far ahead `/api/status` looks for the "next event".
const NEXT_EVENT_HORIZON_HOURS: i64 = 168;

/// The API sub-router (mounted under `/api`), with the auth/security middleware applied.
pub fn api_router(state: AppState) -> Router {
    Router::new()
        // auth
        .route("/auth/login", post(login))
        .route("/auth/logout", post(logout))
        .route("/auth/session", get(session))
        .route("/auth/setup", post(setup))
        .route("/auth/invite", get(invite_info))
        .route("/auth/invite/accept", post(accept_invite))
        .route("/auth/password", post(change_password))
        // reads
        .route("/health", get(health))
        .route("/meta", get(get_meta))
        .route("/stream", get(stream_changes))
        .route("/tasks", get(list_tasks).post(create_task))
        .route("/tasks/:uid", get(get_task).put(update_task).delete(delete_task))
        .route("/tasks/:uid/status", post(set_status))
        .route("/tasks/:uid/toggle", post(toggle_task))
        .route("/tasks/:uid/priority", post(cycle_priority))
        .route("/tasks/:uid/project", post(set_project))
        .route("/board", get(get_board))
        .route("/agenda", get(get_agenda))
        .route("/events", get(list_events).post(create_event))
        .route("/events/:uid", get(get_event).put(update_event).delete(delete_event))
        .route("/calendars", get(list_calendars))
        .route("/projects", get(list_projects).post(create_project))
        .route(
            "/projects/:name",
            axum::routing::put(update_project).delete(delete_project),
        )
        .route("/status", get(get_status))
        .route("/state", get(get_state))
        .route("/settings", get(get_settings).put(put_settings))
        .route("/focus/:action", post(focus))
        .route("/undo", post(undo))
        .route("/redo", post(redo))
        .route("/reload", post(reload))
        // trash
        .route("/trash", get(get_trash))
        .route("/trash/restore", post(trash_restore))
        .route("/trash/purge", post(trash_purge))
        .route("/trash/empty", post(trash_empty))
        // admin-only user management (each handler enforces the admin principal)
        .route("/admin/users", get(crate::admin::list_users).post(crate::admin::create_user))
        .route("/admin/users/:id", axum::routing::delete(crate::admin::delete_user))
        .route("/admin/users/:id/tokens", post(crate::admin::mint_token))
        .route("/admin/users/:id/tokens/:name", axum::routing::delete(crate::admin::revoke_token))
        .route("/admin/users/:id/pair-url", post(crate::admin::pair_url))
        .route("/admin/users/:id/invite", post(crate::admin::invite_user))
        // admin-only CalDAV config (web-managed accounts/collections + discovery)
        .route("/config/caldav", get(crate::admin::caldav_config))
        .route("/config/caldav/discover", post(crate::admin::caldav_discover))
        .route("/config/caldav/accounts", post(crate::admin::caldav_save_account))
        .route("/config/caldav/accounts/:name", axum::routing::delete(crate::admin::caldav_delete_account))
        // Google "Connect" OAuth flow (browser redirect; the admin session cookie rides the callback)
        .route("/config/google-oauth", get(crate::admin::google_oauth_status).put(crate::admin::google_oauth_set))
        .route("/config/google-oauth/connect", get(crate::admin::google_connect))
        .route("/oauth/google/callback", get(crate::admin::google_callback))
        .layer(axum::middleware::from_fn_with_state(state.clone(), crate::middleware::guard))
        .with_state(state)
}

// ---- auth -------------------------------------------------------------------------

#[derive(Deserialize)]
struct LoginBody {
    #[serde(default)]
    email: Option<String>,
    password: String,
    #[serde(default)]
    totp: Option<String>,
}

async fn login(
    State(st): State<AppState>,
    mut auth_session: AuthSession,
    peer: Option<ConnectInfo<SocketAddr>>,
    headers: HeaderMap,
    Json(body): Json<LoginBody>,
) -> Response {
    let creds = st.creds();
    if creds.is_open().await {
        return Json(json!({ "ok": true, "note": "authentication is disabled" })).into_response();
    }
    let ip = client_ip(peer.map(|p| p.0), &headers);
    if let Some(secs) = creds.locked_secs(ip, Utc::now()).await {
        return (
            StatusCode::TOO_MANY_REQUESTS,
            Json(json!({ "error": format!("too many attempts, locked for {secs}s") })),
        )
            .into_response();
    }
    let credentials = Credentials { email: body.email, password: body.password, totp: body.totp, ip };
    match auth_session.authenticate(credentials).await {
        Ok(Some(user)) => {
            if auth_session.login(&user).await.is_err() {
                return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": "session error" }))).into_response();
            }
            Json(json!({ "ok": true })).into_response()
        }
        Ok(None) => (StatusCode::UNAUTHORIZED, Json(json!({ "error": "invalid credentials" }))).into_response(),
        Err(_) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": "auth error" }))).into_response(),
    }
}

async fn logout(mut auth_session: AuthSession) -> Response {
    let _ = auth_session.logout().await;
    Json(json!({ "ok": true })).into_response()
}

async fn session(State(st): State<AppState>, auth_session: AuthSession) -> Json<Value> {
    let creds = st.creds();
    let enabled = creds.enabled().await;
    let needs_setup = creds.needs_setup().await;
    // In setup mode the client should show the create-admin screen, not treat itself as signed in.
    let authed = !needs_setup && (creds.is_open().await || auth_session.user.is_some());
    Json(json!({
        "enabled": enabled,
        "authenticated": authed,
        "needs_setup": needs_setup,
        "totp": creds.totp_enrolled().await,
    }))
}

#[derive(Deserialize)]
struct SetupBody {
    password: String,
    /// Optionally enroll a TOTP secret at the same time (base32).
    #[serde(default)]
    totp_secret: Option<String>,
}

/// First-run admin creation. Allowed only while no admin exists; on success the caller is logged in.
async fn setup(State(st): State<AppState>, mut auth_session: AuthSession, Json(body): Json<SetupBody>) -> Response {
    let creds = st.creds();
    if creds.enabled().await {
        return (StatusCode::CONFLICT, Json(json!({ "error": "admin already configured" }))).into_response();
    }
    if body.password.len() < 8 {
        return (StatusCode::BAD_REQUEST, Json(json!({ "error": "password too short (min 8)" }))).into_response();
    }
    if let Err(e) = creds.set_admin_password(&body.password, body.totp_secret.as_deref()).await {
        // A concurrent setup may have won the claim — report it as the same 409, not a 500.
        let already = e.to_string().contains("already configured");
        let code = if already { StatusCode::CONFLICT } else { StatusCode::INTERNAL_SERVER_ERROR };
        return (code, Json(json!({ "error": e.to_string() }))).into_response();
    }
    // Log the freshly-created admin straight in.
    let user = auth_session.backend.admin_user().await;
    if auth_session.login(&user).await.is_err() {
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": "session error" }))).into_response();
    }
    Json(json!({ "ok": true })).into_response()
}

#[derive(Deserialize)]
struct InviteBody {
    token: String,
    password: String,
}

/// `POST /api/auth/invite/accept` — consume a one-time invite token and set the user's password,
/// logging them in. Public (the invitee is not yet authenticated).
async fn accept_invite(State(st): State<AppState>, mut auth_session: AuthSession, Json(body): Json<InviteBody>) -> Response {
    if body.password.len() < 8 {
        return (StatusCode::BAD_REQUEST, Json(json!({ "error": "password too short (min 8)" }))).into_response();
    }
    let id = match st.creds().accept_invite(&body.token, &body.password).await {
        Ok(id) => id,
        Err(e) => return (StatusCode::NOT_FOUND, Json(json!({ "error": e.to_string() }))).into_response(),
    };
    let user = auth_session.backend.session_user(&id).await;
    if auth_session.login(&user).await.is_err() {
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": "session error" }))).into_response();
    }
    Json(json!({ "ok": true, "id": id })).into_response()
}

#[derive(Deserialize)]
struct ChangePasswordBody {
    current_password: String,
    new_password: String,
    #[serde(default)]
    totp: Option<String>,
}

/// `POST /api/auth/password` — rotate the logged-in user's password after re-verifying the
/// current credentials. Other sessions are invalidated (their auth hash no longer matches);
/// the current session is re-logged-in so the caller stays signed in.
async fn change_password(State(st): State<AppState>, mut auth_session: AuthSession, Json(body): Json<ChangePasswordBody>) -> Response {
    let Some(user) = auth_session.user.clone() else {
        // Requires a session login — bearer tokens are sync-scoped and can't rotate passwords.
        return (StatusCode::UNAUTHORIZED, Json(json!({ "error": "unauthorized" }))).into_response();
    };
    let creds = st.creds();
    if !creds.verify_user_credentials(&user.id, &body.current_password, body.totp.as_deref(), Utc::now()).await {
        return (StatusCode::FORBIDDEN, Json(json!({ "error": "current credentials are wrong" }))).into_response();
    }
    if body.new_password.len() < 8 {
        return (StatusCode::BAD_REQUEST, Json(json!({ "error": "password too short (min 8)" }))).into_response();
    }
    if let Err(e) = creds.change_password(&user.id, &body.new_password).await {
        return ApiError::from(e).into_response();
    }
    // Keep this session alive under the new auth hash.
    let refreshed = auth_session.backend.session_user(&user.id).await;
    if auth_session.login(&refreshed).await.is_err() {
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": "session error" }))).into_response();
    }
    Json(json!({ "ok": true })).into_response()
}

/// `GET /api/auth/invite?token=` — validate an invite token without consuming it, returning who
/// it's for (so the invite page can greet the user). Public.
async fn invite_info(State(st): State<AppState>, Query(q): Query<HashMap<String, String>>) -> Result<Json<Value>, ApiError> {
    let token = q.get("token").filter(|s| !s.is_empty()).ok_or_else(|| bad_request("token is required"))?;
    match st.creds().invite_owner(token).await {
        Some(u) => Ok(Json(json!({ "valid": true, "id": u.id, "email": u.email, "name": u.name }))),
        None => Ok(Json(json!({ "valid": false }))),
    }
}

// ---- reads ------------------------------------------------------------------------

async fn health() -> Json<Value> {
    Json(json!({ "ok": true }))
}

/// Server-sent events: one `changed` ping per vault mutation, so an open PWA refetches without
/// waiting for the user to refocus the tab. Scoped to the requesting principal's own vault.
async fn stream_changes(
    State(st): State<AppState>,
    Extension(p): Extension<Principal>,
) -> Result<Sse<impl tokio_stream::Stream<Item = Result<SseEvent, std::convert::Infallible>>>, ApiError> {
    use tokio_stream::StreamExt;
    let uc = st.user_ctx(&p.0)?;
    // A `Lagged` error only means pings were dropped; "something changed" is idempotent, so it
    // maps to the same event as a delivered ping.
    let changed = tokio_stream::wrappers::BroadcastStream::new(uc.subscribe())
        .map(|_| Ok(SseEvent::default().event("changed").data("1")));
    // An immediate `ready` flushes the response headers, so clients (and proxies) see the stream
    // open right away instead of waiting for the first change or keep-alive tick.
    let ready = tokio_stream::iter([Ok(SseEvent::default().event("ready").data("1"))]);
    let stream = ready.chain(changed);
    // Comment pings keep idle proxies from cutting the connection.
    Ok(Sse::new(stream).keep_alive(KeepAlive::new().interval(std::time::Duration::from_secs(25))))
}

async fn get_meta(State(st): State<AppState>) -> Json<Value> {
    let ctx = st.read().await;
    Json(meta_json(&ctx, st.root()))
}

async fn list_tasks(State(st): State<AppState>, Query(q): Query<HashMap<String, String>>) -> Json<Vec<Task>> {
    let ctx = st.read().await;
    let filter = filter_from_query(&ctx, &q);
    Json(ctx.filtered_tasks(&filter, sort_from_query(&q)))
}

async fn get_task(State(st): State<AppState>, Path(uid): Path<String>) -> Result<Json<Task>, ApiError> {
    let ctx = st.read().await;
    ctx.task(&Uid::from(uid.as_str()))
        .cloned()
        .map(Json)
        .ok_or_else(|| not_found(format!("task {uid}")))
}

async fn get_board(State(st): State<AppState>, Query(q): Query<HashMap<String, String>>) -> Json<Value> {
    let ctx = st.read().await;
    let filter = Filter {
        project: q.get("project").filter(|s| !s.is_empty()).cloned(),
        projects: projects_from_query(&q),
        ..Default::default()
    };
    let columns: Vec<Value> = ctx
        .board(&filter)
        .into_iter()
        .map(|(status, tasks)| {
            let label = ctx.status_label(&status).to_string();
            json!({ "status": status, "label": label, "tasks": tasks })
        })
        .collect();
    Json(json!(columns))
}

async fn list_events(State(st): State<AppState>, Query(q): Query<HashMap<String, String>>) -> Result<Json<Vec<Event>>, ApiError> {
    let ctx = st.read().await;
    let from = parse_rfc3339(q.get("from")).ok_or_else(|| bad_request("'from' is required (RFC 3339)"))?;
    let to = parse_rfc3339(q.get("to")).ok_or_else(|| bad_request("'to' is required (RFC 3339)"))?;
    Ok(Json(ctx.events_in_range(from, to)))
}

async fn get_event(State(st): State<AppState>, Path(uid): Path<String>) -> Result<Json<Event>, ApiError> {
    let ctx = st.read().await;
    ctx.event(&Uid::from(uid.as_str()))
        .cloned()
        .map(Json)
        .ok_or_else(|| not_found(format!("event {uid}")))
}

/// One round-trip for a calendar range: recurrence-expanded events plus the tasks whose
/// scheduled/due date falls in `[from, to)` (the calendar's task overlay).
async fn get_agenda(State(st): State<AppState>, Query(q): Query<HashMap<String, String>>) -> Result<Json<Value>, ApiError> {
    let ctx = st.read().await;
    let from = parse_rfc3339(q.get("from")).ok_or_else(|| bad_request("'from' is required (RFC 3339)"))?;
    let to = parse_rfc3339(q.get("to")).ok_or_else(|| bad_request("'to' is required (RFC 3339)"))?;
    let selected = projects_from_query(&q);
    let events: Vec<Event> = ctx
        .events_in_range(from, to)
        .into_iter()
        .filter(|e| project_selected(selected.as_ref(), e.project.as_deref()))
        .collect();
    let tasks: Vec<Task> = ctx
        .tasks()
        .iter()
        .filter(|t| t.calendar_date().map(|d| d >= from && d < to).unwrap_or(false))
        .filter(|t| project_selected(selected.as_ref(), t.project.as_deref()))
        .cloned()
        .collect();
    Ok(Json(json!({ "events": events, "tasks": tasks })))
}

async fn list_projects(State(st): State<AppState>) -> Json<Value> {
    let ctx = st.read().await;
    let projects: Vec<Value> = ctx
        .projects()
        .into_iter()
        .map(|name| {
            let color = ctx.project_color(&name);
            json!({ "name": name, "color": color })
        })
        .collect();
    Json(json!(projects))
}

async fn get_status(State(st): State<AppState>) -> Json<Value> {
    let now = Utc::now();
    let pomo = PomodoroState::load(&pomodoro_path(st.root()));
    let ctx = st.read().await;
    let next = ctx.next_event(now, Duration::hours(NEXT_EVENT_HORIZON_HOURS));
    Json(wire_payload(pomo.as_ref(), next.as_ref(), now))
}

/// UI state flags: unsaved-for-sync dot + whether undo/redo are available.
async fn get_state(State(st): State<AppState>) -> Json<Value> {
    let ctx = st.read().await;
    Json(json!({ "dirty": ctx.is_dirty(), "can_undo": ctx.can_undo(), "can_redo": ctx.can_redo() }))
}

fn settings_path(st: &AppState) -> std::path::PathBuf {
    st.root().join(".state").join("web-settings.json")
}

/// Persisted web/UI preferences (theme, time format, keybindings, …). Stored as an opaque JSON
/// blob under the data root so the same settings follow the vault across machines.
async fn get_settings(State(st): State<AppState>) -> Json<Value> {
    let v = std::fs::read_to_string(settings_path(&st))
        .ok()
        .and_then(|s| serde_json::from_str::<Value>(&s).ok())
        .unwrap_or_else(|| json!({}));
    Json(v)
}

async fn put_settings(State(st): State<AppState>, Json(body): Json<Value>) -> Result<Json<Value>, ApiError> {
    let path = settings_path(&st);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let text = serde_json::to_string_pretty(&body).map_err(|e| bad_request(format!("invalid settings: {e}")))?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, &path)?;
    Ok(Json(body))
}

// ---- task mutations ---------------------------------------------------------------

#[derive(Deserialize)]
struct NewTask {
    title: String,
    #[serde(default)]
    project: Option<String>,
}

async fn create_task(State(st): State<AppState>, Json(body): Json<NewTask>) -> Result<Json<Task>, ApiError> {
    if body.title.trim().is_empty() {
        return Err(bad_request("title is empty"));
    }
    let mut ctx = st.write().await;
    let uid = ctx.quick_add(body.title, body.project)?;
    let task = ctx.task(&uid).cloned().ok_or_else(|| not_found("just-created task"))?;
    Ok(Json(task))
}

async fn update_task(State(st): State<AppState>, Path(uid): Path<String>, Json(mut task): Json<Task>) -> Result<Json<Task>, ApiError> {
    task.uid = Uid::from(uid.as_str()); // the path is authoritative
    let mut ctx = st.write().await;
    ctx.put_task(task.clone())?;
    Ok(Json(task))
}

async fn delete_task(State(st): State<AppState>, Path(uid): Path<String>) -> Result<Json<Value>, ApiError> {
    let mut ctx = st.write().await;
    ctx.delete_task(&Uid::from(uid.as_str()))?;
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
struct StatusBody {
    status: String,
}

async fn set_status(State(st): State<AppState>, Path(uid): Path<String>, Json(body): Json<StatusBody>) -> Result<Json<Task>, ApiError> {
    let mut ctx = st.write().await;
    let uid = Uid::from(uid.as_str());
    ctx.set_task_status(&uid, body.status)?;
    Ok(Json(ctx.task(&uid).cloned().ok_or_else(|| not_found(format!("task {uid}")))?))
}

async fn toggle_task(State(st): State<AppState>, Path(uid): Path<String>) -> Result<Json<Value>, ApiError> {
    let mut ctx = st.write().await;
    let status = ctx.toggle_task_done(&Uid::from(uid.as_str()))?;
    Ok(Json(json!({ "status": status })))
}

async fn cycle_priority(State(st): State<AppState>, Path(uid): Path<String>) -> Result<Json<Task>, ApiError> {
    let mut ctx = st.write().await;
    let uid = Uid::from(uid.as_str());
    ctx.cycle_task_priority(&uid)?;
    Ok(Json(ctx.task(&uid).cloned().ok_or_else(|| not_found(format!("task {uid}")))?))
}

#[derive(Deserialize)]
struct ProjectBody {
    #[serde(default)]
    project: Option<String>,
}

async fn set_project(State(st): State<AppState>, Path(uid): Path<String>, Json(body): Json<ProjectBody>) -> Result<Json<Task>, ApiError> {
    let mut ctx = st.write().await;
    let uid = Uid::from(uid.as_str());
    ctx.set_task_project(&uid, body.project.filter(|s| !s.is_empty()))?;
    Ok(Json(ctx.task(&uid).cloned().ok_or_else(|| not_found(format!("task {uid}")))?))
}

// ---- event mutations --------------------------------------------------------------

async fn create_event(State(st): State<AppState>, Json(mut event): Json<Event>) -> Result<Json<Event>, ApiError> {
    // A web client posts an empty uid for a new event — assign a fresh one (else events collide on
    // the same `<uid>.ics` filename and overwrite each other).
    if event.uid.as_str().is_empty() {
        event.uid = Uid::new();
    }
    if event.calendar.trim().is_empty() {
        return Err(bad_request("event needs a calendar"));
    }
    let mut ctx = st.write().await;
    ctx.put_event(event.clone())?;
    Ok(Json(event))
}

async fn update_event(State(st): State<AppState>, Path(uid): Path<String>, Json(mut event): Json<Event>) -> Result<Json<Event>, ApiError> {
    event.uid = Uid::from(uid.as_str());
    let mut ctx = st.write().await;
    ctx.put_event(event.clone())?;
    Ok(Json(event))
}

async fn delete_event(State(st): State<AppState>, Path(uid): Path<String>) -> Result<Json<Value>, ApiError> {
    let mut ctx = st.write().await;
    ctx.delete_event(&Uid::from(uid.as_str()))?;
    Ok(Json(json!({ "ok": true })))
}

// ---- project mutations ------------------------------------------------------------

#[derive(Deserialize)]
struct NewProject {
    name: String,
}

/// `none` names the "no project" bucket in query strings, so it can never be a real project.
fn reject_reserved(name: &str) -> Result<(), ApiError> {
    if name.trim().eq_ignore_ascii_case(NO_PROJECT) {
        return Err(bad_request("'none' is reserved (it names the no-project bucket)"));
    }
    Ok(())
}

async fn create_project(State(st): State<AppState>, Json(body): Json<NewProject>) -> Result<Json<Value>, ApiError> {
    reject_reserved(&body.name)?;
    let mut ctx = st.write().await;
    ctx.add_project(body.name.clone())?;
    Ok(Json(json!({ "name": body.name, "color": ctx.project_color(&body.name) })))
}

async fn delete_project(State(st): State<AppState>, Path(name): Path<String>) -> Result<Json<Value>, ApiError> {
    let mut ctx = st.write().await;
    ctx.delete_project(&name)?;
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
struct ProjectEdit {
    #[serde(default)]
    color: Option<String>,
    #[serde(default)]
    description: Option<String>,
}

/// Set a project's color and/or description (creating it if absent), the only way to color a project.
async fn update_project(State(st): State<AppState>, Path(name): Path<String>, Json(body): Json<ProjectEdit>) -> Result<Json<Value>, ApiError> {
    reject_reserved(&name)?;
    let mut ctx = st.write().await;
    // Preserve unspecified fields from any existing project.
    let mut project = ctx.project(&name).cloned().unwrap_or_else(|| mgmt_domain::Project::new(&name));
    if let Some(c) = body.color {
        project.color = if c.trim().is_empty() { None } else { Some(c) };
    }
    if let Some(d) = body.description {
        project.description = d;
    }
    ctx.put_project(project)?;
    Ok(Json(json!({ "name": name, "color": ctx.project_color(&name) })))
}

// ---- trash ------------------------------------------------------------------------

async fn get_trash(State(st): State<AppState>) -> Json<Value> {
    let ctx = st.read().await;
    Json(json!({
        "tasks": ctx.trashed_tasks(),
        "projects": ctx.trashed_projects(),
        "empty": ctx.trash_is_empty(),
    }))
}

#[derive(Deserialize)]
struct TrashRef {
    /// `"task"` or `"project"`.
    kind: String,
    /// Task uid or project name.
    id: String,
}

async fn trash_restore(State(st): State<AppState>, Json(body): Json<TrashRef>) -> Result<Json<Value>, ApiError> {
    let mut ctx = st.write().await;
    let did = match body.kind.as_str() {
        "task" => ctx.restore_task(&Uid::from(body.id.as_str()))?,
        "project" => ctx.restore_project(&body.id)?,
        other => return Err(bad_request(format!("unknown trash kind '{other}'"))),
    };
    Ok(Json(json!({ "restored": did })))
}

async fn trash_purge(State(st): State<AppState>, Json(body): Json<TrashRef>) -> Result<Json<Value>, ApiError> {
    let mut ctx = st.write().await;
    let did = match body.kind.as_str() {
        "task" => ctx.purge_trashed_task(&Uid::from(body.id.as_str()))?,
        "project" => ctx.purge_trashed_project(&body.id)?,
        other => return Err(bad_request(format!("unknown trash kind '{other}'"))),
    };
    Ok(Json(json!({ "purged": did })))
}

async fn trash_empty(State(st): State<AppState>) -> Result<Json<Value>, ApiError> {
    let mut ctx = st.write().await;
    ctx.empty_trash()?;
    Ok(Json(json!({ "ok": true })))
}

// ---- focus + undo/redo + reload ---------------------------------------------------

async fn focus(State(st): State<AppState>, Path(action): Path<String>, Query(q): Query<HashMap<String, String>>) -> Result<Json<Value>, ApiError> {
    let path = pomodoro_path(st.root());
    let now = Utc::now();
    match action.as_str() {
        "start" => {
            let state = if q.get("engine").map(|e| e == "flowtime").unwrap_or(false) {
                PomodoroState::start_flowtime(now)
            } else {
                PomodoroState::start_pomodoro(now)
            };
            state.save(&path).map_err(mgmt_core::Error::Io)?;
        }
        "toggle" => {
            if let Some(mut s) = PomodoroState::load(&path) {
                s.toggle(now);
                s.save(&path).map_err(mgmt_core::Error::Io)?;
            }
        }
        "skip" => {
            if let Some(mut s) = PomodoroState::load(&path) {
                s.skip(now);
                s.save(&path).map_err(mgmt_core::Error::Io)?;
            }
        }
        "stop" => {
            let _ = std::fs::remove_file(&path);
        }
        other => return Err(bad_request(format!("unknown focus action '{other}'"))),
    }
    let pomo = PomodoroState::load(&path);
    Ok(Json(wire_payload(pomo.as_ref(), None, now)))
}

async fn undo(State(st): State<AppState>) -> Result<Json<Value>, ApiError> {
    let mut ctx = st.write().await;
    let did = ctx.undo()?;
    Ok(Json(json!({ "undone": did })))
}

async fn redo(State(st): State<AppState>) -> Result<Json<Value>, ApiError> {
    let mut ctx = st.write().await;
    let did = ctx.redo()?;
    Ok(Json(json!({ "redone": did })))
}

async fn reload(State(st): State<AppState>) -> Result<Json<Value>, ApiError> {
    let mut ctx: tokio::sync::RwLockWriteGuard<'_, MgmtContext> = st.write().await;
    ctx.reload()?;
    Ok(Json(json!({ "ok": true })))
}

/// Read-only calendar list for the event form's picker and the sidebar toggles: every collection
/// directory on disk, plus the calendars the loaded events name (and `default`, always offered).
async fn list_calendars(State(st): State<AppState>) -> Json<Value> {
    let mut names: std::collections::BTreeSet<String> = mgmt_store::VdirStore::new(mgmt_store::calendars_dir(st.root()))
        .collections()
        .unwrap_or_default()
        .into_iter()
        .collect();
    names.insert("default".into());
    let ctx = st.read().await;
    names.extend(ctx.events().iter().map(|e| e.calendar.clone()));
    Json(json!(names.iter().map(|n| json!({ "name": n })).collect::<Vec<_>>()))
}
