//! Google integration for mgmt: a blocking facade over async `yup-oauth2` and the Google Calendar
//! REST API. One OAuth login (persisted refresh token) is shared by two consumers:
//!
//! * the CalDAV sync, which uses a fresh access token as a **bearer** against Google's CalDAV
//!   endpoint (reusing the generic CalDAV path — Google needs OAuth, not a password), and
//! * [`create_meet`], which mints a Google Meet on an event via the REST API (CalDAV can't create
//!   conferences) and returns the join URL to store in the event's `CONFERENCE` property.
//!
//! Like `mgmt-dav` this owns a Tokio runtime and `block_on`s, so the rest of mgmt stays synchronous.

use std::future::Future;
use std::path::Path;

use serde_json::{json, Value};
use yup_oauth2::{read_application_secret, InstalledFlowAuthenticator, InstalledFlowReturnMethod};

use mgmt_core::{Error, Result};

/// The OAuth scope covering both Google CalDAV and the REST API (read/write events + conferences).
pub const SCOPE: &str = "https://www.googleapis.com/auth/calendar";

/// Build a one-shot runtime and run `fut` to completion (the `mgmt-dav` blocking pattern).
fn block<F: Future<Output = Result<T>>, T>(fut: F) -> Result<T> {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .map_err(Error::Io)?;
    rt.block_on(fut)
}

/// Fetch a token for `scopes`, building the persisted-to-disk InstalledFlow authenticator. The
/// concrete authenticator type names `hyper_util` internals that yup-oauth2 doesn't re-export, so
/// this stays inline (type-inferred) rather than a helper with a written-out return type.
async fn fetch_token(client_secret: &Path, token_store: &Path) -> Result<String> {
    let secret = read_application_secret(client_secret)
        .await
        .map_err(|e| Error::Invalid(format!("reading Google client secret {}: {e}", client_secret.display())))?;
    let auth = InstalledFlowAuthenticator::builder(secret, InstalledFlowReturnMethod::HTTPRedirect)
        .persist_tokens_to_disk(token_store)
        .build()
        .await
        .map_err(|e| Error::Other(format!("building Google authenticator: {e}")))?;
    let tok = auth
        .token(&[SCOPE])
        .await
        .map_err(|e| Error::Other(format!("Google token/consent failed (run `mgmt google login`?): {e}")))?;
    tok.token()
        .map(|s| s.to_string())
        .ok_or_else(|| Error::Other("Google returned no access token".into()))
}

/// Interactive first-time login: run the InstalledFlow (opens a browser + local redirect), obtain
/// consent, and persist the refresh token to `token_store`. Subsequent [`access_token`] calls
/// refresh non-interactively.
pub fn login(client_secret: &Path, token_store: &Path) -> Result<()> {
    block(async {
        fetch_token(client_secret, token_store).await?;
        Ok(())
    })
}

/// Return a fresh access token, refreshing via the persisted refresh token. Non-interactive: fails
/// if the user has not run [`login`] yet.
pub fn access_token(client_secret: &Path, token_store: &Path) -> Result<String> {
    block(async { fetch_token(client_secret, token_store).await })
}

const API: &str = "https://www.googleapis.com/calendar/v3";

