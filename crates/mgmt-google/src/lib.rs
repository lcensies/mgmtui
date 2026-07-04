//! Google integration for mgmt: a blocking facade over the Google OAuth2 + Calendar REST APIs.
//!
//! OAuth is hand-rolled (no SDK) around a single durable credential — the **refresh token** — so the
//! *same* persisted token powers every consumer and every entry point:
//!
//! * the **CalDAV sync**, which refreshes a short-lived access token and uses it as a bearer against
//!   Google's CalDAV endpoint (Google needs OAuth, not a password — so it reuses the generic CalDAV
//!   path with `Auth::Bearer`), and
//! * [`create_meet`], which mints a Google Meet via the REST API (CalDAV can't create conferences).
//!
//! The refresh token is obtained one of two ways, both of which write the same [`GoogleToken`]
//! store: the CLI loopback flow ([`login_loopback`]) or the web redirect flow (the server calls
//! [`authorize_url`] then [`exchange_code`]). Everything downstream is identical.
//!
//! Like `mgmt-dav`, this owns a Tokio runtime and `block_on`s so the rest of mgmt stays synchronous.

use std::future::Future;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::Path;

use oauth2::basic::BasicClient;
use oauth2::{
    AuthUrl, AuthorizationCode, ClientId, ClientSecret, CsrfToken, PkceCodeChallenge, PkceCodeVerifier,
    RedirectUrl, RefreshToken, Scope, TokenResponse, TokenUrl,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use mgmt_core::{Error, Result};

const AUTH_URL: &str = "https://accounts.google.com/o/oauth2/v2/auth";
const TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
const API: &str = "https://www.googleapis.com/calendar/v3";
/// Scope covering both Google CalDAV and the REST API (read/write events + conferences).
pub const SCOPE: &str = "https://www.googleapis.com/auth/calendar";

/// The durable Google credential for one account: the OAuth client plus the long-lived refresh
/// token. Persisted (0600) at `<config>/google/<account>-token.json`; refreshing it yields the
/// short-lived access tokens used as CalDAV bearers and REST auth.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GoogleToken {
    pub client_id: String,
    pub client_secret: String,
    pub refresh_token: String,
}

impl GoogleToken {
    pub fn load(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path)
            .map_err(|_| Error::NotFound(format!("no Google token at {} — connect the account first", path.display())))?;
        serde_json::from_str(&text).map_err(|e| Error::Parse(format!("parsing {}: {e}", path.display())))
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, serde_json::to_string_pretty(self).map_err(|e| Error::Other(e.to_string()))?)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
        }
        Ok(())
    }
}

/// Build a one-shot runtime and run `fut` (the `mgmt-dav` blocking pattern).
fn block<F: Future<Output = Result<T>>, T>(fut: F) -> Result<T> {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .map_err(Error::Io)?;
    rt.block_on(fut)
}

/// Build the configured `oauth2` client. The token exchange, refresh, CSRF, and PKCE handling all
/// live in the `oauth2` crate — we only wire the endpoints + our client credentials.
fn oauth_client(client_id: &str, client_secret: &str, redirect_uri: &str) -> Result<
    oauth2::Client<
        oauth2::basic::BasicErrorResponse,
        oauth2::basic::BasicTokenResponse,
        oauth2::basic::BasicTokenIntrospectionResponse,
        oauth2::StandardRevocableToken,
        oauth2::basic::BasicRevocationErrorResponse,
        oauth2::EndpointSet,
        oauth2::EndpointNotSet,
        oauth2::EndpointNotSet,
        oauth2::EndpointNotSet,
        oauth2::EndpointSet,
    >,
> {
    let bad = |e: oauth2::url::ParseError| Error::Invalid(format!("bad OAuth URL: {e}"));
    Ok(BasicClient::new(ClientId::new(client_id.to_string()))
        .set_client_secret(ClientSecret::new(client_secret.to_string()))
        .set_auth_uri(AuthUrl::new(AUTH_URL.to_string()).map_err(bad)?)
        .set_token_uri(TokenUrl::new(TOKEN_URL.to_string()).map_err(bad)?)
        .set_redirect_uri(RedirectUrl::new(redirect_uri.to_string()).map_err(bad)?))
}

