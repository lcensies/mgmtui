//! Deterministic auto-coloring for labels that have no explicit color (projects, statuses).
//! The same name always maps to the same palette entry, so colors are stable across runs
//! without storing anything.

/// The auto-assign palette — ANSI color names ratatui understands. Chosen to be visually
/// distinct and to avoid the reserved selection/dim colors used by the base theme.
pub const PALETTE: [&str; 10] = [
    "blue",
    "green",
    "magenta",
    "cyan",
    "yellow",
    "red",
    "lightblue",
    "lightgreen",
    "lightmagenta",
    "lightcyan",
];

/// Pick a stable palette color for `name` by hashing it. Deterministic and allocation-free.
pub fn auto_color(name: &str) -> &'static str {
    // FNV-1a over the bytes — small, stable, and not dependent on std's randomized hasher.
    let mut hash: u64 = 0xcbf29ce484222325;
    for b in name.bytes() {
        hash ^= b as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    PALETTE[(hash % PALETTE.len() as u64) as usize]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_color_is_stable_for_a_name() {
        assert_eq!(auto_color("wng"), auto_color("wng"));
    }

    #[test]
    fn auto_color_returns_a_palette_member() {
        assert!(PALETTE.contains(&auto_color("anything")));
    }
}
