import { api } from "../../api";
import { invalidate, resource } from "../../lib/cache";
import { closeModal } from "../../state/ui";
import { Overlay } from "./ModalHost";

export function Trash() {
  const res = resource("trash", api.trash);
  const t = res.data.value ?? { tasks: [], projects: [], empty: true };

  const act = async (fn: () => Promise<unknown>) => {
    await fn();
    invalidate("all");
  };

  return (
    <Overlay>
      <h2>Trash</h2>
      {t.empty && <div class="muted">Trash is empty.</div>}
      <div style={{ maxHeight: "56vh", overflow: "auto" }}>
        {t.tasks.map((task) => (
          <div class="pickrow row">
            <span class="pill">task</span>
            <span class="grow">{task.title}</span>
            <button class="icon" title="Restore" onClick={() => act(() => api.trashRestore("task", task.uid))}>↩</button>
            <button class="icon" title="Delete forever" onClick={() => act(() => api.trashPurge("task", task.uid))}>🗑</button>
          </div>
        ))}
        {t.projects.map((p) => (
          <div class="pickrow row">
            <span class="pill">proj</span>
            <span class="grow">{p.name}</span>
            <button class="icon" title="Restore" onClick={() => act(() => api.trashRestore("project", p.name))}>↩</button>
            <button class="icon" title="Delete forever" onClick={() => act(() => api.trashPurge("project", p.name))}>🗑</button>
          </div>
        ))}
      </div>
      <div class="actions">
        {!t.empty && <button style={{ marginRight: "auto", color: "var(--red)" }} onClick={() => act(() => api.trashEmpty())}>Empty trash</button>}
        <button onClick={closeModal}>Close</button>
      </div>
    </Overlay>
  );
}
