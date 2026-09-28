// Renders the currently-open modal, plus the scope prompt on top of it. Grows as modals land
// (TaskForm in M2; EventForm/palette/trash/help in M6). A shared overlay wrapper handles the
// backdrop + Escape-to-close.

import type { ComponentChildren } from "preact";
import { useEffect, useRef, useState } from "preact/hooks";
import type { OccurrenceScope } from "../../api";
import { showToast } from "../../lib/cache";
import { t } from "../../lib/i18n";
import { closeModal, closeScope, modal, scopeAsk } from "../../state/ui";
import { TaskForm } from "./TaskForm";
import { ProjectPicker } from "./ProjectPicker";
import { EventForm } from "./EventForm";
import { CommandPalette } from "./CommandPalette";
import { Trash } from "./Trash";
import { Help } from "./Help";
import { Settings } from "./Settings";

export function ModalHost() {
  const ask = scopeAsk.value;
  return (
    <>
      {/* `inert` while the prompt is up: the form behind the scrim keeps enabled controls outside its
          fieldset (Delete/Duplicate/Cancel), and tabbing into Delete would swap the pending save
          prompt for a delete prompt — the next pick would then delete what the user meant to save. */}
      <div inert={ask ? true : undefined} aria-hidden={ask ? "true" : undefined}>
        <CurrentModal />
      </div>
      {ask && <ScopePrompt message={ask.message} onPick={ask.onPick} />}
    </>
  );
}

function CurrentModal() {
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

/** Escape/backdrop/Cancel abandon the pick without saving or deleting. Only the prompt closes, so
 *  whatever modal is behind it (the event form) keeps the user's edits, and every scope prompt —
 *  form save, form delete, drag-reschedule — reports the dismissal. */
export function dismissScope() {
  const ask = scopeAsk.value;
  closeScope();
  ask?.onDismiss?.();
  showToast(ask?.dismissMessage ?? t("not saved"));
}

/** Three-way scope chooser for a recurring occurrence (opened via `askScope(…)`). It is layered over
 *  the modal that opened it, so it takes focus on mount and hands it back on close — otherwise a
 *  keyboard/screen-reader user would have to tab through the whole form behind the scrim. */
function ScopePrompt({ message, onPick }: { message: string; onPick: (scope: OccurrenceScope) => void }) {
  const primary = useRef<HTMLButtonElement>(null);
  useEffect(() => {
    // Not `document.activeElement`: the opener disables its fieldset in the same flush that mounts
    // this prompt, so focus has already fallen back to <body> by now. The opener restores its own
    // focus from `onDismiss` instead.
    primary.current?.focus();
  }, []);
  const pick = (scope: OccurrenceScope) => {
    closeScope();
    onPick(scope);
  };
  return (
    <Overlay onClose={dismissScope} labelledBy="scope-prompt-message">
      <p id="scope-prompt-message" style={{ margin: "4px 0 12px" }}>{message}</p>
      <div class="actions" style={{ flexWrap: "wrap" }}>
        <button type="button" onClick={dismissScope}>{t("Cancel")}</button>
        <button type="button" onClick={() => pick("this")}>{t("This event")}</button>
        <button type="button" onClick={() => pick("following")}>{t("This and following")}</button>
        <button ref={primary} class="primary" type="button" onClick={() => pick("all")}>{t("All events")}</button>
      </div>
    </Overlay>
  );
}

const FOCUSABLE =
  'a[href],button,input,select,textarea,[tabindex]:not([tabindex="-1"])';
let titleSeq = 0;

/** Shared modal chrome: centered card over a dismissable backdrop. */
export function Overlay(
  { children, onClose, labelledBy }: { children: ComponentChildren; onClose?: () => void; labelledBy?: string },
) {
  const close = onClose ?? closeModal;
  const card = useRef<HTMLDivElement>(null);
  const [titleId, setTitleId] = useState<string>();
  useEffect(() => {
    const el = card.current;
    if (!el) return;
    // Name the dialog from its own <h2>; every modal that has a heading gets one for free.
    const h = el.querySelector("h2");
    if (h) {
      if (!h.id) h.id = `modal-title-${++titleSeq}`;
      setTitleId(h.id);
    }
    // Nothing inside autofocused (Settings, Help, Trash, Confirm): put focus on the card so the
    // keyboard user starts inside the dialog instead of behind the scrim.
    if (!el.contains(document.activeElement)) el.focus();
  }, []);
  /** Wrap Tab inside the card — without it focus walks into the page behind the scrim. */
  const onKeyDown = (e: KeyboardEvent) => {
    const el = card.current;
    if (e.key !== "Tab" || !el) return;
    const items = [...el.querySelectorAll<HTMLElement>(FOCUSABLE)]
      .filter((n) => !n.matches(":disabled") && n.offsetParent !== null);
    if (items.length === 0) return;
    const first = items[0];
    const last = items[items.length - 1];
    const active = document.activeElement;
    if (e.shiftKey && (active === first || active === el)) {
      e.preventDefault();
      last.focus();
    } else if (!e.shiftKey && active === last) {
      e.preventDefault();
      first.focus();
    }
  };
  return (
    <div class="overlay" onClick={close}>
      <div
        class="modal"
        ref={card}
        role="dialog"
        aria-modal="true"
        aria-labelledby={labelledBy ?? titleId}
        tabIndex={-1}
        onClick={(e) => e.stopPropagation()}
        onKeyDown={onKeyDown}
      >
        {children}
      </div>
    </div>
  );
}
