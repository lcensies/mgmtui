import { useState } from "preact/hooks";
import { api, type Priority, type Task } from "../../api";
import { invalidate, showToast } from "../../lib/cache";
import { t } from "../../lib/i18n";
import { parseOffsetList } from "../../lib/notify";
import { meta } from "../../state/meta";
import { closeModal } from "../../state/ui";
import { Overlay } from "./ModalHost";

const PRIORITIES: Priority[] = ["None", "Low", "Medium", "High"];

// The due picker is a LOCAL calendar date; the API carries a true UTC instant (see lib/time.ts).
function toDateInput(rfc?: string): string {
  if (!rfc) return "";
  const d = new Date(rfc);
  const p = (n: number) => String(n).padStart(2, "0");
  return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())}`;
}

function fromDateInput(v: string): string | undefined {
  if (!v) return undefined;
  const [y, m, d] = v.split("-").map(Number);
  return new Date(y, m - 1, d, 23, 59, 0).toISOString(); // due = end of local day, like the TUI
}

export function TaskForm({ task }: { task?: Task }) {
  const editing = !!task;
  const [title, setTitle] = useState(task?.title ?? "");
  const [project, setProject] = useState(task?.project ?? "");
  const [due, setDue] = useState(toDateInput(task?.due));
  const [priority, setPriority] = useState<Priority>(task?.priority ?? "None");
  const [body, setBody] = useState(task?.body ?? "");
  // New tasks start with the configured reminder defaults (applied when a due date is set).
  const [remindersText, setRemindersText] = useState(
    editing ? (task?.reminders ?? []).join(", ") : (meta.value?.default_reminders ?? []).join(", "),
  );
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);

  async function submit(e: Event) {
    e.preventDefault();
    if (!title.trim() || busy) return;
    setError("");
    const reminders = parseOffsetList(remindersText);
    if (reminders === null) {
      setError(t("Reminders must be offsets like 15m, 1h, 1d"));
      return;
    }
    setBusy(true);
    try {
      const dueISO = fromDateInput(due);
      const patch: Partial<Task> = {
        title: title.trim(),
        project: project.trim() || undefined,
        due: dueISO,
        priority,
        body,
        // Reminders fire relative to the due date — only meaningful when one is set.
        reminders: dueISO && reminders.length ? reminders : undefined,
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
      showToast(err instanceof Error ? err.message : t("save failed"));
      setBusy(false);
    }
  }

  return (
    <Overlay>
      <form onSubmit={submit} style={{ display: "flex", flexDirection: "column", gap: "10px" }}>
        <h2>{editing ? t("Edit task") : t("New task")}</h2>
        <div class="field">
          <label>{t("Title")}</label>
          <input autofocus value={title} onInput={(e) => setTitle((e.target as HTMLInputElement).value)} />
        </div>
        <div class="field">
          <label>{t("Project")}</label>
          <input list="projects" value={project} onInput={(e) => setProject((e.target as HTMLInputElement).value)} />
          <datalist id="projects">
            {(meta.value?.projects ?? []).map((p) => <option value={p.name} />)}
          </datalist>
        </div>
        <div class="row">
          <div class="field grow">
            <label>{t("Due")}</label>
            <input type="date" value={due} onInput={(e) => setDue((e.target as HTMLInputElement).value)} />
          </div>
          <div class="field grow">
            <label>{t("Priority")}</label>
            <select value={priority} onChange={(e) => setPriority((e.target as HTMLSelectElement).value as Priority)}>
              {PRIORITIES.map((p) => <option value={p}>{t(p)}</option>)}
            </select>
          </div>
        </div>
        <div class="field">
          <label>{t("Reminders")}</label>
          <input
            placeholder={t("e.g. 15m, 1h, 1d — empty for none")}
            value={remindersText}
            onInput={(e) => setRemindersText((e.target as HTMLInputElement).value)}
          />
        </div>
        <div class="field">
          <label>{t("Notes")}</label>
          <textarea rows={4} value={body} onInput={(e) => setBody((e.target as HTMLTextAreaElement).value)} />
        </div>
        {error && <div class="error">{error}</div>}
        <div class="actions">
          <button type="button" onClick={closeModal}>{t("Cancel")}</button>
          <button class="primary" type="submit" disabled={busy}>{editing ? t("Save") : t("Create")}</button>
        </div>
      </form>
    </Overlay>
  );
}
