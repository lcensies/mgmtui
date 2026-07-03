// A single Pointer Events drag primitive shared by board card DnD and calendar reschedule/resize.
// Disambiguates tap vs drag vs long-press, works for mouse + touch, and tracks movement on window
// so the drag continues outside the origin element.

export interface DragHandlers {
  /** Fired once, when movement first crosses the drag threshold. */
  onStart?: (e: PointerEvent) => void;
  /** Fired on each move while dragging, with total delta from the press point. */
  onMove?: (dx: number, dy: number, e: PointerEvent) => void;
  /** Fired on release after a drag. */
  onEnd?: (dx: number, dy: number, e: PointerEvent) => void;
  /** Fired on release when movement never crossed the threshold (a click/tap). */
  onTap?: (e: PointerEvent) => void;
  /** Fired when held past `longPressMs` without moving (touch context menu / select). */
  onLongPress?: (e: PointerEvent) => void;
  threshold?: number;
  longPressMs?: number;
}

/** Attach to `onPointerDown`. Returns nothing; wires temporary window listeners until release. */
export function startDrag(e: PointerEvent, h: DragHandlers) {
  // Ignore secondary buttons.
  if (e.button && e.button !== 0) return;
  const threshold = h.threshold ?? 5;
  const longPressMs = h.longPressMs ?? 500;
  const startX = e.clientX;
  const startY = e.clientY;
  let dragging = false;
  let done = false;
  let longPressed = false;

  const longTimer = window.setTimeout(() => {
    if (!dragging && !done) {
      longPressed = true;
      h.onLongPress?.(e);
    }
  }, longPressMs);

  const move = (ev: PointerEvent) => {
    if (done) return;
    const dx = ev.clientX - startX;
    const dy = ev.clientY - startY;
    if (!dragging) {
      if (longPressed) return; // a long-press consumed the gesture
      if (Math.hypot(dx, dy) < threshold) return;
      dragging = true;
      clearTimeout(longTimer);
      h.onStart?.(ev);
    }
    h.onMove?.(dx, dy, ev);
  };

  const up = (ev: PointerEvent) => {
    if (done) return;
    done = true;
    clearTimeout(longTimer);
    window.removeEventListener("pointermove", move);
    window.removeEventListener("pointerup", up);
    window.removeEventListener("pointercancel", up);
    const dx = ev.clientX - startX;
    const dy = ev.clientY - startY;
    if (dragging) h.onEnd?.(dx, dy, ev);
    else if (!longPressed) h.onTap?.(ev);
  };

  window.addEventListener("pointermove", move);
  window.addEventListener("pointerup", up);
  window.addEventListener("pointercancel", up);
}
