//! Admin-only user management: create/list/delete users, mint & revoke their scoped sync tokens,
//! and generate the `mgmt://pair/...` export URL another node imports to clone + keep syncing.
//!
//! Every handler requires the request principal to be the admin (a logged-in web session, or an
//! admin-owned bearer token). A regular user's sync token resolves to *their* id, so it can never
//! reach these routes.

use axum::extract::{Extension, Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

use mgmt_config::{Account, CalDavFile, Collection};
use mgmt_dav::Auth;

use crate::auth::{new_api_token, TokenEntry, WebUser};
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
        .map(|u| json!({ "id": u.id, "name": u.name, "tokens": u.tokens.iter().map(|t| &t.name).collect::<Vec<_>>() }))
        .collect();
    Ok(Json(json!({ "users": users })))
}

#[derive(Deserialize)]
pub struct NewUser {
    id: String,
    #[serde(default)]
    name: String,
}

/// `POST /api/admin/users` — create a user + its isolated vault directory.
pub async fn create_user(State(st): State<AppState>, Extension(p): Extension<Principal>, Json(body): Json<NewUser>) -> Result<Response, ApiError> {
    require_admin(&p)?;
    if body.id == mgmt_store::ADMIN_USER || !is_safe_user_id(&body.id) {
        return Err(bad_request("invalid user id (use a-z, 0-9, - or _)"));
    }
    let exists = st.creds().snapshot().users.iter().any(|u| u.id == body.id);
    if exists {
        return Err(ApiError::new(StatusCode::CONFLICT, "user already exists"));
    }
    // Establish the isolated vault so the user is immediately syncable.
    let root = st.users_base().join(&body.id);
    for dir in [mgmt_store::tasks_dir(&root), mgmt_store::calendars_dir(&root), mgmt_store::projects_dir(&root)] {
        std::fs::create_dir_all(dir).map_err(|e| ApiError::from(mgmt_core::Error::Io(e)))?;
    }
    let name = if body.name.is_empty() { body.id.clone() } else { body.name.clone() };
    st.creds().mutate_file(|f| f.users.push(WebUser { id: body.id.clone(), name: name.clone(), tokens: Vec::new() }))?;
    Ok((StatusCode::CREATED, Json(json!({ "id": body.id, "name": name }))).into_response())
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
