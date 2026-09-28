//! Read-only ICS/webcal subscriptions: fetch a remote calendar and replace a local collection
//! with its contents. Refreshed wholesale (no reconcile) — the remote is authoritative.

use std::io::Read;
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
    read_capped(resp, &url)
}

/// Read `resp`'s body capped at `MAX_FEED_BYTES`: `Read::take` stops the transfer one byte past
/// the cap, so an oversized feed errors before the rest is pulled off the wire and buffered.
fn read_capped(resp: reqwest::blocking::Response, url: &str) -> Result<String> {
    let mut buf = Vec::new();
    resp.take(MAX_FEED_BYTES as u64 + 1)
        .read_to_end(&mut buf)
        .map_err(|e| Error::Other(format!("reading {url}: {e}")))?;
    if buf.len() > MAX_FEED_BYTES {
        return Err(Error::Other(format!("feed {url} is larger than {MAX_FEED_BYTES} bytes")));
    }
    String::from_utf8(buf).map_err(|e| Error::Other(format!("feed {url} is not valid utf-8: {e}")))
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

    #[test]
    fn read_capped_stops_the_transfer_instead_of_buffering_an_oversized_body() {
        use std::io::Write;
        use std::net::TcpListener;

        // The server advertises 16x the cap and keeps writing until the client hangs up, so a
        // buffer-then-check reader would pull all of it down; the cap must cut the transfer off.
        let advertised = MAX_FEED_BYTES * 16;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut discard = [0u8; 1024];
            let _ = stream.read(&mut discard); // drain the request, ignore its contents
            let head = format!("HTTP/1.1 200 OK\r\nContent-Length: {advertised}\r\nConnection: close\r\n\r\n");
            let _ = stream.write_all(head.as_bytes());
            let chunk = vec![b'a'; 64 * 1024];
            let mut written = 0usize;
            while written < advertised {
                match stream.write(&chunk) {
                    Ok(0) | Err(_) => break, // client hung up: the cap did its job
                    Ok(n) => written += n,
                }
            }
            written
        });

        let url = format!("http://{addr}/feed.ics");
        let client = reqwest::blocking::Client::builder().build().unwrap();
        let resp = client.get(&url).send().unwrap();
        let err = read_capped(resp, &url).unwrap_err().to_string();
        assert!(err.contains("larger than"), "unexpected error: {err}");

        // The bound is loose (socket + client read-ahead buffers accept a few MiB past the cap),
        // but a buffer-then-check reader would have drained all `advertised` bytes.
        let written = server.join().unwrap();
        assert!(
            written < MAX_FEED_BYTES * 4,
            "server wrote {written} of {advertised} bytes: the client kept reading past the {MAX_FEED_BYTES}-byte cap"
        );
    }
}
