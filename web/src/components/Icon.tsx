// Minimal inline SVG icon set (Lucide-derived, 24px grid, currentColor stroke). No emoji, no
// raster — scalable and theme-aware.

import type { JSX } from "preact";

const P = (d: string) => <path d={d} />;

const ICONS: Record<string, JSX.Element[]> = {
  calendar: [<rect x="3" y="4" width="18" height="18" rx="2" />, P("M16 2v4"), P("M8 2v4"), P("M3 10h18")],
  board: [<rect x="3" y="3" width="18" height="18" rx="2" />, P("M9 3v18"), P("M15 3v18")],
  tasks: [<rect x="3" y="3" width="18" height="18" rx="2" />, P("m9 12 2 2 4-4")],
  focus: [<circle cx="12" cy="13" r="8" />, P("M12 9v4l2 2"), P("M9 2h6")],
  plus: [P("M12 5v14"), P("M5 12h14")],
  command: [P("M15 6a3 3 0 1 0-3 3H9a3 3 0 1 0 3 3v-3h3a3 3 0 1 0-3-3z"), P("M9 9h6v6H9z")],
  undo: [P("M9 14 4 9l5-5"), P("M4 9h11a5 5 0 0 1 0 10h-4")],
  redo: [P("m15 14 5-5-5-5"), P("M20 9H9a5 5 0 0 0 0 10h4")],
  sun: [<circle cx="12" cy="12" r="4" />, P("M12 2v2"), P("M12 20v2"), P("m4.9 4.9 1.4 1.4"), P("m17.7 17.7 1.4 1.4"), P("M2 12h2"), P("M20 12h2"), P("m6.3 17.7-1.4 1.4"), P("m19.1 4.9-1.4 1.4")],
  moon: [P("M12 3a6 6 0 0 0 9 9 9 9 0 1 1-9-9Z")],
  settings: [P("M20 7h-9"), P("M14 17H5"), <circle cx="17" cy="17" r="3" />, <circle cx="7" cy="7" r="3" />],
  x: [P("M18 6 6 18"), P("M6 6l12 12")],
  chevronLeft: [P("m15 18-6-6 6-6")],
  chevronRight: [P("m9 18 6-6-6-6")],
  chevronUp: [P("m18 15-6-6-6 6")],
  trash: [P("M3 6h18"), P("M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6"), P("M8 6V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2")],
  logout: [P("M9 21H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h4"), P("m16 17 5-5-5-5"), P("M21 12H9")],
  check: [P("M20 6 9 17l-5-5")],
};

export function Icon({ name, size = 20 }: { name: string; size?: number }) {
  return (
    <svg width={size} height={size} viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
      {ICONS[name] ?? null}
    </svg>
  );
}
