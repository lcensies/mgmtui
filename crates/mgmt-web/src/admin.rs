//! Admin-only user management: create/list/delete users, mint & revoke their scoped sync tokens,
//! and generate the `mgmt://pair/...` export URL another node imports to clone + keep syncing.
//!
//! Every handler requires the request principal to be the admin (a logged-in web session, or an
//! admin-owned bearer token). A regular user's sync token resolves to *their* id, so it can never
//! reach these routes.

use std::collections::HashMap;

use axum::extract::{Extension, Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Redirect, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

use mgmt_config::{Account, CalDavFile, Collection};
use mgmt_dav::Auth;

use crate::auth::{new_api_token, GoogleOAuth, TokenEntry, WebUser};
use crate::error::{bad_request, not_found, ApiError};
use crate::middleware::Principal;
use crate::state::{is_safe_user_id, AppState};

/// Reject the request unless the principal is the admin.
fn require_admin(p: &Principal) -> Result<(), ApiError> {
    if p.0 == mgmt_store::ADMIN_USER {
        Ok(())
    } else {
        Err(ApiError::new(StatusCode::FORBIDDEN, "admin only"))
    }
}

/// `GET /api/admin/users` — list managed users (never exposes token hashes).
pub async fn list_users(State(st): State<AppState>, Extension(p): Extension<Principal>) -> Result<Json<Value>, ApiError> {
    require_admin(&p)?;
    let file = st.creds().snapshot();
    let users: Vec<Value> = file
        .users
        .iter()
        .map(|u| json!({ "id": u.id, "name": u.name, "email": u.email, "pending_invite": u.invite_token.is_some(), "tokens": u.tokens.iter().map(|t| &t.name).collect::<Vec<_>>() }))
        .collect();
    Ok(Json(json!({ "users": users })))
}

#[derive(Deserialize)]
pub struct NewUser {
    id: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    email: Option<String>,
}

/// `POST /api/admin/users` — create a user + its isolated vault directory. An optional `email`
/// becomes the login identifier (the user sets a password by accepting an invite; until then they
/// are reachable over `/api/sync` via a scoped bearer token).
pub async fn create_user(State(st): State<AppState>, Extension(p): Extension<Principal>, Json(body): Json<NewUser>) -> Result<Response, ApiError> {
    require_admin(&p)?;
    if body.id == mgmt_store::ADMIN_USER || !is_safe_user_id(&body.id) {
        return Err(bad_request("invalid user id (use a-z, 0-9, - or _)"));
    }
    let snap = st.creds().snapshot();
    if snap.users.iter().any(|u| u.id == body.id) {
        return Err(ApiError::new(StatusCode::CONFLICT, "user already exists"));
    }
    // Normalize + validate the email (login identifier), and ensure it's unique.
    let email = body.email.as_deref().map(|e| e.trim().to_lowercase()).filter(|e| !e.is_empty());
    if let Some(e) = &email {
        if !e.contains('@') || !e.contains('.') {
            return Err(bad_request("email looks invalid"));
        }
        if snap.users.iter().any(|u| u.email.as_deref() == Some(e.as_str())) {
            return Err(ApiError::new(StatusCode::CONFLICT, "email already in use"));
        }
    }
    // Establish the isolated vault so the user is immediately syncable.
    let root = st.users_base().join(&body.id);
    for dir in [mgmt_store::tasks_dir(&root), mgmt_store::calendars_dir(&root), mgmt_store::projects_dir(&root)] {
        std::fs::create_dir_all(dir).map_err(|e| ApiError::from(mgmt_core::Error::Io(e)))?;
    }
    let name = if body.name.is_empty() { body.id.clone() } else { body.name.clone() };
    st.creds().mutate_file(|f| f.users.push(WebUser {
        id: body.id.clone(),
        name: name.clone(),
        email: email.clone(),
        password_hash: None,
        totp_secret: None,
        invite_token: None,
        is_admin: false,
        tokens: Vec::new(),
    }))?;
    Ok((StatusCode::CREATED, Json(json!({ "id": body.id, "name": name, "email": email }))).into_response())
}

/// `DELETE /api/admin/users/:id` — remove a user (their vault files are left on disk to avoid
/// accidental data loss; the admin can delete `users/<id>` manually).
pub async fn delete_user(State(st): State<AppState>, Extension(p): Extension<Principal>, Path(id): Path<String>) -> Result<Json<Value>, ApiError> {
    require_admin(&p)?;
    let removed = st.creds().mutate_file(|f| {
        let before = f.users.len();
        f.users.retain(|u| u.id != id);
        before != f.users.len()
    })?;
    if !removed {
        return Err(not_found("no such user"));
    }
    st.forget_user(&id);
    Ok(Json(json!({ "ok": true, "note": "vault files left on disk" })))
}

#[derive(Deserialize)]
pub struct NewToken {
    #[serde(default)]
    name: String,
}

/// `POST /api/admin/users/:id/tokens` — mint a scoped sync token (returned once, in the clear).
pub async fn mint_token(State(st): State<AppState>, Extension(p): Extension<Principal>, Path(id): Path<String>, body: Option<Json<NewToken>>) -> Result<Json<Value>, ApiError> {
    require_admin(&p)?;
    let label = body.map(|b| b.0.name).unwrap_or_default();
    let label = if label.is_empty() { "sync".to_string() } else { label };
    let (raw, hash) = new_api_token();
    let ok = st.creds().mutate_file(|f| match f.users.iter_mut().find(|u| u.id == id) {
        Some(u) => {
            u.tokens.push(TokenEntry { name: label.clone(), hash });
            true
        }
        None => false,
    })?;
    if !ok {
        return Err(not_found("no such user"));
    }
    Ok(Json(json!({ "name": label, "token": raw })))
}

/// `DELETE /api/admin/users/:id/tokens/:name` — revoke a scoped token by label.
pub async fn revoke_token(State(st): State<AppState>, Extension(p): Extension<Principal>, Path((id, name)): Path<(String, String)>) -> Result<Json<Value>, ApiError> {
    require_admin(&p)?;
    let ok = st.creds().mutate_file(|f| match f.users.iter_mut().find(|u| u.id == id) {
        Some(u) => {
            let before = u.tokens.len();
            u.tokens.retain(|t| t.name != name);
            before != u.tokens.len()
        }
        None => false,
    })?;
    if !ok {
        return Err(not_found("no such user or token"));
    }
    Ok(Json(json!({ "ok": true })))
}

/// `POST /api/admin/users/:id/pair-url` — mint a token and return the `mgmt://pair/...` URL that
/// another node imports to clone the user's vault and keep syncing. The importer decodes the
/// base64url payload `{host, token, user}` and syncs against `<host>/api/sync`.
pub async fn pair_url(State(st): State<AppState>, Extension(p): Extension<Principal>, Path(id): Path<String>, body: Option<Json<NewToken>>) -> Result<Json<Value>, ApiError> {
    require_admin(&p)?;
    let host = st
        .public_origin()
        .ok_or_else(|| ApiError::new(StatusCode::BAD_REQUEST, "set web.public_origin to generate pair URLs"))?
        .to_string();
    let label = body.map(|b| b.0.name).unwrap_or_default();
    let label = if label.is_empty() { "pair".to_string() } else { label };
    let (raw, hash) = new_api_token();
    let ok = st.creds().mutate_file(|f| match f.users.iter_mut().find(|u| u.id == id) {
        Some(u) => {
            u.tokens.push(TokenEntry { name: label.clone(), hash });
            true
        }
        None => false,
    })?;
    if !ok {
        return Err(not_found("no such user"));
    }
    let payload = json!({ "host": host, "token": raw, "user": id });
    let blob = data_encoding::BASE64URL_NOPAD.encode(serde_json::to_vec(&payload).unwrap().as_slice());
    Ok(Json(json!({ "url": format!("mgmt://pair/{blob}"), "token": raw })))
}

/// `POST /api/admin/users/:id/invite` — mint (or re-mint) a one-time invite token the user redeems
/// at `/api/auth/invite/accept` to set their password. Returned once, in the clear; `url` is
/// included when `web.public_origin` is configured.
pub async fn invite_user(State(st): State<AppState>, Extension(p): Extension<Principal>, Path(id): Path<String>) -> Result<Json<Value>, ApiError> {
    require_admin(&p)?;
    let raw = st.creds().mint_invite(&id).map_err(|_| not_found("no such user"))?;
    let url = st.public_origin().map(|o| format!("{o}/invite?token={raw}"));
    Ok(Json(json!({ "token": raw, "url": url })))
}

// ---- CalDAV config (web-managed accounts/collections in caldav.yaml) ----------------

fn caldav_path(st: &AppState) -> Result<&std::path::Path, ApiError> {
    let p = st.caldav_file();
    if p.as_os_str().is_empty() {
        return Err(ApiError::new(StatusCode::NOT_IMPLEMENTED, "caldav config not available"));
    }
    Ok(p)
}

/// Build an `Auth` from an account block; missing/`none` → anonymous.
fn account_auth(auth: &str, username: Option<&str>, password: Option<&str>, token: Option<&str>) -> Auth {
    match auth {
        "bearer" => Auth::Bearer { token: token.unwrap_or_default().to_string() },
        "none" => Auth::None,
        _ => Auth::Basic {
            user: username.unwrap_or_default().to_string(),
            password: password.unwrap_or_default().to_string(),
        },
    }
}

/// `GET /api/config/caldav` — list accounts (secrets redacted) + collections.
pub async fn caldav_config(State(st): State<AppState>, Extension(p): Extension<Principal>) -> Result<Json<Value>, ApiError> {
    require_admin(&p)?;
    let file = CalDavFile::load(caldav_path(&st)?).map_err(ApiError::from)?;
    let accounts: Vec<Value> = file
        .accounts
        .iter()
        .map(|a| json!({
            "name": a.name, "auth": a.auth, "username": a.username,
            "has_password": a.password.is_some(), "has_token": a.token.is_some(),
        }))
        .collect();
    let collections: Vec<Value> = file
        .collections
        .iter()
        .map(|c| json!({ "name": c.name, "kind": c.kind, "url": c.url, "account": c.account, "protocol": c.protocol }))
        .collect();
    Ok(Json(json!({ "accounts": accounts, "collections": collections })))
}

#[derive(Deserialize)]
pub struct DiscoverBody {
    server_url: String,
    #[serde(default = "default_basic")]
    auth: String,
    #[serde(default)]
    username: Option<String>,
    #[serde(default)]
    password: Option<String>,
    #[serde(default)]
    token: Option<String>,
}

fn default_basic() -> String {
    "basic".into()
}

/// `POST /api/config/caldav/discover` — enumerate the server's calendars for the given credentials.
pub async fn caldav_discover(Extension(p): Extension<Principal>, Json(body): Json<DiscoverBody>) -> Result<Json<Value>, ApiError> {
    require_admin(&p)?;
    let auth = account_auth(&body.auth, body.username.as_deref(), body.password.as_deref(), body.token.as_deref());
    let url = body.server_url.trim().to_string();
    if url.is_empty() {
        return Err(bad_request("server_url is required"));
    }
    // mgmt-dav owns its own runtime + block_on, so run discovery off the async worker.
    let found = tokio::task::spawn_blocking(move || mgmt_dav::discover_calendars(url, auth))
        .await
        .map_err(|e| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, format!("discovery task failed: {e}")))?
        .map_err(ApiError::from)?;
    let calendars: Vec<Value> = found
        .into_iter()
        .map(|c| json!({ "name": c.name, "url": c.url, "supports_events": c.supports_events, "supports_tasks": c.supports_tasks }))
        .collect();
    Ok(Json(json!({ "calendars": calendars })))
}

