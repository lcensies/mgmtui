// Stale-while-revalidate data layer over signals + localStorage. Reads hydrate synchronously from
// the last cached value (instant paint), then revalidate in the background. Mutations are optimistic
// with rollback. This is what makes the app "lag less".

import { signal, type Signal } from "@preact/signals";

const PREFIX = "mgmt:";

/** Transient error toast (auto-cleared by the UI). */
export const toast = signal<string | null>(null);
export function showToast(msg: string) {
  toast.value = msg;
}

export interface Resource<T> {
  data: Signal<T | null>;
  error: Signal<string | null>;
  loading: Signal<boolean>;
  revalidate: () => Promise<void>;
  key: string;
}

interface Entry<T> extends Resource<T> {
  fetcher: () => Promise<T>;
  gen: number;
}

const registry = new Map<string, Entry<unknown>>();

function readLS<T>(key: string): T | null {
  try {
    const raw = localStorage.getItem(PREFIX + key);
    return raw ? (JSON.parse(raw) as T) : null;
  } catch {
    return null;
  }
}

function writeLS<T>(key: string, value: T | null) {
  try {
    if (value === null) localStorage.removeItem(PREFIX + key);
    else localStorage.setItem(PREFIX + key, JSON.stringify(value));
  } catch {
    /* quota / private mode — ignore, memory cache still works */
  }
}

/** Get (or create) a keyed resource. First access hydrates from localStorage and kicks a revalidate. */
export function resource<T>(key: string, fetcher: () => Promise<T>): Resource<T> {
  const existing = registry.get(key) as Entry<T> | undefined;
  if (existing) {
    existing.fetcher = fetcher; // keep the latest closure (query params captured)
    return existing;
  }
  const entry: Entry<T> = {
    key,
    fetcher,
    gen: 0,
    data: signal<T | null>(readLS<T>(key)),
    error: signal<string | null>(null),
    loading: signal<boolean>(false),
    revalidate: async () => {
      const gen = ++entry.gen;
      entry.loading.value = true;
      try {
        const value = await entry.fetcher();
        if (gen !== entry.gen) return; // a newer revalidate superseded us
        entry.data.value = value;
        entry.error.value = null;
        writeLS(key, value);
      } catch (e) {
        if (gen !== entry.gen) return;
        entry.error.value = e instanceof Error ? e.message : String(e);
        // keep stale data on screen
      } finally {
        if (gen === entry.gen) entry.loading.value = false;
      }
    },
  };
  registry.set(key, entry as Entry<unknown>);
  void entry.revalidate();
  return entry;
}

/** Revalidate every resource whose key starts with any of the given prefixes ("all" = everything). */
export function invalidate(prefixes: string[] | "all") {
  for (const entry of registry.values()) {
    if (prefixes === "all" || prefixes.some((p) => entry.key.startsWith(p))) {
      void entry.revalidate();
    }
  }
}

/**
 * Optimistic mutation. Applies `patch` to the given resources immediately (returning a rollback),
 * runs `request`, then revalidates `after` (default: all). On failure, rolls back and toasts.
 */
export async function mutate(opts: {
  patch?: () => void; // mutate resource signals in place for instant feedback
  rollback?: () => void; // restore prior signal values
  request: () => Promise<unknown>;
  after?: string[] | "all";
}): Promise<boolean> {
  opts.patch?.();
  try {
    await opts.request();
    invalidate(opts.after ?? "all");
    invalidate(["state"]);
    return true;
  } catch (e) {
    opts.rollback?.();
    invalidate(opts.after ?? "all");
    showToast(e instanceof Error ? e.message : "request failed");
    return false;
  }
}

/** Snapshot a resource's current value for rollback. */
export function snapshot<T>(r: Resource<T>): T | null {
  return r.data.value;
}

/** Wire background revalidation on window focus / visibility (fresh data when you return). */
export function installRevalidateOnFocus() {
  const onFocus = () => invalidate("all");
  window.addEventListener("focus", onFocus);
  document.addEventListener("visibilitychange", () => {
    if (document.visibilityState === "visible") invalidate("all");
  });
}
