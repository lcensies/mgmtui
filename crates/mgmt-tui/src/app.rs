//! `MgmtApp` — the embeddable aggregate that owns app state and renders the views. It never
//! touches the terminal: a host calls [`MgmtApp::draw`] with a `Frame`/`Rect` and feeds keys
//! to [`MgmtApp::handle_key`]. The standalone `mgmt` binary provides the terminal + loop; the
//! wng dashboard can host it the same way.

use std::collections::HashSet;
use std::path::PathBuf;
use std::time::{Duration as StdDuration, Instant};

use chrono::{Datelike, Duration, Local, NaiveDate, Timelike, Utc, Weekday};
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Tabs, Wrap};

use mgmt_core::Uid;
use mgmt_domain::{Event, Filter, Priority, SmartView, SortMode, StatusKind, Task};
use mgmt_service::{MgmtContext, Phase, Pomodoro, Technique};

use crate::keymap::{Action, Context, action_for_key};
use crate::theme::{parse_color, Theme};

/// Result of handling a key: what the host should do next.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Continue,
    Quit,
    /// Open the given file in `$EDITOR`. The host suspends the terminal, runs the editor, then
    /// calls [`MgmtApp::reload`] before redrawing. Keeps this crate terminal-agnostic.
    OpenEditor(PathBuf),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tab {
    Calendar,
    Board,
    Tasks,
    Focus,
}

impl Tab {
    const ALL: [Tab; 4] = [Tab::Calendar, Tab::Board, Tab::Tasks, Tab::Focus];

    fn title(self) -> &'static str {
        match self {
            Tab::Calendar => "Calendar",
            Tab::Board => "Board",
            Tab::Tasks => "Tasks",
            Tab::Focus => "Focus",
        }
    }

    fn context(self) -> Context {
        match self {
            Tab::Calendar => Context::Calendar,
            Tab::Board => Context::Board,
            Tab::Tasks => Context::Tasks,
            Tab::Focus => Context::Focus,
        }
    }

    fn index(self) -> usize {
        Tab::ALL.iter().position(|t| *t == self).unwrap()
    }
}

/// Calendar zoom level.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CalView {
    Month,
    Week,
    Day,
}

impl CalView {
    fn next(self) -> Self {
        match self {
            CalView::Month => CalView::Week,
            CalView::Week => CalView::Day,
            CalView::Day => CalView::Month,
        }
    }

    fn label(self) -> &'static str {
        match self {
            CalView::Month => "month",
            CalView::Week => "week",
            CalView::Day => "day",
        }
    }
}

/// Which pane of the calendar tab has keyboard focus.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CalFocus {
    Date,
    Agenda,
}

/// A modal overlay capturing keys until dismissed.
enum Modal {
    /// Single-line text input.
    Input { prompt: String, buffer: String, purpose: InputPurpose },
    /// Task-creation form (title + optional project/due/priority).
    Task(TaskForm),
    /// Event-creation form.
    Event(EventForm),
    /// Fuzzy project picker.
    Picker(Picker),
    /// Yes/no confirmation.
    Confirm { prompt: String, action: ConfirmAction },
    /// Vim-style command palette (`:`).
    Palette(CommandPalette),
}

enum InputPurpose {
    Search,
    SearchEvents,
}

/// A destructive action awaiting confirmation.
enum ConfirmAction {
    DeleteProject(String),
}

/// Recurrence choices offered in the event form.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RecurChoice {
    None,
    Daily,
    Weekly,
    Monthly,
    Yearly,
}

impl RecurChoice {
    const ALL: [RecurChoice; 5] = [
        RecurChoice::None,
        RecurChoice::Daily,
        RecurChoice::Weekly,
        RecurChoice::Monthly,
        RecurChoice::Yearly,
    ];

    fn label(self) -> &'static str {
        match self {
            RecurChoice::None => "does not repeat",
            RecurChoice::Daily => "daily",
            RecurChoice::Weekly => "weekly",
            RecurChoice::Monthly => "monthly",
            RecurChoice::Yearly => "yearly",
        }
    }

    fn cycle(self, dir: i32) -> Self {
        let i = Self::ALL.iter().position(|c| *c == self).unwrap() as i32;
        let n = Self::ALL.len() as i32;
        Self::ALL[((i + dir).rem_euclid(n)) as usize]
    }

    fn to_rule(self) -> Option<mgmt_domain::RecurrenceRule> {
        use mgmt_domain::{Frequency, RecurrenceRule};
        let freq = match self {
            RecurChoice::None => return None,
            RecurChoice::Daily => Frequency::Daily,
            RecurChoice::Weekly => Frequency::Weekly,
            RecurChoice::Monthly => Frequency::Monthly,
            RecurChoice::Yearly => Frequency::Yearly,
        };
        Some(RecurrenceRule::every(freq, 1))
    }

    fn from_rule(rule: &Option<mgmt_domain::RecurrenceRule>) -> Self {
        use mgmt_domain::Frequency;
        match rule.as_ref().map(|r| r.freq) {
            None => RecurChoice::None,
            Some(Frequency::Daily) => RecurChoice::Daily,
            Some(Frequency::Weekly) => RecurChoice::Weekly,
            Some(Frequency::Monthly) => RecurChoice::Monthly,
            Some(Frequency::Yearly) => RecurChoice::Yearly,
        }
    }
}

/// Task-creation form. Title is required; project/due/priority are optional. `Enter` submits
/// from any field, `Tab`/`BackTab` move between fields, so title-only quick capture stays fast.
struct TaskForm {
    title: String,
    project: String,
    due: String,
    priority: Priority,
    field: usize, // 0=title 1=project 2=due 3=priority
}

impl TaskForm {
    const FIELDS: usize = 4;
    const PRIORITY_FIELD: usize = 3;

    fn new(project: Option<String>) -> Self {
        TaskForm {
            title: String::new(),
            project: project.unwrap_or_default(),
            due: String::new(),
            priority: Priority::None,
            field: 0,
        }
    }

    fn field_mut(&mut self) -> Option<&mut String> {
        match self.field {
            0 => Some(&mut self.title),
            1 => Some(&mut self.project),
            2 => Some(&mut self.due),
            _ => None, // priority is cycled, not typed
        }
    }
}

/// Human-readable event create/edit form.
struct EventForm {
    edit_uid: Option<Uid>,
    summary: String,
    date: String,     // YYYY-MM-DD
    start: String,    // HH:MM
    end: String,      // HH:MM
    location: String,
    project: String,
    recur: RecurChoice,
    field: usize, // 0=summary 1=date 2=start 3=end 4=location 5=project 6=recur
}

impl EventForm {
    const FIELDS: usize = 7;
    const RECUR_FIELD: usize = 6;

    fn new(day: NaiveDate, project: Option<String>) -> Self {
        EventForm {
            edit_uid: None,
            summary: String::new(),
            date: day.format("%Y-%m-%d").to_string(),
            start: "09:00".to_string(),
            end: "10:00".to_string(),
            location: String::new(),
            project: project.unwrap_or_default(),
            recur: RecurChoice::None,
            field: 0,
        }
    }

    fn from_event(ev: &Event) -> Self {
        EventForm {
            edit_uid: Some(ev.uid.clone()),
            summary: ev.summary.clone(),
            date: ev.start.format("%Y-%m-%d").to_string(),
            start: ev.start.format("%H:%M").to_string(),
            end: ev.end.format("%H:%M").to_string(),
            location: ev.location.clone().unwrap_or_default(),
            project: ev.project.clone().unwrap_or_default(),
            recur: RecurChoice::from_rule(&ev.rrule),
            field: 0,
        }
    }

    fn field_mut(&mut self) -> Option<&mut String> {
        match self.field {
            0 => Some(&mut self.summary),
            1 => Some(&mut self.date),
            2 => Some(&mut self.start),
            3 => Some(&mut self.end),
            4 => Some(&mut self.location),
            5 => Some(&mut self.project),
            _ => None, // recur is toggled, not typed
        }
    }
}

/// A fuzzy project picker: type to filter the known projects, with synthetic "(none)" and
/// "(new: <query>)" entries.
struct Picker {
    target: Uid,
    projects: Vec<String>,
    query: String,
    sel: usize,
}

/// One resolved entry in the [`Picker`] list.
#[derive(Clone)]
enum PickEntry {
    None,
    Project(String),
    New(String),
}

impl Picker {
    /// The current entries given the query: fuzzy-matched projects, plus a create-new entry when
    /// the query doesn't match an existing project, plus the clear-project "(none)" entry.
    fn entries(&self) -> Vec<PickEntry> {
        let q = self.query.trim();
        let mut out = Vec::new();
        if q.is_empty() {
            out.push(PickEntry::None);
            out.extend(self.projects.iter().cloned().map(PickEntry::Project));
        } else {
            let mut scored: Vec<(i32, &String)> =
                self.projects.iter().filter_map(|p| fuzzy_score(q, p).map(|s| (s, p))).collect();
            scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.len().cmp(&b.1.len())));
            out.extend(scored.into_iter().map(|(_, p)| PickEntry::Project(p.clone())));
            if !self.projects.iter().any(|p| p.eq_ignore_ascii_case(q)) {
                out.push(PickEntry::New(q.to_string()));
            }
            out.push(PickEntry::None);
        }
        out
    }
}

/// A static command definition for the command palette.
struct CmdDef {
    name: &'static str,
    aliases: &'static [&'static str],
    desc: &'static str,
}

const PALETTE_CMDS: &[CmdDef] = &[
    // Due-date rescheduling (tasks) / day move (events)
    CmdDef { name: "today",       aliases: &["tod"],           desc: "set due: today" },
    CmdDef { name: "tomorrow",    aliases: &["tom"],           desc: "set due: tomorrow" },
    CmdDef { name: "monday",      aliases: &["mon"],           desc: "set due: next Monday" },
    CmdDef { name: "tuesday",     aliases: &["tue"],           desc: "set due: next Tuesday" },
    CmdDef { name: "wednesday",   aliases: &["wed"],           desc: "set due: next Wednesday" },
    CmdDef { name: "thursday",    aliases: &["thu"],           desc: "set due: next Thursday" },
    CmdDef { name: "friday",      aliases: &["fri"],           desc: "set due: next Friday" },
    CmdDef { name: "saturday",    aliases: &["sat"],           desc: "set due: next Saturday" },
    CmdDef { name: "sunday",      aliases: &["sun"],           desc: "set due: next Sunday" },
    CmdDef { name: "end-of-week", aliases: &["eow"],           desc: "set due: Friday of this week" },
    CmdDef { name: "next-week",   aliases: &["nw"],            desc: "set due: next Monday" },
    CmdDef { name: "no-due",      aliases: &["clear-due"],     desc: "clear due date" },
    // Priority
    CmdDef { name: "high",        aliases: &[],                desc: "set priority: high" },
    CmdDef { name: "medium",      aliases: &["med"],           desc: "set priority: medium" },
    CmdDef { name: "low",         aliases: &[],                desc: "set priority: low" },
    CmdDef { name: "no-priority", aliases: &["np"],            desc: "clear priority" },
    // Status / lifecycle
    CmdDef { name: "done",        aliases: &[],                desc: "toggle done on selected task" },
    CmdDef { name: "delete",      aliases: &["del"],           desc: "delete selected item" },
    // Navigation
    CmdDef { name: "calendar",    aliases: &["cal"],           desc: "switch to Calendar tab" },
    CmdDef { name: "board",       aliases: &[],                desc: "switch to Board tab" },
    CmdDef { name: "tasks",       aliases: &[],                desc: "switch to Tasks tab" },
    CmdDef { name: "focus",       aliases: &[],                desc: "switch to Focus tab" },
    // Creation
    CmdDef { name: "new-task",    aliases: &["add"],           desc: "add a new task" },
    CmdDef { name: "new-event",   aliases: &["event"],         desc: "add a new event" },
    // Undo / redo
    CmdDef { name: "undo",        aliases: &[],                desc: "undo last change" },
    CmdDef { name: "redo",        aliases: &[],                desc: "redo" },
];

