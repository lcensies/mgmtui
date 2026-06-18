//! CalDAV client — a small blocking facade over the async `libdav` crate. We keep the rest
//! of mgmt synchronous, so this owns a Tokio runtime and `block_on`s libdav requests. The
//! messy concrete client type is erased behind the private [`Backend`] trait.

use http::Uri;
use hyper_rustls::HttpsConnectorBuilder;
use hyper_util::client::legacy::Client;
use hyper_util::rt::TokioExecutor;
use libdav::caldav::GetCalendarResources;
use libdav::dav::{Delete, ListResources, PutResource, WebDavClient};
use libdav::{CalDavClient as LibCalDav, HttpClient};
use tokio::runtime::{Handle, Runtime};
use tower_http::auth::AddAuthorization;

use mgmt_core::{Error, Result};

const CALENDAR_MIME: &str = "text/calendar; charset=utf-8";

/// How to authenticate to the server.
#[derive(Debug, Clone)]
pub enum Auth {
    None,
    Basic { user: String, password: String },
    Bearer { token: String },
}

/// One resource on a CalDAV collection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteItem {
    pub href: String,
    pub etag: Option<String>,
    /// The iCalendar body, when fetched (a listing leaves this `None`).
    pub data: Option<String>,
}

/// Type-erased CalDAV operations over server-absolute paths.
trait Backend: Send + Sync {
    fn list(&self, collection_path: &str) -> Result<Vec<(String, Option<String>)>>;
    fn get(&self, path: &str) -> Result<(Option<String>, String)>;
    fn create(&self, path: &str, body: &str) -> Result<Option<String>>;
    fn update(&self, path: &str, body: &str, etag: &str) -> Result<Option<String>>;
    fn delete(&self, path: &str, etag: Option<&str>) -> Result<()>;
}

struct LibBackend<C: HttpClient> {
    client: LibCalDav<C>,
    handle: Handle,
}

impl<C: HttpClient> Backend for LibBackend<C> {
    fn list(&self, collection_path: &str) -> Result<Vec<(String, Option<String>)>> {
        let resp = self
            .handle
            .block_on(self.client.request(ListResources::new(collection_path)))
            .map_err(dav_err)?;
        let coll = collection_path.trim_end_matches('/');
        Ok(resp
            .resources
            .into_iter()
            .filter(|r| r.href.trim_end_matches('/') != coll)
            .map(|r| (r.href, r.etag))
            .collect())
    }

    fn get(&self, path: &str) -> Result<(Option<String>, String)> {
        let collection = parent_path(path);
        let resp = self
            .handle
            .block_on(self.client.request(GetCalendarResources::new(&collection).with_hrefs([path])))
            .map_err(dav_err)?;
        let fetched = resp
            .resources
            .into_iter()
            .next()
            .ok_or_else(|| Error::NotFound(format!("resource {path}")))?;
        match fetched.content {
            Ok(content) => Ok((Some(content.etag), content.data)),
            Err(status) => Err(Error::Other(format!("GET {path}: HTTP {status}"))),
        }
    }

    fn create(&self, path: &str, body: &str) -> Result<Option<String>> {
        let resp = self
            .handle
            .block_on(self.client.request(PutResource::new(path).create(body.to_string(), CALENDAR_MIME)))
            .map_err(dav_err)?;
        Ok(resp.etag)
    }

    fn update(&self, path: &str, body: &str, etag: &str) -> Result<Option<String>> {
        let resp = self
            .handle
            .block_on(self.client.request(PutResource::new(path).update(
                body.to_string(),
                CALENDAR_MIME,
                etag.to_string(),
            )))
            .map_err(dav_err)?;
        Ok(resp.etag)
    }

    fn delete(&self, path: &str, etag: Option<&str>) -> Result<()> {
        match etag {
            Some(etag) => self
                .handle
                .block_on(self.client.request(Delete::new(path).with_etag(etag.to_string())))
                .map(|_| ())
                .map_err(dav_err),
            None => self
                .handle
                .block_on(self.client.request(Delete::new(path).force()))
                .map(|_| ())
                .map_err(dav_err),
        }
    }
}

pub struct CalDavClient {
    origin: String,
    backend: Box<dyn Backend>,
    // Field order matters: `backend` (holding a runtime Handle) is dropped before `_rt`.
    _rt: Runtime,
}

