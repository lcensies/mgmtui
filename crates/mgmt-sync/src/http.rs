//! Native sync client: reconcile the local vault against an `mgmt web` server's `/api/sync`
//! endpoints. Unlike the CalDAV path this ships raw markdown/iCalendar bytes (full fidelity, no
//! VTODO round-trip).
//!
//! It drives the three-way [`plan_sync3`] planner: a persisted *base* snapshot (the state at the
//! last successful sync) lets one poller push its own edits and pull the remote's in a single pass,
//! propagate deletes in both directions, and resolve a genuine both-sides edit by `modified` time.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::Deserialize;
use sha2::{Digest, Sha256};

use mgmt_core::{Error, Result, Store, Uid};
use mgmt_domain::{Event, SyncMeta, Task};
use mgmt_store::{safe_stem, VaultStore, VdirStore};

use crate::engine::SyncReport;
use crate::reconcile::{plan_sync3, BaseRef, LocalItem, RemoteRef, SyncOp3};

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

    /// `PUT base/<sub>/<href>` the body. `create` sends `If-None-Match: *` (fail if it exists);
    /// otherwise an `If-Match` is sent when `if_match` is set (fail if changed). Returns the new
    /// ETag.
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

    /// `DELETE base/<sub>/<href>`, guarded by `If-Match` when the caller knows the current ETag.
    fn delete(&self, sub: &str, href: &str, if_match: Option<&str>) -> Result<()> {
        let url = format!("{}/{}/{}", self.base, sub, href);
        let mut rb = self.auth(self.client.delete(&url));
        if let Some(m) = if_match {
            rb = rb.header(reqwest::header::IF_MATCH, m);
        }
        check(rb.send().map_err(net)?)?;
        Ok(())
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

/// Our own hash of a clean-serialized body — the local-change signal stored in the base snapshot.
/// It is intentionally independent of the server's ETag scheme; it only needs to be self-consistent
/// across passes.
fn body_hash(body: &str) -> String {
    let digest = Sha256::digest(body.as_bytes());
    let mut hex = String::with_capacity(digest.len() * 2);
    for b in digest {
        hex.push_str(&format!("{b:02x}"));
    }
    hex
}

/// The persisted three-way merge base for one collection.
struct BaseSnapshot {
    path: PathBuf,
    refs: Vec<BaseRef>,
}

impl BaseSnapshot {
    fn load(path: &Path) -> Self {
        let refs = std::fs::read_to_string(path)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default();
        BaseSnapshot { path: path.to_path_buf(), refs }
    }

    fn save(&self) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let text = serde_json::to_string_pretty(&self.refs)
            .map_err(|e| Error::Other(format!("serializing sync base: {e}")))?;
        std::fs::write(&self.path, text)?;
        Ok(())
    }

    fn upsert(&mut self, r: BaseRef) {
        if let Some(slot) = self.refs.iter_mut().find(|b| b.href == r.href) {
            *slot = r;
        } else {
            self.refs.push(r);
        }
    }

    fn remove(&mut self, href: &str) {
        self.refs.retain(|b| b.href != href);
    }
}

/// True only when `local` is present, `remote` is present, and `local` is strictly newer. Used to
/// resolve a both-sides conflict: when timestamps are missing or equal we fall back to remote-wins
/// (the remote — e.g. the web hub holding mobile edits — is the safer default).
fn local_wins(local: Option<DateTime<Utc>>, remote: Option<DateTime<Utc>>) -> bool {
    matches!((local, remote), (Some(l), Some(r)) if l > r)
}

