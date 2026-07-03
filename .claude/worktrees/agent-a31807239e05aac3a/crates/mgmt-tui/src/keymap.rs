//! Actions and key contexts — the same shape as wng's dashboard keymap
//! (`Context` + `action_for_key(ctx, key) -> Option<Action>` + `Action`) so the views can be
//! hosted there later by adding a `Context` variant and routing keys to [`crate::MgmtApp`].

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// Which view/mode keys are interpreted in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Context {
    Calendar,
    Board,
    Tasks,
    Focus,
    /// Single-line text entry (quick-add / new project). Captures most keys literally.
    Input,
    /// Multi-field event-creation form.
    Form,
    /// List picker (e.g. choose a project).
    Picker,
    /// Yes/no confirmation prompt.
    Confirm,
    /// Vim-style command palette (`:` to open, fuzzy-filter, Tab to complete, Enter to run).
    CommandPalette,
}

/// Everything a key can trigger. Views interpret the subset relevant to them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    // global
    Quit,
    Help,
    OpenCommandPalette,
    NextTab,
    PrevTab,
    Undo,
    Redo,

    // navigation (meaning is view-specific: day vs card vs list row)
    Left,
    Right,
    Up,
    Down,
    Today,

    // calendar: nudge the selected event's start (H/L) and end (J/K) by a step
    StartEarlier,
    StartLater,
    EndEarlier,
    EndLater,

    // board: move selected card to the next/previous column
    MoveNext,
    MovePrev,

    // tasks/board: cycle task state
    ToggleDone,
    /// Enter / exit vim-style visual (multi-select) mode.
    ToggleVisual,

    // editing
    Edit,          // open the selected item in $EDITOR
    EditProject,   // open the project picker for the selected task/event
    CyclePriority, // cycle the selected task's priority

    // calendar
    ViewCycle,   // cycle month / week / day
    Select,      // toggle focus between the date grid and the day's agenda
    JumpToDate,  // open a date-jump input

    // filtering
    PrevProject, // scope to previous project
    NextProject, // scope to next project
    Search,      // text-filter the task list

    // creation / editing
    QuickAdd,
    Delete,

    // focus timer
    StartPauseTimer,
    SkipPhase,

    // input mode
    InputChar(char),
    InputBackspace,
    InputSubmit,
    InputCancel,
}

/// Map a key to an action for the given context. Mirrors the structure of wng's
/// `dashboard::keymap::action_for_key`.
pub fn action_for_key(ctx: Context, key: KeyEvent) -> Option<Action> {
    if ctx == Context::Input {
        return input_key(key);
    }
    // Form, Picker and Confirm modals are handled with raw keys by the app, not via this table.
    if matches!(ctx, Context::Form | Context::Picker | Context::Confirm) {
        return None;
    }

    // global bindings shared by all non-input contexts
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    match key.code {
        KeyCode::Char('q') | KeyCode::Esc => return Some(Action::Quit),
        KeyCode::Char('c') if ctrl => return Some(Action::Quit),
        KeyCode::Char('?') => return Some(Action::Help),
        KeyCode::Char(':') => return Some(Action::OpenCommandPalette),
        KeyCode::Tab => return Some(Action::NextTab),
        KeyCode::BackTab => return Some(Action::PrevTab),
        KeyCode::Char('u') => return Some(Action::Undo),
        KeyCode::Char('r') if ctrl => return Some(Action::Redo),
        _ => {}
    }

    match ctx {
        Context::Calendar => calendar_key(key),
        Context::Board => board_key(key),
        Context::Tasks => tasks_key(key),
        Context::Focus => focus_key(key),
        Context::Input | Context::Form | Context::Picker | Context::Confirm | Context::CommandPalette => unreachable!(),
    }
}

fn calendar_key(key: KeyEvent) -> Option<Action> {
    Some(match key.code {
        KeyCode::Char('h') | KeyCode::Left => Action::Left,
        KeyCode::Char('l') | KeyCode::Right => Action::Right,
        KeyCode::Char('j') | KeyCode::Down => Action::Down,
        KeyCode::Char('k') | KeyCode::Up => Action::Up,
        KeyCode::Char('H') => Action::StartEarlier,
        KeyCode::Char('L') => Action::StartLater,
        KeyCode::Char('J') => Action::EndLater,
        KeyCode::Char('K') => Action::EndEarlier,
        KeyCode::Char('t') => Action::Today,
        KeyCode::Char('v') => Action::ViewCycle,
        KeyCode::Enter => Action::Select,
        KeyCode::Char('a') => Action::QuickAdd,
        KeyCode::Char('e') => Action::Edit,
        KeyCode::Char('p') => Action::EditProject,
        KeyCode::Char('d') => Action::Delete,
        KeyCode::Char('/') => Action::Search,
        KeyCode::Char(' ') => Action::ToggleDone,
        KeyCode::Char('g') => Action::JumpToDate,
        _ => return None,
    })
}