/// The command palette state: what the user has typed and which entry is highlighted.
struct CommandPalette {
    query: String,
    sel: usize,
}

impl CommandPalette {
    fn new() -> Self {
        CommandPalette { query: String::new(), sel: 0 }
    }

    fn entries(&self) -> Vec<&'static CmdDef> {
        let q = self.query.trim();
        if q.is_empty() {
            return PALETTE_CMDS.iter().collect();
        }
        let mut scored: Vec<(i32, &CmdDef)> = PALETTE_CMDS
            .iter()
            .filter_map(|cmd| {
                let best = std::iter::once(cmd.name)
                    .chain(cmd.aliases.iter().copied())
                    .filter_map(|n| fuzzy_score(q, n))
                    .max()?;
                Some((best, cmd))
            })
            .collect();
        scored.sort_by(|a, b| b.0.cmp(&a.0));
        scored.into_iter().map(|(_, cmd)| cmd).collect()
    }
}

/// Case-insensitive fuzzy score: substring matches rank highest (earlier = better), then
/// subsequence matches, else no match.
fn fuzzy_score(query: &str, cand: &str) -> Option<i32> {
    let q = query.to_lowercase();
    let c = cand.to_lowercase();
    if let Some(pos) = c.find(&q) {
        return Some(1000 - pos as i32);
    }
    let mut chars = c.chars();
    for qc in q.chars() {
        if !chars.any(|cc| cc == qc) {
            return None;
        }
    }
    Some(0)
}

/// One row in the Tasks-tab sidebar: a smart view, the "all projects" reset, or a project.
enum SidebarRow {
    View(SmartView),
    AllProjects,
    Project(String),
}

/// A simple session timer driving the pomodoro/flowtime display.
struct Timer {
    technique: Pomodoro,
    phase: Phase,
    elapsed_before: StdDuration,
    running_since: Option<Instant>,
}

impl Timer {
    fn new() -> Self {
        let technique = Pomodoro::standard();
        let phase = technique.initial();
        Timer {
            technique,
            phase,
            elapsed_before: StdDuration::ZERO,
            running_since: None,
        }
    }

    fn elapsed(&self) -> StdDuration {
        self.elapsed_before + self.running_since.map(|t| t.elapsed()).unwrap_or(StdDuration::ZERO)
    }

    fn toggle(&mut self) {
        match self.running_since.take() {
            Some(since) => self.elapsed_before += since.elapsed(),
            None => self.running_since = Some(Instant::now()),
        }
    }

    fn skip(&mut self) {
        let elapsed = self.elapsed();
        self.phase = self.technique.next(self.phase, elapsed);
        self.elapsed_before = StdDuration::ZERO;
        self.running_since = if self.running_since.is_some() {
            Some(Instant::now())
        } else {
            None
        };
    }

    fn running(&self) -> bool {
        self.running_since.is_some()
    }
}

pub struct MgmtApp {
    ctx: MgmtContext,
    theme: Theme,
    tab: Tab,

    // calendar
    day: NaiveDate,
    cal_view: CalView,
    cal_focus: CalFocus,
    agenda_sel: usize,

    // board
    board_col: usize,
    board_row: usize,

    // tasks
    task_sel: usize,
    filter: Filter,
    sort: SortMode,
    project_scope: Option<String>,
    task_view: SmartView,
    sidebar_focus: bool,
    sidebar_sel: usize,

    // calendar search
    event_query: Option<String>,

    // focus
    timer: Timer,
    phase_notified: bool,

    // reminders already shown this session (dedup key set)
    fired_reminders: HashSet<String>,

    modal: Option<Modal>,
    show_help: bool,
    status: String,
}

impl MgmtApp {
    pub fn new(ctx: MgmtContext) -> Self {
        // Base palette plus any per-slot overrides from the user's config.
        let theme = Theme::default().with_overrides(ctx.config().theme_overrides());
        MgmtApp {
            ctx,
            theme,
            tab: Tab::Calendar,
            day: Local::now().date_naive(),
            cal_view: CalView::Month,
            cal_focus: CalFocus::Date,
            agenda_sel: 0,
            board_col: 0,
            board_row: 0,
            task_sel: 0,
            filter: Filter::default(),
            sort: SortMode::DueDate,
            project_scope: None,
            task_view: SmartView::All,
            sidebar_focus: false,
            sidebar_sel: 0,
            event_query: None,
            timer: Timer::new(),
            phase_notified: false,
            fired_reminders: HashSet::new(),
            modal: None,
            show_help: false,
            status: "? for help".to_string(),
        }
    }

    pub fn with_theme(mut self, theme: Theme) -> Self {
        self.theme = theme;
        self
    }

    /// The key context currently active (modals override the tab).
    pub fn context(&self) -> Context {
        match &self.modal {
            Some(Modal::Input { .. }) => Context::Input,
            Some(Modal::Task(_)) => Context::Form,
            Some(Modal::Event(_)) => Context::Form,
            Some(Modal::Picker(_)) => Context::Picker,
            Some(Modal::Confirm { .. }) => Context::Confirm,
            Some(Modal::Palette(_)) => Context::CommandPalette,
            None => self.tab.context(),
        }
    }

    pub fn is_dirty(&self) -> bool {
        self.ctx.is_dirty()
    }

    pub fn context_mut(&mut self) -> &mut MgmtContext {
        &mut self.ctx
    }

    /// Reload state from disk (after an external `$EDITOR` edit) and clamp selections.
    pub fn reload(&mut self) {
        if let Err(e) = self.ctx.reload() {
            self.status = format!("reload error: {e}");
        }
        self.clamp_board();
    }

    // ---- input handling ------------------------------------------------------------

    pub fn handle_key(&mut self, key: KeyEvent) -> Outcome {
        if self.show_help {
            self.show_help = false;
            return Outcome::Continue;
        }
        if self.modal.is_some() {
            return self.handle_modal_key(key);
        }
        let Some(action) = action_for_key(self.context(), key) else {
            return Outcome::Continue;
        };
        self.dispatch(action)
    }

    fn dispatch(&mut self, action: Action) -> Outcome {
        match action {
            Action::Quit => return Outcome::Quit,
            Action::Help => self.show_help = true,
            Action::NextTab => self.cycle_tab(1),
            Action::PrevTab => self.cycle_tab(-1),
            Action::Undo => {
                let r = self.try_undo();
                self.report(r);
            }
            Action::Redo => {
                let r = self.try_redo();
                self.report(r);
            }
            Action::OpenCommandPalette => self.modal = Some(Modal::Palette(CommandPalette::new())),
            Action::QuickAdd => self.begin_quick_add(),
            Action::Edit => return self.edit_selected(),
            Action::EditProject => self.begin_project_picker(),
            Action::CyclePriority => self.cycle_priority(),
            Action::PrevProject => self.cycle_project_scope(-1),
            Action::NextProject => self.cycle_project_scope(1),
            Action::Search => self.begin_search(),
            other => match self.tab {
                Tab::Calendar => self.calendar_action(other),
                Tab::Board => self.board_action(other),
                Tab::Tasks => self.tasks_action(other),
                Tab::Focus => self.focus_action(other),
            },
        }
        Outcome::Continue
    }

    /// Called by the host on each loop iteration (even without a key) so the pomodoro timer
    /// can fire a desktop notification and auto-advance when a phase completes.
    pub fn tick(&mut self) {
        self.fire_due_reminders();
        if !self.timer.running() {
            return;
        }
        let Some(target) = self.timer.phase.target() else { return };
        if self.timer.elapsed() >= target {
            if !self.phase_notified {
                self.notify_phase_done();
                self.phase_notified = true;
            }
            self.timer.skip();
            self.phase_notified = false;
        }
    }

    /// Fire any task/event reminders that have come due since the last tick, once per session.
    fn fire_due_reminders(&mut self) {
        let hits = self.ctx.pending_reminders(Utc::now(), &self.fired_reminders);
        for hit in hits {
            let _ = notify_rust::Notification::new().summary(&hit.title).body(&hit.body).appname("mgmt").show();
            self.fired_reminders.insert(hit.key);
        }
    }

    fn notify_phase_done(&self) {
        let (title, body) = match self.timer.phase {
            Phase::Focus { .. } => ("Focus done", "Time for a break."),
            Phase::Break { .. } => ("Break over", "Back to focus."),
        };
        let _ = notify_rust::Notification::new().summary(title).body(body).appname("mgmt").show();
    }

    fn cycle_project_scope(&mut self, dir: i32) {
        // Scope list: None followed by every known project.
        let mut options: Vec<Option<String>> = vec![None];
        options.extend(self.ctx.projects().into_iter().map(Some));
        let current = options.iter().position(|p| p == &self.project_scope).unwrap_or(0) as i32;
        let n = options.len() as i32;
        let next = options[((current + dir).rem_euclid(n)) as usize].clone();
        self.project_scope = next.clone();
        self.filter.project = next.clone();
        self.task_sel = 0;
        self.clamp_board();
        self.status = match next {
            Some(p) => format!("project: {p}"),
            None => "all projects".into(),
        };
    }

    fn begin_search(&mut self) {
        // On the calendar, `/` searches events; elsewhere it filters the task list.
        if self.tab == Tab::Calendar {
            self.modal = Some(Modal::Input {
                prompt: "Search events".to_string(),
                buffer: self.event_query.clone().unwrap_or_default(),
                purpose: InputPurpose::SearchEvents,
            });
        } else {
            self.modal = Some(Modal::Input {
                prompt: "Filter tasks".to_string(),
                buffer: self.filter.text.clone().unwrap_or_default(),
                purpose: InputPurpose::Search,
            });
        }
    }

    fn cycle_tab(&mut self, dir: i32) {
        let n = Tab::ALL.len() as i32;
        let idx = (self.tab.index() as i32 + dir).rem_euclid(n) as usize;
        self.tab = Tab::ALL[idx];
    }

    // ---- modals --------------------------------------------------------------------

