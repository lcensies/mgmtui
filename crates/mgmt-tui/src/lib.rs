//! Embeddable ratatui views for mgmt.
//!
//! **Embeddability invariant:** this crate must never own the terminal — no `enable_raw_mode`,
//! `EnterAlternateScreen`, or `crossterm::event::read`. A host supplies a `Frame`/`Rect` to
//! [`MgmtApp::draw`] and feeds keys to [`MgmtApp::handle_key`]. The `mgmt` binary owns the
//! terminal; the wng dashboard can host `MgmtApp` the same way by adding a key context.

mod app;
mod keymap;
mod theme;

pub use app::{MgmtApp, Outcome};
pub use keymap::{Action, Context, action_for_key};
pub use theme::Theme;
