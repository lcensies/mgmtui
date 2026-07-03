import { useState } from "preact/hooks";
import { api } from "../api";
import { invalidate } from "../lib/cache";
import { Icon } from "./Icon";

export function QuickAdd() {
  const [title, setTitle] = useState("");
  const [busy, setBusy] = useState(false);

  async function add(e: Event) {
    e.preventDefault();
    const t = title.trim();
    if (!t || busy) return;
    setBusy(true);
    try {
      await api.createTask(t);
      setTitle("");
      invalidate("all");
    } finally {
      setBusy(false);
    }
  }

  return (
    <form class="quickadd" onSubmit={add}>
      <input
        placeholder="Quick add task…"
        value={title}
        onInput={(e) => setTitle((e.target as HTMLInputElement).value)}
      />
      <button class="primary" type="submit" disabled={busy} aria-label="Add task">
        <Icon name="plus" size={18} />
      </button>
    </form>
  );
}
