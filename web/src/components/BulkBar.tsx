// Bulk-action bar shown while a multi-selection is active (shared by Tasks and Board).

import { api } from "../api";
import { mutate } from "../lib/cache";
import { t } from "../lib/i18n";
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

  const confirmDelete = () =>
    openModal({
      kind: "confirm",
      message: t("Delete {n} selected task(s)?").replace("{n}", String(n)),
      onConfirm: () => void bulk((uid) => api.deleteTask(uid)),
    });

  return (
    <div class="bulkbar">
      <span>{n} {t("selected")}</span>
      <button onClick={() => bulk((uid) => api.toggle(uid))}>{t("Done")}</button>
      <button onClick={() => bulk((uid) => api.cyclePriority(uid))}>{t("Priority")}</button>
      <button onClick={() => openModal({ kind: "projectPicker", taskUids: sel.orderedSelection(orderedIds) })}>
        {t("Project")}
      </button>
      <button onClick={confirmDelete}>{t("Delete")}</button>
      <button class="icon" aria-label={t("Clear selection")} onClick={() => sel.clear()}>✕</button>
    </div>
  );
}