/// Create (idempotently) a Google Meet on the event identified by its iCalendar `uid` in
/// `calendar_id`, and return the video join URL. `access_token` is a bearer from [`access_token`].
///
/// Uses a request id derived from the uid so retrying yields the same conference (Google's
/// documented idempotency). Conference creation is asynchronous, so this polls the event until the
/// video entry point is populated.
pub fn create_meet(access_token: &str, calendar_id: &str, uid: &str) -> Result<String> {
    block(async move {
        let http = reqwest::Client::builder()
            .build()
            .map_err(|e| Error::Other(format!("building HTTP client: {e}")))?;
        let bearer = |rb: reqwest::RequestBuilder| rb.bearer_auth(access_token);

        // 1) Resolve the Google event id from the iCalendar UID.
        let list_url = format!("{API}/calendars/{}/events", enc(calendar_id));
        let listing: Value = bearer(http.get(&list_url))
            .query(&[("iCalUID", uid), ("maxResults", "1")])
            .send()
            .await
            .and_then(|r| r.error_for_status())
            .map_err(net)?
            .json()
            .await
            .map_err(net)?;
        let event_id = listing["items"][0]["id"]
            .as_str()
            .ok_or_else(|| Error::NotFound(format!("event {uid} not found on Google (is it synced yet?)")))?
            .to_string();

        // 2) Patch the event to request a Meet conference.
        let patch_url = format!("{API}/calendars/{}/events/{}", enc(calendar_id), enc(&event_id));
        let body = json!({
            "conferenceData": {
                "createRequest": {
                    "requestId": format!("mgmt-{uid}"),
                    "conferenceSolutionKey": { "type": "hangoutsMeet" }
                }
            }
        });
        let patched: Value = bearer(http.patch(&patch_url))
            .query(&[("conferenceDataVersion", "1")])
            .json(&body)
            .send()
            .await
            .and_then(|r| r.error_for_status())
            .map_err(net)?
            .json()
            .await
            .map_err(net)?;
        if let Some(url) = meet_url(&patched) {
            return Ok(url);
        }

        // 3) Creation is async ("pending"): poll the event until the video entry point appears.
        for _ in 0..6 {
            tokio::time::sleep(std::time::Duration::from_millis(700)).await;
            let ev: Value = bearer(http.get(&patch_url))
                .query(&[("conferenceDataVersion", "1")])
                .send()
                .await
                .and_then(|r| r.error_for_status())
                .map_err(net)?
                .json()
                .await
                .map_err(net)?;
            if let Some(url) = meet_url(&ev) {
                return Ok(url);
            }
        }
        Err(Error::Other("Google Meet creation is still pending — try again shortly".into()))
    })
}

/// Extract the Google calendar id from a CalDAV collection URL like
/// `https://apidata.googleusercontent.com/caldav/v2/<calId>/events` (the `<calId>` is usually the
/// account email). Returns `None` if the URL isn't a Google CalDAV path.
pub fn calendar_id_from_caldav_url(url: &str) -> Option<String> {
    let rest = url.split("/caldav/v2/").nth(1)?;
    let id = rest.split('/').next()?;
    if id.is_empty() {
        return None;
    }
    // The segment is percent-encoded (email `@` -> `%40`); decode the one escape we emit.
    Some(id.replace("%40", "@"))
}

/// Extract the video join URL from a Calendar API event resource.
fn meet_url(event: &Value) -> Option<String> {
    if let Some(points) = event["conferenceData"]["entryPoints"].as_array() {
        for p in points {
            if p["entryPointType"].as_str() == Some("video") {
                if let Some(uri) = p["uri"].as_str() {
                    return Some(uri.to_string());
                }
            }
        }
    }
    event["hangoutLink"].as_str().map(|s| s.to_string())
}

/// Minimal percent-encoding for a single URL path segment (Google calendar ids can contain `@`).
fn enc(seg: &str) -> String {
    let mut out = String::with_capacity(seg.len());
    for b in seg.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

fn net(e: reqwest::Error) -> Error {
    Error::Other(format!("Google API request failed: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn meet_url_prefers_video_entry_point() {
        let ev = json!({
            "conferenceData": { "entryPoints": [
                { "entryPointType": "more", "uri": "https://meet.google.com/settings" },
                { "entryPointType": "video", "uri": "https://meet.google.com/abc-defg-hij" }
            ]},
            "hangoutLink": "https://meet.google.com/legacy"
        });
        assert_eq!(meet_url(&ev).as_deref(), Some("https://meet.google.com/abc-defg-hij"));
    }

    #[test]
    fn meet_url_falls_back_to_hangout_link() {
        let ev = json!({ "hangoutLink": "https://meet.google.com/xyz" });
        assert_eq!(meet_url(&ev).as_deref(), Some("https://meet.google.com/xyz"));
    }

    #[test]
    fn enc_encodes_email_calendar_id() {
        assert_eq!(enc("me@gmail.com"), "me%40gmail.com");
        assert_eq!(enc("primary"), "primary");
    }

    #[test]
    fn calendar_id_parsed_from_caldav_url() {
        assert_eq!(
            calendar_id_from_caldav_url("https://apidata.googleusercontent.com/caldav/v2/me%40gmail.com/events").as_deref(),
            Some("me@gmail.com"),
        );
        assert_eq!(calendar_id_from_caldav_url("https://caldav.fastmail.com/dav/calendars/x/"), None);
    }
}
