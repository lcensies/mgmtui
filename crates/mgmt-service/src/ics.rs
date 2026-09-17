//! ICS import/export over a [`MgmtContext`] — shared by `mgmt import`/`mgmt export` and the web
//! calendar upload/download routes.

use mgmt_core::Result;
use mgmt_ical::Component;

use crate::MgmtContext;

/// Import every `VEVENT` in `text` into `calendar`, returning how many were stored. Existing
/// events with the same uid are overwritten (an import is an upsert).
pub fn import_ics(ctx: &mut MgmtContext, text: &str, calendar: &str) -> Result<usize> {
    let root = mgmt_ical::parse(text)?;
    let mut vevents = Vec::new();
    collect_named(&root, "VEVENT", &mut vevents);
    let mut n = 0;
    for ve in vevents {
        ctx.put_event(mgmt_ical::event_from_component(ve, calendar)?)?;
        n += 1;
    }
    Ok(n)
}

/// Serialize the events of `calendar` (all calendars when `None`) as a concatenated ICS document.
pub fn export_ics(ctx: &MgmtContext, calendar: Option<&str>) -> String {
    ctx.events()
        .iter()
        .filter(|ev| calendar.map(|c| c == ev.calendar).unwrap_or(true))
        .map(mgmt_ical::event_to_ics)
        .collect()
}

fn collect_named<'a>(c: &'a Component, name: &str, out: &mut Vec<&'a Component>) {
    if c.name.eq_ignore_ascii_case(name) {
        out.push(c);
    }
    for child in &c.children {
        collect_named(child, name, out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mgmt_store::{VaultStore, VdirStore};

    #[test]
    fn imports_every_vevent_then_exports_them_back() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let mut ctx = MgmtContext::open(
            VaultStore::new(mgmt_store::tasks_dir(root)),
            VdirStore::new(mgmt_store::calendars_dir(root)),
        )
        .unwrap();

        let ics = (1..=3)
            .map(|i| {
                format!(
                    "BEGIN:VEVENT\r\nUID:e{i}\r\nSUMMARY:Ev {i}\r\nDTSTART:20260618T09{i}000Z\r\nDTEND:20260618T10{i}000Z\r\nEND:VEVENT\r\n"
                )
            })
            .collect::<String>();
        let doc = format!("BEGIN:VCALENDAR\r\nVERSION:2.0\r\n{ics}END:VCALENDAR\r\n");

        assert_eq!(import_ics(&mut ctx, &doc, "work").unwrap(), 3);
        assert_eq!(ctx.events().len(), 3);
        assert_eq!(export_ics(&ctx, Some("work")).matches("BEGIN:VEVENT").count(), 3);
        assert_eq!(export_ics(&ctx, Some("other")), "");
    }
}
