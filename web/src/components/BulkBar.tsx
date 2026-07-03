// Bulk-action bar shown while a multi-selection is active (shared by Tasks and Board).

import { api } from "../api";
import { mutate } from "../lib/cache";
import * as sel from "../lib/selection";
import { openModal, selected } from "../state/ui";

export function BulkBar({ orderedIds }: { orderedIds: string[] }) {
  const n = selected.value.size;
  if (!n) return null;

  async function bulk(fn: (uid: string) => Promise<unknown>) {
    const ids = sel.orderedSelection(orderedIds);
    await mutate({ request: () => Promise.all(ids.map(fn)), after: ["tasks", "board", "agenda", "trash"] });
    sel.clear();
  }

  return (
    <div class="bulkbar">
      <span>{n} selected</span>
      <button onClick={() => bulk((uid) => api.toggle(uid))}>Done</button>
      <button onClick={() => bulk((uid) => api.cyclePriority(uid))}>Priority</button>
      <button onClick={() => openModal({ kind: "projectPicker", taskUids: sel.orderedSelection(orderedIds) })}>
        Project
      </button>
      <button onClick={() => bulk((uid) => api.deleteTask(uid))}>Delete</button>
      <button class="icon" onClick={() => sel.clear()}>✕</button>
    </div>
  );
}
