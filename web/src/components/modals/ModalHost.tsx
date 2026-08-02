// Renders the currently-open modal. Grows as modals land (TaskForm in M2; EventForm/palette/trash/
// help in M6). A shared overlay wrapper handles the backdrop + Escape-to-close.

import type { ComponentChildren } from "preact";
import { t } from "../../lib/i18n";
import { closeModal, modal } from "../../state/ui";
import { TaskForm } from "./TaskForm";
import { ProjectPicker } from "./ProjectPicker";
import { EventForm } from "./EventForm";
import { CommandPalette } from "./CommandPalette";
import { Trash } from "./Trash";
import { Help } from "./Help";
import { Settings } from "./Settings";

export function ModalHost() {
  const m = modal.value;
  if (!m) return null;
  switch (m.kind) {
    case "taskForm":
      return <TaskForm task={m.task} prefill={m.prefill} />;
    case "eventForm":
      return <EventForm event={m.event} date={m.date} end={m.end} />;
    case "projectPicker":
      return <ProjectPicker taskUids={m.taskUids} />;
    case "palette":
      return <CommandPalette />;
    case "trash":
      return <Trash />;
    case "help":
      return <Help />;
    case "settings":
      return <Settings />;
    case "confirm":
      return <Confirm message={m.message} onConfirm={m.onConfirm} />;
    default:
      return null;
  }
}

/** Small OK/Cancel confirmation dialog (opened via `openModal({ kind: "confirm", … })`). */
function Confirm({ message, onConfirm }: { message: string; onConfirm: () => void }) {
  const ok = () => {
    const cur = modal.value;
    onConfirm();
    // Close unless the confirm action already navigated to another modal (e.g. back to Trash).
    if (modal.value === cur) closeModal();
  };
  return (
    <Overlay>
      <p style={{ margin: "4px 0 12px" }}>{message}</p>
      <div class="actions">
        <button type="button" onClick={closeModal}>{t("Cancel")}</button>
        <button class="primary" type="button" onClick={ok}>{t("OK")}</button>
      </div>
    </Overlay>
  );
}

/** Shared modal chrome: centered card over a dismissable backdrop. */
export function Overlay({ children, onClose }: { children: ComponentChildren; onClose?: () => void }) {
  const close = onClose ?? closeModal;
  return (
    <div class="overlay" onClick={close}>
      <div class="modal" onClick={(e) => e.stopPropagation()}>
        {children}
      </div>
    </div>
  );
}
