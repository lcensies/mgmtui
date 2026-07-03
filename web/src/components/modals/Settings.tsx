// Settings: theme, time format, secondary timezone, and keyboard-shortcut overrides. Theme is
// device-local; the rest persist server-side (follow the vault) via /api/settings.

import { useEffect, useState } from "preact/hooks";
import { setTheme, themePref, type ThemePref } from "../../state/theme";
import { DEFAULT_KEYS, saveSettings, settings, type Action } from "../../state/settings";
import { closeModal } from "../../state/ui";
import { Overlay } from "./ModalHost";

const THEMES: ThemePref[] = ["light", "dark", "system"];
const ZONES = ["", "UTC", "America/Los_Angeles", "America/New_York", "Europe/London", "Europe/Berlin", "Europe/Moscow", "Asia/Kolkata", "Asia/Tokyo", "Australia/Sydney"];
const ACTIONS: [Action, string][] = [
  ["calendar", "Calendar panel"], ["board", "Board panel"], ["tasks", "Tasks panel"], ["focus", "Focus panel"],
  ["palette", "Command palette"], ["new", "New item"], ["help", "Help"], ["trash", "Trash"],
  ["undo", "Undo"], ["redo", "Redo"],
];

export function Settings() {
  const s = settings.value;
  const [recording, setRecording] = useState<Action | null>(null);

  useEffect(() => {
    if (!recording) return;
    const onKey = (e: KeyboardEvent) => {
      e.preventDefault();
      e.stopPropagation();
      if (e.key !== "Escape" && e.key.length >= 1) {
        saveSettings({ keys: { ...settings.value.keys, [recording]: e.key } });
      }
      setRecording(null);
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [recording]);

  return (
    <Overlay>
      <h2>Settings</h2>

      <div class="field">
        <label>Theme</label>
        <div class="chips">
          {THEMES.map((t) => (
            <span class={`chip ${themePref.value === t ? "on" : ""}`} onClick={() => setTheme(t)}>{t}</span>
          ))}
        </div>
      </div>

      <div class="field">
        <label>Time format</label>
        <div class="chips">
          {(["24", "12"] as const).map((f) => (
            <span class={`chip ${s.timeFormat === f ? "on" : ""}`} onClick={() => saveSettings({ timeFormat: f })}>
              {f === "24" ? "24-hour" : "12-hour"}
            </span>
          ))}
        </div>
      </div>

      <div class="field">
        <label>Secondary timezone (calendar gutter)</label>
        <select value={s.secondaryTz} onChange={(e) => saveSettings({ secondaryTz: (e.target as HTMLSelectElement).value })}>
          {ZONES.map((z) => <option value={z}>{z || "None"}</option>)}
        </select>
      </div>

      <div class="field">
        <label>Keyboard shortcuts</label>
        <div class="keymap-edit">
          {ACTIONS.map(([a, label]) => (
            <div class="km-row">
              <span class="grow">{label}</span>
              <button class="km-key" onClick={() => setRecording(a)}>
                {recording === a ? "press a key…" : keyLabel(s.keys[a])}
              </button>
            </div>
          ))}
        </div>
        <button class="km-reset" onClick={() => saveSettings({ keys: { ...DEFAULT_KEYS } })}>Reset shortcuts</button>
      </div>

      <div class="actions">
        <button onClick={closeModal}>Close</button>
      </div>
    </Overlay>
  );
}

function keyLabel(k: string): string {
  if (k === " ") return "Space";
  return k;
}
