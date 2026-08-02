import { useState } from "preact/hooks";
import { api, type Priority, type Task } from "../../api";
import { invalidate, showToast } from "../../lib/cache";
import { t } from "../../lib/i18n";
import { parseOffsetList } from "../../lib/notify";
import { meta } from "../../state/meta";
import { closeModal, flashCreated } from "../../state/ui";
import { Overlay } from "./ModalHost";

const PRIORITIES: Priority[] = ["None", "Low", "Medium", "High"];

// The due picker is a LOCAL calendar date; the API carries a true UTC instant (see lib/time.ts).
function toDateInput(rfc?: string): string {
  if (!rfc) return "";
  const d = new Date(rfc);
  const p = (n: number) => String(n).padStart(2, "0");
  return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())}`;
}

function fromDateInput(v: string, hour = 23, min = 59): string | undefined {
  if (!v) return undefined;
  const [y, m, d] = v.split("-").map(Number);
  return new Date(y, m - 1, d, hour, min, 0).toISOString(); // due = end of local day, like the TUI
}

/** First status of kind "open" — where a new task belongs unless the caller says otherwise. */
function defaultStatus(): string {
  const statuses = meta.value?.statuses ?? [];
  return (statuses.find((s) => s.kind === "open") ?? statuses[0])?.id ?? "todo";
}

export function TaskForm({ task, prefill }: { task?: Task; prefill?: Partial<Task> }) {
  const editing = !!task;
  const init = task ?? prefill ?? {};
  const [title, setTitle] = useState(init.title ?? "");
  const [project, setProject] = useState(init.project ?? "");
  const [due, setDue] = useState(toDateInput(init.due));
  // Scheduled is when you plan to work on it (calendar placement); due is the deadline.
  const [scheduled, setScheduled] = useState(toDateInput(init.scheduled));
  const [priority, setPriority] = useState<Priority>(init.priority ?? "None");
  const [status, setStatus] = useState(init.status ?? defaultStatus());
  const [tagsText, setTagsText] = useState((init.tags ?? []).join(", "));
  const [body, setBody] = useState(init.body ?? "");
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
      const tags = tagsText.split(",").map((s) => s.trim()).filter(Boolean);
      const patch: Partial<Task> = {
        title: title.trim(),
        project: project.trim() || undefined,
        due: dueISO,
        // Scheduled is a working day, anchored at the start of it rather than the end.
        scheduled: fromDateInput(scheduled, 9, 0),
        priority,
        status,
        tags: tags.length ? tags : undefined,
        body,
        // Reminders fire relative to the due date — only meaningful when one is set.
        reminders: dueISO && reminders.length ? reminders : undefined,
      };
      if (editing) {
        await api.updateTask({ ...task!, ...patch } as Task);
      } else {
        const created = await api.createTaskFull({ ...patch, title: title.trim() } as Partial<Task> & { title: string });
        flashCreated(created.uid);
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
          <label for="tf-title">{t("Title")}</label>
          <input id="tf-title" autofocus value={title} onInput={(e) => setTitle((e.target as HTMLInputElement).value)} />
        </div>
        <div class="row">
          <div class="field grow">
            <label for="tf-project">{t("Project")}</label>
            <input id="tf-project" list="projects" value={project} onInput={(e) => setProject((e.target as HTMLInputElement).value)} />
            <datalist id="projects">
              {(meta.value?.projects ?? []).map((p) => <option value={p.name} />)}
            </datalist>
          </div>
          <div class="field grow">
            <label for="tf-status">{t("Status")}</label>
            <select id="tf-status" value={status} onChange={(e) => setStatus((e.target as HTMLSelectElement).value)}>
              {(meta.value?.statuses ?? []).map((s) => <option value={s.id}>{s.label}</option>)}
            </select>
          </div>
        </div>
        <div class="row">
          <div class="field grow">
            <label for="tf-due">{t("Due")}</label>
            <input id="tf-due" type="date" value={due} onInput={(e) => setDue((e.target as HTMLInputElement).value)} />
          </div>
          <div class="field grow">
            <label for="tf-sched">{t("Scheduled")}</label>
            <input id="tf-sched" type="date" value={scheduled} onInput={(e) => setScheduled((e.target as HTMLInputElement).value)} />
          </div>
          <div class="field grow">
            <label for="tf-prio">{t("Priority")}</label>
            <select id="tf-prio" value={priority} onChange={(e) => setPriority((e.target as HTMLSelectElement).value as Priority)}>
              {PRIORITIES.map((p) => <option value={p}>{t(p)}</option>)}
            </select>
          </div>
        </div>
        <div class="row">
          <div class="field grow">
            <label for="tf-tags">{t("Tags")}</label>
            <input
              id="tf-tags"
              placeholder={t("comma-separated")}
              value={tagsText}
              onInput={(e) => setTagsText((e.target as HTMLInputElement).value)}
            />
          </div>
          <div class="field grow">
            <label for="tf-rem">{t("Reminders")}</label>
            <input
              id="tf-rem"
              placeholder={t("e.g. 15m, 1h, 1d — empty for none")}
              value={remindersText}
              onInput={(e) => setRemindersText((e.target as HTMLInputElement).value)}
            />
          </div>
        </div>
        <div class="field">
          <label for="tf-body">{t("Notes")}</label>
          <textarea id="tf-body" rows={4} value={body} onInput={(e) => setBody((e.target as HTMLTextAreaElement).value)} />
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