impl CalDavClient {
    /// Build a client whose `base_url` may point at a collection; only its origin is used to
    /// resolve resource paths.
    pub fn new(base_url: impl AsRef<str>, auth: Auth) -> Result<Self> {
        let uri: Uri = base_url
            .as_ref()
            .parse()
            .map_err(|e| Error::Invalid(format!("bad base url: {e}")))?;
        let scheme = uri.scheme_str().ok_or_else(|| Error::Invalid("base url missing scheme".into()))?;
        let authority = uri.authority().ok_or_else(|| Error::Invalid("base url missing host".into()))?;
        let origin = format!("{scheme}://{authority}");
        let base: Uri = origin.parse().map_err(|e| Error::Invalid(format!("bad origin: {e}")))?;

        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .map_err(Error::Io)?;
        let handle = rt.handle().clone();

        let connector = HttpsConnectorBuilder::new()
            .with_webpki_roots()
            .https_or_http()
            .enable_http1()
            .build();
        let http = Client::builder(TokioExecutor::new()).build(connector);

        let backend: Box<dyn Backend> = match auth {
            Auth::None => Box::new(LibBackend {
                client: LibCalDav::new(WebDavClient::new(base, http)),
                handle,
            }),
            Auth::Basic { user, password } => {
                let svc = AddAuthorization::basic(http, &user, &password);
                Box::new(LibBackend {
                    client: LibCalDav::new(WebDavClient::new(base, svc)),
                    handle,
                })
            }
            Auth::Bearer { token } => {
                let svc = AddAuthorization::bearer(http, &token);
                Box::new(LibBackend {
                    client: LibCalDav::new(WebDavClient::new(base, svc)),
                    handle,
                })
            }
        };

        Ok(CalDavClient {
            origin,
            backend,
            _rt: rt,
        })
    }

    fn path_of(&self, href_or_url: &str) -> String {
        uri_path(href_or_url)
    }

    fn full(&self, path: &str) -> String {
        rebase(&self.origin, path)
    }

    /// Normalize any href/URL to a full URL on this client's origin.
    pub fn url_for(&self, href: &str) -> String {
        self.full(&self.path_of(href))
    }

    /// List a collection's resources with their etags.
    pub fn list(&self, collection_url: &str) -> Result<Vec<RemoteItem>> {
        let path = self.path_of(collection_url);
        Ok(self
            .backend
            .list(&path)?
            .into_iter()
            .map(|(p, etag)| RemoteItem {
                href: self.full(&p),
                etag,
                data: None,
            })
            .collect())
    }

    /// Fetch a single resource's body and etag.
    pub fn get(&self, href: &str) -> Result<RemoteItem> {
        let (etag, data) = self.backend.get(&self.path_of(href))?;
        Ok(RemoteItem {
            href: href.to_string(),
            etag,
            data: Some(data),
        })
    }

    /// Create a resource that does not yet exist.
    pub fn put_new(&self, href: &str, body: &str) -> Result<Option<String>> {
        self.backend.create(&self.path_of(href), body)
    }

    /// Create or update a resource; pass the current etag in `if_match` to update safely.
    pub fn put(&self, href: &str, body: &str, if_match: Option<&str>) -> Result<Option<String>> {
        let path = self.path_of(href);
        match if_match {
            Some(etag) => self.backend.update(&path, body, etag),
            None => self.backend.create(&path, body),
        }
    }

    /// Delete a resource, optionally guarded by an etag.
    pub fn delete(&self, href: &str, if_match: Option<&str>) -> Result<()> {
        self.backend.delete(&self.path_of(href), if_match)
    }
}

fn dav_err<E: std::fmt::Display>(e: E) -> Error {
    Error::Other(format!("caldav: {e}"))
}

/// Extract the server-absolute path (and query) from an href or full URL.
fn uri_path(s: &str) -> String {
    match s.parse::<Uri>() {
        Ok(uri) => {
            let pq = uri.path_and_query().map(|p| p.as_str()).unwrap_or("/");
            if pq.starts_with('/') {
                pq.to_string()
            } else {
                format!("/{pq}")
            }
        }
        Err(_) => {
            if s.starts_with('/') {
                s.to_string()
            } else {
                format!("/{s}")
            }
        }
    }
}

/// Combine an origin (`scheme://host`) with a path.
fn rebase(origin: &str, path: &str) -> String {
    if path.starts_with('/') {
        format!("{origin}{path}")
    } else {
        format!("{origin}/{path}")
    }
}

/// The collection path containing `path` (everything up to and including the last `/`).
fn parent_path(path: &str) -> String {
    match path.rfind('/') {
        Some(i) => path[..=i].to_string(),
        None => "/".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uri_path_extracts_path_from_full_url() {
        assert_eq!(uri_path("http://localhost:8080/cal/work/a.ics"), "/cal/work/a.ics");
    }

    #[test]
    fn uri_path_passes_through_absolute_paths() {
        assert_eq!(uri_path("/cal/work/a.ics"), "/cal/work/a.ics");
    }

    #[test]
    fn rebase_joins_origin_and_path() {
        assert_eq!(rebase("http://localhost:8080", "/x.ics"), "http://localhost:8080/x.ics");
    }

    #[test]
    fn parent_path_is_collection() {
        assert_eq!(parent_path("/cal/work/a.ics"), "/cal/work/");
        assert_eq!(parent_path("a.ics"), "/");
    }
}
