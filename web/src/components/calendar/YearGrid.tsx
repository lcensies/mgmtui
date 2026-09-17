// Year view: 12 mini months for the anchor's year, with a dot on days that have events.

import type { EventItem } from "../../api";
import { eventDays, MiniMonth } from "./MiniMonth";

export function YearGrid({ anchor, events, onDay }: { anchor: Date; events: EventItem[]; onDay: (d: Date) => void }) {
  const marks = eventDays(events);
  return (
    <div class="year">
      {Array.from({ length: 12 }, (_, m) => (
        <MiniMonth month={new Date(anchor.getFullYear(), m, 1)} marks={marks} selected={anchor} onDay={onDay} />
      ))}
    </div>
  );
}
