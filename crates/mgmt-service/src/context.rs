//! `MgmtContext` — the service layer the TUI and CLI talk to. Owns the local stores, an
//! in-memory cache, dirty tracking for sync, and undo/redo.

use std::path::PathBuf;

use chrono::{DateTime, Duration, NaiveDate, Utc};

use mgmt_config::Config;
use mgmt_core::{Result, Store, Uid};
use mgmt_domain::{Event, Filter, Project, SortMode, Task, Workflow};
use mgmt_store::{ProjectStore, VaultStore, VdirStore};

/// A reversible record of one item's state: `Some` means "this is the content", `None` means
/// "this item is absent". Applying a snapshot returns the inverse, enabling undo/redo.
enum Snapshot {
    Task(Uid, Box<Option<Task>>),
    Event(Uid, Box<Option<Event>>),
}

pub struct MgmtContext {
    tasks: VaultStore,
    events: VdirStore,
    config: Config,
    workflow: Workflow,
    task_cache: Vec<Task>,
    event_cache: Vec<Event>,
    projects: ProjectStore,
    project_cache: Vec<Project>,
    undo_stack: Vec<Snapshot>,
    redo_stack: Vec<Snapshot>,
    dirty: bool,
}

impl MgmtContext {
    /// Open a context over the given stores with default configuration.
    pub fn open(tasks: VaultStore, events: VdirStore) -> Result<Self> {
        Self::open_with(tasks, events, Config::default())
    }