fn http_client() -> Result<reqwest::Client> {
    // Following redirects on the token endpoint would open an SSRF hole (per oauth2 docs).
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|e| Error::Other(format!("building HTTP client: {e}")))
}

/// The start of an authorization-code flow: the consent URL to send the browser to, plus the CSRF
/// `state` and PKCE `verifier` the caller must stash (keyed by `state`) until the callback.
#[derive(Debug, Clone)]
pub struct AuthStart {
    pub url: String,
    pub state: String,
    pub pkce_verifier: String,
}

/// Begin an authorization-code flow (PKCE + CSRF via the `oauth2` crate). `access_type=offline` +
/// `prompt=consent` make Google return a refresh token.
pub fn begin_auth(client_id: &str, client_secret: &str, redirect_uri: &str) -> Result<AuthStart> {
    let client = oauth_client(client_id, client_secret, redirect_uri)?;
    let (challenge, verifier) = PkceCodeChallenge::new_random_sha256();
    let (url, state) = client
        .authorize_url(CsrfToken::new_random)
        .add_scope(Scope::new(SCOPE.to_string()))
        .add_extra_param("access_type", "offline")
        .add_extra_param("prompt", "consent")
        .set_pkce_challenge(challenge)
        .url();
    Ok(AuthStart { url: url.to_string(), state: state.secret().clone(), pkce_verifier: verifier.into_secret() })
}

/// Finish the flow: exchange the callback `code` (with its PKCE `verifier`) for tokens and return
/// the refresh token. The caller has already verified `state`.
pub fn finish_auth(client_id: &str, client_secret: &str, redirect_uri: &str, code: &str, pkce_verifier: &str) -> Result<String> {
    let client = oauth_client(client_id, client_secret, redirect_uri)?;
    let verifier = PkceCodeVerifier::new(pkce_verifier.to_string());
    let code = AuthorizationCode::new(code.to_string());
    block(async move {
        let http = http_client()?;
        let token = client
            .exchange_code(code)
            .set_pkce_verifier(verifier)
            .request_async(&http)
            .await
            .map_err(|e| Error::Other(format!("Google code exchange failed: {e}")))?;
        token
            .refresh_token()
            .map(|r| r.secret().clone())
            .ok_or_else(|| Error::Other("Google did not return a refresh token (re-consent)".into()))
    })
}

/// Refresh a stored [`GoogleToken`] into a fresh access token (bearer).
pub fn access_token(store: &Path) -> Result<String> {
    let tok = GoogleToken::load(store)?;
    // Redirect URL is irrelevant to the refresh grant; a placeholder keeps the typed client happy.
    let client = oauth_client(&tok.client_id, &tok.client_secret, "http://127.0.0.1/")?;
    block(async move {
        let http = http_client()?;
        let resp = client
            .exchange_refresh_token(&RefreshToken::new(tok.refresh_token))
            .request_async(&http)
            .await
            .map_err(|e| Error::Other(format!("Google token refresh failed (reconnect the account?): {e}")))?;
        Ok(resp.access_token().secret().clone())
    })
}

