//! CalDAV client for mgmt. A blocking facade over the `libdav` crate, so the sync engine
//! and CLI stay synchronous.

mod client;
mod discover;

pub use client::{Auth, CalDavClient, RemoteItem};
pub use discover::{discover_calendars, DiscoveredCalendar};
