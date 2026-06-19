//! `mgmt meta --json` — emit the task-metadata schema the editor/nvim completion plugin uses to
//! offer frontmatter field and value completions. Everything is derived from the live config +
//! vault so completions stay in sync with the user's actual workflow, projects, and tags.

use std::collections::BTreeSet;
use std::path::Path;

use mgmt_service::MgmtContext;

/// Build the schema document as compact JSON.
pub fn schema_json(ctx: &MgmtContext, root: &Path) -> String {
    let statuses = ctx.workflow().ids();
    let projects = ctx.projects();

    // Distinct areas and tags seen across the vault — handy completion values.
    let mut areas: BTreeSet<String> = BTreeSet::new();
    let mut tags: BTreeSet<String> = BTreeSet::new();
    for t in ctx.tasks() {
        if let Some(a) = &t.area {
            areas.insert(a.clone());
        }
        for tag in &t.tags {
            tags.insert(tag.clone());
        }
    }

    let tasks_dir = mgmt_store::tasks_dir(root);

    let fields = [
        "title", "status", "priority", "project", "area", "tags", "due", "scheduled", "reminders",
    ];
    let priorities = ["none", "low", "medium", "high"];
    let reminder_examples = ["1d", "2h", "30m"];

    let mut obj = String::from("{");
    push_str_field(&mut obj, "data_dir", &root.display().to_string());
    obj.push(',');
    push_str_field(&mut obj, "tasks_dir", &tasks_dir.display().to_string());
    obj.push(',');
    push_arr(&mut obj, "fields", fields.iter().map(|s| s.to_string()));
    obj.push(',');
    push_arr(&mut obj, "statuses", statuses.into_iter());
    obj.push(',');
    push_arr(&mut obj, "priorities", priorities.iter().map(|s| s.to_string()));
    obj.push(',');
    push_arr(&mut obj, "projects", projects.into_iter());
    obj.push(',');
    push_arr(&mut obj, "areas", areas.into_iter());
    obj.push(',');
    push_arr(&mut obj, "tags", tags.into_iter());
    obj.push(',');
    push_arr(&mut obj, "reminders", reminder_examples.iter().map(|s| s.to_string()));
    obj.push('}');
    obj
}

fn push_str_field(out: &mut String, key: &str, val: &str) {
    out.push_str(&quote(key));
    out.push(':');
    out.push_str(&quote(val));
}

fn push_arr(out: &mut String, key: &str, vals: impl Iterator<Item = String>) {
    out.push_str(&quote(key));
    out.push_str(":[");
    let items: Vec<String> = vals.map(|v| quote(&v)).collect();
    out.push_str(&items.join(","));
    out.push(']');
}

/// JSON-escape and quote a string.
fn quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}
