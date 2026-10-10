//! CalDAV auto-discovery: given a server base URL + credentials, enumerate the user's calendars
//! (name, URL, and whether each holds events and/or tasks) by walking the RFC-6764/4791 chain —
//! current-user-principal → calendar-home-set → calendars → displayname + supported-component-set.
//!
//! Like [`crate::client`] this is a blocking facade over async `libdav`: it owns a Tokio runtime and
//! `block_on`s. Callers already inside a runtime (e.g. `mgmt-web` handlers) must invoke it from
//! `tokio::task::spawn_blocking`.

use http::header::LOCATION;
use http::{Method, Request, Uri};
use hyper_rustls::HttpsConnectorBuilder;
use hyper_util::client::legacy::Client;
use hyper_util::rt::TokioExecutor;
use libdav::caldav::{CalendarComponent, FindCalendarHomeSet, FindCalendars, GetSupportedComponents};
use libdav::dav::{GetProperties, WebDavClient};
use libdav::{names, CalDavClient as LibCalDav, HttpClient};
use tower_http::auth::AddAuthorization;

use mgmt_core::{Error, Result};

use crate::client::{rebase, uri_path, Auth};

/// A calendar found by discovery. `url` is a full URL ready to store as a `Collection.url`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredCalendar {
    pub name: String,
    pub url: String,
    /// The calendar advertises VEVENT support (→ an `events` collection).
    pub supports_events: bool,
    /// The calendar advertises VTODO support (→ a `tasks` collection).
    pub supports_tasks: bool,
}

/// Discover the calendars reachable from `base_url` with `auth`. `base_url` may be a provider root
/// (e.g. `https://caldav.yandex.ru`) or a fuller path; discovery follows the principal chain from
/// there.
pub fn discover_calendars(base_url: impl AsRef<str>, auth: Auth) -> Result<Vec<DiscoveredCalendar>> {
    let uri: Uri = base_url
        .as_ref()
        .parse()
        .map_err(|e| Error::Invalid(format!("bad server url: {e}")))?;
    let scheme = uri.scheme_str().ok_or_else(|| Error::Invalid("server url missing scheme".into()))?;
    let authority = uri.authority().ok_or_else(|| Error::Invalid("server url missing host".into()))?;
    let origin = format!("{scheme}://{authority}");

    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .map_err(Error::Io)?;

    let connector = HttpsConnectorBuilder::new()
        .with_webpki_roots()
        .https_or_http()
        .enable_http1()
        .build();
    let http = Client::builder(TokioExecutor::new()).build(connector);

    // The full entered URL is the discovery base so `find_current_user_principal` starts there.
    match auth {
        Auth::None => {
            let mut client = LibCalDav::new(WebDavClient::new(uri, http));
            rt.block_on(run_discovery(&mut client, &origin))
        }
        Auth::Basic { user, password } => {
            let svc = AddAuthorization::basic(http, &user, &password);
            let mut client = LibCalDav::new(WebDavClient::new(uri, svc));
            rt.block_on(run_discovery(&mut client, &origin))
        }
        Auth::Bearer { token } => {
            let svc = AddAuthorization::bearer(http, &token);
            let mut client = LibCalDav::new(WebDavClient::new(uri, svc));
            rt.block_on(run_discovery(&mut client, &origin))
        }
    }
}