/// CLI login: begin the flow, run a localhost redirect server to catch the code, and persist the
/// [`GoogleToken`]. `client_id`/`client_secret` come from a Desktop-app OAuth client (Google
/// auto-allows loopback redirects). Blocks until the browser completes consent.
pub fn login_loopback(client_id: &str, client_secret: &str, store: &Path, port: u16) -> Result<()> {
    let listener = TcpListener::bind(("127.0.0.1", port))
        .map_err(|e| Error::Other(format!("cannot bind 127.0.0.1:{port} for the OAuth redirect: {e}")))?;
    let redirect_uri = format!("http://127.0.0.1:{port}");
    let start = begin_auth(client_id, client_secret, &redirect_uri)?;

    println!("Opening your browser to authorize Google. If it doesn't open, visit:\n{}", start.url);
    let _ = open_browser(&start.url);

    // Accept exactly one request — the redirect carrying ?code=...&state=...
    let (mut stream, _) = listener.accept().map_err(Error::Io)?;
    let mut buf = [0u8; 8192];
    let n = stream.read(&mut buf).map_err(Error::Io)?;
    let request = String::from_utf8_lossy(&buf[..n]);
    let (code, got_state) =
        parse_redirect(&request).ok_or_else(|| Error::Other("no authorization code in the redirect".into()))?;
    if got_state != start.state {
        let _ = stream.write_all(http_page("State mismatch — try again.").as_bytes());
        return Err(Error::Other("OAuth state mismatch (possible CSRF) — aborted".into()));
    }
    let _ = stream.write_all(http_page("Authorized. You can close this tab and return to the terminal.").as_bytes());

    let refresh_token = finish_auth(client_id, client_secret, &redirect_uri, &code, &start.pkce_verifier)?;
    GoogleToken { client_id: client_id.to_string(), client_secret: client_secret.to_string(), refresh_token }.save(store)
}

/// Extract `(client_id, client_secret)` from a Google-downloaded OAuth client JSON
/// (`{"installed":{…}}` for Desktop apps or `{"web":{…}}` for Web apps).
pub fn parse_client_secret(json_text: &str) -> Result<(String, String)> {
    let v: Value = serde_json::from_str(json_text).map_err(|e| Error::Parse(format!("bad client secret JSON: {e}")))?;
    let node = v.get("installed").or_else(|| v.get("web")).unwrap_or(&v);
    let id = node["client_id"].as_str().ok_or_else(|| Error::Invalid("client secret JSON missing client_id".into()))?;
    let secret = node["client_secret"].as_str().ok_or_else(|| Error::Invalid("client secret JSON missing client_secret".into()))?;
    Ok((id.to_string(), secret.to_string()))
}