#[derive(Deserialize)]
pub struct SaveAccountBody {
    account: AccountBody,
    #[serde(default)]
    collections: Vec<CollectionBody>,
}

#[derive(Deserialize)]
pub struct AccountBody {
    name: String,
    #[serde(default = "default_basic")]
    auth: String,
    #[serde(default)]
    username: Option<String>,
    #[serde(default)]
    password: Option<String>,
    #[serde(default)]
    token: Option<String>,
}

#[derive(Deserialize)]
pub struct CollectionBody {
    name: String,
    kind: String,
    url: String,
}

/// `POST /api/config/caldav/accounts` — add/update an account and its collections in caldav.yaml.
/// Secrets left empty on an update keep the previously-stored value.
pub async fn caldav_save_account(State(st): State<AppState>, Extension(p): Extension<Principal>, Json(body): Json<SaveAccountBody>) -> Result<Json<Value>, ApiError> {
    require_admin(&p)?;
    let path = caldav_path(&st)?.to_path_buf();
    let name = body.account.name.trim().to_string();
    if name.is_empty() {
        return Err(bad_request("account name is required"));
    }
    let mut file = CalDavFile::load(&path).map_err(ApiError::from)?;
    let prev = file.accounts.iter().find(|a| a.name == name).cloned();
    // Keep the existing secret when the client didn't supply a new one (edit-without-retyping).
    let keep = |new: Option<String>, old: Option<String>| new.filter(|s| !s.is_empty()).or(old);
    let account = Account {
        name: name.clone(),
        auth: body.account.auth,
        username: body.account.username.filter(|s| !s.is_empty()),
        password: keep(body.account.password, prev.as_ref().and_then(|a| a.password.clone())),
        token: keep(body.account.token, prev.and_then(|a| a.token)),
    };
    file.accounts.retain(|a| a.name != name);
    file.accounts.push(account);
    for c in body.collections {
        let coll = Collection {
            name: c.name.trim().to_string(),
            kind: c.kind,
            url: c.url.trim().to_string(),
            account: name.clone(),
            protocol: "caldav".into(),
        };
        file.collections.retain(|x| x.name != coll.name);
        file.collections.push(coll);
    }
    file.save(&path).map_err(ApiError::from)?;
    Ok(Json(json!({ "ok": true })))
}

