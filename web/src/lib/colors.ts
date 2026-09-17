// Color resolution mirroring the TUI (mgmt-tui/src/theme.rs) and the config/domain color model, so
// the web app colors projects/statuses/events identically to the terminal. Values from the server
// are strings: a `#rrggbb` hex, an ANSI index ("8"), or a named ANSI color. We resolve them to CSS.

// A curated 16-color palette (indices 0..15) — Google/Material-ish event hues that read well on
// both light and dark backgrounds with black-or-white label text. Exact RGB only needs internal
// consistency; the 10 auto-color slots map here by name.
const ANSI16: string[] = [
  "#3c4257", // 0 black (slate)
  "#ea4335", // 1 red (tomato)
  "#1e8e3e", // 2 green (basil)
  "#f9ab00", // 3 yellow (banana)
  "#4285f4", // 4 blue (peacock/primary)
  "#a142f4", // 5 magenta (grape)
  "#12b5cb", // 6 cyan
  "#cbd5e1", // 7 white (light gray)
  "#64748b", // 8 bright black (dark gray)
  "#ff7043", // 9 bright red (tangerine)
  "#34a853", // 10 bright green (sage)
  "#fbbc04", // 11 bright yellow
  "#5c6bc0", // 12 bright blue (blueberry)
  "#d081e8", // 13 bright magenta (lavender-pink)
  "#26a69a", // 14 bright cyan (teal)
  "#f1f5f9", // 15 bright white
];

// Named ANSI colors → index into ANSI16 (matches theme.rs::parse_color's accepted names).
const NAMES: Record<string, number> = {
  black: 0, red: 1, green: 2, yellow: 3, blue: 4, magenta: 5, cyan: 6,
  gray: 7, grey: 7, white: 15,
  darkgray: 8, darkgrey: 8,
  lightred: 9, lightgreen: 10, lightyellow: 11, lightblue: 12, lightmagenta: 13, lightcyan: 14,
};

// The 10-entry auto-assign palette (must match mgmt-domain/src/palette.rs order exactly).
const PALETTE = [
  "blue", "green", "magenta", "cyan", "yellow", "red",
  "lightblue", "lightgreen", "lightmagenta", "lightcyan",
];

/** Resolve an xterm-256 ANSI index to a hex string. */
function ansiIndexToHex(i: number): string {
  if (i < 16) return ANSI16[i];
  if (i >= 232) {
    const v = 8 + (i - 232) * 10; // grayscale ramp
    return rgb(v, v, v);
  }
  const n = i - 16; // 6x6x6 cube
  const r = Math.floor(n / 36);
  const g = Math.floor((n % 36) / 6);
  const b = n % 6;
  const c = (x: number) => (x === 0 ? 0 : 55 + x * 40);
  return rgb(c(r), c(g), c(b));
}

function rgb(r: number, g: number, b: number): string {
  const h = (x: number) => x.toString(16).padStart(2, "0");
  return `#${h(r)}${h(g)}${h(b)}`;
}

/** Parse a color string to CSS, mirroring theme.rs::parse_color. Returns null if unrecognized. */
export function parseColor(s: string | null | undefined): string | null {
  if (!s) return null;
  const t = s.trim();
  if (t.startsWith("#")) return /^#[0-9a-fA-F]{6}$/.test(t) ? t : null;
  if (/^\d+$/.test(t)) {
    const idx = parseInt(t, 10);
    return idx >= 0 && idx <= 255 ? ansiIndexToHex(idx) : null;
  }
  const key = t.toLowerCase().replace(/[ _-]/g, "");
  return key in NAMES ? ANSI16[NAMES[key]] : null;
}

/** Stable auto-color by FNV-1a hash (u64 — must use BigInt), matching palette.rs::auto_color. */
export function autoColor(name: string): string {
  let hash = 0xcbf29ce484222325n;
  const mask = 0xffffffffffffffffn;
  for (const byte of new TextEncoder().encode(name)) {
    hash = (hash ^ BigInt(byte)) & mask;
    hash = (hash * 0x100000001b3n) & mask;
  }
  const idx = Number(hash % BigInt(PALETTE.length));
  return parseColor(PALETTE[idx]) ?? "#89b4fa";
}

/** djb2 hash (32-bit) over a string, matching the TUI's event_palette selection. */
function djb2(s: string): number {
  let h = 5381 >>> 0;
  for (let i = 0; i < s.length; i++) h = (((h * 33) >>> 0) ^ s.charCodeAt(i)) >>> 0;
  return h >>> 0;
}

/** Contrasting text color (black/white) for a hex background, by relative luminance. */
export function contrastText(hex: string): string {
  const m = /^#([0-9a-fA-F]{2})([0-9a-fA-F]{2})([0-9a-fA-F]{2})$/.exec(hex);
  if (!m) return "#11111b";
  const [r, g, b] = [1, 2, 3].map((i) => parseInt(m[i], 16) / 255);
  // Rec. 709 relative luminance. (The blue coefficient was previously 0.4722 — a typo that made
  // blue backgrounds read as "light" and pick dark text, wrecking contrast on the default palette.)
  const lum = 0.2126 * r + 0.7152 * g + 0.0722 * b;
  return lum > 0.6 ? "#11111b" : "#f8f8f2";
}

// --- domain color resolution (all take the resolved meta so precedence is already applied) -------

export interface MetaColors {
  projects: { name: string; color: string }[];
  statuses: { id: string; label: string; kind: string; color: string }[];
  calendar?: { event_palette?: string[] };
  theme?: Record<string, string>;
}

/** Project color: prefer the server-resolved value in meta (which already applied .md → config →
 *  auto precedence); fall back to the auto hash. */
export function projectColor(name: string | undefined, meta: MetaColors | null): string {
  if (!name) return themeSlot("task", meta);
  const p = meta?.projects.find((x) => x.name === name);
  return parseColor(p?.color) ?? autoColor(name);
}

/** Status color: server-resolved in meta, else auto. */
export function statusColor(id: string, meta: MetaColors | null): string {
  const s = meta?.statuses.find((x) => x.id === id);
  return parseColor(s?.color) ?? autoColor(id);
}

/** Event color: the event's own `color` if set, then the project color, then the config
 *  event_palette by djb2(calendar), then the theme slot. */
export function eventColor(
  ev: { project?: string; calendar: string; color?: string },
  meta: MetaColors | null,
): string {
  const own = parseColor(ev.color);
  if (own) return own;
  if (ev.project) return projectColor(ev.project, meta);
  const palette = (meta?.calendar?.event_palette ?? []).map((c) => parseColor(c)).filter(Boolean) as string[];
  if (palette.length) return palette[djb2(ev.calendar) % palette.length];
  return themeSlot("event", meta);
}

// Theme slot defaults (used for uncolored fallbacks), overridable by meta.theme.
const THEME_DEFAULTS: Record<string, string> = {
  accent: "blue", selected_bg: "lightblue", selected_fg: "black", dim: "darkgray",
  today: "yellow", event: "blue", task: "magenta", done: "darkgray", border: "gray",
};

export function themeSlot(slot: string, meta: MetaColors | null): string {
  const override = meta?.theme?.[slot];
  return parseColor(override) ?? parseColor(THEME_DEFAULTS[slot]) ?? "#cdd6f4";
}

/** Build CSS custom properties for the theme, applied once at meta load. */
export function themeVars(meta: MetaColors | null): Record<string, string> {
  const vars: Record<string, string> = {};
  for (const slot of Object.keys(THEME_DEFAULTS)) {
    vars[`--th-${slot.replace(/_/g, "-")}`] = themeSlot(slot, meta);
  }
  return vars;
}
