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

/// Sync one events collection with its remote CalDAV counterpart. The sync unit is the whole
/// series (master + `RECURRENCE-ID` overrides share one uid, one href, one multi-VEVENT body).
pub fn sync_events(
    client: &CalDavClient,
    collection_url: &str,
    store: &mut VdirStore,
    calendar: &str,
) -> Result<SyncReport> {
    let mut report = SyncReport::default();
    let series = crate::http::group_series(store.load_all()?.into_iter().filter(|e| e.calendar == calendar).collect());
    let local: Vec<LocalRef> = series
        .iter()
        .map(|c| LocalRef {
            uid: c[0].uid.clone(),
            href: c[0].sync.href.clone(),
            etag: c[0].sync.etag.clone(),
        })
        .collect();
    let remote = remote_refs(client, collection_url)?;

    for op in plan_sync(&local, &remote) {
        match op {
            SyncOp::Push(uid) => {
                if let Some(comps) = series.iter().find(|c| c[0].uid == uid) {
                    let href = href_for(collection_url, &uid);
                    let etag = client.put_new(&href, &mgmt_ical::series_to_ics(comps))?;
                    let mut ev = comps[0].clone();
                    ev.sync.href = Some(href);
                    ev.sync.etag = etag;
                    store.upsert(ev)?;
                    report.pushed += 1;
                }
            }
            SyncOp::Pull(href) => {
                let item = client.get(&href)?;
                if let Some(data) = item.data {
                    let mut comps = mgmt_ical::events_from_ics(&data, calendar)?;
                    let Some(master) = comps.first_mut() else { continue };
                    master.sync.href = Some(href);
                    master.sync.etag = item.etag;
                    let (master, overrides) = comps.split_first().expect("non-empty");
                    store.put_series(master, overrides)?;
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