/// Reconcile the task vault with the server (raw markdown bodies), three-way against `base_path`.
pub fn sync_tasks_http(remote: &HttpRemote, store: &mut VaultStore, base_path: &Path) -> Result<SyncReport> {
    let sub = "tasks";
    let mut report = SyncReport::default();
    let mut base = BaseSnapshot::load(base_path);
    let tasks = store.load_all()?;

    // Clean body = what we'd upload (no local sync bookkeeping); its hash is the local-change signal.
    let clean_body = |t: &Task| -> Result<String> {
        let mut c = t.clone();
        c.sync = SyncMeta::default();
        mgmt_markdown::serialize_task(&c)
    };
    let mut bodies: HashMap<Uid, String> = HashMap::new();
    let mut locals = Vec::with_capacity(tasks.len());
    for t in &tasks {
        let body = clean_body(t)?;
        locals.push(LocalItem {
            uid: t.uid.clone(),
            href: t.sync.href.clone(),
            etag: t.sync.etag.clone(),
            hash: body_hash(&body),
        });
        bodies.insert(t.uid.clone(), body);
    }
    let remote_refs = remote.list(sub)?;
    let remote_etag = |href: &str| remote_refs.iter().find(|r| r.href == href).and_then(|r| r.etag.clone());

    prepare_base(&mut base, &locals, &remote_refs);

    let mut first_err = None;
    for op in plan_sync3(&base.refs, &locals, &remote_refs) {
        let step = (|| -> Result<()> {
            match op {
                SyncOp3::PushCreate(uid) => {
                    let Some(t) = tasks.iter().find(|t| t.uid == uid) else { return Ok(()) };
                    let href = t.sync.href.clone().unwrap_or_else(|| format!("{}.md", safe_stem(uid.as_str())));
                    let body = &bodies[&uid];
                    let etag = remote.put(sub, &href, body, None, true)?;
                    stamp_task(store, t, &href, etag.clone())?;
                    base.upsert(BaseRef { uid, href, remote_etag: etag, local_hash: body_hash(body) });
                    report.pushed += 1;
                }
                SyncOp3::PushUpdate(uid, href) => {
                    let Some(t) = tasks.iter().find(|t| t.uid == uid) else { return Ok(()) };
                    let body = &bodies[&uid];
                    let etag = remote.put(sub, &href, body, remote_etag(&href).as_deref(), false)?;
                    stamp_task(store, t, &href, etag.clone())?;
                    base.upsert(BaseRef { uid, href, remote_etag: etag, local_hash: body_hash(body) });
                    report.pushed += 1;
                }
                SyncOp3::Pull(href) => {
                    let (uid, hash, etag) = pull_task(remote, store, sub, &href)?;
                    base.upsert(BaseRef { uid, href, remote_etag: etag, local_hash: hash });
                    report.pulled += 1;
                }
                SyncOp3::DeleteLocal(uid) => {
                    if let Some(t) = tasks.iter().find(|t| t.uid == uid) {
                        if let Some(href) = &t.sync.href {
                            base.remove(href);
                        }
                    }
                    store.delete(&uid)?;
                    report.deleted += 1;
                }
                SyncOp3::DeleteRemote(href) => {
                    remote.delete(sub, &href, remote_etag(&href).as_deref())?;
                    base.remove(&href);
                    report.deleted += 1;
                }
                SyncOp3::Conflict(uid, href) => {
                    let Some(t) = tasks.iter().find(|t| t.uid == uid) else { return Ok(()) };
                    let (body, etag) = remote.get(sub, &href)?;
                    let remote_mod = mgmt_markdown::parse_task(&body).ok().and_then(|r| r.modified);
                    if local_wins(t.modified, remote_mod) {
                        let local_body = &bodies[&uid];
                        let new_etag = remote.put(sub, &href, local_body, etag.as_deref(), false)?;
                        stamp_task(store, t, &href, new_etag.clone())?;
                        base.upsert(BaseRef { uid, href, remote_etag: new_etag, local_hash: body_hash(local_body) });
                        report.pushed += 1;
                    } else {
                        let mut task = mgmt_markdown::parse_task(&body)?;
                        task.sync = SyncMeta { href: Some(href.clone()), etag: etag.clone() };
                        let hash = body_hash(&clean_body(&task)?);
                        store.upsert(task)?;
                        base.upsert(BaseRef { uid, href, remote_etag: etag, local_hash: hash });
                        report.pulled += 1;
                    }
                }
            }
            Ok(())
        })();
        if let Err(e) = step {
            first_err = Some(e);
            break;
        }
    }

    // Persist the base even when an op failed mid-loop: ops that already reached the server must
    // be recorded, or the next pass sees its own pushes as spurious conflicts.
    base.save()?;
    match first_err {
        Some(e) => Err(e),
        None => Ok(report),
    }
}

