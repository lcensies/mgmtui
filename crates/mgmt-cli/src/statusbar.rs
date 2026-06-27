//! Status-bar backends — how the daemon pushes the rendered status to a desktop bar. This is the
//! pluggable "interface for working with status bars": GNOME today, any other bar via `command`
//! or `file`.
//!
//! - [`GnomeBar`] calls the bundled GNOME Shell extension over D-Bus (`gdbus`) with the
//!   *tick-stable* JSON payload; the extension animates the countdown locally, so the daemon
//!   pushes only when the underlying state changes.
//! - [`CommandBar`] runs a user command with the rendered line as its final argument
//!   (waybar/polybar/i3blocks/… via a one-line setter script).
//! - [`FileBar`] writes the rendered line to a file a bar can watch/tail.
//!
//! Every backend is best-effort: a missing extension or a failing command is swallowed so the
//! daemon never stalls. The daemon de-duplicates with [`StatusBar::change_key`], which lets a
//! smart bar (GNOME) refresh only on real changes while a dumb bar still gets each text update.

use std::path::PathBuf;
use std::process::{Command, Stdio};

use serde_json::Value;

use mgmt_config::StatusBarCfg;

/// One status push: the tick-stable JSON (smart bars animate from it) plus the pre-rendered text
/// (dumb bars display it verbatim).
pub struct Update<'a> {
    pub json: &'a Value,
    pub text: &'a str,
}

pub trait StatusBar {
    /// Push the current status. Best-effort; returns whether it succeeded.
    fn set(&mut self, update: &Update) -> bool;
    /// Clear the widget (nothing to show).
    fn clear(&mut self);
    /// The value whose change should trigger a push: the JSON for smart bars (so a running
    /// countdown — stable JSON — is pushed once), the text for dumb bars (which can't self-tick).
    fn change_key(&self, update: &Update) -> String;
    fn name(&self) -> &'static str;
}

// ── GNOME (D-Bus to the bundled shell extension) ─────────────────────────────

const GNOME_DEST: &str = "org.gnome.Shell";
const GNOME_OBJ: &str = "/org/gnome/Shell/Extensions/MgmtStatus";
const GNOME_IFACE: &str = "org.gnome.Shell.Extensions.MgmtStatus";

pub struct GnomeBar;

impl StatusBar for GnomeBar {
    fn set(&mut self, update: &Update) -> bool {
        gdbus_call("Update", Some(&gvariant_str(&update.json.to_string())))
    }
    fn clear(&mut self) {
        gdbus_call("Clear", None);
    }
    fn change_key(&self, update: &Update) -> String {
        update.json.to_string()
    }
    fn name(&self) -> &'static str {
        "gnome"
    }
}

fn gdbus_call(method: &str, arg: Option<&str>) -> bool {
    let full = format!("{GNOME_IFACE}.{method}");
    let mut argv = vec!["call", "--session", "--dest", GNOME_DEST, "--object-path", GNOME_OBJ, "--method", &full];
    if let Some(a) = arg {
        argv.push(a);
    }
    Command::new("gdbus")
        .args(&argv)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Encode an arbitrary string as a GVariant text-format string literal so `gdbus` parses it back
/// byte-for-byte. `serde_json` output never contains raw control characters (it escapes them), so
/// escaping `\` and `"` is sufficient and round-trips any event summary safely.
fn gvariant_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            _ => out.push(c),
        }
    }
    out.push('"');
    out
}

// ── Command (any bar via a setter script) ────────────────────────────────────

pub struct CommandBar {
    argv: Vec<String>,
}

impl StatusBar for CommandBar {
    fn set(&mut self, update: &Update) -> bool {
        let Some((head, tail)) = self.argv.split_first() else { return false };
        Command::new(head)
            .args(tail)
            .arg(update.text)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }
    fn clear(&mut self) {
        let update = Update { json: &Value::Null, text: "" };
        self.set(&update);
    }
    fn change_key(&self, update: &Update) -> String {
        update.text.to_string()
    }
    fn name(&self) -> &'static str {
        "command"
    }
}

// ── File (a bar tails it) ────────────────────────────────────────────────────

pub struct FileBar {
    path: PathBuf,
}

impl StatusBar for FileBar {
    fn set(&mut self, update: &Update) -> bool {
        std::fs::write(&self.path, format!("{}\n", update.text)).is_ok()
    }
    fn clear(&mut self) {
        let _ = std::fs::write(&self.path, "\n");
    }
    fn change_key(&self, update: &Update) -> String {
        update.text.to_string()
    }
    fn name(&self) -> &'static str {
        "file"
    }
}

// ── Selection ────────────────────────────────────────────────────────────────

/// Pick the backend from config. `auto` resolves to GNOME inside a GNOME session and to nothing
/// elsewhere (the `command`/`file` backends need explicit config and so are never auto-selected).
pub fn select(cfg: &StatusBarCfg) -> Option<Box<dyn StatusBar>> {
    if !cfg.enabled {
        return None;
    }
    let backend = if cfg.backend == "auto" { detect() } else { cfg.backend.as_str() };
    match backend {
        "gnome" => Some(Box::new(GnomeBar) as Box<dyn StatusBar>),
        "command" if !cfg.command.is_empty() => Some(Box::new(CommandBar { argv: cfg.command.clone() })),
        "file" => cfg.file.clone().map(|path| Box::new(FileBar { path }) as Box<dyn StatusBar>),
        _ => None,
    }
}

/// Resolve `auto`. Trust an explicit GNOME desktop hint from the environment, but fall back to
/// *probing the session bus* for `org.gnome.Shell` — robust even when `XDG_CURRENT_DESKTOP` isn't
/// propagated into the systemd user service the daemon usually runs as.
fn detect() -> &'static str {
    let hint = |k: &str| std::env::var(k).unwrap_or_default().to_lowercase();
    if hint("XDG_CURRENT_DESKTOP").contains("gnome")
        || hint("ORIGINAL_XDG_CURRENT_DESKTOP").contains("gnome")
        || !hint("GNOME_SHELL_SESSION_MODE").is_empty()
        || gnome_shell_on_bus()
    {
        "gnome"
    } else {
        "none"
    }
}

/// Whether GNOME Shell currently owns its well-known name on the session bus.
fn gnome_shell_on_bus() -> bool {
    Command::new("gdbus")
        .args([
            "call", "--session", "--dest", "org.freedesktop.DBus",
            "--object-path", "/org/freedesktop/DBus",
            "--method", "org.freedesktop.DBus.GetNameOwner", "org.gnome.Shell",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gvariant_escapes_round_trip_chars() {
        // A summary with quotes, a backslash, and an apostrophe.
        assert_eq!(gvariant_str(r#"a"b\c'd"#), r#""a\"b\\c'd""#);
    }

    #[test]
    fn auto_is_off_outside_gnome_and_command_needs_argv() {
        let mut cfg = StatusBarCfg::default();
        cfg.backend = "command".into();
        cfg.command = Vec::new();
        assert!(select(&cfg).is_none(), "command backend with no argv is inert");
        cfg.enabled = false;
        cfg.backend = "gnome".into();
        assert!(select(&cfg).is_none(), "disabled means no backend");
    }
}
