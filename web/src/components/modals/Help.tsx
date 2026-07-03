import { BINDINGS } from "../../lib/keymap";
import { closeModal } from "../../state/ui";
import { Overlay } from "./ModalHost";

export function Help() {
  return (
    <Overlay>
      <h2>Keyboard shortcuts</h2>
      <div class="help-list">
        {BINDINGS.map((b) => (
          <div class="help-row">
            <kbd>{b.keys}</kbd>
            <span>{b.label}</span>
          </div>
        ))}
      </div>
      <div class="muted" style={{ padding: "6px 0 0", textAlign: "left" }}>
        Drag events to reschedule; drag their edges to resize. Long-press a card to multi-select.
      </div>
      <div class="actions">
        <button onClick={closeModal}>Close</button>
      </div>
    </Overlay>
  );
}