fn board_key(key: KeyEvent) -> Option<Action> {
    Some(match key.code {
        KeyCode::Char('j') | KeyCode::Down => Action::Down,
        KeyCode::Char('k') | KeyCode::Up => Action::Up,
        KeyCode::Char('h') | KeyCode::Left => Action::Left,
        KeyCode::Char('l') | KeyCode::Right => Action::Right,
        KeyCode::Char('H') => Action::MovePrev,
        KeyCode::Char('L') => Action::MoveNext,
        KeyCode::Char('v') => Action::ToggleVisual,
        KeyCode::Char(' ') => Action::ToggleDone,
        KeyCode::Char('a') => Action::QuickAdd,
        KeyCode::Char('e') => Action::Edit,
        KeyCode::Char('p') => Action::EditProject,
        KeyCode::Char('P') => Action::CyclePriority,
        KeyCode::Char('[') => Action::PrevProject,
        KeyCode::Char(']') => Action::NextProject,
        KeyCode::Char('/') => Action::Search,
        KeyCode::Char('d') => Action::Delete,
        _ => return None,
    })
}

fn tasks_key(key: KeyEvent) -> Option<Action> {
    Some(match key.code {
        KeyCode::Char('j') | KeyCode::Down => Action::Down,
        KeyCode::Char('k') | KeyCode::Up => Action::Up,
        KeyCode::Char('h') | KeyCode::Left => Action::Left,
        KeyCode::Char('l') | KeyCode::Right => Action::Right,
        KeyCode::Char('v') => Action::ToggleVisual,
        KeyCode::Char(' ') => Action::ToggleDone,
        KeyCode::Char('a') => Action::QuickAdd,
        KeyCode::Char('e') => Action::Edit,
        KeyCode::Char('p') => Action::EditProject,
        KeyCode::Char('P') => Action::CyclePriority,
        KeyCode::Char('[') => Action::PrevProject,
        KeyCode::Char(']') => Action::NextProject,
        KeyCode::Char('/') => Action::Search,
        KeyCode::Char('d') => Action::Delete,
        _ => return None,
    })
}

fn focus_key(key: KeyEvent) -> Option<Action> {
    Some(match key.code {
        KeyCode::Char(' ') => Action::StartPauseTimer,
        KeyCode::Char('s') => Action::SkipPhase,
        _ => return None,
    })
}

fn input_key(key: KeyEvent) -> Option<Action> {
    Some(match key.code {
        KeyCode::Esc => Action::InputCancel,
        KeyCode::Enter => Action::InputSubmit,
        KeyCode::Backspace => Action::InputBackspace,
        KeyCode::Char(c) => Action::InputChar(c),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn k(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
    }

    #[test]
    fn quit_is_global() {
        assert_eq!(action_for_key(Context::Board, k('q')), Some(Action::Quit));
        assert_eq!(action_for_key(Context::Calendar, k('q')), Some(Action::Quit));
    }

    #[test]
    fn board_move_uses_shift_hl() {
        assert_eq!(action_for_key(Context::Board, k('L')), Some(Action::MoveNext));
        assert_eq!(action_for_key(Context::Board, k('H')), Some(Action::MovePrev));
    }

    #[test]
    fn calendar_start_is_capital_hl_end_is_capital_jk() {
        assert_eq!(action_for_key(Context::Calendar, k('H')), Some(Action::StartEarlier));
        assert_eq!(action_for_key(Context::Calendar, k('L')), Some(Action::StartLater));
        assert_eq!(action_for_key(Context::Calendar, k('J')), Some(Action::EndLater));
        assert_eq!(action_for_key(Context::Calendar, k('K')), Some(Action::EndEarlier));
    }

    #[test]
    fn input_context_captures_literal_chars() {
        assert_eq!(action_for_key(Context::Input, k('q')), Some(Action::InputChar('q')));
        assert_eq!(
            action_for_key(Context::Input, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            Some(Action::InputSubmit)
        );
    }
}
