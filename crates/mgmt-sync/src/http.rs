//! Native sync client: reconcile the local vault against an `mgmt web` server's `/api/sync`
//! endpoints. Unlike the CalDAV path this ships raw markdown/iCalendar bytes (full fidelity, no
//! VTODO round-trip), reusing the same pure [`plan_sync`] (href/etag, remote-wins) algorithm.

use serde::Deserialize;

use mgmt_core::{Error, Result, Store};
use mgmt_domain::SyncMeta;
use mgmt_store::{safe_stem, VaultStore, VdirStore};

use crate::engine::SyncReport;
use crate::reconcile::{plan_sync, LocalRef, RemoteRef, SyncOp};

/// A blocking HTTP client for the native sync protocol. `base` is the server's `/api/sync` URL.
pub struct HttpRemote {
    base: String,
    token: Option<String>,
    client: reqwest::blocking::Client,
}

#[derive(Deserialize)]
struct HrefEtag {
    href: String,
    etag: String,
}

impl HttpRemote {
    pub fn new(base: impl Into<String>, token: Option<String>) -> Result<Self> {
        let client = reqwest::blocking::Client::builder()
            .build()
            .map_err(|e| Error::Other(format!("building HTTP client: {e}")))?;
        Ok(HttpRemote { base: base.into().trim_end_matches('/').to_string(), token, client })
    }

    fn auth(&self, rb: reqwest::blocking::RequestBuilder) -> reqwest::blocking::RequestBuilder {
        match &self.token {
            Some(t) => rb.bearer_auth(t),
            None => rb,
        }
    }

    /// `GET base/<sub>` → the `[{href, etag}]` listing.
    fn list(&self, sub: &str) -> Result<Vec<RemoteRef>> {
        let url = format!("{}/{}", self.base, sub);
        let resp = self.auth(self.client.get(&url)).send().map_err(net)?;
        let resp = check(resp)?;
        let text = resp.text().map_err(net)?;
        let items: Vec<HrefEtag> =
            serde_json::from_str(&text).map_err(|e| Error::Parse(format!("parsing sync listing: {e}")))?;
        Ok(items.into_iter().map(|i| RemoteRef { href: i.href, etag: Some(i.etag) }).collect())
    }

    /// `GET base/<sub>/<href>` → the raw body + its ETag.
    fn get(&self, sub: &str, href: &str) -> Result<(String, Option<String>)> {
        let url = format!("{}/{}/{}", self.base, sub, href);
        let resp = self.auth(self.client.get(&url)).send().map_err(net)?;
        let resp = check(resp)?;
        let etag = header(&resp, reqwest::header::ETAG);
        let body = resp.text().map_err(net)?;
        Ok((body, etag))
    }

    /// `PUT base/<sub>/<href>` the body. `create` sends `If-None-Match: *`; otherwise an
    /// `If-Match` is sent when `if_match` is set. Returns the new ETag.
    fn put(&self, sub: &str, href: &str, body: &str, if_match: Option<&str>, create: bool) -> Result<Option<String>> {
        let url = format!("{}/{}/{}", self.base, sub, href);
        let mut rb = self.auth(self.client.put(&url)).body(body.to_string());
        if create {
            rb = rb.header(reqwest::header::IF_NONE_MATCH, "*");
        } else if let Some(m) = if_match {
            rb = rb.header(reqwest::header::IF_MATCH, m);
        }
        let resp = check(rb.send().map_err(net)?)?;
        Ok(header(&resp, reqwest::header::ETAG))
    }
}

fn net(e: reqwest::Error) -> Error {
    Error::Other(format!("sync request failed: {e}"))
}

fn check(resp: reqwest::blocking::Response) -> Result<reqwest::blocking::Response> {
    let status = resp.status();
    if status.is_success() {
        Ok(resp)
    } else if status == reqwest::StatusCode::PRECONDITION_FAILED {
        Err(Error::Conflict("etag precondition failed".into()))
    } else if status == reqwest::StatusCode::UNAUTHORIZED {
        Err(Error::Invalid("sync unauthorized — check the account token".into()))
    } else {
        Err(Error::Other(format!("sync server returned {status}")))
    }
}