    fn handle_modal_key(&mut self, key: KeyEvent) -> Outcome {
        let modal = self.modal.take().unwrap();
        match modal {
            Modal::Input { prompt, mut buffer, purpose } => match key.code {
                KeyCode::Esc => {}
                KeyCode::Enter => self.submit_input(purpose, buffer.trim().to_string()),
                KeyCode::Backspace => {
                    buffer.pop();
                    self.modal = Some(Modal::Input { prompt, buffer, purpose });
                }
                KeyCode::Char(c) => {
                    buffer.push(c);
                    self.modal = Some(Modal::Input { prompt, buffer, purpose });
                }
                _ => self.modal = Some(Modal::Input { prompt, buffer, purpose }),
            },
            Modal::Task(mut form) => match key.code {
                KeyCode::Esc => {}
                // Enter submits from any field (title-only capture stays fast).
                KeyCode::Enter => self.submit_task(form),
                KeyCode::Tab => {
                    form.field = (form.field + 1) % TaskForm::FIELDS;
                    self.modal = Some(Modal::Task(form));
                }
                KeyCode::BackTab => {
                    form.field = (form.field + TaskForm::FIELDS - 1) % TaskForm::FIELDS;
                    self.modal = Some(Modal::Task(form));
                }
                KeyCode::Left if form.field == TaskForm::PRIORITY_FIELD => {
                    form.priority = cycle_priority(form.priority, -1);
                    self.modal = Some(Modal::Task(form));
                }
                KeyCode::Right | KeyCode::Char(' ') if form.field == TaskForm::PRIORITY_FIELD => {
                    form.priority = cycle_priority(form.priority, 1);
                    self.modal = Some(Modal::Task(form));
                }
                KeyCode::Backspace => {
                    if let Some(f) = form.field_mut() {
                        f.pop();
                    }
                    self.modal = Some(Modal::Task(form));
                }
                KeyCode::Char(c) => {
                    if let Some(f) = form.field_mut() {
                        f.push(c);
                    }
                    self.modal = Some(Modal::Task(form));
                }
                _ => self.modal = Some(Modal::Task(form)),
            },
            Modal::Event(mut form) => match key.code {
                KeyCode::Esc => {}
                KeyCode::Enter | KeyCode::Tab => {
                    if form.field + 1 < EventForm::FIELDS {
                        form.field += 1;
                        self.modal = Some(Modal::Event(form));
                    } else {
                        self.submit_event(form);
                    }
                }
                KeyCode::BackTab => {
                    form.field = form.field.saturating_sub(1);
                    self.modal = Some(Modal::Event(form));
                }
                // On the recurrence field, left/right/space cycle the choice.
                KeyCode::Left if form.field == EventForm::RECUR_FIELD => {
                    form.recur = form.recur.cycle(-1);
                    self.modal = Some(Modal::Event(form));
                }
                KeyCode::Right | KeyCode::Char(' ') if form.field == EventForm::RECUR_FIELD => {
                    form.recur = form.recur.cycle(1);
                    self.modal = Some(Modal::Event(form));
                }
                KeyCode::Backspace => {
                    if let Some(f) = form.field_mut() {
                        f.pop();
                    }
                    self.modal = Some(Modal::Event(form));
                }
                KeyCode::Char(c) => {
                    if let Some(f) = form.field_mut() {
                        f.push(c);
                    }
                    self.modal = Some(Modal::Event(form));
                }
                _ => self.modal = Some(Modal::Event(form)),
            },
            Modal::Picker(mut picker) => match key.code {
                KeyCode::Esc => {}
                // Arrows (and Ctrl-n/p) navigate; printable chars filter the list.
                KeyCode::Down => {
                    let n = picker.entries().len();
                    picker.sel = (picker.sel + 1).min(n.saturating_sub(1));
                    self.modal = Some(Modal::Picker(picker));
                }
                KeyCode::Up => {
                    picker.sel = picker.sel.saturating_sub(1);
                    self.modal = Some(Modal::Picker(picker));
                }
                KeyCode::Char('n') if key.modifiers.contains(crossterm::event::KeyModifiers::CONTROL) => {
                    let n = picker.entries().len();
                    picker.sel = (picker.sel + 1).min(n.saturating_sub(1));
                    self.modal = Some(Modal::Picker(picker));
                }
                KeyCode::Char('p') if key.modifiers.contains(crossterm::event::KeyModifiers::CONTROL) => {
                    picker.sel = picker.sel.saturating_sub(1);
                    self.modal = Some(Modal::Picker(picker));
                }
                KeyCode::Backspace => {
                    picker.query.pop();
                    picker.sel = 0;
                    self.modal = Some(Modal::Picker(picker));
                }
                KeyCode::Char(c) => {
                    picker.query.push(c);
                    picker.sel = 0;
                    self.modal = Some(Modal::Picker(picker));
                }
                KeyCode::Enter => self.pick_project(picker),
                _ => self.modal = Some(Modal::Picker(picker)),
            },
            Modal::Confirm { prompt, action } => match key.code {
                // y / Enter confirm; anything else (n, Esc, …) cancels.
                KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => self.perform_confirm(action),
                _ => {
                    let _ = prompt;
                }
            },
            Modal::Palette(mut cp) => match key.code {
                KeyCode::Esc => {}
                KeyCode::Enter => {
                    let entries = cp.entries();
                    if let Some(cmd) = entries.get(cp.sel) {
                        let name = cmd.name;
                        self.execute_cmd(name);
                    }
                }
                // Tab completes the query to the selected command's full name.
                KeyCode::Tab => {
                    let entries = cp.entries();
                    if let Some(cmd) = entries.get(cp.sel) {
                        cp.query = cmd.name.to_string();
                        cp.sel = 0;
                    }
                    self.modal = Some(Modal::Palette(cp));
                }
                KeyCode::Down => {
                    let n = cp.entries().len();
                    cp.sel = (cp.sel + 1).min(n.saturating_sub(1));
                    self.modal = Some(Modal::Palette(cp));
                }
                KeyCode::Up => {
                    cp.sel = cp.sel.saturating_sub(1);
                    self.modal = Some(Modal::Palette(cp));
                }
                KeyCode::Char('n') if key.modifiers.contains(crossterm::event::KeyModifiers::CONTROL) => {
                    let n = cp.entries().len();
                    cp.sel = (cp.sel + 1).min(n.saturating_sub(1));
                    self.modal = Some(Modal::Palette(cp));
                }
                KeyCode::Char('p') if key.modifiers.contains(crossterm::event::KeyModifiers::CONTROL) => {
                    cp.sel = cp.sel.saturating_sub(1);
                    self.modal = Some(Modal::Palette(cp));
                }
                KeyCode::Backspace => {
                    cp.query.pop();
                    cp.sel = 0;
                    self.modal = Some(Modal::Palette(cp));
                }
                KeyCode::Char(c) => {
                    cp.query.push(c);
                    cp.sel = 0;
                    self.modal = Some(Modal::Palette(cp));
                }
                _ => self.modal = Some(Modal::Palette(cp)),
            },
        }
        Outcome::Continue
    }

    fn perform_confirm(&mut self, action: ConfirmAction) {
        match action {
            ConfirmAction::DeleteProject(name) => {
                let r = self.ctx.delete_project(&name);
                if self.project_scope.as_deref() == Some(name.as_str()) {
                    self.project_scope = None;
                    self.filter.project = None;
                }
                self.sidebar_sel = 0;
                self.task_sel = 0;
                self.report(r.map(|_| format!("deleted project {name}")));
            }
        }
    }

    fn begin_quick_add(&mut self) {
        // On the calendar tab, `a` opens the event form instead of a task quick-add.
        if self.tab == Tab::Calendar {
            self.modal = Some(Modal::Event(EventForm::new(self.day, self.project_scope.clone())));
            return;
        }
        self.modal = Some(Modal::Task(TaskForm::new(self.project_scope.clone())));
    }

    fn submit_input(&mut self, purpose: InputPurpose, text: String) {
        // Search applies even when empty (empty clears the filter).
        match purpose {
            InputPurpose::Search => {
                self.filter.text = if text.is_empty() { None } else { Some(text) };
                self.task_sel = 0;
                self.status = "filtered".into();
            }
            InputPurpose::SearchEvents => {
                self.event_query = if text.is_empty() { None } else { Some(text) };
                self.agenda_sel = 0;
                self.status = "event filter set".into();
            }
        }
    }

    /// Create a task from the form. Title is required; project/due/priority optional. Switches to
    /// a view that actually shows the new task so it's visible immediately.
    fn submit_task(&mut self, form: TaskForm) {
        let title = form.title.trim().to_string();
        if title.is_empty() {
            self.status = "task needs a title".into();
            self.modal = Some(Modal::Task(form));
            return;
        }
        let due = if form.due.trim().is_empty() {
            None
        } else {
            match parse_due(form.due.trim()) {
                Some(d) => Some(d),
                None => {
                    self.status = "due: use YYYY-MM-DD, today, tomorrow, or +Nd".into();
                    self.modal = Some(Modal::Task(form));
                    return;
                }
            }
        };
        let project = (!form.project.trim().is_empty()).then(|| form.project.trim().to_string());

        let uid = match self.ctx.quick_add(title, project) {
            Ok(uid) => uid,
            Err(e) => {
                self.status = format!("error: {e}");
                return;
            }
        };
        // Apply the optional fields in one follow-up write.
        if due.is_some() || form.priority != Priority::None {
            if let Some(mut t) = self.ctx.task(&uid).cloned() {
                t.due = due;
                t.priority = form.priority;
                if due.is_some() && t.reminders.is_empty() {
                    t.reminders = self.ctx.default_reminders();
                }
                let _ = self.ctx.put_task(t);
            }
        }
        self.ensure_task_visible(&uid);
        self.status = "added".into();
    }

    /// Make sure the just-created task shows under the active Tasks filter; if the current smart
    /// view would hide it, fall back to "All" and select it.
    fn ensure_task_visible(&mut self, uid: &Uid) {
        let visible = self.visible_tasks();
        match visible.iter().position(|t| &t.uid == uid) {
            Some(i) => self.task_sel = i,
            None => {
                self.task_view = SmartView::All;
                let visible = self.visible_tasks();
                self.task_sel = visible.iter().position(|t| &t.uid == uid).unwrap_or(0);
            }
        }
    }

    fn submit_event(&mut self, form: EventForm) {
        let summary = form.summary.trim().to_string();
        if summary.is_empty() {
            self.status = "event needs a summary".into();
            self.modal = Some(Modal::Event(form));
            return;
        }
        let Some(date) = chrono::NaiveDate::parse_from_str(form.date.trim(), "%Y-%m-%d").ok() else {
            self.status = "date must be YYYY-MM-DD".into();
            self.modal = Some(Modal::Event(form));
            return;
        };
        let (Some(start), Some(end)) = (parse_hhmm(date, &form.start), parse_hhmm(date, &form.end)) else {
            self.status = "times must be HH:MM".into();
            self.modal = Some(Modal::Event(form));
            return;
        };
        let location = (!form.location.trim().is_empty()).then(|| form.location.trim().to_string());
        let project = (!form.project.trim().is_empty()).then(|| form.project.trim().to_string());
        let rrule = form.recur.to_rule();
        if let Some(p) = &project {
            let _ = self.ctx.add_project(p.clone());
        }

        let result = match &form.edit_uid {
            Some(uid) if self.ctx.event(uid).is_some() => {
                let mut ev = self.ctx.event(uid).cloned().unwrap();
                ev.summary = summary;
                ev.start = start;
                ev.end = end;
                ev.location = location;
                ev.project = project;
                ev.rrule = rrule;
                self.ctx.put_event(ev).map(|_| "event updated".to_string())
            }
            _ => {
                let calendar = self
                    .ctx
                    .events()
                    .first()
                    .map(|e| e.calendar.clone())
                    .unwrap_or_else(|| "default".to_string());
                let mut ev = Event::new(calendar, summary, start, end);
                ev.location = location;
                ev.project = project;
                ev.rrule = rrule;
                self.ctx.put_event(ev).map(|_| "event created".to_string())
            }
        };
        self.report(result);
    }

    fn begin_project_picker(&mut self) {
        let Some(uid) = self.selected_task_uid() else { return };
        let projects = self.ctx.projects();
        self.modal = Some(Modal::Picker(Picker { target: uid, projects, query: String::new(), sel: 0 }));
    }

    fn pick_project(&mut self, picker: Picker) {
        let entries = picker.entries();
        let Some(entry) = entries.get(picker.sel).cloned() else { return };
        let project = match entry {
            PickEntry::None => None,
            PickEntry::Project(name) | PickEntry::New(name) => Some(name),
        };
        // set_task_project registers a brand-new project name as it assigns it.
        let r = self.ctx.set_task_project(&picker.target, project);
        self.report(r.map(|_| "project set".into()));
    }

    fn cycle_priority(&mut self) {
        if let Some(uid) = self.selected_task_uid() {
            let r = self.ctx.cycle_task_priority(&uid);
            self.report(r.map(|_| "priority changed".into()));
        }
    }

    /// The task currently selected on whichever tab is active (board or tasks list).
    fn selected_task_uid(&self) -> Option<Uid> {
        match self.tab {
            Tab::Board => self.selected_card_uid(),
            Tab::Tasks => {
                let tasks = self.visible_tasks();
                tasks.get(self.task_sel).map(|t| t.uid.clone())
            }
            _ => None,
        }
    }

    /// Resolve the edit action for the current selection. Tasks open in `$EDITOR` (their
    /// markdown is already human-readable); events open the in-TUI form (pre-filled).
    fn edit_selected(&mut self) -> Outcome {
        match self.tab {
            Tab::Calendar => {
                if let Some(uid) = self.selected_event_uid() {
                    if let Some(ev) = self.ctx.event(&uid) {
                        self.modal = Some(Modal::Event(EventForm::from_event(ev)));
                    }
                } else {
                    self.status = "no event selected".into();
                }
                Outcome::Continue
            }
            Tab::Board | Tab::Tasks => match self.selected_task_uid().and_then(|u| self.ctx.task_file(&u)) {
                Some(p) => Outcome::OpenEditor(p),
                None => {
                    self.status = "nothing to edit".into();
                    Outcome::Continue
                }
            },
            Tab::Focus => Outcome::Continue,
        }
    }

