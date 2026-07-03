// Multi-select model shared by Tasks and Board. Backed by the `selected` signal in state/ui.
// Supports tap-to-toggle (touch/mouse) and inclusive range extension (keyboard visual mode).

import { selected } from "../state/ui";

export function isSelected(id: string): boolean {
  return selected.value.has(id);
}

export function toggle(id: string) {
  const next = new Set(selected.value);
  if (next.has(id)) next.delete(id);
  else next.add(id);
  selected.value = next;
}

export function selectOnly(id: string) {
  selected.value = new Set([id]);
}

export function clear() {
  if (selected.value.size) selected.value = new Set();
}

export function count(): number {
  return selected.value.size;
}

/** Set the selection to the inclusive range between `anchor` and `cursor` within `ordered`. */
export function rangeSelect(ordered: string[], anchor: string, cursor: string) {
  const a = ordered.indexOf(anchor);
  const c = ordered.indexOf(cursor);
  if (a < 0 || c < 0) {
    selectOnly(cursor);
    return;
  }
  const [lo, hi] = a <= c ? [a, c] : [c, a];
  selected.value = new Set(ordered.slice(lo, hi + 1));
}

/** The selected ids in the order they appear in `ordered` (for deterministic bulk ops). */
export function orderedSelection(ordered: string[]): string[] {
  return ordered.filter((id) => selected.value.has(id));
}
