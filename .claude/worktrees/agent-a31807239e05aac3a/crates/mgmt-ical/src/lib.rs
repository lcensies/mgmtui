//! iCalendar (RFC 5545) serialization for mgmt — clean-room, covering the subset we need:
//! `VEVENT`, `VTODO`, `VALARM`, and the common `RRULE` parts.
//!
//! Public entry points:
//! - [`event_to_ics`] / [`event_from_ics`] — calendar events
//! - [`task_to_ics`] / [`task_from_ics`] — tasks projected to/from `VTODO`

mod parser;
mod rrule;
mod value;
mod vevent;
mod vtodo;

pub use parser::{Component, Prop, parse};
pub use rrule::{from_rrule, to_rrule};

use mgmt_core::Result;
use mgmt_domain::{Event, Task};

/// Serialize an event as a standalone `VCALENDAR` document for sending to a server.
pub fn event_to_ics(ev: &Event) -> String {
    vevent::to_ics(ev)
}

/// Serialize an event for the local vdir store, preserving sync metadata.
pub fn event_to_ics_local(ev: &Event) -> String {
    vevent::to_ics_local(ev)
}

/// Parse the first `VEVENT` in `input` into an event under `calendar`.
pub fn event_from_ics(input: &str, calendar: &str) -> Result<Event> {
    vevent::from_ics(input, calendar)
}

/// Serialize a task as a `VCALENDAR` document containing a single `VTODO`.
pub fn task_to_ics(task: &Task) -> String {
    vtodo::to_ics(task)
}

/// Parse the first `VTODO` in `input` into a task.
pub fn task_from_ics(input: &str) -> Result<Task> {
    vtodo::from_ics(input)
}

/// Map a parsed component (e.g. a `VEVENT` pulled out of a multi-item REPORT response)
/// into an event without re-serializing.
pub fn event_from_component(c: &Component, calendar: &str) -> Result<Event> {
    vevent::from_component(c, calendar)
}

/// Map a parsed `VTODO` component into a task.
pub fn task_from_component(c: &Component) -> Result<Task> {
    vtodo::from_component(c)
}