    // ---- calendar ------------------------------------------------------------------

    fn calendar_action(&mut self, action: Action) {
        match action {
            Action::ViewCycle => {
                self.cal_view = self.cal_view.next();
                self.status = format!("{} view", self.cal_view.label());
            }
            Action::Select => {
                self.cal_focus = match self.cal_focus {
                    CalFocus::Date => CalFocus::Agenda,
                    CalFocus::Agenda => CalFocus::Date,
                };
            }
            Action::Left => self.day -= Duration::days(1),
            Action::Right => self.day += Duration::days(1),
            Action::Up | Action::Down => self.calendar_vertical(action),
            Action::Today => self.day = Local::now().date_naive(),
            Action::StartEarlier => self.adjust_selected_event(true, Duration::minutes(-15)),
            Action::StartLater => self.adjust_selected_event(true, Duration::minutes(15)),
            Action::EndEarlier => self.adjust_selected_event(false, Duration::minutes(-15)),
            Action::EndLater => self.adjust_selected_event(false, Duration::minutes(15)),
            Action::Delete => self.delete_selected_event(),
            _ => {}
        }
        let count = self.visible_events_on(self.day).len();
        if self.agenda_sel >= count {
            self.agenda_sel = count.saturating_sub(1);
        }
    }

    fn calendar_vertical(&mut self, action: Action) {
        let down = action == Action::Down;
        match self.cal_focus {
            CalFocus::Agenda => {
                // move the event selection within the day
                let count = self.visible_events_on(self.day).len();
                if down {
                    self.agenda_sel = (self.agenda_sel + 1).min(count.saturating_sub(1));
                } else {
                    self.agenda_sel = self.agenda_sel.saturating_sub(1);
                }
            }
            CalFocus::Date => {
                // move the selected date: a week in month view, a day otherwise
                let step = match self.cal_view {
                    CalView::Month => Duration::weeks(1),
                    CalView::Week | CalView::Day => Duration::days(1),
                };
                self.day += if down { step } else { -step };
            }
        }
    }

    fn selected_event_uid(&self) -> Option<Uid> {
        self.visible_events_on(self.day).get(self.agenda_sel).map(|e| e.uid.clone())
    }

    /// Events on `day` after applying the calendar search query and the active project scope —
    /// the single source of truth so display and selection indices stay aligned.
    fn visible_events_on(&self, day: NaiveDate) -> Vec<Event> {
        self.ctx
            .events_on(day)
            .into_iter()
            .filter(|e| self.event_query.as_deref().map(|q| e.matches_text(q)).unwrap_or(true))
            .filter(|e| self.project_scope.as_deref().map(|p| e.project.as_deref() == Some(p)).unwrap_or(true))
            .collect()
    }

    /// Resolve a project's display color through the theme's string parser.
    fn project_color(&self, name: &str) -> Color {
        parse_color(&self.ctx.project_color(name)).unwrap_or(self.theme.task)
    }

    /// An event's display color: its project's color if bound, else the base event color.
    fn event_color(&self, e: &Event) -> Color {
        e.project.as_deref().map(|p| self.project_color(p)).unwrap_or(self.theme.event)
    }

    /// The status-id color (config override or auto-assigned), falling back to the task color.
    fn status_color(&self, id: &str) -> Color {
        let raw = self.ctx.workflow().color(id).map(str::to_string).unwrap_or_else(|| mgmt_domain::auto_color(id).to_string());
        parse_color(&raw).unwrap_or(self.theme.accent)
    }

    /// The Tasks-view smart lists configured by the user (defaults to all built-ins).
    fn smart_views(&self) -> Vec<SmartView> {
        self.ctx.config().views()
    }

    /// The concrete filter for the Tasks list: the active smart view, narrowed by the project
    /// scope and any text query.
    fn current_task_filter(&self) -> Filter {
        let today = Local::now().date_naive();
        let open = self.ctx.workflow().open_ids();
        let mut f = self.task_view.to_filter(today, open);
        if self.project_scope.is_some() {
            f.project = self.project_scope.clone();
            f.no_project = false;
        }
        f.text = self.filter.text.clone();
        f
    }

    /// Tasks shown in the Tasks list, given the current view/scope/query.
    fn visible_tasks(&self) -> Vec<Task> {
        self.ctx.filtered_tasks(&self.current_task_filter(), self.sort)
    }

    /// Sidebar rows: the smart views followed by the known projects (with an "All projects"
    /// reset at the head of the project section).
    fn sidebar_rows(&self) -> Vec<SidebarRow> {
        let mut rows: Vec<SidebarRow> = self.smart_views().into_iter().map(SidebarRow::View).collect();
        rows.push(SidebarRow::AllProjects);
        rows.extend(self.ctx.projects().into_iter().map(SidebarRow::Project));
        rows
    }

    /// Open a confirmation to delete the highlighted sidebar project (no-op on view rows).
    fn begin_delete_project(&mut self) {
        if let Some(SidebarRow::Project(name)) = self.sidebar_rows().get(self.sidebar_sel) {
            let name = name.clone();
            self.modal = Some(Modal::Confirm {
                prompt: format!("Delete project '{name}'? Its tasks/events move to the inbox."),
                action: ConfirmAction::DeleteProject(name),
            });
        }
    }

    /// Apply the currently-highlighted sidebar row to the active view/scope.
    fn apply_sidebar_selection(&mut self) {
        let rows = self.sidebar_rows();
        let Some(row) = rows.get(self.sidebar_sel) else { return };
        match row {
            SidebarRow::View(v) => {
                self.task_view = *v;
                self.status = format!("view: {}", v.label());
            }
            SidebarRow::AllProjects => {
                self.project_scope = None;
                self.filter.project = None;
                self.status = "all projects".into();
            }
            SidebarRow::Project(p) => {
                self.project_scope = Some(p.clone());
                self.filter.project = Some(p.clone());
                self.status = format!("project: {p}");
            }
        }
        self.task_sel = 0;
    }

    /// Nudge the selected event's start (`start = true`) or end by `delta`, keeping the other
    /// edge fixed.
    fn adjust_selected_event(&mut self, start: bool, delta: Duration) {
        if let Some(uid) = self.selected_event_uid() {
            let r = if start {
                self.ctx.adjust_event_start(&uid, delta)
            } else {
                self.ctx.adjust_event_end(&uid, delta)
            };
            let what = if start { "start" } else { "end" };
            self.report(r.map(|_| format!("{what} adjusted")));
        }
    }

    fn delete_selected_event(&mut self) {
        if let Some(uid) = self.selected_event_uid() {
            let r = self.ctx.delete_event(&uid);
            self.report(r.map(|_| "deleted".into()));
        }
    }

    // ---- board ---------------------------------------------------------------------

    fn board_action(&mut self, action: Action) {
        let board = self.ctx.board(&self.filter);
        match action {
            Action::Left => self.board_col = self.board_col.saturating_sub(1),
            Action::Right => self.board_col = (self.board_col + 1).min(board.len().saturating_sub(1)),
            Action::Up => self.board_row = self.board_row.saturating_sub(1),
            Action::Down => self.board_row += 1,
            Action::MoveNext => self.move_card(1),
            Action::MovePrev => self.move_card(-1),
            Action::ToggleDone => self.toggle_selected_card_done(),
            Action::Delete => self.delete_selected_card(),
            _ => {}
        }
        self.clamp_board();
    }

    fn clamp_board(&mut self) {
        let board = self.ctx.board(&self.filter);
        if board.is_empty() {
            return;
        }
        self.board_col = self.board_col.min(board.len() - 1);
        let col_len = board[self.board_col].1.len();
        self.board_row = self.board_row.min(col_len.saturating_sub(1));
    }

    fn selected_card_uid(&self) -> Option<Uid> {
        let board = self.ctx.board(&self.filter);
        board.get(self.board_col).and_then(|(_, cards)| cards.get(self.board_row)).map(|t| t.uid.clone())
    }

    fn move_card(&mut self, dir: i32) {
        let Some(uid) = self.selected_card_uid() else { return };
        match self.ctx.move_task(&uid, dir) {
            Ok(new_status) => {
                // Follow the card to whichever column the workflow landed it in.
                if let Some(idx) = self.ctx.board(&self.filter).iter().position(|(s, _)| s == &new_status) {
                    self.board_col = idx;
                }
                self.status = "moved".into();
            }
            Err(e) => self.status = format!("error: {e}"),
        }
    }

    fn toggle_selected_card_done(&mut self) {
        if let Some(uid) = self.selected_card_uid() {
            let r = self.ctx.toggle_task_done(&uid);
            self.report(r.map(|_| "toggled".into()));
        }
    }

    fn delete_selected_card(&mut self) {
        if let Some(uid) = self.selected_card_uid() {
            let r = self.ctx.delete_task(&uid);
            self.report(r.map(|_| "deleted".into()));
        }
    }

    // ---- tasks ---------------------------------------------------------------------

    fn tasks_action(&mut self, action: Action) {
        // `h`/`l` move focus between the sidebar (smart views + projects) and the task list.
        match action {
            Action::Left => {
                self.sidebar_focus = true;
                return;
            }
            Action::Right => {
                self.sidebar_focus = false;
                return;
            }
            _ => {}
        }
        if self.sidebar_focus {
            let n = self.sidebar_rows().len();
            match action {
                Action::Up => self.sidebar_sel = self.sidebar_sel.saturating_sub(1),
                Action::Down => self.sidebar_sel = (self.sidebar_sel + 1).min(n.saturating_sub(1)),
                Action::ToggleDone | Action::Select => self.apply_sidebar_selection(),
                Action::Delete => self.begin_delete_project(),
                _ => {}
            }
            // Apply live as the highlight moves, so the list updates without an extra keypress.
            if matches!(action, Action::Up | Action::Down) {
                self.apply_sidebar_selection();
            }
            return;
        }
        let tasks = self.visible_tasks();
        match action {
            Action::Up => self.task_sel = self.task_sel.saturating_sub(1),
            Action::Down => self.task_sel = (self.task_sel + 1).min(tasks.len().saturating_sub(1)),
            Action::ToggleDone => {
                if let Some(t) = tasks.get(self.task_sel) {
                    let r = self.ctx.toggle_task_done(&t.uid);
                    self.report(r.map(|_| "toggled".into()));
                }
            }
            Action::Delete => {
                if let Some(t) = tasks.get(self.task_sel) {
                    let r = self.ctx.delete_task(&t.uid);
                    self.report(r.map(|_| "deleted".into()));
                }
            }
            _ => {}
        }
    }

    // ---- focus ---------------------------------------------------------------------

    fn focus_action(&mut self, action: Action) {
        match action {
            Action::StartPauseTimer => self.timer.toggle(),
            Action::SkipPhase => self.timer.skip(),
            _ => {}
        }
    }

    // ---- helpers -------------------------------------------------------------------

    fn try_undo(&mut self) -> mgmt_core::Result<String> {
        Ok(if self.ctx.undo()? { "undo".into() } else { "nothing to undo".into() })
    }

    fn try_redo(&mut self) -> mgmt_core::Result<String> {
        Ok(if self.ctx.redo()? { "redo".into() } else { "nothing to redo".into() })
    }

    fn report(&mut self, r: mgmt_core::Result<String>) {
        self.status = match r {
            Ok(s) => s,
            Err(e) => format!("error: {e}"),
        };
    }

