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