/// Pre-pass base maintenance shared by the task and event drivers.
///
/// *Prune* entries whose href is gone from both sides — they can never match again, and a later
/// re-creation at the same uid-derived href with a coincidentally-equal etag would otherwise be
/// planned as a remote delete. Then *seed* entries for items that are in sync (matching etags)
/// but missing from the base — a vault migrating from two-way sync, or a lost/corrupt base file.
/// Without a base entry a later local edit has no change signal: it would never push and be
/// silently overwritten by the next remote edit.
fn prepare_base(base: &mut BaseSnapshot, locals: &[LocalItem], remote_refs: &[RemoteRef]) {
    let live: std::collections::HashSet<&str> = locals
        .iter()
        .filter_map(|l| l.href.as_deref())
        .chain(remote_refs.iter().map(|r| r.href.as_str()))
        .collect();
    base.refs.retain(|b| live.contains(b.href.as_str()));

    for l in locals {
        let Some(href) = l.href.as_deref() else { continue };
        if base.refs.iter().any(|b| b.href == href) {
            continue;
        }
        let Some(r) = remote_refs.iter().find(|r| r.href == href) else { continue };
        if l.etag.as_deref() == r.etag.as_deref() {
            base.upsert(BaseRef {
                uid: l.uid.clone(),
                href: href.to_string(),
                remote_etag: r.etag.clone(),
                local_hash: l.hash.clone(),
            });
        }
    }
}

/// Stamp a pushed task's sync meta (href/etag) back onto the stored file.
fn stamp_task(store: &mut VaultStore, t: &Task, href: &str, etag: Option<String>) -> Result<()> {
    let mut nt = t.clone();
    nt.sync.href = Some(href.to_string());
    nt.sync.etag = etag;
    store.upsert(nt)?;
    Ok(())
}

/// Download, store, and report a task; returns (uid, local_hash, etag) for the base snapshot.
fn pull_task(remote: &HttpRemote, store: &mut VaultStore, sub: &str, href: &str) -> Result<(Uid, String, Option<String>)> {
    let (body, etag) = remote.get(sub, href)?;
    let mut task = mgmt_markdown::parse_task(&body)?;
    let uid = task.uid.clone();
    task.sync = SyncMeta { href: Some(href.to_string()), etag: etag.clone() };
    let mut clean = task.clone();
    clean.sync = SyncMeta::default();
    let hash = body_hash(&mgmt_markdown::serialize_task(&clean)?);
    store.upsert(task)?;
    Ok((uid, hash, etag))
}