    /// Execute a command from the palette by name. Commands are resolved from PALETTE_CMDS;
    /// context-sensitive commands (due-date, priority) operate on the current selection.
    fn execute_cmd(&mut self, cmd: &str) {
        let today = Local::now().date_naive();
        match cmd {
            "today"        => self.set_selected_due(today),
            "tomorrow"     => self.set_selected_due(today + Duration::days(1)),
            "monday"       => self.set_selected_due(next_weekday(today, Weekday::Mon)),
            "tuesday"      => self.set_selected_due(next_weekday(today, Weekday::Tue)),
            "wednesday"    => self.set_selected_due(next_weekday(today, Weekday::Wed)),
            "thursday"     => self.set_selected_due(next_weekday(today, Weekday::Thu)),
            "friday"       => self.set_selected_due(next_weekday(today, Weekday::Fri)),
            "saturday"     => self.set_selected_due(next_weekday(today, Weekday::Sat)),
            "sunday"       => self.set_selected_due(next_weekday(today, Weekday::Sun)),
            "end-of-week"  => self.set_selected_due(end_of_week(today)),
            "next-week"    => self.set_selected_due(next_monday(today)),
            "no-due"       => self.clear_selected_due(),
            "high"         => self.set_selected_priority(Priority::High),
            "medium"       => self.set_selected_priority(Priority::Medium),
            "low"          => self.set_selected_priority(Priority::Low),
            "no-priority"  => self.set_selected_priority(Priority::None),
            "done" => {
                if let Some(uid) = self.selected_task_uid() {
                    let r = self.ctx.toggle_task_done(&uid);
                    self.report(r.map(|s| format!("status: {s}")));
                } else {
                    self.status = "no task selected".into();
                }
            }
            "delete" => match self.tab {
                Tab::Calendar => self.delete_selected_event(),
                Tab::Board => self.delete_selected_card(),
                Tab::Tasks => {
                    if let Some(t) = self.visible_tasks().get(self.task_sel).cloned() {
                        let r = self.ctx.delete_task(&t.uid);
                        self.report(r.map(|_| "deleted".into()));
                    }
                }
                _ => self.status = "nothing to delete".into(),
            },
            "calendar" => self.tab = Tab::Calendar,
            "board"    => self.tab = Tab::Board,
            "tasks"    => self.tab = Tab::Tasks,
            "focus"    => self.tab = Tab::Focus,
            "new-task" => {
                if !matches!(self.tab, Tab::Board | Tab::Tasks) {
                    self.tab = Tab::Tasks;
                }
                self.begin_quick_add();
            }
            "new-event" => {
                self.tab = Tab::Calendar;
                self.begin_quick_add();
            }
            "undo" => { let r = self.try_undo(); self.report(r); }
            "redo" => { let r = self.try_redo(); self.report(r); }
            other => self.status = format!("unknown command: {other}"),
        }
    }

    /// Set the due date on the selected task (Board/Tasks), or move the selected event to the
    /// given date (Calendar) while preserving its start and end times.
    fn set_selected_due(&mut self, date: NaiveDate) {
        match self.tab {
            Tab::Board | Tab::Tasks => {
                if let Some(uid) = self.selected_task_uid() {
                    if let Some(mut t) = self.ctx.task(&uid).cloned() {
                        t.due = date.and_hms_opt(23, 59, 0).map(|dt| dt.and_utc());
                        if t.reminders.is_empty() {
                            t.reminders = self.ctx.default_reminders();
                        }
                        let r = self.ctx.put_task(t);
                        self.report(r.map(|_| format!("due: {}", date.format("%Y-%m-%d"))));
                    }
                } else {
                    self.status = "no task selected".into();
                }
            }
            Tab::Calendar => {
                if let Some(uid) = self.selected_event_uid() {
                    if let Some(mut ev) = self.ctx.event(&uid).cloned() {
                        let old_date = ev.start.date_naive();
                        let delta = date.signed_duration_since(old_date);
                        ev.start = ev.start + delta;
                        ev.end = ev.end + delta;
                        let r = self.ctx.put_event(ev);
                        self.day = date;
                        self.report(r.map(|_| format!("moved to {}", date.format("%Y-%m-%d"))));
                    }
                } else {
                    self.status = "no event selected".into();
                }
            }
            _ => self.status = "no item to reschedule".into(),
        }
    }

    fn clear_selected_due(&mut self) {
        if let Some(uid) = self.selected_task_uid() {
            if let Some(mut t) = self.ctx.task(&uid).cloned() {
                t.due = None;
                t.reminders.clear();
                let r = self.ctx.put_task(t);
                self.report(r.map(|_| "due cleared".into()));
            }
        } else {
            self.status = "no task selected".into();
        }
    }

    fn set_selected_priority(&mut self, priority: Priority) {
        if let Some(uid) = self.selected_task_uid() {
            if let Some(mut t) = self.ctx.task(&uid).cloned() {
                t.priority = priority;
                let label = match priority {
                    Priority::High => "high",
                    Priority::Medium => "medium",
                    Priority::Low => "low",
                    Priority::None => "none",
                };
                let r = self.ctx.put_task(t);
                self.report(r.map(|_| format!("priority: {label}")));
            }
        } else {
            self.status = "no task selected".into();
        }
    }

    // ---- rendering -----------------------------------------------------------------

