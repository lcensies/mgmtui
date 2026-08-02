import { useState } from "preact/hooks";
import { api, type Task } from "../api";
import { invalidate, showToast } from "../lib/cache";
import { t } from "../lib/i18n";
import { parseQuickAdd } from "../lib/quickparse";
import { meta } from "../state/meta";
import { flashCreated, inheritedProject, openModal, pinScope, scopedProject, setPinScope } from "../state/ui";
import { Icon } from "./Icon";

export function QuickAdd() {
  const [title, setTitle] = useState("");
  const [busy, setBusy] = useState(false);

  /** Typed text → task fields. An explicit `#project` token beats the active project scope. */
  function fields(text: string): Partial<Task> & { title: string } {
    const parsed = parseQuickAdd(text, (meta.value?.projects ?? []).map((p) => p.name));
    return { ...parsed, project: parsed.project ?? inheritedProject() };
  }

  async function add(e: Event) {
    e.preventDefault();
    const text = title.trim();
    if (!text || busy) return;
    const parsed = fields(text);
    if (!parsed.title) {
      showToast(t("Give the task a title"));
      return;
    }
    setBusy(true);
    try {
      const created = await api.createTaskFull(parsed);
      setTitle("");
      flashCreated(created.uid);
      invalidate("all");
    } catch (err) {
      showToast(err instanceof Error ? err.message : t("request failed"));
    } finally {
      setBusy(false);
    }
  }

  // Hand the typed text to the full form instead of creating right away.
  function expand() {
    openModal({ kind: "taskForm", prefill: title.trim() ? fields(title.trim()) : { project: inheritedProject() } });
    setTitle("");
  }

  // The scoped project is app-wide state, so this reads the same from any panel — the toggle
  // just says whether quick-add should use it.
  const scoped = scopedProject();
  const on = pinScope.value && !!scoped;

  return (
    <form class="quickadd" onSubmit={add}>
      <button
        type="button"
        class={`scope-pin ${on ? "on" : ""}`}
        disabled={!scoped}
        aria-pressed={on}
        title={
          scoped
            ? `${t("Add to current project")}: ${scoped}`
            : t("Select a single project to file new tasks under it")
        }
        onClick={() => setPinScope(!pinScope.value)}
      >
        {scoped ? `#${scoped}` : t("No project")}
      </button>
      <input
        placeholder={t("Quick add task…  #project @tomorrow !high")}
        value={title}
        onInput={(e) => setTitle((e.target as HTMLInputElement).value)}
      />
      <button class="icon" type="button" title={t("More options")} aria-label={t("More options")} onClick={expand}>
        <Icon name="chevronUp" size={18} />
      </button>
      <button class="primary" type="submit" disabled={busy} aria-label={t("Add task")}>
        <Icon name="plus" size={18} />
      </button>
    </form>
  );
}
