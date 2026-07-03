//! The `/api/meta` payload: workflow columns, projects (with resolved colors), smart views, and
//! sort modes — everything the SPA needs to render its chrome without hard-coding the taxonomy.

use std::path::Path;

use serde_json::{json, Value};

use mgmt_domain::SortMode;
use mgmt_service::MgmtContext;

pub fn meta_json(ctx: &MgmtContext, root: &Path) -> Value {
    let wf = ctx.workflow();
    let statuses: Vec<Value> = wf
        .order()
        .iter()
        .map(|d| {
            json!({
                "id": d.id,
                "label": d.label,
                "kind": serde_json::to_value(d.kind).unwrap_or(Value::Null),
                "color": d.color.clone().unwrap_or_else(|| ctx.project_color(&d.id)),
            })
        })
        .collect();

    let projects: Vec<Value> = ctx
        .projects()
        .into_iter()
        .map(|p| {
            let color = ctx.project_color(&p);
            json!({ "name": p, "color": color })
        })
        .collect();

    // The user's configured smart lists (Config::views), not the hard-coded ALL set.
    let views: Vec<Value> =
        ctx.config().views().iter().map(|v| json!({ "id": v.id(), "label": v.label() })).collect();
    let sorts: Vec<Value> = SortMode::ALL.iter().map(|s| json!({ "id": s.id(), "label": s.label() })).collect();

    // Calendar display settings (CalendarCfg derives no Serialize — build it from its fields).
    let cal = ctx.config().calendar();
    let calendar = json!({
        "show_end_time": cal.show_end_time,
        "month_event_lines": cal.month_event_lines,
        "month_panel_style": cal.month_panel_style,
        "event_palette": cal.event_palette,
    });

    // Reminder prefills for the forms (ReminderOffset serializes as "1d"/"2h"; Alarm derives serde).
    let default_reminders = serde_json::to_value(ctx.config().reminder_defaults()).unwrap_or(json!([]));
    let event_alarm_defaults = serde_json::to_value(ctx.config().event_alarm_defaults()).unwrap_or(json!([]));

    json!({
        "data_root": root.display().to_string(),
        "statuses": statuses,
        "projects": projects,
        "views": views,
        "sorts": sorts,
        "calendar": calendar,
        "theme": ctx.config().theme_overrides(),
        "default_reminders": default_reminders,
        "event_alarm_defaults": event_alarm_defaults,
    })
}