/// `DELETE /api/config/caldav/accounts/:name` — remove an account and the collections using it.
pub async fn caldav_delete_account(State(st): State<AppState>, Extension(p): Extension<Principal>, Path(name): Path<String>) -> Result<Json<Value>, ApiError> {
    require_admin(&p)?;
    let path = caldav_path(&st)?.to_path_buf();
    let mut file = CalDavFile::load(&path).map_err(ApiError::from)?;
    let before = file.accounts.len();
    file.accounts.retain(|a| a.name != name);
    if file.accounts.len() == before {
        return Err(not_found("no such account"));
    }
    file.collections.retain(|c| c.account != name);
    file.save(&path).map_err(ApiError::from)?;
    Ok(Json(json!({ "ok": true })))
}

// ---- Google "Connect" OAuth flow ----------------------------------------------------

/// Directory holding per-account Google token stores (`<config>/google/`), shared with the CLI.
fn google_dir(st: &AppState) -> Result<std::path::PathBuf, ApiError> {
    Ok(caldav_path(st)?.parent().unwrap_or(std::path::Path::new(".")).join("google"))
}

/// `GET /api/config/google-oauth` — whether the Google OAuth client is configured (never leaks it).
pub async fn google_oauth_status(State(st): State<AppState>, Extension(p): Extension<Principal>) -> Result<Json<Value>, ApiError> {
    require_admin(&p)?;
    let configured = st.creds().snapshot().google_oauth.is_some();
    let redirect_uri = st.public_origin().map(|o| format!("{o}/api/oauth/google/callback"));
    Ok(Json(json!({ "configured": configured, "redirect_uri": redirect_uri })))
}