    /// Open a context over the given stores and config, loading everything into memory.
    pub fn open_with(tasks: VaultStore, events: VdirStore, config: Config) -> Result<Self> {
        let task_cache = tasks.load_all()?;
        let event_cache = events.load_all()?;
        // The project registry lives alongside the tasks/calendars dirs, under the data root.
        let data_root = tasks
            .root()
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| tasks.root().to_path_buf());
        let projects = ProjectStore::new(mgmt_store::projects_dir(&data_root));
        let project_cache = projects.load_all()?;
        let workflow = config.workflow();
        Ok(MgmtContext {
            tasks,
            events,
            config,
            workflow,
            task_cache,
            event_cache,
            projects,
            project_cache,
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            dirty: false,
        })
    }

    // ---- config / workflow ---------------------------------------------------------

    pub fn config(&self) -> &Config {
        &self.config
    }

    /// The active task workflow (statuses / kanban columns).
    pub fn workflow(&self) -> &Workflow {
        &self.workflow
    }

    /// A known project by name, if it has a backing `.md` file.
    pub fn project(&self, name: &str) -> Option<&Project> {
        self.project_cache.iter().find(|p| p.name == name)
    }

    /// Resolve a project's display color. Precedence: the project's own `.md` color (portable),
    /// then a config override, then a stable auto-assigned color.
    pub fn project_color(&self, project: &str) -> String {
        if let Some(c) = self.project(project).and_then(|p| p.color.clone()) {
            return c;
        }
        if let Some(c) = self.config.project_color_override(project) {
            return c;
        }
        mgmt_domain::auto_color(project).to_string()
    }

    /// On-disk path of a project's markdown file (for `$EDITOR` integration).
    pub fn project_file(&self, name: &str) -> PathBuf {
        self.projects.project_path(name)
    }

    // ---- queries -------------------------------------------------------------------

    pub fn tasks(&self) -> &[Task] {
        &self.task_cache
    }

    pub fn events(&self) -> &[Event] {
        &self.event_cache
    }

    pub fn task(&self, uid: &Uid) -> Option<&Task> {
        self.task_cache.iter().find(|t| &t.uid == uid)
    }

    pub fn event(&self, uid: &Uid) -> Option<&Event> {
        self.event_cache.iter().find(|e| &e.uid == uid)
    }

    /// Tasks matching `filter`, ordered by `sort`.
    pub fn filtered_tasks(&self, filter: &Filter, sort: SortMode) -> Vec<Task> {
        let mut out: Vec<Task> = self.task_cache.iter().filter(|t| filter.matches(t)).cloned().collect();
        sort.apply(&mut out);
        out
    }

    /// The kanban board: every workflow column (by status id) with its tasks (filtered, if
    /// given). Any status id present on a task but absent from the workflow is appended as a
    /// trailing column so renamed/custom statuses never silently vanish.
    pub fn board(&self, filter: &Filter) -> Vec<(String, Vec<Task>)> {
        let mut columns: Vec<String> = self.workflow.ids();
        for t in &self.task_cache {
            if !columns.iter().any(|c| c == &t.status) {
                columns.push(t.status.clone());
            }
        }
        columns
            .into_iter()
            .map(|status| {
                let col: Vec<Task> = self
                    .task_cache
                    .iter()
                    .filter(|t| t.status == status && filter.matches(t))
                    .cloned()
                    .collect();
                (status, col)
            })
            .collect()
    }

    /// The display label for a status id (workflow label, or the id itself if unknown).
    pub fn status_label<'a>(&'a self, id: &'a str) -> &'a str {
        self.workflow.label(id)
    }

    /// Events overlapping the given UTC day.
    pub fn events_on(&self, day: NaiveDate) -> Vec<Event> {
        let (from, to) = day_bounds(day);
        self.events_in_range(from, to)
    }

    /// Events (expanded across recurrences) overlapping the half-open window `[from, to)`.
    pub fn events_in_range(&self, from: DateTime<Utc>, to: DateTime<Utc>) -> Vec<Event> {
        let mut out: Vec<Event> = self.event_cache.iter().flat_map(|e| e.occurrences_in(from, to)).collect();
        out.sort_by_key(|e| e.start);
        out
    }

    /// Tasks scheduled/due on the given UTC day (for the calendar's task overlay).
    pub fn tasks_on(&self, day: NaiveDate) -> Vec<Task> {
        let (from, to) = day_bounds(day);
        let mut out: Vec<Task> = self
            .task_cache
            .iter()
            .filter(|t| t.calendar_date().map(|d| d >= from && d < to).unwrap_or(false))
            .cloned()
            .collect();
        out.sort_by_key(|t| t.calendar_date());
        out
    }

    /// Task + event reminders due to fire at `now` and not already in `fired` (see
    /// [`crate::reminders`]).
    pub fn pending_reminders(&self, now: DateTime<Utc>, fired: &std::collections::HashSet<String>) -> Vec<crate::ReminderHit> {
        crate::reminders::pending(&self.task_cache, &self.event_cache, now, fired)
    }

    /// The reminder offsets configured as defaults for newly-dated tasks.
    pub fn default_reminders(&self) -> Vec<mgmt_domain::ReminderOffset> {
        self.config.reminder_defaults().to_vec()
    }

    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    pub fn mark_clean(&mut self) {
        self.dirty = false;
    }

    pub fn can_undo(&self) -> bool {
        !self.undo_stack.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo_stack.is_empty()
    }

    // ---- mutations -----------------------------------------------------------------

    /// Insert or update a task.
    pub fn put_task(&mut self, task: Task) -> Result<()> {
        self.record(Snapshot::Task(task.uid.clone(), Box::new(Some(task))))
    }

    pub fn delete_task(&mut self, uid: &Uid) -> Result<()> {
        self.record(Snapshot::Task(uid.clone(), Box::new(None)))
    }

    /// Quick-add a task to the inbox (or a project), returning its UID. The task starts in the
    /// workflow's first column.
    pub fn quick_add(&mut self, title: impl Into<String>, project: Option<String>) -> Result<Uid> {
        let mut t = Task::new(title);
        t.status = self.workflow.default_id().to_string();
        t.project = project;
        t.created = Some(Utc::now());
        let uid = t.uid.clone();
        self.put_task(t)?;
        Ok(uid)
    }

    /// Move a task to a different kanban column (by status id).
    pub fn set_task_status(&mut self, uid: &Uid, status: impl Into<String>) -> Result<()> {
        let mut t = self
            .task(uid)
            .cloned()
            .ok_or_else(|| mgmt_core::Error::NotFound(format!("task {uid}")))?;
        t.status = status.into();
        self.put_task(t)
    }

    /// Toggle a task between "done" and the workflow's default column. Returns the new status.
    pub fn toggle_task_done(&mut self, uid: &Uid) -> Result<String> {
        let cur = self
            .task(uid)
            .map(|t| t.status.clone())
            .ok_or_else(|| mgmt_core::Error::NotFound(format!("task {uid}")))?;
        let new = if self.workflow.is_done(&cur) {
            self.workflow.default_id().to_string()
        } else {
            self.workflow.first_done().unwrap_or("done").to_string()
        };
        self.set_task_status(uid, new.clone())?;
        Ok(new)
    }

    /// Move a task one column left (`-1`) or right (`+1`) in the workflow. Returns the new
    /// status id (unchanged at the ends).
    pub fn move_task(&mut self, uid: &Uid, dir: i32) -> Result<String> {
        let cur = self
            .task(uid)
            .map(|t| t.status.clone())
            .ok_or_else(|| mgmt_core::Error::NotFound(format!("task {uid}")))?;
        let new = self.workflow.neighbor(&cur, dir).unwrap_or(&cur).to_string();
        self.set_task_status(uid, new.clone())?;
        Ok(new)
    }

    // ---- projects ------------------------------------------------------------------

    /// All known project names: those with a `.md` file plus any referenced by a task or event,
    /// sorted and de-duplicated.
    pub fn projects(&self) -> Vec<String> {
        let mut set: Vec<String> = self.project_cache.iter().map(|p| p.name.clone()).collect();
        for t in &self.task_cache {
            if let Some(p) = &t.project {
                set.push(p.clone());
            }
        }
        for e in &self.event_cache {
            if let Some(p) = &e.project {
                set.push(p.clone());
            }
        }
        set.sort();
        set.dedup();
        set
    }

    /// Register a project (creating its `.md` even with no tasks). Persisted immediately.
    pub fn add_project(&mut self, name: impl Into<String>) -> Result<()> {
        let name = name.into();
        if name.trim().is_empty() {
            return Err(mgmt_core::Error::Invalid("empty project name".into()));
        }
        if !self.project_cache.iter().any(|p| p.name == name) {
            let project = Project::new(name);
            self.projects.upsert(&project)?;
            self.project_cache.push(project);
        }
        Ok(())
    }

    /// Remove a project: delete its `.md` and unassign it from every task and event that
    /// references it (those move back to the inbox). This is a deliberate, confirmed action, so
    /// it bypasses the undo stack and clears it to keep history consistent.
    pub fn delete_project(&mut self, name: &str) -> Result<()> {
        for t in &mut self.task_cache {
            if t.project.as_deref() == Some(name) {
                t.project = None;
                self.tasks.upsert(t.clone())?;
            }
        }
        for e in &mut self.event_cache {
            if e.project.as_deref() == Some(name) {
                e.project = None;
                self.events.upsert(e.clone())?;
            }
        }
        self.projects.delete(name)?;
        self.project_cache.retain(|p| p.name != name);
        self.undo_stack.clear();
        self.redo_stack.clear();
        self.dirty = true;
        Ok(())
    }

    /// Assign (or clear, with `None`) a task's project. Registers the project too. Undoable.
    pub fn set_task_project(&mut self, uid: &Uid, project: Option<String>) -> Result<()> {
        let mut t = self
            .task(uid)
            .cloned()
            .ok_or_else(|| mgmt_core::Error::NotFound(format!("task {uid}")))?;
        if let Some(p) = &project {
            self.add_project(p.clone())?;
        }
        t.project = project;
        self.put_task(t)
    }

    /// Cycle a task's priority None → Low → Medium → High → None. Undoable.
    pub fn cycle_task_priority(&mut self, uid: &Uid) -> Result<()> {
        use mgmt_domain::Priority;
        let mut t = self
            .task(uid)
            .cloned()
            .ok_or_else(|| mgmt_core::Error::NotFound(format!("task {uid}")))?;
        t.priority = match t.priority {
            Priority::None => Priority::Low,
            Priority::Low => Priority::Medium,
            Priority::Medium => Priority::High,
            Priority::High => Priority::None,
        };
        self.put_task(t)
    }

    // ---- $EDITOR integration -------------------------------------------------------

    /// On-disk path of a task's markdown file, if the task exists.
    pub fn task_file(&self, uid: &Uid) -> Option<PathBuf> {
        self.task(uid).map(|_| self.tasks.task_path(uid))
    }

    /// On-disk path of an event's `.ics` file, if it exists.
    pub fn event_file(&self, uid: &Uid) -> Result<Option<PathBuf>> {
        self.events.find_path(uid)
    }

    /// Reload all caches from disk (e.g. after an external `$EDITOR` edit). Undo history is
    /// preserved but no longer applies to the reloaded items.
    pub fn reload(&mut self) -> Result<()> {
        self.task_cache = self.tasks.load_all()?;
        self.event_cache = self.events.load_all()?;
        self.project_cache = self.projects.load_all()?;
        Ok(())
    }

    pub fn put_event(&mut self, event: Event) -> Result<()> {
        self.record(Snapshot::Event(event.uid.clone(), Box::new(Some(event))))
    }

    pub fn delete_event(&mut self, uid: &Uid) -> Result<()> {
        self.record(Snapshot::Event(uid.clone(), Box::new(None)))
    }

    /// Reschedule an event by `delta`, preserving its duration. Drives the TUI's
    /// move-event-up/down action.
    pub fn reschedule_event(&mut self, uid: &Uid, delta: Duration) -> Result<()> {
        let mut e = self
            .event(uid)
            .cloned()
            .ok_or_else(|| mgmt_core::Error::NotFound(format!("event {uid}")))?;
        e.shift(delta);
        self.put_event(e)
    }

    /// Nudge an event's start by `delta`, keeping its end fixed (so the duration changes). A
    /// no-op if it would push the start to or past the end.
    pub fn adjust_event_start(&mut self, uid: &Uid, delta: Duration) -> Result<()> {
        let mut e = self
            .event(uid)
            .cloned()
            .ok_or_else(|| mgmt_core::Error::NotFound(format!("event {uid}")))?;
        let new_start = e.start + delta;
        if new_start < e.end {
            e.start = new_start;
            self.put_event(e)
        } else {
            Ok(())
        }
    }

    /// Nudge an event's end by `delta`, keeping its start fixed. A no-op if it would push the end
    /// to or before the start.
    pub fn adjust_event_end(&mut self, uid: &Uid, delta: Duration) -> Result<()> {
        let mut e = self
            .event(uid)
            .cloned()
            .ok_or_else(|| mgmt_core::Error::NotFound(format!("event {uid}")))?;
        let new_end = e.end + delta;
        if new_end > e.start {
            e.end = new_end;
            self.put_event(e)
        } else {
            Ok(())
        }
    }

    pub fn undo(&mut self) -> Result<bool> {
        match self.undo_stack.pop() {
            Some(snap) => {
                let inverse = self.apply(snap)?;
                self.redo_stack.push(inverse);
                Ok(true)
            }
            None => Ok(false),
        }
    }

    pub fn redo(&mut self) -> Result<bool> {
        match self.redo_stack.pop() {
            Some(snap) => {
                let inverse = self.apply(snap)?;
                self.undo_stack.push(inverse);
                Ok(true)
            }
            None => Ok(false),
        }
    }

    // ---- internals -----------------------------------------------------------------

    /// Apply a mutation and push its inverse onto the undo stack, clearing redo history.
    fn record(&mut self, snap: Snapshot) -> Result<()> {
        let inverse = self.apply(snap)?;
        self.undo_stack.push(inverse);
        self.redo_stack.clear();
        Ok(())
    }

    /// Apply `snap` to cache + store and return the inverse snapshot (the prior state).
    fn apply(&mut self, snap: Snapshot) -> Result<Snapshot> {
        self.dirty = true;
        match snap {
            Snapshot::Task(uid, val) => {
                let prev = self.task_cache.iter().find(|t| t.uid == uid).cloned();
                match *val {
                    Some(t) => {
                        self.tasks.upsert(t.clone())?;
                        upsert_into(&mut self.task_cache, t, |x| &x.uid);
                    }
                    None => {
                        self.tasks.delete(&uid)?;
                        self.task_cache.retain(|t| t.uid != uid);
                    }
                }
                Ok(Snapshot::Task(uid, Box::new(prev)))
            }
            Snapshot::Event(uid, val) => {
                let prev = self.event_cache.iter().find(|e| e.uid == uid).cloned();
                match *val {
                    Some(e) => {
                        self.events.upsert(e.clone())?;
                        upsert_into(&mut self.event_cache, e, |x| &x.uid);
                    }
                    None => {
                        self.events.delete(&uid)?;
                        self.event_cache.retain(|e| e.uid != uid);
                    }
                }
                Ok(Snapshot::Event(uid, Box::new(prev)))
            }
        }
    }
}

