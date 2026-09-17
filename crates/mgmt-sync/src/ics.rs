//! Read-only ICS/webcal subscriptions: fetch a remote calendar and replace a local collection
//! with its contents. Refreshed wholesale (no reconcile) — the remote is authoritative.

use std::path::Path;
use std::time::Duration;

use mgmt_core::{Error, Result, Store};
use mgmt_domain::{Collection, Event};
use mgmt_ical::Component;
use mgmt_store::VdirStore;

/// A subscription feed is remote input: cap the body and the wait.
const MAX_FEED_BYTES: usize = 8 * 1024 * 1024;
const FETCH_TIMEOUT: Duration = Duration::from_secs(30);

/// Fetch an ICS document. `url` may be `webcal://`; anything not http(s) is rejected.
pub fn fetch_ics(url: &str) -> Result<String> {
    let url = mgmt_domain::normalize_feed_url(url)
        .ok_or_else(|| Error::Invalid(format!("not an http(s)/webcal URL: {url}")))?;
    let client = reqwest::blocking::Client::builder()
        .timeout(FETCH_TIMEOUT)
        .build()
        .map_err(|e| Error::Other(e.to_string()))?;
    let resp = client
        .get(&url)
        .header("accept", "text/calendar")
        .send()
        .map_err(|e| Error::Other(format!("fetching {url}: {e}")))?;
    if !resp.status().is_success() {
        return Err(Error::Other(format!("fetching {url}: HTTP {}", resp.status())));
    }
    // ponytail: the cap is checked after the body is buffered (reqwest blocking has no
    // streaming limit helper). Upgrade path if feeds get huge: read from `resp` via `Read::take`.
    let text = resp.text().map_err(|e| Error::Other(format!("reading {url}: {e}")))?;
    if text.len() > MAX_FEED_BYTES {
        return Err(Error::Other(format!("feed {url} is larger than {MAX_FEED_BYTES} bytes")));
    }
    Ok(text)
}

/// Make `calendar` contain exactly the `VEVENT`s in `ics` (events that no longer exist upstream
/// disappear). Returns how many events were stored.
pub fn replace_collection(store: &mut VdirStore, calendar: &str, ics: &str) -> Result<usize> {
    if calendar.is_empty() || !calendar.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_')) {
        return Err(Error::Invalid(format!("invalid calendar id '{calendar}'")));
    }
    let root = mgmt_ical::parse(ics)?;
    let mut comps: Vec<&Component> = Vec::new();
    collect_named(&root, "VEVENT", &mut comps);
    let events: Vec<Event> = comps
        .iter()
        .filter_map(|c| mgmt_ical::event_from_component(c, calendar).ok())
        .collect();

    store.ensure_collection(calendar)?;
    for file in mgmt_store::collect_files(&store.root().join(calendar), "ics")? {
        std::fs::remove_file(file)?;
    }
    for ev in &events {
        store.upsert(ev.clone())?;
    }
    Ok(events.len())
}

/// One refresh of a subscribed collection: fetch + replace.
pub fn refresh_subscription(store: &mut VdirStore, coll: &Collection) -> Result<usize> {
    let (url, _) = coll
        .subscription()
        .ok_or_else(|| Error::Invalid(format!("collection '{}' is not a subscription", coll.id)))?;
    let ics = fetch_ics(url)?;
    replace_collection(store, &coll.id, &ics)
}

/// Refresh every subscribed collection recorded for the vault at `vault_root`.
/// Returns `(collection id, result)` per subscription.
pub fn refresh_all(vault_root: &Path) -> Vec<(String, Result<usize>)> {
    let cols = mgmt_store::load_calendars(vault_root).unwrap_or_default();
    let mut store = VdirStore::new(mgmt_store::calendars_dir(vault_root));
    cols.iter()
        .filter(|c| c.is_read_only())
        .map(|c| (c.id.clone(), refresh_subscription(&mut store, c)))
        .collect()
}

fn collect_named<'a>(c: &'a Component, name: &str, out: &mut Vec<&'a Component>) {
    if c.name == name {
        out.push(c);
    }
    for child in &c.children {
        collect_named(child, name, out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(uids: &[&str]) -> String {
        let mut s = String::from("BEGIN:VCALENDAR\r\nVERSION:2.0\r\n");
        for uid in uids {
            s.push_str(&format!(
                "BEGIN:VEVENT\r\nUID:{uid}\r\nDTSTART:20260618T090000Z\r\nDTEND:20260618T100000Z\r\nSUMMARY:{uid}\r\nEND:VEVENT\r\n"
            ));
        }
        s.push_str("END:VCALENDAR\r\n");
        s
    }

    #[test]
    fn replace_mirrors_the_feed_and_drops_vanished_events() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = VdirStore::new(dir.path());
        assert_eq!(replace_collection(&mut store, "holidays", &doc(&["a", "b"])).unwrap(), 2);
        assert_eq!(store.load_all().unwrap().len(), 2);

        // Second refresh: "b" is gone upstream, so it must be gone locally too.
        assert_eq!(replace_collection(&mut store, "holidays", &doc(&["a"])).unwrap(), 1);
        let left = store.load_all().unwrap();
        assert_eq!(left.len(), 1);
        assert_eq!(left[0].uid.as_str(), "a");
    }

    #[test]
    fn rejects_traversal_in_the_calendar_id() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = VdirStore::new(dir.path());
        assert!(replace_collection(&mut store, "../evil", &doc(&["a"])).is_err());
    }

    #[test]
    fn fetch_rejects_non_http_schemes() {
        assert!(fetch_ics("file:///etc/passwd").is_err());
    }
}