    pub fn draw(&self, frame: &mut Frame, area: Rect) {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3),
                Constraint::Min(1),
                Constraint::Length(1), // keybinding hints
                Constraint::Length(1), // status
            ])
            .split(area);

        self.draw_tabs(frame, chunks[0]);
        match self.tab {
            Tab::Calendar => self.draw_calendar(frame, chunks[1]),
            Tab::Board => self.draw_board(frame, chunks[1]),
            Tab::Tasks => self.draw_tasks(frame, chunks[1]),
            Tab::Focus => self.draw_focus(frame, chunks[1]),
        }
        self.draw_hints(frame, chunks[2]);
        self.draw_status(frame, chunks[3]);

        match &self.modal {
            Some(Modal::Input { prompt, buffer, .. }) => self.draw_input(frame, area, prompt, buffer),
            Some(Modal::Task(form)) => self.draw_task_form(frame, area, form),
            Some(Modal::Event(form)) => self.draw_event_form(frame, area, form),
            Some(Modal::Picker(picker)) => self.draw_picker(frame, area, picker),
            Some(Modal::Confirm { prompt, .. }) => self.draw_confirm(frame, area, prompt),
            Some(Modal::Palette(cp)) => self.draw_command_palette(frame, area, cp),
            None => {}
        }
        if self.show_help {
            self.draw_help(frame, area);
        }
    }

    /// A lazygit-style context hint line of the most useful keys for the current view.
    fn hint_text(&self) -> &'static str {
        if let Some(Modal::Palette(_)) = &self.modal {
            return "type to fuzzy-filter · Tab complete · ↑↓/ctrl-n/p navigate · Enter run · Esc cancel";
        }
        if self.modal.is_some() {
            return "type to edit · Tab/Enter next · Enter submits · Esc cancel";
        }
        match self.tab {
            Tab::Calendar => match self.cal_focus {
                CalFocus::Date => "h/l day · j/k week · v cycle view · t today · Enter→agenda · a new · e edit · : cmd · ? help",
                CalFocus::Agenda => "j/k select · H/L start ±15m · J/K end ±15m · e edit · d delete · Enter→grid · a new · : cmd · ? help",
            },
            Tab::Board => "h/l col · j/k card · H/L move · space done · a add · e edit · p project · P prio · : cmd · ? help",
            Tab::Tasks if self.sidebar_focus => "j/k pick list · l→tasks · space apply · [ ] scope · : cmd · ? help",
            Tab::Tasks => "j/k select · h→lists · space done · a add · e edit · p project · P prio · : cmd · ? help",
            Tab::Focus => "space start/pause · s skip phase · Tab switch view · : cmd · ? help",
        }
    }

    fn draw_hints(&self, frame: &mut Frame, area: Rect) {
        let line = Line::from(Span::styled(
            format!(" {}", self.hint_text()),
            Style::default().fg(self.theme.accent),
        ));
        frame.render_widget(Paragraph::new(line), area);
    }

    fn draw_tabs(&self, frame: &mut Frame, area: Rect) {
        let titles: Vec<Line> = Tab::ALL.iter().map(|t| Line::from(t.title())).collect();
        let tabs = Tabs::new(titles)
            .block(Block::default().borders(Borders::ALL).title(" mgmt "))
            .select(self.tab.index())
            .highlight_style(Style::default().fg(self.theme.selected_fg).bg(self.theme.selected_bg).add_modifier(Modifier::BOLD));
        frame.render_widget(tabs, area);
    }

    fn draw_status(&self, frame: &mut Frame, area: Rect) {
        let dirty = if self.ctx.is_dirty() { " ●" } else { "" };
        let line = Line::from(vec![
            Span::styled(format!(" {}", self.status), Style::default().fg(self.theme.dim)),
            Span::styled(dirty.to_string(), Style::default().fg(self.theme.today)),
        ]);
        frame.render_widget(Paragraph::new(line), area);
    }

    fn draw_calendar(&self, frame: &mut Frame, area: Rect) {
        match self.cal_view {
            CalView::Month => {
                let cols = Layout::default()
                    .direction(Direction::Horizontal)
                    .constraints([Constraint::Length(24), Constraint::Min(20)])
                    .split(area);
                self.draw_month(frame, cols[0]);
                self.draw_agenda(frame, cols[1], false);
            }
            CalView::Week => self.draw_week(frame, area),
            CalView::Day => self.draw_day_grid(frame, area),
        }
    }

    /// A day timeline that lays overlapping ("colliding") events side-by-side in lanes, like a
    /// calendar app. Timed events are packed greedily into the fewest lanes; all-day events show
    /// in a header band.
    fn draw_day_grid(&self, frame: &mut Frame, area: Rect) {
        let events = self.visible_events_on(self.day);
        let (all_day, timed): (Vec<&Event>, Vec<&Event>) = events.iter().partition(|e| e.all_day);

        let query = self.event_query.as_deref().map(|q| format!(" /{q}")).unwrap_or_default();
        let title = format!(" {}{} ", self.day.format("%a %d %b %Y"), query);
        let block = Block::default()
            .borders(Borders::ALL)
            .title(title)
            .border_style(self.focus_border(CalFocus::Agenda));
        let inner = block.inner(area);
        frame.render_widget(block, area);
        if inner.width < 8 || inner.height < 2 {
            return;
        }

        // All-day band (one line) at the top.
        let mut grid = inner;
        if !all_day.is_empty() {
            let labels: Vec<String> = all_day.iter().map(|e| e.summary.clone()).collect();
            frame.render_widget(
                Paragraph::new(Line::from(Span::styled(
                    format!("all-day: {}", labels.join(" · ")),
                    Style::default().fg(self.theme.event),
                ))),
                Rect { height: 1, ..inner },
            );
            grid = Rect { y: inner.y + 1, height: inner.height - 1, ..inner };
        }
        if timed.is_empty() {
            frame.render_widget(
                Paragraph::new(Span::styled("(nothing scheduled — 'a' to add)", Style::default().fg(self.theme.dim))),
                grid,
            );
            return;
        }

        // Visible window: from the first event's hour to the last event's end-hour, min 6h.
        let span_min = |d: chrono::DateTime<chrono::Utc>| d.hour() as i64 * 60 + d.minute() as i64;
        let win_start = timed.iter().map(|e| span_min(e.start)).min().unwrap() / 60 * 60;
        let mut win_end = timed.iter().map(|e| (span_min(e.end)).max(span_min(e.start) + 30)).max().unwrap();
        win_end = ((win_end + 59) / 60) * 60;
        if win_end - win_start < 360 {
            win_end = (win_start + 360).min(24 * 60);
        }
        let win = (win_end - win_start).max(60) as u16;

        // Greedy lane packing: each event gets the first lane free at its start time.
        let mut lane_end: Vec<i64> = Vec::new();
        let mut lanes: Vec<usize> = Vec::with_capacity(timed.len());
        for e in &timed {
            let s = span_min(e.start);
            let lane = lane_end.iter().position(|&end| end <= s).unwrap_or_else(|| {
                lane_end.push(0);
                lane_end.len() - 1
            });
            lane_end[lane] = span_min(e.end).max(s + 1);
            lanes.push(lane);
        }
        let n_lanes = lane_end.len().max(1) as u16;

        let gutter = 6u16.min(grid.width.saturating_sub(2));
        let lane_w = (grid.width - gutter) / n_lanes;
        if lane_w == 0 {
            return;
        }
        let selected_uid = self.selected_event_uid();
        let buf = frame.buffer_mut();

        // Hour ruler in the gutter.
        let rows = grid.height;
        for h in (win_start / 60)..=(win_end / 60) {
            let m = h * 60;
            let y = grid.y + (((m - win_start) as u16) * rows / win);
            if y < grid.y + rows {
                buf.set_string(grid.x, y, format!("{:02}:00", h % 24), Style::default().fg(self.theme.dim));
            }
        }

        // Event blocks.
        for (i, e) in timed.iter().enumerate() {
            let s = span_min(e.start).max(win_start);
            let en = span_min(e.end).min(win_end);
            let y0 = grid.y + (((s - win_start) as u16) * rows / win);
            let y1 = (grid.y + (((en - win_start) as u16) * rows / win)).max(y0 + 1).min(grid.y + rows);
            let x0 = grid.x + gutter + lanes[i] as u16 * lane_w;
            let selected = self.cal_focus == CalFocus::Agenda && selected_uid.as_ref() == Some(&e.uid);
            let color = self.event_color(e);
            let fill = if selected {
                Style::default().bg(self.theme.selected_bg).fg(self.theme.selected_fg).add_modifier(Modifier::BOLD)
            } else {
                Style::default().bg(color).fg(Color::Black)
            };
            for y in y0..y1 {
                buf.set_string(x0, y, " ".repeat(lane_w as usize), fill);
            }
            // Label: time + summary, clipped to the lane width, on the first row of the block.
            let label = format!("{:02}:{:02} {}", e.start.hour(), e.start.minute(), e.summary);
            let clipped: String = label.chars().take(lane_w as usize).collect();
            buf.set_string(x0, y0, clipped, fill);
        }
    }

    fn draw_month(&self, frame: &mut Frame, area: Rect) {
        let first = self.day.with_day(1).unwrap();
        let title = format!(" {} ", self.day.format("%B %Y"));
        let mut lines: Vec<Line> = Vec::new();
        lines.push(Line::from(Span::styled("Mo Tu We Th Fr Sa Su", Style::default().fg(self.theme.accent))));

        let today = Local::now().date_naive();
        let lead = first.weekday().num_days_from_monday() as i64;
        let mut cur = first - Duration::days(lead);
        for _week in 0..6 {
            let mut spans: Vec<Span> = Vec::new();
            for d in 0..7 {
                let day = cur + Duration::days(d);
                let in_month = day.month() == self.day.month();
                let has_items = !self.visible_events_on(day).is_empty() || !self.ctx.tasks_on(day).is_empty();
                let label = format!("{:>2}", day.day());
                let mut style = if !in_month {
                    Style::default().fg(self.theme.dim)
                } else if has_items {
                    Style::default().fg(self.theme.event)
                } else {
                    Style::default()
                };
                if day == today {
                    style = style.fg(self.theme.today).add_modifier(Modifier::BOLD);
                }
                if day == self.day {
                    style = style.bg(self.theme.selected_bg).fg(self.theme.selected_fg).add_modifier(Modifier::BOLD);
                }
                spans.push(Span::styled(label, style));
                spans.push(Span::raw(" "));
            }
            lines.push(Line::from(spans));
            cur += Duration::days(7);
        }

        let border = self.focus_border(CalFocus::Date);
        let para = Paragraph::new(lines).block(
            Block::default().borders(Borders::ALL).title(title).border_style(border),
        );
        frame.render_widget(para, area);
    }

    fn draw_week(&self, frame: &mut Frame, area: Rect) {
        let monday = self.day - Duration::days(self.day.weekday().num_days_from_monday() as i64);
        let constraints: Vec<Constraint> = (0..7).map(|_| Constraint::Ratio(1, 7)).collect();
        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints(constraints)
            .split(area);
        let today = Local::now().date_naive();
        for i in 0..7 {
            let day = monday + Duration::days(i as i64);
            let events = self.visible_events_on(day);
            let mut items: Vec<ListItem> = Vec::new();
            for (ei, e) in events.iter().enumerate() {
                let selected = day == self.day && self.cal_focus == CalFocus::Agenda && ei == self.agenda_sel;
                let style = if selected {
                    Style::default().bg(self.theme.selected_bg).fg(self.theme.selected_fg)
                } else {
                    Style::default().fg(self.event_color(e))
                };
                let t = if e.all_day { "··".into() } else { format!("{:02}:{:02}", e.start.hour(), e.start.minute()) };
                items.push(ListItem::new(Line::from(format!("{t} {}", e.summary))).style(style));
            }
            for t in self.ctx.tasks_on(day) {
                items.push(ListItem::new(Line::from(format!("○ {}", t.title))).style(Style::default().fg(self.theme.task)));
            }
            let mut title_style = Style::default();
            if day == today {
                title_style = title_style.fg(self.theme.today).add_modifier(Modifier::BOLD);
            }
            let border = if day == self.day { self.focus_border(self.cal_focus) } else { Style::default().fg(self.theme.border) };
            let title = Span::styled(format!(" {} ", day.format("%a %d")), title_style);
            let list = List::new(items).block(
                Block::default().borders(Borders::ALL).title(title).border_style(border),
            );
            frame.render_widget(list, cols[i]);
        }
    }

    /// The day agenda. `full` renders an hour-prefixed day view; otherwise a compact list.
    fn draw_agenda(&self, frame: &mut Frame, area: Rect, full: bool) {
        let events = self.visible_events_on(self.day);
        let tasks = self.ctx.tasks_on(self.day);
        let mut items: Vec<ListItem> = Vec::new();
        for (i, e) in events.iter().enumerate() {
            let time = if e.all_day {
                "all-day".to_string()
            } else if full {
                format!("{:02}:{:02}-{:02}:{:02}", e.start.hour(), e.start.minute(), e.end.hour(), e.end.minute())
            } else {
                format!("{:02}:{:02}", e.start.hour(), e.start.minute())
            };
            let sel = self.cal_focus == CalFocus::Agenda && i == self.agenda_sel;
            let style = if sel {
                Style::default().bg(self.theme.selected_bg).fg(self.theme.selected_fg).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(self.event_color(e))
            };
            let proj = e.project.as_deref().map(|p| format!(" #{p}")).unwrap_or_default();
            let recur = if e.rrule.is_some() { " ↻" } else { "" };
            items.push(ListItem::new(Line::from(format!("{time}  {}{proj}{recur}", e.summary))).style(style));
        }
        let tasks: Vec<_> = tasks.into_iter().filter(|t| self.filter.matches(t)).collect();
        if !tasks.is_empty() {
            items.push(ListItem::new(Line::from(Span::styled("— tasks —", Style::default().fg(self.theme.dim)))));
            for t in &tasks {
                let mark = self.task_mark(&t.status);
                items.push(ListItem::new(Line::from(format!("{mark}  {}", t.title))).style(Style::default().fg(self.theme.task)));
            }
        }
        if items.is_empty() {
            items.push(ListItem::new(Line::from(Span::styled("(nothing scheduled — 'a' to add)", Style::default().fg(self.theme.dim)))));
        }
        let hint = if self.cal_focus == CalFocus::Agenda { " [agenda] " } else { "" };
        let query = self.event_query.as_deref().map(|q| format!(" /{q}")).unwrap_or_default();
        let title = format!(" {}{}{} ", self.day.format("%a %d %b %Y"), hint, query);
        let border = self.focus_border(CalFocus::Agenda);
        let list = List::new(items).block(
            Block::default().borders(Borders::ALL).title(title).border_style(border),
        );
        frame.render_widget(list, area);
    }

    fn focus_border(&self, pane: CalFocus) -> Style {
        if self.cal_focus == pane {
            Style::default().fg(self.theme.accent)
        } else {
            Style::default().fg(self.theme.border)
        }
    }

    fn draw_board(&self, frame: &mut Frame, area: Rect) {
        let board = self.ctx.board(&self.filter);
        let constraints: Vec<Constraint> = board.iter().map(|_| Constraint::Ratio(1, board.len() as u32)).collect();
        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints(constraints)
            .split(area);

        for (ci, (status, cards)) in board.iter().enumerate() {
            let mut items: Vec<ListItem> = Vec::new();
            for (ri, card) in cards.iter().enumerate() {
                let selected = ci == self.board_col && ri == self.board_row;
                items.push(ListItem::new(self.card_line(card, selected, false)));
            }
            let active = ci == self.board_col;
            let border_style = if active {
                Style::default().fg(self.status_color(status))
            } else {
                Style::default().fg(self.theme.border)
            };
            let title = Span::styled(
                format!(" {} ({}) ", self.ctx.status_label(status), cards.len()),
                Style::default().fg(self.status_color(status)).add_modifier(Modifier::BOLD),
            );
            let list = List::new(items).block(
                Block::default().borders(Borders::ALL).title(title).border_style(border_style),
            );
            frame.render_widget(list, cols[ci]);
        }
    }

    /// A task/card line as styled spans: a status mark, the title, a project tag in the
    /// project's color, and a priority marker. `selected` paints the selection background;
    /// `struck` strikes through completed tasks in the list view.
    fn card_line(&self, t: &Task, selected: bool, struck: bool) -> Line<'static> {
        let base = if selected {
            Style::default().bg(self.theme.selected_bg).fg(self.theme.selected_fg).add_modifier(Modifier::BOLD)
        } else if struck {
            Style::default().fg(self.theme.done).add_modifier(Modifier::CROSSED_OUT)
        } else {
            Style::default()
        };
        let mut spans = vec![Span::styled(t.title.clone(), base)];
        if let Some(p) = &t.project {
            let proj_style = if selected { base } else { base.fg(self.project_color(p)) };
            spans.push(Span::styled(format!(" #{p}"), proj_style));
        }
        let prio = match t.priority {
            Priority::High => " !!!",
            Priority::Medium => " !!",
            Priority::Low => " !",
            Priority::None => "",
        };
        if !prio.is_empty() {
            spans.push(Span::styled(prio, base));
        }
        Line::from(spans)
    }

    fn draw_tasks(&self, frame: &mut Frame, area: Rect) {
        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(22), Constraint::Min(20)])
            .split(area);
        self.draw_task_sidebar(frame, cols[0]);
        self.draw_task_list(frame, cols[1]);
    }

    /// The TickTick-style left rail: smart views, then the project list.
    fn draw_task_sidebar(&self, frame: &mut Frame, area: Rect) {
        let rows = self.sidebar_rows();
        let mut items: Vec<ListItem> = Vec::new();
        for (i, row) in rows.iter().enumerate() {
            let selected = self.sidebar_focus && i == self.sidebar_sel;
            let active = !self.sidebar_focus && self.row_is_active(row);
            let base = if selected {
                Style::default().bg(self.theme.selected_bg).fg(self.theme.selected_fg).add_modifier(Modifier::BOLD)
            } else if active {
                Style::default().fg(self.theme.accent).add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            let line = match row {
                SidebarRow::View(v) => Line::from(Span::styled(format!("  {}", v.label()), base)),
                SidebarRow::AllProjects => Line::from(Span::styled("  All projects", base)),
                SidebarRow::Project(p) => {
                    let dot = if selected { base } else { base.fg(self.project_color(p)) };
                    Line::from(vec![Span::styled("  ● ", dot), Span::styled(p.clone(), base)])
                }
            };
            items.push(ListItem::new(line));
        }
        let border = if self.sidebar_focus { self.theme.accent } else { self.theme.border };
        let list = List::new(items).block(
            Block::default().borders(Borders::ALL).title(" Lists ").border_style(Style::default().fg(border)),
        );
        frame.render_widget(list, area);
    }

    fn draw_task_list(&self, frame: &mut Frame, area: Rect) {
        let tasks = self.visible_tasks();
        let mut items: Vec<ListItem> = Vec::new();
        for (i, t) in tasks.iter().enumerate() {
            let mark = self.task_mark(&t.status);
            let sel = !self.sidebar_focus && i == self.task_sel;
            let struck = self.ctx.workflow().is_done(&t.status);
            let mut line = self.card_line(t, sel, struck);
            line.spans.insert(0, Span::styled(format!("{mark} "), Style::default().fg(self.status_color(&t.status))));
            items.push(ListItem::new(line));
        }
        if items.is_empty() {
            items.push(ListItem::new(Line::from(Span::styled("(no tasks — 'a' to add)", Style::default().fg(self.theme.dim)))));
        }
        let border = if self.sidebar_focus { self.theme.border } else { self.theme.accent };
        let list = List::new(items).block(
            Block::default().borders(Borders::ALL).title(self.list_title(self.task_view.label())).border_style(Style::default().fg(border)),
        );
        frame.render_widget(list, area);
    }

    /// The glyph for a status, by semantic kind.
    fn task_mark(&self, status: &str) -> &'static str {
        match self.ctx.workflow().kind(status) {
            StatusKind::Done => "✓",
            StatusKind::Cancelled => "✗",
            StatusKind::Active => "▸",
            StatusKind::Open => "○",
        }
    }

    /// Whether a sidebar row reflects the currently-applied view/scope (for the unfocused hint).
    fn row_is_active(&self, row: &SidebarRow) -> bool {
        match row {
            SidebarRow::View(v) => self.project_scope.is_none() && *v == self.task_view,
            SidebarRow::AllProjects => self.project_scope.is_none(),
            SidebarRow::Project(p) => self.project_scope.as_deref() == Some(p.as_str()),
        }
    }

    /// A panel title that reflects the active project scope and text filter.
    fn list_title(&self, base: &str) -> String {
        let mut t = format!(" {base}");
        if let Some(p) = &self.project_scope {
            t.push_str(&format!("  #{p}"));
        }
        if let Some(q) = &self.filter.text {
            t.push_str(&format!("  /{q}"));
        }
        t.push(' ');
        t
    }

    fn draw_focus(&self, frame: &mut Frame, area: Rect) {
        let elapsed = self.timer.elapsed();
        let (label, target) = match self.timer.phase {
            Phase::Focus { target } => ("FOCUS", target),
            Phase::Break { target } => ("BREAK", target),
        };
        let remaining = target.map(|t| t.saturating_sub(elapsed)).unwrap_or(elapsed);
        let mins = remaining.as_secs() / 60;
        let secs = remaining.as_secs() % 60;
        let state = if self.timer.running() { "running" } else { "paused" };
        let big = format!("{label}\n\n{:02}:{:02}\n\n[{state}]", mins, secs);
        let hint = "\n\nspace: start/pause   s: skip phase";
        let para = Paragraph::new(format!("{big}{hint}"))
            .alignment(Alignment::Center)
            .block(Block::default().borders(Borders::ALL).title(" Pomodoro "));
        frame.render_widget(para, area);
    }

    fn draw_input(&self, frame: &mut Frame, area: Rect, prompt: &str, buffer: &str) {
        let rect = centered(area, 60, 3);
        frame.render_widget(Clear, rect);
        let para = Paragraph::new(format!("{buffer}_")).block(
            Block::default()
                .borders(Borders::ALL)
                .title(format!(" {prompt} (enter to confirm, esc to cancel) "))
                .border_style(Style::default().fg(self.theme.accent)),
        );
        frame.render_widget(para, rect);
    }

    fn draw_task_form(&self, frame: &mut Frame, area: Rect, form: &TaskForm) {
        let rect = centered(area, 60, 9);
        frame.render_widget(Clear, rect);
        let prio = match form.priority {
            Priority::None => "none",
            Priority::Low => "low",
            Priority::Medium => "medium",
            Priority::High => "high",
        };
        let fields: [(&str, &str); 4] = [
            ("Title", &form.title),
            ("Project", &form.project),
            ("Due (opt)", &form.due),
            ("Priority", prio),
        ];
        let mut lines = Vec::new();
        for (i, (label, val)) in fields.iter().enumerate() {
            let active = i == form.field;
            let is_prio = i == TaskForm::PRIORITY_FIELD;
            let cursor = if active && !is_prio { "_" } else { "" };
            let shown = if is_prio { format!("‹ {val} ›") } else { format!("{val}{cursor}") };
            let style = if active {
                Style::default().fg(self.theme.accent).add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            lines.push(Line::from(Span::styled(format!("{label:>12}: {shown}"), style)));
        }
        // Live project suggestions when editing the project field.
        if form.field == 1 && !form.project.trim().is_empty() {
            let q = form.project.trim();
            let matches: Vec<String> = self
                .ctx
                .projects()
                .into_iter()
                .filter(|p| fuzzy_score(q, p).is_some())
                .take(4)
                .collect();
            let hint = if matches.is_empty() {
                format!("              ↳ new project '{q}'")
            } else {
                format!("              ↳ {}", matches.join("  "))
            };
            lines.push(Line::from(Span::styled(hint, Style::default().fg(self.theme.dim))));
        } else {
            lines.push(Line::from(""));
        }
        lines.push(Line::from(Span::styled(
            "Enter: save · Tab: next field · ←/→ priority · Esc cancel",
            Style::default().fg(self.theme.dim),
        )));
        let para = Paragraph::new(lines).block(
            Block::default().borders(Borders::ALL).title(" New task ").border_style(Style::default().fg(self.theme.accent)),
        );
        frame.render_widget(para, rect);
    }

    fn draw_event_form(&self, frame: &mut Frame, area: Rect, form: &EventForm) {
        let rect = centered(area, 60, 11);
        frame.render_widget(Clear, rect);
        let recur = form.recur.label().to_string();
        let fields: [(&str, &str); 7] = [
            ("Summary", &form.summary),
            ("Date (YYYY-MM-DD)", &form.date),
            ("Start (HH:MM)", &form.start),
            ("End (HH:MM)", &form.end),
            ("Location", &form.location),
            ("Project", &form.project),
            ("Repeats", &recur),
        ];
        let mut lines = Vec::new();
        for (i, (label, val)) in fields.iter().enumerate() {
            let active = i == form.field;
            let is_recur = i == EventForm::RECUR_FIELD;
            let cursor = if active && !is_recur { "_" } else { "" };
            let shown = if is_recur { format!("‹ {val} ›") } else { format!("{val}{cursor}") };
            let style = if active {
                Style::default().fg(self.theme.accent).add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            lines.push(Line::from(Span::styled(format!("{label:>18}: {shown}"), style)));
        }
        lines.push(Line::from(Span::styled(
            "Tab/Enter next · ←/→ change repeats · Enter on last: save · Esc cancel",
            Style::default().fg(self.theme.dim),
        )));
        let title = if form.edit_uid.is_some() { " Edit event " } else { " New event " };
        let para = Paragraph::new(lines).block(
            Block::default().borders(Borders::ALL).title(title).border_style(Style::default().fg(self.theme.accent)),
        );
        frame.render_widget(para, rect);
    }

    fn draw_picker(&self, frame: &mut Frame, area: Rect, picker: &Picker) {
        let entries = picker.entries();
        let h = (entries.len() as u16 + 4).min(area.height).max(5);
        let rect = centered(area, 44, h);
        frame.render_widget(Clear, rect);

        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(1), Constraint::Min(1)])
            .split(rect.inner(ratatui::layout::Margin { horizontal: 1, vertical: 1 }));

        let query = Line::from(vec![
            Span::styled("› ", Style::default().fg(self.theme.accent)),
            Span::raw(picker.query.clone()),
            Span::styled("_", Style::default().fg(self.theme.dim)),
        ]);

        let items: Vec<ListItem> = entries
            .iter()
            .enumerate()
            .map(|(i, e)| {
                let selected = i == picker.sel;
                let base = if selected {
                    Style::default().bg(self.theme.selected_bg).fg(self.theme.selected_fg).add_modifier(Modifier::BOLD)
                } else {
                    Style::default()
                };
                let line = match e {
                    PickEntry::None => Line::from(Span::styled("  (clear project)", base.fg(if selected { self.theme.selected_fg } else { self.theme.dim }))),
                    PickEntry::New(name) => Line::from(Span::styled(format!("  + new: {name}"), base.fg(if selected { self.theme.selected_fg } else { self.theme.event }))),
                    PickEntry::Project(name) => {
                        let dot = if selected { base } else { base.fg(self.project_color(name)) };
                        Line::from(vec![Span::styled("  ● ", dot), Span::styled(name.clone(), base)])
                    }
                };
                ListItem::new(line)
            })
            .collect();

        frame.render_widget(
            Block::default()
                .borders(Borders::ALL)
                .title(" Project (type to filter · ↑↓ · enter · esc) ")
                .border_style(Style::default().fg(self.theme.accent)),
            rect,
        );
        frame.render_widget(Paragraph::new(query), rows[0]);
        frame.render_widget(List::new(items), rows[1]);
    }

    fn draw_confirm(&self, frame: &mut Frame, area: Rect, prompt: &str) {
        let rect = centered(area, 60, 4);
        frame.render_widget(Clear, rect);
        let lines = vec![
            Line::from(Span::raw(prompt.to_string())),
            Line::from(Span::styled("y/enter: confirm   ·   n/esc: cancel", Style::default().fg(self.theme.dim))),
        ];
        let para = Paragraph::new(lines).block(
            Block::default().borders(Borders::ALL).title(" Confirm ").border_style(Style::default().fg(self.theme.today)),
        );
        frame.render_widget(para, rect);
    }

    fn draw_command_palette(&self, frame: &mut Frame, area: Rect, cp: &CommandPalette) {
        let entries = cp.entries();
        let content_h = (entries.len() as u16).min(14);
        // border(2) + query(1) + separator(1) + list
        let h = (content_h + 4).max(5).min(area.height);
        let w = 70u16.min(area.width);
        let rect = centered(area, w, h);
        frame.render_widget(Clear, rect);
        frame.render_widget(
            Block::default()
                .borders(Borders::ALL)
                .title(" : command  (tab complete · ↑↓ navigate · enter run · esc cancel) ")
                .border_style(Style::default().fg(self.theme.accent)),
            rect,
        );

        let inner = rect.inner(ratatui::layout::Margin { horizontal: 1, vertical: 1 });
        if inner.height < 2 {
            return;
        }
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(1), Constraint::Length(1), Constraint::Min(0)])
            .split(inner);
        let (query_area, sep_area, list_area) = (rows[0], rows[1], rows[2]);

        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled("› ", Style::default().fg(self.theme.accent)),
                Span::raw(cp.query.clone()),
                Span::styled("_", Style::default().fg(self.theme.dim)),
            ])),
            query_area,
        );
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "─".repeat(inner.width as usize),
                Style::default().fg(self.theme.border),
            ))),
            sep_area,
        );

        let list_h = list_area.height as usize;
        let visible_start = cp.sel.saturating_sub(list_h.saturating_sub(1));
        let items: Vec<ListItem> = entries
            .iter()
            .enumerate()
            .skip(visible_start)
            .take(list_h)
            .map(|(i, cmd)| {
                let sel = i == cp.sel;
                let base = if sel {
                    Style::default().bg(self.theme.selected_bg).fg(self.theme.selected_fg).add_modifier(Modifier::BOLD)
                } else {
                    Style::default()
                };
                let desc_style = if sel { base } else { Style::default().fg(self.theme.dim) };
                let indicator = if sel { "▶ " } else { "  " };
                ListItem::new(Line::from(vec![
                    Span::styled(format!("{indicator}{:<20}", cmd.name), base),
                    Span::styled(cmd.desc, desc_style),
                ]))
            })
            .collect();
        frame.render_widget(List::new(items), list_area);
    }

    fn draw_help(&self, frame: &mut Frame, area: Rect) {
        let rect = centered(area, 72, 18);
        frame.render_widget(Clear, rect);
        let help = "\
Global    tab: switch view   :: command palette   u: undo   ctrl-r: redo   q: quit   ?: help
Calendar  h/l: day   j/k: week/day or event   v: month/week/day   enter: focus agenda
          H/L: start ∓15m   J/K: end ±15m   a: new event   e: edit event   d: delete   /: search
Board     h/l: column   j/k: card   H/L: move card   space: done
          a: add   e: edit   p: project   P: priority   d: delete   /: search
Tasks     h: lists sidebar   l: task list   j/k: move   space: done/apply
          a: add   e: edit   p: project   P: priority   d: delete   /: search
Focus     space: start/pause   s: skip phase
Commands  today/tomorrow/monday…sunday/eow/nw: reschedule   high/med/low: priority   done: toggle";
        let para = Paragraph::new(help)
            .wrap(Wrap { trim: false })
            .block(Block::default().borders(Borders::ALL).title(" Help (any key to close) "));
        frame.render_widget(para, rect);
    }
}

