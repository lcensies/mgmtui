// Desktop keyboard bindings. Kept modest and browser-safe (no Tab/Ctrl+R hijacking); touch users
// use the on-screen controls. Help renders this list so the two never drift.

export interface Binding {
  keys: string;
  label: string;
}

export const BINDINGS: Binding[] = [
  { keys: "1 / 2 / 3 / 4", label: "Calendar / Board / Tasks / Focus" },
  { keys: ":", label: "Command palette" },
  { keys: "?", label: "Keyboard help" },
  { keys: "n", label: "New (event on Calendar, else task)" },
  { keys: "u / U", label: "Undo / Redo" },
  { keys: "g", label: "Trash" },
  { keys: "Esc", label: "Close modal / clear selection" },
];
