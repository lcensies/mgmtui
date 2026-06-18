//! `MgmtApp` — the embeddable aggregate that owns app state and renders the views. It never
//! touches the terminal: a host calls [`MgmtApp::draw`] with a `Frame`/`Rect` and feeds keys
//! to [`MgmtApp::handle_key`]. The standalone `mgmt` binary provides the terminal + loop; the
//! wng dashboard can host it the same way.

use std::path::PathBuf;
use std::time::{Duration as StdDuration, Instant};

use chrono::{Datelike, Duration, Local, NaiveDate, Timelike};
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Tabs, Wrap};

use mgmt_core::Uid;
use mgmt_domain::{Event, Filter, Priority, SortMode, TaskStatus};
use mgmt_service::{MgmtContext, Phase, Pomodoro, Technique};

use crate::keymap::{Action, Context, action_for_key};
use crate::theme::Theme;

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
    /// Event-creation form.
    Event(EventForm),
    /// List picker (projects).
    Picker(Picker),
}

enum InputPurpose {
    QuickAddTask,
    NewProjectFor(Uid),
}

struct EventForm {
    day: NaiveDate,
    summary: String,
    start: String, // HH:MM
    end: String,   // HH:MM
    field: usize,  // 0=summary 1=start 2=end
}

impl EventForm {
    fn new(day: NaiveDate) -> Self {
        EventForm {
            day,
            summary: String::new(),
            start: "09:00".to_string(),
            end: "10:00".to_string(),
            field: 0,
        }
    }

    fn field_mut(&mut self) -> &mut String {
        match self.field {
            0 => &mut self.summary,
            1 => &mut self.start,
            _ => &mut self.end,
        }
    }
}

struct Picker {
    target: Uid,
    items: Vec<String>, // includes synthetic "(none)" and "(new project…)"
    sel: usize,
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

    // focus
    timer: Timer,

    modal: Option<Modal>,
    show_help: bool,
    status: String,
}

