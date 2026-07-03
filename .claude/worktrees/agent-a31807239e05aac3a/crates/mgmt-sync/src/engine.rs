//! Sync orchestration: drive [`plan_sync`] against a `CalDavClient` and a local store.
//! Events sync via `VdirStore`, tasks via `VaultStore` (markdown <-> VTODO).

use mgmt_core::{Result, Store, Uid};
use mgmt_dav::CalDavClient;
use mgmt_store::{VaultStore, VdirStore};

use crate::reconcile::{LocalRef, RemoteRef, SyncOp, plan_sync};

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct SyncReport {
    pub pushed: usize,
    pub pulled: usize,
    pub deleted: usize,
}

fn href_for(collection_url: &str, uid: &Uid) -> String {
    format!("{}/{}.ics", collection_url.trim_end_matches('/'), uid.as_str())
}

fn remote_refs(client: &CalDavClient, collection_url: &str) -> Result<Vec<RemoteRef>> {
    Ok(client
        .list(collection_url)?
        .into_iter()
        .map(|i| RemoteRef {
            href: i.href,
            etag: i.etag,
        })
        .collect())
}

/// Sync one events collection with its remote CalDAV counterpart.
pub fn sync_events(
    client: &CalDavClient,
    collection_url: &str,
    store: &mut VdirStore,
    calendar: &str,
) -> Result<SyncReport> {
    let mut report = SyncReport::default();
    let events: Vec<_> = store.load_all()?.into_iter().filter(|e| e.calendar == calendar).collect();
    let local: Vec<LocalRef> = events
        .iter()
        .map(|e| LocalRef {
            uid: e.uid.clone(),
            href: e.sync.href.clone(),
            etag: e.sync.etag.clone(),
        })
        .collect();
    let remote = remote_refs(client, collection_url)?;

    for op in plan_sync(&local, &remote) {
        match op {
            SyncOp::Push(uid) => {
                if let Some(ev) = events.iter().find(|e| e.uid == uid) {
                    let href = href_for(collection_url, &uid);
                    let etag = client.put_new(&href, &mgmt_ical::event_to_ics(ev))?;
                    let mut ev = ev.clone();
                    ev.sync.href = Some(href);
                    ev.sync.etag = etag;
                    store.upsert(ev)?;
                    report.pushed += 1;
                }
            }
            SyncOp::Pull(href) => {
                let item = client.get(&href)?;
                if let Some(data) = item.data {
                    let mut ev = mgmt_ical::event_from_ics(&data, calendar)?;
                    ev.sync.href = Some(href);
                    ev.sync.etag = item.etag;
                    store.upsert(ev)?;
                    report.pulled += 1;
                }
            }
            SyncOp::DeleteLocal(uid) => {
                store.delete(&uid)?;
                report.deleted += 1;
            }
        }
    }
    Ok(report)
}

/// Sync the task vault with a remote CalDAV collection holding `VTODO`s.
pub fn sync_tasks(client: &CalDavClient, collection_url: &str, store: &mut VaultStore) -> Result<SyncReport> {
    let mut report = SyncReport::default();
    let tasks = store.load_all()?;
    let local: Vec<LocalRef> = tasks
        .iter()
        .map(|t| LocalRef {
            uid: t.uid.clone(),
            href: t.sync.href.clone(),
            etag: t.sync.etag.clone(),
        })
        .collect();
    let remote = remote_refs(client, collection_url)?;

    for op in plan_sync(&local, &remote) {
        match op {
            SyncOp::Push(uid) => {
                if let Some(task) = tasks.iter().find(|t| t.uid == uid) {
                    let href = href_for(collection_url, &uid);
                    let etag = client.put_new(&href, &mgmt_ical::task_to_ics(task))?;
                    let mut task = task.clone();
                    task.sync.href = Some(href);
                    task.sync.etag = etag;
                    store.upsert(task)?;
                    report.pushed += 1;
                }
            }
            SyncOp::Pull(href) => {
                let item = client.get(&href)?;
                if let Some(data) = item.data {
                    let mut task = mgmt_ical::task_from_ics(&data)?;
                    task.sync.href = Some(href);
                    task.sync.etag = item.etag;
                    store.upsert(task)?;
                    report.pulled += 1;
                }
            }
            SyncOp::DeleteLocal(uid) => {
                store.delete(&uid)?;
                report.deleted += 1;
            }
        }
    }
    Ok(report)
}
