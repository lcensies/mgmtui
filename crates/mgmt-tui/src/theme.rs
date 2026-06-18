//! Theme — the small palette views use, kept separate so a host (e.g. the wng dashboard)
//! can inject its own colors when embedding mgmt.

use ratatui::style::Color;

#[derive(Debug, Clone)]
pub struct Theme {
    pub accent: Color,
    pub selected_bg: Color,
    pub selected_fg: Color,
    pub dim: Color,
    pub today: Color,
    pub event: Color,
    pub task: Color,
    pub done: Color,
    pub border: Color,
}

impl Default for Theme {
    fn default() -> Self {
        Theme {
            accent: Color::Cyan,
            selected_bg: Color::Blue,
            selected_fg: Color::White,
            dim: Color::DarkGray,
            today: Color::Yellow,
            event: Color::Green,
            task: Color::Magenta,
            done: Color::DarkGray,
            border: Color::Gray,
        }
    }
}