/// Cycle a priority up (`dir > 0`) or down through None→Low→Medium→High (clamped).
fn cycle_priority(p: Priority, dir: i32) -> Priority {
    let order = [Priority::None, Priority::Low, Priority::Medium, Priority::High];
    let i = order.iter().position(|x| *x == p).unwrap_or(0) as i32;
    order[(i + dir).clamp(0, order.len() as i32 - 1) as usize]
}

/// Parse a due-date string into a UTC instant (end of that day). Accepts `YYYY-MM-DD`, `today`,
/// `tomorrow`, or `+Nd` (N days from today). Returns `None` on anything else.
fn parse_due(s: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    let today = Local::now().date_naive();
    let date = match s.trim().to_lowercase().as_str() {
        "today" => today,
        "tomorrow" => today + Duration::days(1),
        other if other.starts_with('+') && other.ends_with('d') => {
            let n: i64 = other[1..other.len() - 1].parse().ok()?;
            today + Duration::days(n)
        }
        other => NaiveDate::parse_from_str(other, "%Y-%m-%d").ok()?,
    };
    // Anchor at end of day so a "due today" task still counts as due within today.
    Some(date.and_hms_opt(23, 59, 0)?.and_utc())
}

/// Parse `HH:MM` against a date into a UTC instant. Returns `None` on malformed input.
fn parse_hhmm(day: NaiveDate, s: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    let (h, m) = s.trim().split_once(':')?;
    let h: u32 = h.parse().ok()?;
    let m: u32 = m.parse().ok()?;
    let naive = day.and_hms_opt(h, m, 0)?;
    Some(naive.and_utc())
}

