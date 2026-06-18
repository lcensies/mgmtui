//! `MgmtContext` — the service layer the TUI and CLI talk to. Owns the local stores, an
//! in-memory cache, dirty tracking for sync, and undo/redo.

use std::path::PathBuf;

use chrono::{DateTime, Duration, NaiveDate, Utc};

use mgmt_core::{Result, Store, Uid};
use mgmt_domain::{Event, Filter, SortMode, Task, TaskStatus};
use mgmt_store::{VaultStore, VdirStore};

/// A reversible record of one item's state: `Some` means "this is the content", `None` means
/// "this item is absent". Applying a snapshot returns the inverse, enabling undo/redo.
enum Snapshot {
    Task(Uid, Box<Option<Task>>),
    Event(Uid, Box<Option<Event>>),
}

pub struct MgmtContext {
    tasks: VaultStore,
    events: VdirStore,
    task_cache: Vec<Task>,
    event_cache: Vec<Event>,
    projects_path: PathBuf,
    project_cache: Vec<String>,
    undo_stack: Vec<Snapshot>,
    redo_stack: Vec<Snapshot>,
    dirty: bool,
}

impl MgmtContext {
    /// Open a context over the given stores and load everything into memory.
    pub fn open(tasks: VaultStore, events: VdirStore) -> Result<Self> {
        let task_cache = tasks.load_all()?;
        let event_cache = events.load_all()?;
        // The project registry lives alongside the tasks/calendars dirs, under the data root.
        let data_root = tasks
            .root()
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| tasks.root().to_path_buf());
        let projects_path = mgmt_store::projects_file(&data_root);
        let project_cache = mgmt_store::load_projects(&projects_path)?;
        Ok(MgmtContext {
            tasks,
            events,
            task_cache,
            event_cache,
            projects_path,
            project_cache,
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            dirty: false,
        })
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

    /// The kanban board: every column in board order with its tasks (filtered, if given).
    pub fn board(&self, filter: &Filter) -> Vec<(TaskStatus, Vec<Task>)> {
        TaskStatus::BOARD_ORDER
            .iter()
            .map(|&status| {
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

    /// Quick-add a task to the inbox (or a project), returning its UID.
    pub fn quick_add(&mut self, title: impl Into<String>, project: Option<String>) -> Result<Uid> {
        let mut t = Task::new(title);
        t.project = project;
        t.created = Some(Utc::now());
        let uid = t.uid.clone();
        self.put_task(t)?;
        Ok(uid)
    }

    /// Move a task to a different kanban column.
    pub fn set_task_status(&mut self, uid: &Uid, status: TaskStatus) -> Result<()> {
        let mut t = self
            .task(uid)
            .cloned()
            .ok_or_else(|| mgmt_core::Error::NotFound(format!("task {uid}")))?;
        t.status = status;
        self.put_task(t)
    }

    // ---- projects ------------------------------------------------------------------

    /// All known projects: the registry plus any project referenced by a task, sorted unique.
    pub fn projects(&self) -> Vec<String> {
        let mut set: Vec<String> = self.project_cache.clone();
        for t in &self.task_cache {
            if let Some(p) = &t.project {
                set.push(p.clone());
            }
        }
        set.sort();
        set.dedup();
        set
    }

    /// Register a project (creating it even with no tasks). Persisted immediately.
    pub fn add_project(&mut self, name: impl Into<String>) -> Result<()> {
        let name = name.into();
        if name.trim().is_empty() {
            return Err(mgmt_core::Error::Invalid("empty project name".into()));
        }
        if !self.project_cache.iter().any(|p| p == &name) {
            self.project_cache.push(name);
            mgmt_store::save_projects(&self.projects_path, &self.project_cache)?;
        }
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
        self.project_cache = mgmt_store::load_projects(&self.projects_path)?;
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
        let todo = board.iter().find(|(s, _)| *s == TaskStatus::Todo).unwrap();
        assert_eq!(todo.1.len(), 1);
        assert_eq!(todo.1[0].project.as_deref(), Some("home"));
    }

    #[test]
    fn status_change_moves_columns_and_undoes() {
        let mut c = ctx();
        let uid = c.quick_add("ship it", None).unwrap();
        c.set_task_status(&uid, TaskStatus::Done).unwrap();
        assert_eq!(c.task(&uid).unwrap().status, TaskStatus::Done);
        assert!(c.undo().unwrap());
        assert_eq!(c.task(&uid).unwrap().status, TaskStatus::Todo);
        assert!(c.redo().unwrap());
        assert_eq!(c.task(&uid).unwrap().status, TaskStatus::Done);
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
