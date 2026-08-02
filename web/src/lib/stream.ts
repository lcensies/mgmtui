// Server-sent change stream. The server pings whenever the vault changes — from this browser,
// another device, the CLI, or a sync run — and we revalidate everything. Purely additive: if the
// stream can't be established the app still refreshes on focus/visibility as before.

import { invalidate } from "./cache";

let es: EventSource | null = null;
let timer: number | undefined;
/** True once a ping arrived while we were disconnected, so reconnecting catches up. */
let missed = false;

function refreshSoon() {
  // A sync run rewrites many files at once; coalesce that burst into a single revalidation.
  clearTimeout(timer);
  timer = window.setTimeout(() => invalidate("all"), 500);
}

/** True while the change stream is connected (the app can then skip its own polling). */
export function streamConnected(): boolean {
  return es?.readyState === EventSource.OPEN;
}

export function startStream() {
  if (es || typeof EventSource === "undefined") return;
  es = new EventSource("/api/stream", { withCredentials: true });
  es.addEventListener("changed", refreshSoon);
  es.addEventListener("open", () => {
    // Anything that changed while we were disconnected is invisible until we refetch once.
    if (missed) {
      missed = false;
      refreshSoon();
    }
  });
  // EventSource reconnects on its own; just remember that we have catching up to do.
  es.addEventListener("error", () => {
    missed = true;
  });
}

export function stopStream() {
  es?.close();
  es = null;
  clearTimeout(timer);
  missed = false;
}