fn header(resp: &reqwest::blocking::Response, name: reqwest::header::HeaderName) -> Option<String> {
    resp.headers().get(name).and_then(|v| v.to_str().ok()).map(|s| s.to_string())
}

/// Reconcile the task vault with the server (raw markdown bodies).
pub fn sync_tasks_http(remote: &HttpRemote, store: &mut VaultStore) -> Result<SyncReport> {
    let mut report = SyncReport::default();
    let tasks = store.load_all()?;
    let local: Vec<LocalRef> = tasks
        .iter()
        .map(|t| LocalRef { uid: t.uid.clone(), href: t.sync.href.clone(), etag: t.sync.etag.clone() })
        .collect();
    let remote_refs = remote.list("tasks")?;

    for op in plan_sync(&local, &remote_refs) {
        match op {
            SyncOp::Push(uid) => {
                if let Some(task) = tasks.iter().find(|t| t.uid == uid) {
                    let href = format!("{}.md", safe_stem(uid.as_str()));
                    let mut clean = task.clone();
                    clean.sync = SyncMeta::default(); // never upload local sync bookkeeping
                    let body = mgmt_markdown::serialize_task(&clean)?;
                    let etag = remote.put("tasks", &href, &body, None, true)?;
                    let mut t = task.clone();
                    t.sync.href = Some(href);
                    t.sync.etag = etag;
                    store.upsert(t)?;
                    report.pushed += 1;
                }
            }
            SyncOp::Pull(href) => {
                let (body, etag) = remote.get("tasks", &href)?;
                let mut task = mgmt_markdown::parse_task(&body)?;
                task.sync.href = Some(href);
                task.sync.etag = etag;
                store.upsert(task)?;
                report.pulled += 1;
            }
            SyncOp::DeleteLocal(uid) => {
                store.delete(&uid)?;
                report.deleted += 1;
            }
        }
    }
    Ok(report)
}

/// Reconcile one events collection with the server (raw iCalendar bodies).
pub fn sync_events_http(remote: &HttpRemote, store: &mut VdirStore, calendar: &str) -> Result<SyncReport> {
    let mut report = SyncReport::default();
    let events: Vec<_> = store.load_all()?.into_iter().filter(|e| e.calendar == calendar).collect();
    let local: Vec<LocalRef> = events
        .iter()
        .map(|e| LocalRef { uid: e.uid.clone(), href: e.sync.href.clone(), etag: e.sync.etag.clone() })
        .collect();
    let sub = format!("calendars/{calendar}");
    let remote_refs = remote.list(&sub)?;

    for op in plan_sync(&local, &remote_refs) {
        match op {
            SyncOp::Push(uid) => {
                if let Some(ev) = events.iter().find(|e| e.uid == uid) {
                    let href = format!("{}.ics", safe_stem(uid.as_str()));
                    let body = mgmt_ical::event_to_ics(ev); // clean: strips X-MGMT sync props
                    let etag = remote.put(&sub, &href, &body, None, true)?;
                    let mut ev = ev.clone();
                    ev.sync.href = Some(href);
                    ev.sync.etag = etag;
                    store.upsert(ev)?;
                    report.pushed += 1;
                }
            }
            SyncOp::Pull(href) => {
                let (body, etag) = remote.get(&sub, &href)?;
                let mut ev = mgmt_ical::event_from_ics(&body, calendar)?;
                ev.sync.href = Some(href);
                ev.sync.etag = etag;
                store.upsert(ev)?;
                report.pulled += 1;
            }
            SyncOp::DeleteLocal(uid) => {
                store.delete(&uid)?;
                report.deleted += 1;
            }
        }
    }
    Ok(report)
}