async fn run_discovery<C: HttpClient>(client: &mut LibCalDav<C>, origin: &str) -> Result<Vec<DiscoveredCalendar>> {
    let principal_path = find_principal(client, origin).await?;

    let home = client
        .request(FindCalendarHomeSet::new(&principal_path))
        .await
        .map_err(dav_err)?;

    let mut out = Vec::new();
    for hs in home.home_sets {
        let hs_path = uri_path(&hs.to_string());
        let found = client.request(FindCalendars::new(&hs_path)).await.map_err(dav_err)?;
        for cal in found.calendars {
            let href_path = uri_path(&cal.href);

            // Display name (best-effort; fall back to the last path segment).
            let name = match client.request(GetProperties::new(&href_path, &[&names::DISPLAY_NAME])).await {
                Ok(resp) => resp.values.into_iter().next().and_then(|(_, v)| v).unwrap_or_default(),
                Err(_) => String::new(),
            };
            let name = if name.trim().is_empty() {
                href_path.trim_end_matches('/').rsplit('/').next().unwrap_or("calendar").to_string()
            } else {
                name.trim().to_string()
            };

            // Supported components → events vs tasks. If the server reports none, assume events.
            let (mut supports_events, mut supports_tasks) = (false, false);
            if let Ok(resp) = client.request(GetSupportedComponents::new(&href_path)).await {
                for comp in resp.components {
                    match comp {
                        CalendarComponent::VEvent => supports_events = true,
                        CalendarComponent::VTodo => supports_tasks = true,
                        _ => {}
                    }
                }
            }
            if !supports_events && !supports_tasks {
                supports_events = true;
            }

            out.push(DiscoveredCalendar {
                name,
                url: rebase(origin, &href_path),
                supports_events,
                supports_tasks,
            });
        }
    }
    Ok(out)
}

/// Locate the user's principal, trying — in RFC 6764 order — the URL as entered, the target of the
/// `/.well-known/caldav` redirect, then the server root. A server that answers 401 on the entered
/// path while the credentials are perfectly good is the common case (Nextcloud's `/`, a provider's
/// marketing root), so one rejected candidate must not end discovery.
async fn find_principal<C: HttpClient>(client: &mut LibCalDav<C>, origin: &str) -> Result<String> {
    let entered = client.base_url.clone();
    let root: Uri = origin.parse().map_err(|e| Error::Invalid(format!("bad origin: {e}")))?;
    let mut last = None;

    if let Some(p) = try_principal(client, entered.clone(), &mut last).await {
        return Ok(p);
    }
    if let Some(ctx) = well_known_context(client, origin).await {
        if let Some(p) = try_principal(client, ctx, &mut last).await {
            return Ok(p);
        }
    }
    if root != entered {
        if let Some(p) = try_principal(client, root, &mut last).await {
            return Ok(p);
        }
    }

    Err(Error::Other(match last {
        Some(e) => format!("could not find your account — check the server URL and credentials ({e})"),
        None => "could not find your account — check the server URL and credentials".into(),
    }))
}

/// One `current-user-principal` probe against `base`, remembering the failure instead of raising it.
async fn try_principal<C: HttpClient>(client: &mut LibCalDav<C>, base: Uri, last: &mut Option<String>) -> Option<String> {
    client.webdav_client.base_url = base;
    match client.find_current_user_principal().await {
        Ok(Some(p)) => Some(uri_path(&p.to_string())),
        Ok(None) => None,
        Err(e) => {
            *last = Some(e.to_string());
            None
        }
    }
}

/// RFC 6764 §6: `GET /.well-known/caldav` and follow its redirects (relative `Location`s resolved
/// against the origin, as servers like Nextcloud send them). Returns where the server points.
async fn well_known_context<C: HttpClient>(client: &LibCalDav<C>, origin: &str) -> Option<Uri> {
    let mut uri: Uri = format!("{origin}/.well-known/caldav").parse().ok()?;
    for hop in 0..5 {
        let req = Request::builder().method(Method::GET).uri(&uri).body(String::new()).ok()?;
        let (head, _body) = client.request_raw(req).await.ok()?;
        if !head.status.is_redirection() {
            return (hop > 0).then_some(uri);
        }
        let loc = head.headers.get(LOCATION)?.to_str().ok()?;
        uri = match loc.starts_with("http") {
            true => loc.parse().ok()?,
            false => format!("{origin}/{}", loc.trim_start_matches('/')).parse().ok()?,
        };
    }
    None
}

fn dav_err<E: std::fmt::Display>(e: E) -> Error {
    Error::Other(format!("caldav discovery: {e}"))
}
