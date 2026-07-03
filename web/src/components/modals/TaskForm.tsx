import { useState } from "preact/hooks";
import { api, type Priority, type Task } from "../../api";
import { invalidate } from "../../lib/cache";
import { meta } from "../../state/meta";
import { closeModal } from "../../state/ui";
import { Overlay } from "./ModalHost";

const PRIORITIES: Priority[] = ["None", "Low", "Medium", "High"];

// Due dates are UTC wall-clock (see lib/time.ts).
function toDateInput(rfc?: string): string {
  if (!rfc) return "";
  const d = new Date(rfc);
  const p = (n: number) => String(n).padStart(2, "0");
  return `${d.getUTCFullYear()}-${p(d.getUTCMonth() + 1)}-${p(d.getUTCDate())}`;
}

function fromDateInput(v: string): string | undefined {
  if (!v) return undefined;
  const [y, m, d] = v.split("-").map(Number);
  return new Date(Date.UTC(y, m - 1, d, 23, 59, 0)).toISOString(); // due = end of day, like the TUI
}

export function TaskForm({ task }: { task?: Task }) {
  const editing = !!task;
  const [title, setTitle] = useState(task?.title ?? "");
  const [project, setProject] = useState(task?.project ?? "");
  const [due, setDue] = useState(toDateInput(task?.due));
  const [priority, setPriority] = useState<Priority>(task?.priority ?? "None");
  const [body, setBody] = useState(task?.body ?? "");
  const [busy, setBusy] = useState(false);

  async function submit(e: Event) {
    e.preventDefault();
    if (!title.trim() || busy) return;
    setBusy(true);
    try {
      const patch: Partial<Task> = {
        title: title.trim(),
        project: project.trim() || undefined,
        due: fromDateInput(due),
        priority,
        body,
      };
      if (editing) {
        await api.updateTask({ ...task!, ...patch } as Task);
      } else {
        const created = await api.createTask(title.trim(), project.trim() || undefined);
        if (due || priority !== "None" || body.trim()) {
          await api.updateTask({ ...created, ...patch } as Task);
        }
      }
      invalidate("all");
      closeModal();
    } catch (err) {
      setBusy(false);
    }
  }

  return (
    <Overlay>
      <form onSubmit={submit} style={{ display: "flex", flexDirection: "column", gap: "10px" }}>
        <h2>{editing ? "Edit task" : "New task"}</h2>
        <div class="field">
          <label>Title</label>
          <input autofocus value={title} onInput={(e) => setTitle((e.target as HTMLInputElement).value)} />
        </div>
        <div class="field">
          <label>Project</label>
          <input list="projects" value={project} onInput={(e) => setProject((e.target as HTMLInputElement).value)} />
          <datalist id="projects">
            {(meta.value?.projects ?? []).map((p) => <option value={p.name} />)}
          </datalist>
        </div>
        <div class="row">
          <div class="field grow">
            <label>Due</label>
            <input type="date" value={due} onInput={(e) => setDue((e.target as HTMLInputElement).value)} />
          </div>
          <div class="field grow">
            <label>Priority</label>
            <select value={priority} onChange={(e) => setPriority((e.target as HTMLSelectElement).value as Priority)}>
              {PRIORITIES.map((p) => <option value={p}>{p}</option>)}
            </select>
          </div>
        </div>
        <div class="field">
          <label>Notes</label>
          <textarea rows={4} value={body} onInput={(e) => setBody((e.target as HTMLTextAreaElement).value)} />
        </div>
        <div class="actions">
          <button type="button" onClick={closeModal}>Cancel</button>
          <button class="primary" type="submit" disabled={busy}>{editing ? "Save" : "Create"}</button>
        </div>
      </form>
    </Overlay>
  );
}
