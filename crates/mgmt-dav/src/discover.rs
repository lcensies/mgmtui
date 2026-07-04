//! CalDAV auto-discovery: given a server base URL + credentials, enumerate the user's calendars
//! (name, URL, and whether each holds events and/or tasks) by walking the RFC-6764/4791 chain —
//! current-user-principal → calendar-home-set → calendars → displayname + supported-component-set.
//!
//! Like [`crate::client`] this is a blocking facade over async `libdav`: it owns a Tokio runtime and
//! `block_on`s. Callers already inside a runtime (e.g. `mgmt-web` handlers) must invoke it from
//! `tokio::task::spawn_blocking`.

use http::Uri;
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
            let client = LibCalDav::new(WebDavClient::new(uri, http));
            rt.block_on(run_discovery(&client, &origin))
        }
        Auth::Basic { user, password } => {
            let svc = AddAuthorization::basic(http, &user, &password);
            let client = LibCalDav::new(WebDavClient::new(uri, svc));
            rt.block_on(run_discovery(&client, &origin))
        }
        Auth::Bearer { token } => {
            let svc = AddAuthorization::bearer(http, &token);
            let client = LibCalDav::new(WebDavClient::new(uri, svc));
            rt.block_on(run_discovery(&client, &origin))
        }
    }
}

async fn run_discovery<C: HttpClient>(client: &LibCalDav<C>, origin: &str) -> Result<Vec<DiscoveredCalendar>> {
    let principal = client
        .find_current_user_principal()
        .await
        .map_err(dav_err)?
        .ok_or_else(|| Error::Other("could not find your account — check the server URL and credentials".into()))?;
    let principal_path = uri_path(&principal.to_string());

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

fn dav_err<E: std::fmt::Display>(e: E) -> Error {
    Error::Other(format!("caldav discovery: {e}"))
}
