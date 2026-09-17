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

    // Open-task counts ride along so the sidebar can show them without a second round-trip.
    let open_ids = ctx.workflow().open_ids();
    let count_open = |project: Option<&str>| {
        ctx.tasks()
            .iter()
            .filter(|t| t.project.as_deref() == project && open_ids.iter().any(|s| s == &t.status))
            .count()
    };
    let projects: Vec<Value> = ctx
        .projects()
        .into_iter()
        .map(|p| {
            let color = ctx.project_color(&p);
            let open = count_open(Some(p.as_str()));
            json!({ "name": p, "color": color, "open": open })
        })
        .collect();
    let no_project_open = count_open(None);

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
        "week_start": cal.week_start,
        "hide_weekends": cal.hide_weekends,
        "work_hours": { "start": cal.work_hours.start, "end": cal.work_hours.end },
        "visible_hours": { "start": cal.visible_hours.start, "end": cal.visible_hours.end },
    });

    // Reminder prefills for the forms (ReminderOffset serializes as "1d"/"2h"; Alarm derives serde).
    let default_reminders = serde_json::to_value(ctx.config().reminder_defaults()).unwrap_or(json!([]));
    let event_alarm_defaults = serde_json::to_value(ctx.config().event_alarm_defaults()).unwrap_or(json!([]));

    json!({
        "data_root": root.display().to_string(),
        "statuses": statuses,
        "projects": projects,
        "no_project_open": no_project_open,
        "views": views,
        "sorts": sorts,
        "calendar": calendar,
        "theme": ctx.config().theme_overrides(),
        "default_reminders": default_reminders,
        "event_alarm_defaults": event_alarm_defaults,
    })
}