impl MgmtApp {
    pub fn new(ctx: MgmtContext) -> Self {
        MgmtApp {
            ctx,
            theme: Theme::default(),
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
            timer: Timer::new(),
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
            Some(Modal::Event(_)) => Context::Form,
            Some(Modal::Picker(_)) => Context::Picker,
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
            Action::QuickAdd => self.begin_quick_add(),
            Action::Edit => return self.edit_selected(),
            Action::EditProject => self.begin_project_picker(),
            Action::CyclePriority => self.cycle_priority(),
            other => match self.tab {
                Tab::Calendar => self.calendar_action(other),
                Tab::Board => self.board_action(other),
                Tab::Tasks => self.tasks_action(other),
                Tab::Focus => self.focus_action(other),
            },
        }
        Outcome::Continue
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
            Modal::Event(mut form) => match key.code {
                KeyCode::Esc => {}
                KeyCode::Enter | KeyCode::Tab => {
                    if form.field < 2 {
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
                KeyCode::Backspace => {
                    form.field_mut().pop();
                    self.modal = Some(Modal::Event(form));
                }
                KeyCode::Char(c) => {
                    form.field_mut().push(c);
                    self.modal = Some(Modal::Event(form));
                }
                _ => self.modal = Some(Modal::Event(form)),
            },
            Modal::Picker(mut picker) => match key.code {
                KeyCode::Esc => {}
                KeyCode::Char('j') | KeyCode::Down => {
                    picker.sel = (picker.sel + 1).min(picker.items.len().saturating_sub(1));
                    self.modal = Some(Modal::Picker(picker));
                }
                KeyCode::Char('k') | KeyCode::Up => {
                    picker.sel = picker.sel.saturating_sub(1);
                    self.modal = Some(Modal::Picker(picker));
                }
                KeyCode::Enter => self.pick_project(picker),
                _ => self.modal = Some(Modal::Picker(picker)),
            },
        }
        Outcome::Continue
    }

    fn begin_quick_add(&mut self) {
        // On the calendar tab, `a` opens the event form instead of a task quick-add.
        if self.tab == Tab::Calendar {
            self.modal = Some(Modal::Event(EventForm::new(self.day)));
            return;
        }
        self.modal = Some(Modal::Input {
            prompt: "New task".to_string(),
            buffer: String::new(),
            purpose: InputPurpose::QuickAddTask,
        });
    }

    fn submit_input(&mut self, purpose: InputPurpose, text: String) {
        if text.is_empty() {
            return;
        }
        match purpose {
            InputPurpose::QuickAddTask => match self.ctx.quick_add(text, self.filter.project.clone()) {
                Ok(_) => self.status = "added".into(),
                Err(e) => self.status = format!("error: {e}"),
            },
            InputPurpose::NewProjectFor(uid) => {
                let r = self.ctx.set_task_project(&uid, Some(text));
                self.report(r.map(|_| "project set".into()));
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
        let (Some(start), Some(end)) = (parse_hhmm(form.day, &form.start), parse_hhmm(form.day, &form.end)) else {
            self.status = "times must be HH:MM".into();
            self.modal = Some(Modal::Event(form));
            return;
        };
        let calendar = self
            .ctx
            .events()
            .first()
            .map(|e| e.calendar.clone())
            .unwrap_or_else(|| "default".to_string());
        let ev = Event::new(calendar, summary, start, end);
        let r = self.ctx.put_event(ev);
        self.report(r.map(|_| "event created".into()));
    }

    fn begin_project_picker(&mut self) {
        let Some(uid) = self.selected_task_uid() else { return };
        let mut items = vec!["(none)".to_string()];
        items.extend(self.ctx.projects());
        items.push("(new project…)".to_string());
        self.modal = Some(Modal::Picker(Picker { target: uid, items, sel: 0 }));
    }

    fn pick_project(&mut self, picker: Picker) {
        let choice = picker.items[picker.sel].clone();
        if choice == "(new project…)" {
            self.modal = Some(Modal::Input {
                prompt: "New project name".to_string(),
                buffer: String::new(),
                purpose: InputPurpose::NewProjectFor(picker.target),
            });
            return;
        }
        let project = if choice == "(none)" { None } else { Some(choice) };
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
                let tasks = self.ctx.filtered_tasks(&self.filter, self.sort);
                tasks.get(self.task_sel).map(|t| t.uid.clone())
            }
            _ => None,
        }
    }

    /// Resolve the file to edit for the current selection and ask the host to open `$EDITOR`.
    fn edit_selected(&mut self) -> Outcome {
        let path = match self.tab {
            Tab::Calendar => self.selected_event_uid().and_then(|u| self.ctx.event_file(&u).ok().flatten()),
            Tab::Board | Tab::Tasks => self.selected_task_uid().and_then(|u| self.ctx.task_file(&u)),
            Tab::Focus => None,
        };
        match path {
            Some(p) => Outcome::OpenEditor(p),
            None => {
                self.status = "nothing to edit".into();
                Outcome::Continue
            }
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
            Action::ShiftLater => self.reschedule_selected(Duration::minutes(15)),
            Action::ShiftEarlier => self.reschedule_selected(Duration::minutes(-15)),
            Action::Delete => self.delete_selected_event(),
            _ => {}
        }
        let count = self.ctx.events_on(self.day).len();
        if self.agenda_sel >= count {
            self.agenda_sel = count.saturating_sub(1);
        }
    }

    fn calendar_vertical(&mut self, action: Action) {
        let down = action == Action::Down;
        match self.cal_focus {
            CalFocus::Agenda => {
                // move the event selection within the day
                let count = self.ctx.events_on(self.day).len();
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
        self.ctx.events_on(self.day).get(self.agenda_sel).map(|e| e.uid.clone())
    }

    fn reschedule_selected(&mut self, delta: Duration) {
        if let Some(uid) = self.selected_event_uid() {
            let r = self.ctx.reschedule_event(&uid, delta);
            self.report(r.map(|_| "rescheduled".into()));
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
        let order = TaskStatus::BOARD_ORDER;
        let new_col = (self.board_col as i32 + dir).clamp(0, order.len() as i32 - 1) as usize;
        let r = self.ctx.set_task_status(&uid, order[new_col]);
        if r.is_ok() {
            self.board_col = new_col;
        }
        self.report(r.map(|_| "moved".into()));
    }

    fn toggle_selected_card_done(&mut self) {
        if let Some(uid) = self.selected_card_uid() {
            let new = match self.ctx.task(&uid).map(|t| t.status) {
                Some(TaskStatus::Done) => TaskStatus::Todo,
                _ => TaskStatus::Done,
            };
            let r = self.ctx.set_task_status(&uid, new);
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
        let tasks = self.ctx.filtered_tasks(&self.filter, self.sort);
        match action {
            Action::Up => self.task_sel = self.task_sel.saturating_sub(1),
            Action::Down => self.task_sel = (self.task_sel + 1).min(tasks.len().saturating_sub(1)),
            Action::ToggleDone => {
                if let Some(t) = tasks.get(self.task_sel) {
                    let new = if t.status == TaskStatus::Done { TaskStatus::Todo } else { TaskStatus::Done };
                    let r = self.ctx.set_task_status(&t.uid, new);
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

    // ---- rendering -----------------------------------------------------------------

    pub fn draw(&self, frame: &mut Frame, area: Rect) {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(3), Constraint::Min(1), Constraint::Length(1)])
            .split(area);

        self.draw_tabs(frame, chunks[0]);
        match self.tab {
            Tab::Calendar => self.draw_calendar(frame, chunks[1]),
            Tab::Board => self.draw_board(frame, chunks[1]),
            Tab::Tasks => self.draw_tasks(frame, chunks[1]),
            Tab::Focus => self.draw_focus(frame, chunks[1]),
        }
        self.draw_status(frame, chunks[2]);

        match &self.modal {
            Some(Modal::Input { prompt, buffer, .. }) => self.draw_input(frame, area, prompt, buffer),
            Some(Modal::Event(form)) => self.draw_event_form(frame, area, form),
            Some(Modal::Picker(picker)) => self.draw_picker(frame, area, picker),
            None => {}
        }
        if self.show_help {
            self.draw_help(frame, area);
        }
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
            CalView::Day => self.draw_agenda(frame, area, true),
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
                let has_items = !self.ctx.events_on(day).is_empty() || !self.ctx.tasks_on(day).is_empty();
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
            let events = self.ctx.events_on(day);
            let mut items: Vec<ListItem> = Vec::new();
            for (ei, e) in events.iter().enumerate() {
                let selected = day == self.day && self.cal_focus == CalFocus::Agenda && ei == self.agenda_sel;
                let style = if selected {
                    Style::default().bg(self.theme.selected_bg).fg(self.theme.selected_fg)
                } else {
                    Style::default().fg(self.theme.event)
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
        let events = self.ctx.events_on(self.day);
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
                Style::default().fg(self.theme.event)
            };
            items.push(ListItem::new(Line::from(format!("{time}  {}", e.summary))).style(style));
        }
        if !tasks.is_empty() {
            items.push(ListItem::new(Line::from(Span::styled("— tasks —", Style::default().fg(self.theme.dim)))));
            for t in &tasks {
                let mark = if t.status == TaskStatus::Done { "✓" } else { "○" };
                items.push(ListItem::new(Line::from(format!("{mark}  {}", t.title))).style(Style::default().fg(self.theme.task)));
            }
        }
        if items.is_empty() {
            items.push(ListItem::new(Line::from(Span::styled("(nothing scheduled — 'a' to add)", Style::default().fg(self.theme.dim)))));
        }
        let hint = if self.cal_focus == CalFocus::Agenda { " [agenda] " } else { "" };
        let title = format!(" {}{} ", self.day.format("%a %d %b %Y"), hint);
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
                let style = if selected {
                    Style::default().bg(self.theme.selected_bg).fg(self.theme.selected_fg).add_modifier(Modifier::BOLD)
                } else {
                    Style::default()
                };
                items.push(ListItem::new(Line::from(card_label(card))).style(style));
            }
            let active = ci == self.board_col;
            let border_style = if active {
                Style::default().fg(self.theme.accent)
            } else {
                Style::default().fg(self.theme.border)
            };
            let title = format!(" {} ({}) ", status.label(), cards.len());
            let list = List::new(items).block(
                Block::default().borders(Borders::ALL).title(title).border_style(border_style),
            );
            frame.render_widget(list, cols[ci]);
        }
    }

    fn draw_tasks(&self, frame: &mut Frame, area: Rect) {
        let tasks = self.ctx.filtered_tasks(&self.filter, self.sort);
        let mut items: Vec<ListItem> = Vec::new();
        for (i, t) in tasks.iter().enumerate() {
            let mark = match t.status {
                TaskStatus::Done => "✓",
                TaskStatus::Cancelled => "✗",
                TaskStatus::Doing => "▸",
                _ => "○",
            };
            let sel = i == self.task_sel;
            let style = if sel {
                Style::default().bg(self.theme.selected_bg).fg(self.theme.selected_fg)
            } else if t.status == TaskStatus::Done {
                Style::default().fg(self.theme.done).add_modifier(Modifier::CROSSED_OUT)
            } else {
                Style::default()
            };
            items.push(ListItem::new(Line::from(format!("{mark} {}", card_label(t)))).style(style));
        }
        if items.is_empty() {
            items.push(ListItem::new(Line::from(Span::styled("(no tasks — 'a' to add)", Style::default().fg(self.theme.dim)))));
        }
        let list = List::new(items).block(Block::default().borders(Borders::ALL).title(" Tasks  (e edit · p project · P priority) "));
        frame.render_widget(list, area);
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

    fn draw_event_form(&self, frame: &mut Frame, area: Rect, form: &EventForm) {
        let rect = centered(area, 56, 8);
        frame.render_widget(Clear, rect);
        let fields = [("Summary", &form.summary), ("Start (HH:MM)", &form.start), ("End (HH:MM)", &form.end)];
        let mut lines = vec![Line::from(Span::styled(
            format!("New event on {}", form.day.format("%a %d %b %Y")),
            Style::default().fg(self.theme.dim),
        ))];
        for (i, (label, val)) in fields.iter().enumerate() {
            let active = i == form.field;
            let cursor = if active { "_" } else { "" };
            let style = if active {
                Style::default().fg(self.theme.accent).add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            lines.push(Line::from(Span::styled(format!("{label:>14}: {val}{cursor}"), style)));
        }
        lines.push(Line::from(Span::styled(
            "Tab/Enter: next · Enter on End: create · Esc: cancel",
            Style::default().fg(self.theme.dim),
        )));
        let para = Paragraph::new(lines).block(
            Block::default().borders(Borders::ALL).title(" New event ").border_style(Style::default().fg(self.theme.accent)),
        );
        frame.render_widget(para, rect);
    }

    fn draw_picker(&self, frame: &mut Frame, area: Rect, picker: &Picker) {
        let h = (picker.items.len() as u16 + 2).min(area.height).max(3);
        let rect = centered(area, 40, h);
        frame.render_widget(Clear, rect);
        let items: Vec<ListItem> = picker
            .items
            .iter()
            .enumerate()
            .map(|(i, name)| {
                let style = if i == picker.sel {
                    Style::default().bg(self.theme.selected_bg).fg(self.theme.selected_fg)
                } else {
                    Style::default()
                };
                ListItem::new(Line::from(name.clone())).style(style)
            })
            .collect();
        let list = List::new(items).block(
            Block::default().borders(Borders::ALL).title(" Project (j/k, enter, esc) ").border_style(Style::default().fg(self.theme.accent)),
        );
        frame.render_widget(list, rect);
    }

    fn draw_help(&self, frame: &mut Frame, area: Rect) {
        let rect = centered(area, 72, 18);
        frame.render_widget(Clear, rect);
        let help = "\
Global    tab/shift-tab: switch view   u: undo   ctrl-r: redo   q: quit   ?: help
Calendar  h/l: day   j/k: week/day or event   v: month/week/day   enter: focus agenda
          J/K: move event ±15m   a: new event   e: edit event   d: delete
Board     h/l: column   j/k: card   H/L: move card   space: done
          a: add   e: edit   p: project   P: priority   d: delete
Tasks     j/k: select   space: done   a: add   e: edit   p: project   P: priority   d: delete
Focus     space: start/pause   s: skip phase";
        let para = Paragraph::new(help)
            .wrap(Wrap { trim: false })
            .block(Block::default().borders(Borders::ALL).title(" Help (any key to close) "));
        frame.render_widget(para, rect);
    }
}

/// A card/task label: title with project tag and priority marker.
fn card_label(t: &mgmt_domain::Task) -> String {
    let proj = t.project.as_deref().map(|p| format!(" #{p}")).unwrap_or_default();
    let prio = match t.priority {
        Priority::High => " !!!",
        Priority::Medium => " !!",
        Priority::Low => " !",
        Priority::None => "",
    };
    format!("{}{proj}{prio}", t.title)
}

/// Parse `HH:MM` against a date into a UTC instant. Returns `None` on malformed input.
fn parse_hhmm(day: NaiveDate, s: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    let (h, m) = s.trim().split_once(':')?;
    let h: u32 = h.parse().ok()?;
    let m: u32 = m.parse().ok()?;
    let naive = day.and_hms_opt(h, m, 0)?;
    Some(naive.and_utc())
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
        assert_eq!(app.context(), Context::Input);
        for c in "milk".chars() {
            app.handle_key(key(c));
        }
        app.handle_key(special(KeyCode::Enter));
        assert_eq!(app.context_mut().tasks().len(), 1);
        assert_eq!(app.context_mut().tasks()[0].title, "milk");
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
        app.handle_key(special(KeyCode::Enter)); // -> start field (default 09:00)
        app.handle_key(special(KeyCode::Enter)); // -> end field (default 10:00)
        app.handle_key(special(KeyCode::Enter)); // submit
        let events = app.context_mut().events();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].summary, "Lunch");
        assert_eq!(events[0].start.hour(), 9);
        assert_eq!(events[0].end.hour(), 10);
    }

    #[test]
    fn project_picker_assigns_project() {
        let mut app = app();
        app.context_mut().quick_add("task", None).unwrap();
        app.handle_key(special(KeyCode::Tab)); // Board
        app.handle_key(special(KeyCode::Tab)); // Tasks
        app.handle_key(key('p')); // open picker
        assert_eq!(app.context(), Context::Picker);
        // pick "(new project…)" -> last item
        for _ in 0..10 {
            app.handle_key(key('j'));
        }
        app.handle_key(special(KeyCode::Enter)); // -> new project input
        assert_eq!(app.context(), Context::Input);
        for c in "wng".chars() {
            app.handle_key(key(c));
        }
        app.handle_key(special(KeyCode::Enter));
        assert_eq!(app.context_mut().tasks()[0].project.as_deref(), Some("wng"));
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
