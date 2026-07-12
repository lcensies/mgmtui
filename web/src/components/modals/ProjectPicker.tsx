import { useState } from "preact/hooks";
import { api } from "../../api";
import { invalidate } from "../../lib/cache";
import { fuzzyFilter } from "../../lib/fuzzy";
import { t } from "../../lib/i18n";
import { projectColor } from "../../lib/colors";
import { meta } from "../../state/meta";
import { closeModal } from "../../state/ui";
import { Overlay } from "./ModalHost";

/** Assign (or clear) a project on one or more tasks, with fuzzy search + "new" + "clear". */
export function ProjectPicker({ taskUids }: { taskUids: string[] }) {
  const [query, setQuery] = useState("");
  const projects = meta.value?.projects ?? [];
  const matches = fuzzyFilter(query, projects, (p) => p.name);
  const exact = projects.some((p) => p.name.toLowerCase() === query.trim().toLowerCase());

  async function apply(project: string | undefined) {
    try {
      await Promise.all(taskUids.map((uid) => api.setTaskProject(uid, project)));
      invalidate("all");
    } finally {
      closeModal();
    }
  }

  return (
    <Overlay>
      <h2>{t("Assign project")}</h2>
      <input
        autofocus
        placeholder={t("Search projects…")}
        value={query}
        onInput={(e) => setQuery((e.target as HTMLInputElement).value)}
      />
      <div style={{ display: "flex", flexDirection: "column", gap: "2px", maxHeight: "50vh", overflow: "auto" }}>
        <button class="pickrow dim" onClick={() => apply(undefined)}>{t("(clear project)")}</button>
        {query.trim() && !exact && (
          <button class="pickrow" style={{ color: "var(--green)" }} onClick={() => apply(query.trim())}>
            + {t("new")}: {query.trim()}
          </button>
        )}
        {matches.map((p) => (
          <button class="pickrow" onClick={() => apply(p.name)}>
            <span style={{ color: projectColor(p.name, meta.value) }}>●</span> {p.name}
          </button>
        ))}
      </div>
      <div class="actions">
        <button type="button" onClick={closeModal}>{t("Cancel")}</button>
      </div>
    </Overlay>
  );
}