/// Next occurrence of `target` weekday after `today` (same weekday → next week).
fn next_weekday(today: NaiveDate, target: Weekday) -> NaiveDate {
    let td = today.weekday().num_days_from_monday() as i64;
    let tg = target.num_days_from_monday() as i64;
    let d = (tg - td).rem_euclid(7);
    today + Duration::days(if d == 0 { 7 } else { d })
}

/// Friday of the ISO week containing `today` (Mon–Sun). If today is already past Friday, it
/// still returns this week's Friday rather than next week's.
fn end_of_week(today: NaiveDate) -> NaiveDate {
    let mon = today - Duration::days(today.weekday().num_days_from_monday() as i64);
    mon + Duration::days(4)
}

/// The Monday starting the next ISO week.
fn next_monday(today: NaiveDate) -> NaiveDate {
    next_weekday(today, Weekday::Mon)
}

/// A centered rectangle `w`×`h` (clamped to `area`).
fn centered(area: Rect, w: u16, h: u16) -> Rect {
    let w = w.min(area.width);
    let h = h.min(area.height);
    let x = area.x + (area.width - w) / 2;
    let y = area.y + (area.height - h) / 2;
    Rect::new(x, y, w, h)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyModifiers;
    use mgmt_store::{VaultStore, VdirStore};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn app() -> MgmtApp {
        let dir = tempfile::tempdir().unwrap();
        let vault = VaultStore::new(dir.path().join("tasks"));
        let vdir = VdirStore::new(dir.path().join("calendars"));
        std::mem::forget(dir);
        MgmtApp::new(MgmtContext::open(vault, vdir).unwrap())
    }

    fn key(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
    }
    fn special(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn render(app: &mut MgmtApp) -> String {
        let backend = TestBackend::new(110, 34);
        let mut term = Terminal::new(backend).unwrap();
        term.draw(|f| app.draw(f, f.area())).unwrap();
        let buf = term.backend().buffer().clone();
        buf.content().iter().map(|c| c.symbol()).collect()
    }

    #[test]
    fn renders_every_tab_without_panicking() {
        let mut app = app();
        for _ in 0..Tab::ALL.len() {
            render(&mut app);
            app.handle_key(special(KeyCode::Tab));
        }
    }

    #[test]
    fn quick_add_flow_creates_a_task() {
        let mut app = app();
        app.handle_key(special(KeyCode::Tab)); // Board
        app.handle_key(special(KeyCode::Tab)); // Tasks
        assert_eq!(app.context(), Context::Tasks);
        app.handle_key(key('a'));
        assert_eq!(app.context(), Context::Form); // the task-creation form
        for c in "milk".chars() {
            app.handle_key(key(c));
        }
        app.handle_key(special(KeyCode::Enter)); // Enter submits from the title field
        assert_eq!(app.context_mut().tasks().len(), 1);
        assert_eq!(app.context_mut().tasks()[0].title, "milk");
    }

    #[test]
    fn task_form_sets_due_and_priority() {
        let mut app = app();
        app.handle_key(special(KeyCode::Tab));
        app.handle_key(special(KeyCode::Tab)); // Tasks
        app.handle_key(key('a'));
        for c in "report".chars() {
            app.handle_key(key(c));
        }
        app.handle_key(special(KeyCode::Tab)); // -> project
        app.handle_key(special(KeyCode::Tab)); // -> due
        for c in "2026-07-01".chars() {
            app.handle_key(key(c));
        }
        app.handle_key(special(KeyCode::Tab)); // -> priority
        app.handle_key(special(KeyCode::Right)); // none -> low
        app.handle_key(special(KeyCode::Enter)); // submit
        let tasks = app.context_mut().tasks();
        assert_eq!(tasks.len(), 1);
        assert!(tasks[0].due.is_some());
        assert_eq!(tasks[0].priority, Priority::Low);
    }

    #[test]
    fn day_grid_renders_overlapping_events_without_panic() {
        let mut app = app();
        let today = Local::now().date_naive();
        // two events that collide (10:00–11:00 and 10:30–11:30)
        for (h, m, name) in [(10u32, 0u32, "a"), (10, 30, "b")] {
            let s = today.and_hms_opt(h, m, 0).unwrap().and_utc();
            let e = today.and_hms_opt(h + 1, m, 0).unwrap().and_utc();
            app.context_mut().put_event(Event::new("work", name, s, e)).unwrap();
        }
        app.handle_key(key('v')); // month -> week
        app.handle_key(key('v')); // week -> day
        assert_eq!(app.cal_view, CalView::Day);
        let screen = render(&mut app);
        assert!(screen.contains('a') && screen.contains('b'));
    }

    #[test]
    fn calendar_view_cycles_month_week_day() {
        let mut app = app();
        assert_eq!(app.cal_view, CalView::Month);
        app.handle_key(key('v'));
        assert_eq!(app.cal_view, CalView::Week);
        app.handle_key(key('v'));
        assert_eq!(app.cal_view, CalView::Day);
        app.handle_key(key('v'));
        assert_eq!(app.cal_view, CalView::Month);
    }

    #[test]
    fn enter_focuses_agenda_and_jk_selects_events() {
        let mut app = app();
        // two events today
        let today = Local::now().date_naive();
        for (h, name) in [(9u32, "a"), (11u32, "b")] {
            let s = today.and_hms_opt(h, 0, 0).unwrap().and_utc();
            let e = today.and_hms_opt(h + 1, 0, 0).unwrap().and_utc();
            app.context_mut().put_event(Event::new("work", name, s, e)).unwrap();
        }
        assert_eq!(app.cal_focus, CalFocus::Date);
        app.handle_key(special(KeyCode::Enter));
        assert_eq!(app.cal_focus, CalFocus::Agenda);
        assert_eq!(app.agenda_sel, 0);
        app.handle_key(key('j'));
        assert_eq!(app.agenda_sel, 1);
        app.handle_key(key('k'));
        assert_eq!(app.agenda_sel, 0);
    }

    #[test]
    fn event_form_creates_event_with_times() {
        let mut app = app();
        app.handle_key(key('a')); // calendar -> event form
        assert_eq!(app.context(), Context::Form);
        for c in "Lunch".chars() {
            app.handle_key(key(c));
        }
        // Walk the form fields (summary → date → start → end → location → project → recur),
        // then submit. Defaults: date = selected day, start 09:00, end 10:00.
        app.handle_key(special(KeyCode::Enter)); // -> date
        app.handle_key(special(KeyCode::Enter)); // -> start
        app.handle_key(special(KeyCode::Enter)); // -> end
        app.handle_key(special(KeyCode::Enter)); // -> location
        app.handle_key(special(KeyCode::Enter)); // -> project
        app.handle_key(special(KeyCode::Enter)); // -> recur
        app.handle_key(special(KeyCode::Enter)); // submit
        let events = app.context_mut().events();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].summary, "Lunch");
        assert_eq!(events[0].start.hour(), 9);
        assert_eq!(events[0].end.hour(), 10);
    }

    #[test]
    fn fuzzy_project_picker_creates_and_assigns() {
        let mut app = app();
        app.context_mut().quick_add("task", None).unwrap();
        app.handle_key(special(KeyCode::Tab)); // Board
        app.handle_key(special(KeyCode::Tab)); // Tasks
        app.handle_key(key('p')); // open fuzzy picker
        assert_eq!(app.context(), Context::Picker);
        // Type a new project name; with no existing match the top entry is "+ new: wng".
        for c in "wng".chars() {
            app.handle_key(key(c));
        }
        app.handle_key(special(KeyCode::Enter));
        assert_eq!(app.context_mut().tasks()[0].project.as_deref(), Some("wng"));
    }

    #[test]
    fn fuzzy_project_picker_filters_existing() {
        let mut app = app();
        app.context_mut().add_project("workmux").unwrap();
        app.context_mut().add_project("home").unwrap();
        let uid = app.context_mut().quick_add("t", None).unwrap();
        app.handle_key(special(KeyCode::Tab));
        app.handle_key(special(KeyCode::Tab)); // Tasks
        app.handle_key(key('p'));
        for c in "work".chars() {
            app.handle_key(key(c));
        }
        // top match is "workmux"; Enter assigns it
        app.handle_key(special(KeyCode::Enter));
        assert_eq!(app.context_mut().task(&uid).unwrap().project.as_deref(), Some("workmux"));
    }

    #[test]
    fn delete_project_confirm_flow() {
        let mut app = app();
        app.context_mut().add_project("doomed").unwrap();
        let uid = app.context_mut().quick_add("t", Some("doomed".into())).unwrap();
        app.handle_key(special(KeyCode::Tab));
        app.handle_key(special(KeyCode::Tab)); // Tasks
        app.handle_key(key('h')); // focus sidebar
        // move down to the "doomed" project row and delete it
        for _ in 0..10 {
            app.handle_key(key('j'));
        }
        // select the project row explicitly: find it
        // (sidebar: views…, All projects, doomed) — walk until on a project
        app.handle_key(key('d'));
        assert_eq!(app.context(), Context::Confirm);
        app.handle_key(key('y'));
        assert!(!app.context_mut().projects().contains(&"doomed".to_string()));
        assert_eq!(app.context_mut().task(&uid).unwrap().project, None);
    }

    #[test]
    fn edit_returns_open_editor_with_task_path() {
        let mut app = app();
        app.context_mut().quick_add("editable", None).unwrap();
        app.handle_key(special(KeyCode::Tab));
        app.handle_key(special(KeyCode::Tab)); // Tasks
        let outcome = app.handle_key(key('e'));
        match outcome {
            Outcome::OpenEditor(p) => assert!(p.to_string_lossy().ends_with(".md")),
            other => panic!("expected OpenEditor, got {other:?}"),
        }
    }

    #[test]
    fn q_quits() {
        let mut app = app();
        assert_eq!(app.handle_key(key('q')), Outcome::Quit);
    }
}
