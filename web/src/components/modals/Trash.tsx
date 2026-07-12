import { api } from "../../api";
import { invalidate, resource, showToast } from "../../lib/cache";
import { t } from "../../lib/i18n";
import { closeModal, openModal } from "../../state/ui";
import { Overlay } from "./ModalHost";

export function Trash() {
  const res = resource("trash", api.trash);
  const tr = res.data.value ?? { tasks: [], projects: [], empty: true };

  const act = async (fn: () => Promise<unknown>) => {
    try {
      await fn();
    } catch (err) {
      showToast(err instanceof Error ? err.message : t("request failed"));
    } finally {
      invalidate("all");
    }
  };

  // Destructive actions go through the confirm modal, which returns to the trash afterwards.
  const confirmThen = (message: string, fn: () => Promise<unknown>) =>
    openModal({
      kind: "confirm",
      message,
      onConfirm: () => {
        openModal({ kind: "trash" });
        void act(fn);
      },
    });

  return (
    <Overlay>
      <h2>{t("Trash")}</h2>
      {tr.empty && <div class="muted">{t("Trash is empty.")}</div>}
      <div style={{ maxHeight: "56vh", overflow: "auto" }}>
        {tr.tasks.map((task) => (
          <div class="pickrow row">
            <span class="pill">{t("task")}</span>
            <span class="grow">{task.title}</span>
            <button class="icon" title={t("Restore")} onClick={() => act(() => api.trashRestore("task", task.uid))}>↩</button>
            <button
              class="icon"
              title={t("Purge")}
              onClick={() => confirmThen(t('Permanently delete "{name}"?').replace("{name}", task.title), () => api.trashPurge("task", task.uid))}
            >🗑</button>
          </div>
        ))}
        {tr.projects.map((p) => (
          <div class="pickrow row">
            <span class="pill">{t("proj")}</span>
            <span class="grow">{p.name}</span>
            <button class="icon" title={t("Restore")} onClick={() => act(() => api.trashRestore("project", p.name))}>↩</button>
            <button
              class="icon"
              title={t("Purge")}
              onClick={() => confirmThen(t('Permanently delete "{name}"?').replace("{name}", p.name), () => api.trashPurge("project", p.name))}
            >🗑</button>
          </div>
        ))}
      </div>
      <div class="actions">
        {!tr.empty && (
          <button
            style={{ marginRight: "auto", color: "var(--red)" }}
            onClick={() => confirmThen(t("Empty the trash? This cannot be undone."), () => api.trashEmpty())}
          >
            {t("Empty trash")}
          </button>
        )}
        <button onClick={closeModal}>{t("Close")}</button>
      </div>
    </Overlay>
  );
}
