import { useState } from "preact/hooks";
import { api } from "../api";
import { invalidate, showToast } from "../lib/cache";
import { t } from "../lib/i18n";
import { Icon } from "./Icon";

export function QuickAdd() {
  const [title, setTitle] = useState("");
  const [busy, setBusy] = useState(false);

  async function add(e: Event) {
    e.preventDefault();
    const text = title.trim();
    if (!text || busy) return;
    setBusy(true);
    try {
      await api.createTask(text);
      setTitle("");
      invalidate("all");
    } catch (err) {
      showToast(err instanceof Error ? err.message : t("request failed"));
    } finally {
      setBusy(false);
    }
  }

  return (
    <form class="quickadd" onSubmit={add}>
      <input
        placeholder={t("Quick add task…")}
        value={title}
        onInput={(e) => setTitle((e.target as HTMLInputElement).value)}
      />
      <button class="primary" type="submit" disabled={busy} aria-label={t("Add task")}>
        <Icon name="plus" size={18} />
      </button>
    </form>
  );
}