/// Reconcile one events collection with the server (raw iCalendar bodies), three-way.
pub fn sync_events_http(remote: &HttpRemote, store: &mut VdirStore, calendar: &str, base_path: &Path) -> Result<SyncReport> {
    let sub = format!("calendars/{calendar}");
    let mut report = SyncReport::default();
    let mut base = BaseSnapshot::load(base_path);
    // The sync unit is the *series*, not the component: one href holds the master and its
    // RECURRENCE-ID overrides, so they travel as one multi-VEVENT body in both directions.
    let series = group_series(store.load_all()?.into_iter().filter(|e| e.calendar == calendar).collect());
    let masters: Vec<&Event> = series.iter().map(|c| &c[0]).collect();

    let mut bodies: HashMap<Uid, String> = HashMap::new();
    let mut locals = Vec::with_capacity(series.len());
    for comps in &series {
        let master = &comps[0];
        let body = mgmt_ical::series_to_ics(comps); // already strips X-MGMT sync props
        locals.push(LocalItem {
            uid: master.uid.clone(),
            href: master.sync.href.clone(),
            etag: master.sync.etag.clone(),
            hash: body_hash(&body),
        });
        bodies.insert(master.uid.clone(), body);
    }
    let remote_refs = remote.list(&sub)?;
    let remote_etag = |href: &str| remote_refs.iter().find(|r| r.href == href).and_then(|r| r.etag.clone());

    prepare_base(&mut base, &locals, &remote_refs);

    let mut first_err = None;
    for op in plan_sync3(&base.refs, &locals, &remote_refs) {
        let step = (|| -> Result<()> {
            match op {
                SyncOp3::PushCreate(uid) => {
                    let Some(ev) = masters.iter().find(|e| e.uid == uid) else { return Ok(()) };
                    let href = ev.sync.href.clone().unwrap_or_else(|| format!("{}.ics", safe_stem(uid.as_str())));
                    let body = &bodies[&uid];
                    let etag = remote.put(&sub, &href, body, None, true)?;
                    stamp_event(store, ev, &href, etag.clone())?;
                    base.upsert(BaseRef { uid, href, remote_etag: etag, local_hash: body_hash(body) });
                    report.pushed += 1;
                }
                SyncOp3::PushUpdate(uid, href) => {
                    let Some(ev) = masters.iter().find(|e| e.uid == uid) else { return Ok(()) };
                    let body = &bodies[&uid];
                    let etag = remote.put(&sub, &href, body, remote_etag(&href).as_deref(), false)?;
                    stamp_event(store, ev, &href, etag.clone())?;
                    base.upsert(BaseRef { uid, href, remote_etag: etag, local_hash: body_hash(body) });
                    report.pushed += 1;
                }
                SyncOp3::Pull(href) => {
                    let (uid, hash, etag) = pull_event(remote, store, &sub, calendar, &href)?;
                    base.upsert(BaseRef { uid, href, remote_etag: etag, local_hash: hash });
                    report.pulled += 1;
                }
                SyncOp3::DeleteLocal(uid) => {
                    if let Some(ev) = masters.iter().find(|e| e.uid == uid) {
                        if let Some(href) = &ev.sync.href {
                            base.remove(href);
                        }
                    }
                    store.delete(&uid)?;
                    report.deleted += 1;
                }
                SyncOp3::DeleteRemote(href) => {
                    remote.delete(&sub, &href, remote_etag(&href).as_deref())?;
                    base.remove(&href);
                    report.deleted += 1;
                }
                SyncOp3::Conflict(uid, href) => {
                    let Some(ev) = masters.iter().find(|e| e.uid == uid) else { return Ok(()) };
                    let (body, etag) = remote.get(&sub, &href)?;
                    let remote_mod = mgmt_ical::event_from_ics(&body, calendar).ok().and_then(|r| r.modified);
                    if local_wins(ev.modified, remote_mod) {
                        let local_body = &bodies[&uid];
                        let new_etag = remote.put(&sub, &href, local_body, etag.as_deref(), false)?;
                        stamp_event(store, ev, &href, new_etag.clone())?;
                        base.upsert(BaseRef { uid, href, remote_etag: new_etag, local_hash: body_hash(local_body) });
                        report.pushed += 1;
                    } else {
                        let comps = mgmt_ical::events_from_ics(&body, calendar)?;
                        let (_, hash, _) = store_series(store, comps, &href, etag.clone())?;
                        base.upsert(BaseRef { uid, href, remote_etag: etag, local_hash: hash });
                        report.pulled += 1;
                    }
                }
            }
            Ok(())
        })();
        if let Err(e) = step {
            first_err = Some(e);
            break;
        }
    }

    // Persist the base even when an op failed mid-loop (see sync_tasks_http).
    base.save()?;
    match first_err {
        Some(e) => Err(e),
        None => Ok(report),
    }
}

fn stamp_event(store: &mut VdirStore, ev: &Event, href: &str, etag: Option<String>) -> Result<()> {
    let mut nev = ev.clone();
    nev.sync.href = Some(href.to_string());
    nev.sync.etag = etag;
    store.upsert(nev)?;
    Ok(())
}

fn pull_event(remote: &HttpRemote, store: &mut VdirStore, sub: &str, calendar: &str, href: &str) -> Result<(Uid, String, Option<String>)> {
    let (body, etag) = remote.get(sub, href)?;
    store_series(store, mgmt_ical::events_from_ics(&body, calendar)?, href, etag)
}

/// Replace the local series file with `comps` (master first), stamping the sync meta on the
/// master. Returns (uid, clean-body hash, etag) for the base snapshot.
fn store_series(
    store: &mut VdirStore,
    mut comps: Vec<Event>,
    href: &str,
    etag: Option<String>,
) -> Result<(Uid, String, Option<String>)> {
    let Some(master) = comps.first_mut() else {
        return Err(Error::Parse("no VEVENT in document".into()));
    };
    master.sync = SyncMeta { href: Some(href.to_string()), etag: etag.clone() };
    let uid = master.uid.clone();
    let hash = body_hash(&mgmt_ical::series_to_ics(&comps));
    let (master, overrides) = comps.split_first().expect("non-empty");
    store.put_series(master, overrides)?;
    Ok((uid, hash, etag))
}

