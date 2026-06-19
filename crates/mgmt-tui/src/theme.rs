//! Theme — the small palette views use, kept separate so a host (e.g. the wng dashboard)
//! can inject its own colors when embedding mgmt. Base slots can be overridden from the YAML
//! config; per-project and per-status colors are resolved separately via [`parse_color`].

use std::collections::BTreeMap;

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
            // High-contrast selection: dark text on a bright bar. White-on-blue (the old default)
            // washed out against terminal default foregrounds, so we pin both fg and bg.
            selected_bg: Color::LightBlue,
            selected_fg: Color::Black,
            dim: Color::DarkGray,
            today: Color::Yellow,
            event: Color::Green,
            task: Color::Magenta,
            done: Color::DarkGray,
            border: Color::Gray,
        }
    }
}

impl Theme {
    /// Apply theme palette overrides from config (slot name → color string). Unknown slots and
    /// unparseable colors are ignored, so a partial/typo'd config never blanks the UI.
    pub fn with_overrides(mut self, overrides: &BTreeMap<String, String>) -> Self {
        for (slot, val) in overrides {
            let Some(color) = parse_color(val) else { continue };
            match slot.as_str() {
                "accent" => self.accent = color,
                "selected_bg" => self.selected_bg = color,
                "selected_fg" => self.selected_fg = color,
                "dim" => self.dim = color,
                "today" => self.today = color,
                "event" => self.event = color,
                "task" => self.task = color,
                "done" => self.done = color,
                "border" => self.border = color,
                _ => {}
            }
        }
        self
    }
}

/// Parse a color string into a ratatui [`Color`]. Accepts the common ANSI names (`red`,
/// `lightblue`, `darkgray`, …), a `#rrggbb` hex triple, or an ANSI index (`"8"`). Returns
/// `None` on anything unrecognized so callers can fall back gracefully.
pub fn parse_color(s: &str) -> Option<Color> {
    let s = s.trim();
    if let Some(hex) = s.strip_prefix('#') {
        if hex.len() == 6 {
            let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
            let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
            let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
            return Some(Color::Rgb(r, g, b));
        }
        return None;
    }
    if let Ok(idx) = s.parse::<u8>() {
        return Some(Color::Indexed(idx));
    }
    Some(match s.to_ascii_lowercase().replace([' ', '_', '-'], "").as_str() {
        "black" => Color::Black,
        "red" => Color::Red,
        "green" => Color::Green,
        "yellow" => Color::Yellow,
        "blue" => Color::Blue,
        "magenta" => Color::Magenta,
        "cyan" => Color::Cyan,
        "gray" | "grey" => Color::Gray,
        "darkgray" | "darkgrey" => Color::DarkGray,
        "lightred" => Color::LightRed,
        "lightgreen" => Color::LightGreen,
        "lightyellow" => Color::LightYellow,
        "lightblue" => Color::LightBlue,
        "lightmagenta" => Color::LightMagenta,
        "lightcyan" => Color::LightCyan,
        "white" => Color::White,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_names_hex_and_index() {
        assert_eq!(parse_color("cyan"), Some(Color::Cyan));
        assert_eq!(parse_color("lightblue"), Some(Color::LightBlue));
        assert_eq!(parse_color("#a6e3a1"), Some(Color::Rgb(0xa6, 0xe3, 0xa1)));
        assert_eq!(parse_color("8"), Some(Color::Indexed(8)));
        assert_eq!(parse_color("not-a-color"), None);
    }

    #[test]
    fn overrides_apply_known_slots_only() {
        let mut m = BTreeMap::new();
        m.insert("accent".to_string(), "magenta".to_string());
        m.insert("bogus".to_string(), "red".to_string());
        let t = Theme::default().with_overrides(&m);
        assert_eq!(t.accent, Color::Magenta);
    }
}
