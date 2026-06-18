//! `MgmtApp` — the embeddable aggregate that owns app state and renders the views. It never
//! touches the terminal: a host calls [`MgmtApp::draw`] with a `Frame`/`Rect` and feeds keys
//! to [`MgmtApp::handle_key`]. The standalone `mgmt` binary provides the terminal + loop; the
//! wng dashboard can host it the same way.

use std::time::{Duration as StdDuration, Instant};

use chrono::{Datelike, Duration, Local, NaiveDate, Timelike};
use crossterm::event::KeyEvent;
use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Tabs, Wrap};

use mgmt_core::Uid;
use mgmt_domain::{Filter, SortMode, TaskStatus};
use mgmt_service::{MgmtContext, Phase, Pomodoro, Technique};

use crate::keymap::{Action, Context, action_for_key};
use crate::theme::Theme;

/// Result of handling a key: keep running or quit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Continue,
    Quit,
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

struct InputState {
    prompt: String,
    buffer: String,
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

    input: Option<InputState>,
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
            agenda_sel: 0,
            board_col: 0,
            board_row: 0,
            task_sel: 0,
            filter: Filter::default(),
            sort: SortMode::DueDate,
            timer: Timer::new(),
            input: None,
            show_help: false,
            status: "? for help".to_string(),
        }
    }

    pub fn with_theme(mut self, theme: Theme) -> Self {
        self.theme = theme;
        self
    }

    /// The key context that is currently active (input overrides the tab).
    pub fn context(&self) -> Context {
        if self.input.is_some() {
            Context::Input
        } else {
            self.tab.context()
        }
    }

    pub fn is_dirty(&self) -> bool {
        self.ctx.is_dirty()
    }

    /// Borrow the underlying context (e.g. so the host can sync or persist).
    pub fn context_mut(&mut self) -> &mut MgmtContext {
        &mut self.ctx
    }

    // ---- input handling ------------------------------------------------------------

    pub fn handle_key(&mut self, key: KeyEvent) -> Outcome {
        if self.show_help {
            self.show_help = false;
            return Outcome::Continue;
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

            // input mode
            Action::InputChar(c) => {
                if let Some(input) = &mut self.input {
                    input.buffer.push(c);
                }
            }
            Action::InputBackspace => {
                if let Some(input) = &mut self.input {
                    input.buffer.pop();
                }
            }
            Action::InputCancel => self.input = None,
            Action::InputSubmit => self.submit_input(),

            Action::QuickAdd => self.begin_quick_add(),

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

    fn begin_quick_add(&mut self) {
        let prompt = match self.tab {
            Tab::Board | Tab::Tasks => "New task".to_string(),
            Tab::Calendar => format!("New task on {}", self.day),
            Tab::Focus => "New task".to_string(),
        };
        self.input = Some(InputState {
            prompt,
            buffer: String::new(),
        });
    }

    fn submit_input(&mut self) {
        let Some(input) = self.input.take() else { return };
        let title = input.buffer.trim().to_string();
        if title.is_empty() {
            return;
        }
        match self.ctx.quick_add(title, self.filter.project.clone()) {
            Ok(uid) => {
                // On the calendar tab, schedule the new task on the selected day.
                if self.tab == Tab::Calendar {
                    if let Some(mut t) = self.ctx.task(&uid).cloned() {
                        t.scheduled = Some(self.day.and_hms_opt(9, 0, 0).unwrap().and_utc());
                        let _ = self.ctx.put_task(t);
                    }
                }
                self.status = "added".into();
            }
            Err(e) => self.status = format!("error: {e}"),
        }
    }

    // ---- calendar ------------------------------------------------------------------

    fn calendar_action(&mut self, action: Action) {
        match action {
            Action::Left => self.day -= Duration::days(1),
            Action::Right => self.day += Duration::days(1),
            Action::Up => self.day -= Duration::weeks(1),
            Action::Down => self.day += Duration::weeks(1),
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

        if let Some(input) = &self.input {
            self.draw_input(frame, area, input);
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
        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(24), Constraint::Min(20)])
            .split(area);
        self.draw_month(frame, cols[0]);
        self.draw_agenda(frame, cols[1]);
    }

    fn draw_month(&self, frame: &mut Frame, area: Rect) {
        let first = self.day.with_day(1).unwrap();
        let title = format!(" {} ", self.day.format("%B %Y"));
        let mut lines: Vec<Line> = Vec::new();
        lines.push(Line::from(Span::styled("Mo Tu We Th Fr Sa Su", Style::default().fg(self.theme.accent))));

        let today = Local::now().date_naive();
        // weekday() Mon=0
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

        let para = Paragraph::new(lines).block(Block::default().borders(Borders::ALL).title(title));
        frame.render_widget(para, area);
    }

    fn draw_agenda(&self, frame: &mut Frame, area: Rect) {
        let events = self.ctx.events_on(self.day);
        let tasks = self.ctx.tasks_on(self.day);
        let mut items: Vec<ListItem> = Vec::new();
        for (i, e) in events.iter().enumerate() {
            let time = if e.all_day {
                "all-day".to_string()
            } else {
                format!("{:02}:{:02}", e.start.hour(), e.start.minute())
            };
            let sel = i == self.agenda_sel;
            let style = if sel {
                Style::default().bg(self.theme.selected_bg).fg(self.theme.selected_fg)
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
        let title = format!(" {} ", self.day.format("%a %d %b %Y"));
        let list = List::new(items).block(Block::default().borders(Borders::ALL).title(title));
        frame.render_widget(list, area);
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
                items.push(ListItem::new(Line::from(card.title.clone())).style(style));
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
            let proj = t.project.as_deref().map(|p| format!(" #{p}")).unwrap_or_default();
            let sel = i == self.task_sel;
            let style = if sel {
                Style::default().bg(self.theme.selected_bg).fg(self.theme.selected_fg)
            } else if t.status == TaskStatus::Done {
                Style::default().fg(self.theme.done).add_modifier(Modifier::CROSSED_OUT)
            } else {
                Style::default()
            };
            items.push(ListItem::new(Line::from(format!("{mark} {}{proj}", t.title))).style(style));
        }
        if items.is_empty() {
            items.push(ListItem::new(Line::from(Span::styled("(no tasks — 'a' to add)", Style::default().fg(self.theme.dim)))));
        }
        let list = List::new(items).block(Block::default().borders(Borders::ALL).title(" Tasks "));
        frame.render_widget(list, area);
    }

    fn draw_focus(&self, frame: &mut Frame, area: Rect) {
        let elapsed = self.timer.elapsed();
        let (label, target) = match self.timer.phase {
            Phase::Focus { target } => ("FOCUS", target),
            Phase::Break { target } => ("BREAK", target),
        };
        let remaining = target
            .map(|t| t.saturating_sub(elapsed))
            .unwrap_or(elapsed);
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

    fn draw_input(&self, frame: &mut Frame, area: Rect, input: &InputState) {
        let rect = centered(area, 60, 3);
        frame.render_widget(Clear, rect);
        let text = format!("{}_", input.buffer);
        let para = Paragraph::new(text).block(
            Block::default()
                .borders(Borders::ALL)
                .title(format!(" {} (enter to confirm, esc to cancel) ", input.prompt))
                .border_style(Style::default().fg(self.theme.accent)),
        );
        frame.render_widget(para, rect);
    }

    fn draw_help(&self, frame: &mut Frame, area: Rect) {
        let rect = centered(area, 60, 16);
        frame.render_widget(Clear, rect);
        let help = "\
Global    tab/shift-tab: switch view   u: undo   ctrl-r: redo   q: quit   ?: help
Calendar  h/l: day   j/k: week   t: today   J/K: move event ±15m   a: add   d: del
Board     h/l: column   j/k: card   H/L: move card   space: done   a: add   d: del
Tasks     j/k: select   space: done   a: add   d: del
Focus     space: start/pause   s: skip phase";
        let para = Paragraph::new(help)
            .wrap(Wrap { trim: false })
            .block(Block::default().borders(Borders::ALL).title(" Help (any key to close) "));
        frame.render_widget(para, rect);
    }
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
    use crossterm::event::{KeyCode, KeyModifiers};
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

    #[test]
    fn renders_every_tab_without_panicking() {
        let mut app = app();
        let backend = TestBackend::new(100, 30);
        let mut term = Terminal::new(backend).unwrap();
        for _ in 0..Tab::ALL.len() {
            term.draw(|f| app.draw(f, f.area())).unwrap();
            app.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        }
    }

    #[test]
    fn quick_add_flow_creates_a_task() {
        let mut app = app();
        // go to Tasks tab (Calendar -> Board -> Tasks)
        app.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        app.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        assert_eq!(app.context(), Context::Tasks);
        app.handle_key(key('a'));
        assert_eq!(app.context(), Context::Input);
        for c in "milk".chars() {
            app.handle_key(key(c));
        }
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(app.context_mut().tasks().len(), 1);
        assert_eq!(app.context_mut().tasks()[0].title, "milk");
    }

    #[test]
    fn q_quits() {
        let mut app = app();
        assert_eq!(app.handle_key(key('q')), Outcome::Quit);
    }
}

