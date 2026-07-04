// Renders the currently-open modal. Grows as modals land (TaskForm in M2; EventForm/palette/trash/
// help in M6). A shared overlay wrapper handles the backdrop + Escape-to-close.

import type { ComponentChildren } from "preact";
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
      return <TaskForm task={m.task} />;
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
    default:
      return null;
  }
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