/// Group loaded components into series keyed by uid, each master-first.
pub(crate) fn group_series(events: Vec<Event>) -> Vec<Vec<Event>> {
    let mut out: Vec<Vec<Event>> = Vec::new();
    let mut index: HashMap<Uid, usize> = HashMap::new();
    for e in events {
        match index.get(&e.uid) {
            Some(&i) => out[i].push(e),
            None => {
                index.insert(e.uid.clone(), out.len());
                out.push(vec![e]);
            }
        }
    }
    for comps in &mut out {
        comps.sort_by_key(|e| e.recurrence_id); // master (`None`) first
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use mgmt_domain::{Frequency, RecurrenceRule};

    #[test]
    fn a_series_travels_as_one_body_and_pulls_back_with_its_overrides() {
        let start = Utc.with_ymd_and_hms(2026, 6, 18, 9, 0, 0).unwrap();
        let mut master = Event::new("work", "Standup", start, start + chrono::Duration::hours(1));
        master.uid = Uid::from_string("s1");
        master.rrule = Some(RecurrenceRule::every(Frequency::Daily, 1));
        let mut over = master.clone();
        over.rrule = None;
        over.recurrence_id = Some(start + chrono::Duration::days(1));

        // Push side: the override does not get its own href — it rides in the master's body.
        let series = group_series(vec![over.clone(), master.clone()]);
        assert_eq!(series.len(), 1, "master and override are one sync unit");
        assert!(series[0][0].recurrence_id.is_none(), "master comes first");
        let body = mgmt_ical::series_to_ics(&series[0]);
        assert!(body.contains("RRULE"), "the pushed body keeps the series rule");
        assert!(body.contains("RECURRENCE-ID"), "the pushed body keeps the override");

        // Pull side: a multi-VEVENT body lands as one file holding both components.
        let dir = tempfile::tempdir().unwrap();
        let mut store = VdirStore::new(dir.path());
        let comps = mgmt_ical::events_from_ics(&body, "work").unwrap();
        let (uid, _, _) = store_series(&mut store, comps, "s1.ics", Some("e1".into())).unwrap();
        assert_eq!(uid.as_str(), "s1");
        let loaded = store.load_all().unwrap();
        assert_eq!(loaded.len(), 2);
        assert!(loaded.iter().any(|e| e.recurrence_id.is_some()), "the override survives the pull");
        assert_eq!(loaded[0].sync.href.as_deref(), Some("s1.ics"));
    }

    fn snap(refs: Vec<BaseRef>) -> BaseSnapshot {
        BaseSnapshot { path: PathBuf::new(), refs }
    }

    fn bref(uid: &str, href: &str, etag: &str, hash: &str) -> BaseRef {
        BaseRef {
            uid: Uid::from_string(uid),
            href: href.into(),
            remote_etag: Some(etag.into()),
            local_hash: hash.into(),
        }
    }

    #[test]
    fn prepare_base_seeds_in_sync_items_without_a_base_entry() {
        // An item synced before the 3-way era: href+etag match on both sides, no base entry.
        let locals = vec![LocalItem {
            uid: Uid::from_string("a"),
            href: Some("a.md".into()),
            etag: Some("e1".into()),
            hash: "h1".into(),
        }];
        let remotes = vec![RemoteRef { href: "a.md".into(), etag: Some("e1".into()) }];
        let mut base = snap(vec![]);
        prepare_base(&mut base, &locals, &remotes);
        assert_eq!(base.refs.len(), 1, "in-sync item is seeded so a later local edit can push");
        assert_eq!(base.refs[0].local_hash, "h1");
        assert_eq!(base.refs[0].remote_etag.as_deref(), Some("e1"));

        // Mismatched etags are NOT seeded — the planner's legacy remote-wins fallback handles them.
        let remotes2 = vec![RemoteRef { href: "a.md".into(), etag: Some("e2".into()) }];
        let mut base2 = snap(vec![]);
        prepare_base(&mut base2, &locals, &remotes2);
        assert!(base2.refs.is_empty());
    }

    #[test]
    fn prepare_base_prunes_hrefs_gone_from_both_sides() {
        let mut base = snap(vec![bref("a", "a.md", "e1", "h1"), bref("b", "b.md", "e2", "h2")]);
        let locals = vec![LocalItem {
            uid: Uid::from_string("a"),
            href: Some("a.md".into()),
            etag: Some("e1".into()),
            hash: "h1".into(),
        }];
        let remotes = vec![RemoteRef { href: "a.md".into(), etag: Some("e1".into()) }];
        prepare_base(&mut base, &locals, &remotes);
        assert_eq!(base.refs.len(), 1, "the both-sides-deleted b.md entry is pruned");
        assert_eq!(base.refs[0].href, "a.md");
    }
}