#[derive(Deserialize)]
pub struct GoogleOAuthBody {
    client_id: String,
    client_secret: String,
}

/// `PUT /api/config/google-oauth` — set the Google OAuth client (id/secret) used by the flow.
pub async fn google_oauth_set(State(st): State<AppState>, Extension(p): Extension<Principal>, Json(body): Json<GoogleOAuthBody>) -> Result<Json<Value>, ApiError> {
    require_admin(&p)?;
    if body.client_id.trim().is_empty() || body.client_secret.trim().is_empty() {
        return Err(bad_request("client_id and client_secret are required"));
    }
    st.creds()
        .mutate_file(|f| f.google_oauth = Some(GoogleOAuth { client_id: body.client_id.trim().into(), client_secret: body.client_secret.trim().into() }))?;
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
pub struct ConnectQuery {
    #[serde(default)]
    account: Option<String>,
}

/// `GET /api/config/google-oauth/connect?account=` — begin the flow: returns the Google consent URL
/// the browser should navigate to. Requires a configured OAuth client + `public_origin`.
pub async fn google_connect(State(st): State<AppState>, Extension(p): Extension<Principal>, Query(q): Query<ConnectQuery>) -> Result<Json<Value>, ApiError> {
    require_admin(&p)?;
    let oauth = st.creds().snapshot().google_oauth.ok_or_else(|| bad_request("set the Google OAuth client id/secret first"))?;
    let origin = st.public_origin().ok_or_else(|| bad_request("set web.public_origin to use the Connect flow"))?;
    let account = q.account.unwrap_or_else(|| "google".into());
    let redirect_uri = format!("{origin}/api/oauth/google/callback");
    let start = mgmt_google::begin_auth(&oauth.client_id, &oauth.client_secret, &redirect_uri).map_err(ApiError::from)?;
    st.oauth_begin(start.state, account, start.pkce_verifier);
    Ok(Json(json!({ "url": start.url })))
}

#[derive(Deserialize)]
pub struct CallbackQuery {
    #[serde(default)]
    code: Option<String>,
    #[serde(default)]
    state: Option<String>,
    #[serde(default)]
    error: Option<String>,
}

/// `GET /api/oauth/google/callback` — Google redirects the browser here after consent. Exchanges the
/// code, persists the token, and provisions the account + its calendars, then bounces to the app.
/// The admin's session cookie rides along (SameSite=Lax top-level GET), so the guard authorizes it.
pub async fn google_callback(State(st): State<AppState>, Extension(p): Extension<Principal>, Query(q): Query<CallbackQuery>) -> Response {
    if require_admin(&p).is_err() {
        return (StatusCode::FORBIDDEN, "admin only").into_response();
    }
    match google_callback_inner(&st, q).await {
        Ok(_) => Redirect::to("/?connected=google").into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, format!("Google connect failed: {}\nYou can close this tab.", e.1)).into_response(),
    }
}