/// Create (idempotently) a Google Meet on the event identified by its iCalendar `uid` in
/// `calendar_id`, returning the video join URL. `access_token` is a bearer from [`access_token`].
pub fn create_meet(access_token: &str, calendar_id: &str, uid: &str) -> Result<String> {
    block(async move {
        let http = client()?;
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

        // 2) Patch the event to request a Meet conference (stable requestId → idempotent).
        let patch_url = format!("{API}/calendars/{}/events/{}", enc(calendar_id), enc(&event_id));
        let body = json!({
            "conferenceData": { "createRequest": {
                "requestId": format!("mgmt-{uid}"),
                "conferenceSolutionKey": { "type": "hangoutsMeet" }
            }}
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

        // 3) Creation is async ("pending"): poll until the video entry point appears.
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

/// Extract the Google calendar id from a CalDAV URL like
/// `https://apidata.googleusercontent.com/caldav/v2/<calId>/events` (`<calId>` is usually the
/// account email). Returns `None` if the URL isn't a Google CalDAV path.
pub fn calendar_id_from_caldav_url(url: &str) -> Option<String> {
    let rest = url.split("/caldav/v2/").nth(1)?;
    let id = rest.split('/').next()?;
    if id.is_empty() {
        return None;
    }
    Some(id.replace("%40", "@"))
}

/// Google's CalDAV events URL for a calendar id (what a Google `Collection.url` should be).
pub fn caldav_url_for(calendar_id: &str) -> String {
    format!("https://apidata.googleusercontent.com/caldav/v2/{}/events", enc(calendar_id))
}

/// A calendar in the user's Google calendar list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoogleCalendar {
    pub id: String,
    pub summary: String,
    pub primary: bool,
}

/// List the user's Google calendars (REST `calendarList`) so each can become a `Collection`.
pub fn list_calendars(access_token: &str) -> Result<Vec<GoogleCalendar>> {
    block(async move {
        let http = client()?;
        let resp: Value = http
            .get(format!("{API}/users/me/calendarList"))
            .bearer_auth(access_token)
            .send()
            .await
            .and_then(|r| r.error_for_status())
            .map_err(net)?
            .json()
            .await
            .map_err(net)?;
        let items = resp["items"].as_array().cloned().unwrap_or_default();
        Ok(items
            .into_iter()
            .filter_map(|c| {
                let id = c["id"].as_str()?.to_string();
                let summary = c["summary"].as_str().unwrap_or(&id).to_string();
                Some(GoogleCalendar { id, summary, primary: c["primary"].as_bool().unwrap_or(false) })
            })
            .collect())
    })
}

// ---- helpers ----------------------------------------------------------------------

fn client() -> Result<reqwest::Client> {
    reqwest::Client::builder().build().map_err(|e| Error::Other(format!("building HTTP client: {e}")))
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

/// Pull `code` and `state` from the first line of the loopback HTTP request.
fn parse_redirect(request: &str) -> Option<(String, String)> {
    let line = request.lines().next()?; // "GET /?code=...&state=... HTTP/1.1"
    let path = line.split_whitespace().nth(1)?;
    let query = path.split_once('?')?.1;
    let mut code = None;
    let mut state = None;
    for pair in query.split('&') {
        if let Some((k, v)) = pair.split_once('=') {
            match k {
                "code" => code = Some(url_decode(v)),
                "state" => state = Some(url_decode(v)),
                _ => {}
            }
        }
    }
    Some((code?, state?))
}

fn http_page(msg: &str) -> String {
    let body = format!("<!doctype html><meta charset=utf-8><title>mgmt</title><body style='font-family:sans-serif'>{msg}</body>");
    format!("HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len())
}

fn open_browser(url: &str) -> std::io::Result<()> {
    let cmd = if cfg!(target_os = "macos") { "open" } else { "xdg-open" };
    std::process::Command::new(cmd).arg(url).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).spawn().map(|_| ())
}

/// Minimal percent-encoding for a URL path segment / query value.
fn enc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

fn url_decode(s: &str) -> String {
    let bytes = s.replace('+', " ");
    let bytes = bytes.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(b) = u8::from_str_radix(&String::from_utf8_lossy(&bytes[i + 1..i + 3]), 16) {
                out.push(b);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
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
        assert_eq!(meet_url(&json!({ "hangoutLink": "https://meet.google.com/xyz" })).as_deref(), Some("https://meet.google.com/xyz"));
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

    #[test]
    fn begin_auth_builds_a_consent_url_with_pkce_and_offline() {
        let s = begin_auth("cid", "csecret", "https://ex.com/cb").unwrap();
        assert!(s.url.contains("client_id=cid"));
        assert!(s.url.contains("access_type=offline"));
        assert!(s.url.contains("code_challenge=")); // PKCE from the oauth2 crate
        assert!(s.url.contains(&format!("state={}", s.state)));
        assert!(!s.pkce_verifier.is_empty());
    }

    #[test]
    fn parse_redirect_extracts_code_and_state() {
        let req = "GET /?code=4%2F0Ab&state=xyz HTTP/1.1\r\nHost: localhost\r\n\r\n";
        assert_eq!(parse_redirect(req), Some(("4/0Ab".to_string(), "xyz".to_string())));
    }

    #[test]
    fn parse_client_secret_reads_installed_and_web() {
        let (id, sec) = parse_client_secret(r#"{"installed":{"client_id":"a","client_secret":"b"}}"#).unwrap();
        assert_eq!((id.as_str(), sec.as_str()), ("a", "b"));
        let (id, sec) = parse_client_secret(r#"{"web":{"client_id":"c","client_secret":"d"}}"#).unwrap();
        assert_eq!((id.as_str(), sec.as_str()), ("c", "d"));
    }

    #[test]
    fn google_token_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("g/token.json");
        let t = GoogleToken { client_id: "i".into(), client_secret: "s".into(), refresh_token: "r".into() };
        t.save(&p).unwrap();
        let back = GoogleToken::load(&p).unwrap();
        assert_eq!(back.refresh_token, "r");
    }
}