/// Replace an item with the same id in `vec`, or push it if absent.
fn upsert_into<T>(vec: &mut Vec<T>, item: T, id: impl Fn(&T) -> &Uid) {
    let target = id(&item).clone();
    if let Some(slot) = vec.iter_mut().find(|x| *id(x) == target) {
        *slot = item;
    } else {
        vec.push(item);
    }
}

/// Half-open UTC bounds `[midnight, next midnight)` for a calendar day.
fn day_bounds(day: NaiveDate) -> (DateTime<Utc>, DateTime<Utc>) {
    let from = day.and_hms_opt(0, 0, 0).unwrap().and_utc();
    (from, from + Duration::days(1))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Timelike};

    fn ctx() -> MgmtContext {
        let dir = tempfile::tempdir().unwrap();
        let vault = VaultStore::new(dir.path().join("tasks"));
        let vdir = VdirStore::new(dir.path().join("calendars"));
        // leak the tempdir so files persist for the test's lifetime
        std::mem::forget(dir);
        MgmtContext::open(vault, vdir).unwrap()
    }

    #[test]
    fn quick_add_then_board_places_in_todo() {
        let mut c = ctx();
        c.quick_add("Buy milk", Some("home".into())).unwrap();
        let board = c.board(&Filter::default());
        let todo = board.iter().find(|(s, _)| s == "todo").unwrap();
        assert_eq!(todo.1.len(), 1);
        assert_eq!(todo.1[0].project.as_deref(), Some("home"));
    }

    #[test]
    fn status_change_moves_columns_and_undoes() {
        let mut c = ctx();
        let uid = c.quick_add("ship it", None).unwrap();
        c.set_task_status(&uid, "done").unwrap();
        assert_eq!(c.task(&uid).unwrap().status, "done");
        assert!(c.undo().unwrap());
        assert_eq!(c.task(&uid).unwrap().status, "todo");
        assert!(c.redo().unwrap());
        assert_eq!(c.task(&uid).unwrap().status, "done");
    }

    #[test]
    fn toggle_and_move_use_the_workflow() {
        let mut c = ctx();
        let uid = c.quick_add("x", None).unwrap();
        assert_eq!(c.toggle_task_done(&uid).unwrap(), "done");
        assert_eq!(c.toggle_task_done(&uid).unwrap(), "todo");
        assert_eq!(c.move_task(&uid, 1).unwrap(), "doing");
        assert_eq!(c.move_task(&uid, -1).unwrap(), "todo");
        assert_eq!(c.move_task(&uid, -1).unwrap(), "todo"); // clamps at the left edge
    }

    #[test]
    fn delete_task_is_undoable() {
        let mut c = ctx();
        let uid = c.quick_add("temp", None).unwrap();
        c.delete_task(&uid).unwrap();
        assert!(c.task(&uid).is_none());
        c.undo().unwrap();
        assert!(c.task(&uid).is_some());
    }

    #[test]
    fn reschedule_shifts_event_and_persists() {
        let mut c = ctx();
        let e = Event::new(
            "work",
            "standup",
            Utc.with_ymd_and_hms(2026, 6, 18, 9, 0, 0).unwrap(),
            Utc.with_ymd_and_hms(2026, 6, 18, 9, 30, 0).unwrap(),
        );
        let uid = e.uid.clone();
        c.put_event(e).unwrap();
        c.reschedule_event(&uid, Duration::hours(1)).unwrap();
        assert_eq!(c.event(&uid).unwrap().start.hour(), 10);
    }

    #[test]
    fn adjust_start_and_end_change_only_one_edge_and_clamp() {
        let mut c = ctx();
        let e = Event::new(
            "work",
            "mtg",
            Utc.with_ymd_and_hms(2026, 6, 18, 9, 0, 0).unwrap(),
            Utc.with_ymd_and_hms(2026, 6, 18, 10, 0, 0).unwrap(),
        );
        let uid = e.uid.clone();
        c.put_event(e).unwrap();
        c.adjust_event_end(&uid, Duration::minutes(30)).unwrap();
        assert_eq!(c.event(&uid).unwrap().end.minute(), 30);
        assert_eq!(c.event(&uid).unwrap().start.hour(), 9); // start untouched
        c.adjust_event_start(&uid, Duration::minutes(-15)).unwrap();
        assert_eq!(c.event(&uid).unwrap().start.minute(), 45);
        // Pushing the start past the end is a no-op.
        let before = c.event(&uid).unwrap().start;
        c.adjust_event_start(&uid, Duration::hours(5)).unwrap();
        assert_eq!(c.event(&uid).unwrap().start, before);
    }

    #[test]
    fn projects_union_includes_used_and_registered() {
        let mut c = ctx();
        c.quick_add("a", Some("from-task".into())).unwrap();
        c.add_project("empty-project").unwrap();
        let projects = c.projects();
        assert!(projects.contains(&"from-task".to_string()));
        assert!(projects.contains(&"empty-project".to_string()));
    }

    #[test]
    fn set_task_project_assigns_and_registers() {
        let mut c = ctx();
        let uid = c.quick_add("x", None).unwrap();
        c.set_task_project(&uid, Some("wng".into())).unwrap();
        assert_eq!(c.task(&uid).unwrap().project.as_deref(), Some("wng"));
        assert!(c.projects().contains(&"wng".to_string()));
        c.undo().unwrap();
        assert_eq!(c.task(&uid).unwrap().project, None);
    }

    #[test]
    fn delete_project_unassigns_and_removes() {
        let mut c = ctx();
        let uid = c.quick_add("x", Some("wng".into())).unwrap();
        c.set_task_project(&uid, Some("wng".into())).unwrap();
        assert!(c.projects().contains(&"wng".to_string()));
        c.delete_project("wng").unwrap();
        assert_eq!(c.task(&uid).unwrap().project, None);
        assert!(!c.projects().contains(&"wng".to_string()));
    }

    #[test]
    fn task_file_points_at_existing_markdown() {
        let mut c = ctx();
        let uid = c.quick_add("notes", None).unwrap();
        let path = c.task_file(&uid).unwrap();
        assert!(path.exists());
        assert!(path.extension().unwrap() == "md");
    }

    #[test]
    fn events_on_day_filters_by_overlap() {
        let mut c = ctx();
        let day = NaiveDate::from_ymd_opt(2026, 6, 18).unwrap();
        c.put_event(Event::new(
            "work",
            "today",
            Utc.with_ymd_and_hms(2026, 6, 18, 9, 0, 0).unwrap(),
            Utc.with_ymd_and_hms(2026, 6, 18, 10, 0, 0).unwrap(),
        ))
        .unwrap();
        c.put_event(Event::new(
            "work",
            "tomorrow",
            Utc.with_ymd_and_hms(2026, 6, 19, 9, 0, 0).unwrap(),
            Utc.with_ymd_and_hms(2026, 6, 19, 10, 0, 0).unwrap(),
        ))
        .unwrap();
        let on = c.events_on(day);
        assert_eq!(on.len(), 1);
        assert_eq!(on[0].summary, "today");
    }
}