async fn google_callback_inner(st: &AppState, q: CallbackQuery) -> Result<(), ApiError> {
    if let Some(err) = q.error {
        return Err(bad_request(format!("consent denied: {err}")));
    }
    let code = q.code.ok_or_else(|| bad_request("missing code"))?;
    let state = q.state.ok_or_else(|| bad_request("missing state"))?;
    let (account, verifier) = st.oauth_take(&state).ok_or_else(|| bad_request("unknown/expired OAuth state"))?;
    let oauth = st.creds().snapshot().google_oauth.ok_or_else(|| bad_request("Google OAuth client not configured"))?;
    let origin = st.public_origin().ok_or_else(|| bad_request("public_origin not set"))?.to_string();
    let redirect_uri = format!("{origin}/api/oauth/google/callback");

    // Exchange (PKCE) + persist the refresh token (shared token store with the CLI).
    let (cid, csec) = (oauth.client_id.clone(), oauth.client_secret.clone());
    let refresh = tokio::task::spawn_blocking(move || mgmt_google::finish_auth(&cid, &csec, &redirect_uri, &code, &verifier))
        .await
        .map_err(|e| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, format!("task: {e}")))?
        .map_err(ApiError::from)?;
    let store = google_dir(st)?.join(format!("{}-token.json", mgmt_store::safe_stem(&account)));
    mgmt_google::GoogleToken { client_id: oauth.client_id, client_secret: oauth.client_secret, refresh_token: refresh }
        .save(&store)
        .map_err(ApiError::from)?;

    // List the user's calendars and provision an account + a collection per calendar.
    let store2 = store.clone();
    let token = tokio::task::spawn_blocking(move || mgmt_google::access_token(&store2))
        .await
        .map_err(|e| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, format!("task: {e}")))?
        .map_err(ApiError::from)?;
    let cals = tokio::task::spawn_blocking(move || mgmt_google::list_calendars(&token))
        .await
        .map_err(|e| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, format!("task: {e}")))?
        .map_err(ApiError::from)?;

    let path = caldav_path(st)?.to_path_buf();
    let mut file = CalDavFile::load(&path).map_err(ApiError::from)?;
    file.accounts.retain(|a| a.name != account);
    file.accounts.push(Account { name: account.clone(), auth: "google".into(), username: None, password: None, token: None });
    let mut seen: HashMap<String, ()> = HashMap::new();
    for c in cals {
        let base = sanitize(&c.summary);
        let name = if c.primary { account.clone() } else { format!("{}-{base}", account) };
        if seen.insert(name.clone(), ()).is_some() {
            continue;
        }
        file.collections.retain(|x| x.name != name);
        file.collections.push(Collection {
            name,
            kind: "events".into(),
            url: mgmt_google::caldav_url_for(&c.id),
            account: account.clone(),
            protocol: "caldav".into(),
        });
    }
    file.save(&path).map_err(ApiError::from)?;
    Ok(())
}

fn sanitize(s: &str) -> String {
    let out: String = s.chars().map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '-' }).collect();
    let trimmed = out.trim_matches('-').to_string();
    if trimmed.is_empty() { "cal".into() } else { trimmed }
}
