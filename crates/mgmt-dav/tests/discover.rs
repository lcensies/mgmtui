//! Discovery against a mock CalDAV server: the happy path must find the calendar and authenticate
//! every request, a server that only answers under a `/.well-known/caldav` redirect (and 401s on
//! its root, like Nextcloud) must still be discovered, and wrong credentials must still fail.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener};
use std::sync::{Arc, Mutex};

use mgmt_dav::Auth;

const EXPECTED_AUTH: &str = "Basic YWxpY2U6czNjcmV0"; // alice:s3cret

#[derive(Debug, Clone)]
struct Seen {
    #[allow(dead_code)] // shown by Debug in failure messages
    method: String,
    path: String,
    auth: Option<String>,
}

/// The CalDAV chain, served under `prefix` (empty = at the server root).
fn body_for(prefix: &str, path: &str, body: &str) -> Option<String> {
    let ms = |inner: String| {
        Some(format!(
            r#"<?xml version="1.0" encoding="utf-8"?><multistatus xmlns="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav">{inner}</multistatus>"#
        ))
    };
    let rest = path.strip_prefix(prefix)?;
    match rest {
        "/" => ms(format!(
            r#"<response><href>{prefix}/</href><propstat><prop><current-user-principal><href>{prefix}/principals/alice/</href></current-user-principal></prop><status>HTTP/1.1 200 OK</status></propstat></response>"#
        )),
        "/principals/alice/" => ms(format!(
            r#"<response><href>{prefix}/principals/alice/</href><propstat><prop><C:calendar-home-set><href>{prefix}/calendars/alice/</href></C:calendar-home-set></prop><status>HTTP/1.1 200 OK</status></propstat></response>"#
        )),
        "/calendars/alice/" => ms(format!(
            concat!(
                r#"<response><href>{prefix}/calendars/alice/</href><propstat><prop><resourcetype><collection/></resourcetype></prop><status>HTTP/1.1 200 OK</status></propstat></response>"#,
                r#"<response><href>{prefix}/calendars/alice/work/</href><propstat><prop><resourcetype><collection/><C:calendar/></resourcetype></prop><status>HTTP/1.1 200 OK</status></propstat></response>"#,
            ),
            prefix = prefix
        )),
        "/calendars/alice/work/" if body.contains("displayname") => ms(format!(
            r#"<response><href>{prefix}/calendars/alice/work/</href><propstat><prop><displayname>Work</displayname></prop><status>HTTP/1.1 200 OK</status></propstat></response>"#
        )),
        "/calendars/alice/work/" => ms(format!(
            r#"<response><href>{prefix}/calendars/alice/work/</href><propstat><prop><C:supported-calendar-component-set><C:comp name="VEVENT"/><C:comp name="VTODO"/></C:supported-calendar-component-set></prop><status>HTTP/1.1 200 OK</status></propstat></response>"#
        )),
        _ => None,
    }
}

/// A one-request-per-connection mock CalDAV server serving its chain under `prefix`. With a
/// non-empty prefix it behaves like Nextcloud: everything outside the prefix is a 401, and
/// `/.well-known/caldav` redirects into it with a relative `Location`.
fn spawn_server(prefix: &'static str) -> (SocketAddr, Arc<Mutex<Vec<Seen>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let log = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&log);

    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut r = BufReader::new(stream.try_clone().unwrap());

            let mut start = String::new();
            if r.read_line(&mut start).is_err() || start.trim().is_empty() {
                continue;
            }
            let mut it = start.split_whitespace();
            let method = it.next().unwrap_or_default().to_string();
            let path = it.next().unwrap_or_default().to_string();

            let (mut len, mut auth) = (0usize, None);
            loop {
                let mut line = String::new();
                if r.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" || line == "\n" {
                    break;
                }
                let (k, v) = line.split_once(':').unwrap_or(("", ""));
                match k.trim().to_ascii_lowercase().as_str() {
                    "content-length" => len = v.trim().parse().unwrap_or(0),
                    "authorization" => auth = Some(v.trim().to_string()),
                    _ => {}
                }
            }
            let mut body = vec![0u8; len];
            let _ = r.read_exact(&mut body);
            let body = String::from_utf8_lossy(&body).to_string();

            sink.lock().unwrap().push(Seen { method, path: path.clone(), auth: auth.clone() });

            let unauthorized = "HTTP/1.1 401 Unauthorized\r\nWWW-Authenticate: Basic realm=\"dav\"\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_string();
            let resp = if path == "/.well-known/caldav" && !prefix.is_empty() {
                format!("HTTP/1.1 301 Moved Permanently\r\nLocation: {prefix}/\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
            } else if auth.as_deref() != Some(EXPECTED_AUTH) {
                unauthorized
            } else {
                match body_for(prefix, &path, &body) {
                    Some(xml) => format!(
                        "HTTP/1.1 207 Multi-Status\r\nContent-Type: application/xml; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{xml}",
                        xml.len()
                    ),
                    // Outside the DAV context path this server authenticates nothing: a 401, not a
                    // 404 — exactly the response that used to abort discovery.
                    None => unauthorized,
                }
            };
            let _ = stream.write_all(resp.as_bytes());
            let _ = stream.flush();
        }
    });

    (addr, log)
}

fn alice() -> Auth {
    Auth::Basic { user: "alice".into(), password: "s3cret".into() }
}

#[test]
fn discovers_calendars_and_authenticates_every_request() {
    let (addr, log) = spawn_server("");
    let found = mgmt_dav::discover_calendars(format!("http://{addr}/"), alice())
        .expect("discovery should succeed against a correct server + credentials");

    assert_eq!(found.len(), 1, "found: {found:?}");
    assert_eq!(found[0].name, "Work");
    assert_eq!(found[0].url, format!("http://{addr}/calendars/alice/work/"));
    assert!(found[0].supports_events && found[0].supports_tasks);

    let seen = log.lock().unwrap();
    assert!(!seen.is_empty());
    for req in seen.iter() {
        assert_eq!(req.auth.as_deref(), Some(EXPECTED_AUTH), "unauthenticated request: {req:?}");
    }
}

#[test]
fn a_401_on_the_entered_url_falls_back_to_well_known() {
    let (addr, log) = spawn_server("/dav");
    let found = mgmt_dav::discover_calendars(format!("http://{addr}/"), alice())
        .expect("a 401 on the entered path must not end discovery");

    assert_eq!(found.len(), 1, "found: {found:?}");
    assert_eq!(found[0].url, format!("http://{addr}/dav/calendars/alice/work/"));

    let seen = log.lock().unwrap();
    assert!(
        seen.iter().any(|r| r.path == "/.well-known/caldav"),
        "discovery should have asked for the well-known URI: {seen:?}"
    );
}

#[test]
fn wrong_credentials_are_an_error() {
    let (addr, _log) = spawn_server("");
    let err = mgmt_dav::discover_calendars(
        format!("http://{addr}/"),
        Auth::Basic { user: "alice".into(), password: "wrong".into() },
    );
    assert!(err.is_err(), "a 401 from the server must not look like success");
}
