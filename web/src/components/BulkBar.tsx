// Bulk-action bar shown while a multi-selection is active (shared by Tasks and Board).

import { api, type Task } from "../api";
import { mutate } from "../lib/cache";
import { t } from "../lib/i18n";
import * as sel from "../lib/selection";
import { meta } from "../state/meta";
import { openModal, selected } from "../state/ui";

export function BulkBar({ orderedIds }: { orderedIds: string[] }) {
  const n = selected.value.size;
  if (!n) return null;

  async function bulk(fn: (uid: string) => Promise<unknown>) {
    const ids = sel.orderedSelection(orderedIds);
    await mutate({ request: () => Promise.all(ids.map(fn)), after: ["tasks", "board", "agenda", "trash"] });
    sel.clear();
  }

  // No bulk endpoint server-side; selections are small, so fan out over the per-uid routes.
  const setDue = (v: string) => {
    if (!v) return;
    const [y, m, d] = v.split("-").map(Number);
    const due = new Date(y, m - 1, d, 23, 59, 0).toISOString();
    void bulk(async (uid) => api.updateTask({ ...(await api.task(uid)), due } as Task));
  };

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
      <select
        title={t("Status")}
        value=""
        onChange={(e) => {
          const status = (e.target as HTMLSelectElement).value;
          (e.target as HTMLSelectElement).value = "";
          if (status) void bulk((uid) => api.setStatus(uid, status));
        }}
      >
        <option value="">{t("Status")}…</option>
        {(meta.value?.statuses ?? []).map((s) => <option value={s.id}>{s.label}</option>)}
      </select>
      <input
        type="date"
        title={t("Due")}
        onChange={(e) => {
          const el = e.target as HTMLInputElement;
          setDue(el.value);
          el.value = "";
        }}
      />
      <button onClick={() => openModal({ kind: "projectPicker", taskUids: sel.orderedSelection(orderedIds) })}>
        {t("Project")}
      </button>
      <button onClick={confirmDelete}>{t("Delete")}</button>
      <button class="icon" aria-label={t("Clear selection")} onClick={() => sel.clear()}>✕</button>
    </div>
  );
}
