// Chat: a message list fed by the server's tool-calling loop over SSE (api.ts's `streamChat`,
// since EventSource can't POST). The LLM-facing conversation (role/content only) lives in a ref
// so it survives renders without becoming the render model itself; `entries` is the render model
// (user/assistant bubbles, tool lines, confirm cards, errors) and can hold things — tool runs,
// confirm prompts — that are never sent back upstream as "messages".
import { useEffect, useRef, useState } from "preact/hooks";
import { useLocation } from "preact-iso";
import { api, chatEnabled, streamChat, type ChatMessage } from "../api";
import { showToast } from "../lib/cache";
import { t } from "../lib/i18n";
import { doUndo } from "../app";

type Entry =
  | { kind: "user"; content: string }
  | { kind: "assistant"; content: string }
  | { kind: "tool"; name: string; summary: string; ok: boolean; undo: boolean; undone?: boolean }
  | { kind: "confirm"; id: string; name: string; summary: string; resolved?: "approved" | "discarded" }
  | { kind: "error"; content: string };

export function Chat() {
  const loc = useLocation();
  const [ready, setReady] = useState(false);
  const [entries, setEntries] = useState<Entry[]>([]);
  const [draft, setDraft] = useState("");
  const [streaming, setStreaming] = useState(false);
  const history = useRef<ChatMessage[]>([]);
  const pending = useRef<{ id: string; hist: ChatMessage[] } | null>(null);
  const bottomRef = useRef<HTMLDivElement>(null);

  // The feature probe doubles as the route guard: chat has no UI at all while disabled, and a
  // direct hit on /chat bounces to the calendar instead of showing a dead composer.
  useEffect(() => {
    api
      .chatConfig()
      .then((c) => {
        chatEnabled.value = c.enabled;
        if (c.enabled) setReady(true);
        else loc.route("/", true);
      })
      .catch(() => {
        chatEnabled.value = false;
        loc.route("/", true);
      });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  useEffect(() => {
    bottomRef.current?.scrollIntoView({ block: "end" });
  }, [entries]);

  async function run(hist: ChatMessage[], approve: string | null) {
    setStreaming(true);
    let text = "";
    let idx = -1;
    try {
      await streamChat(hist, approve, (ev) => {
        if (ev.event === "token") {
          text += ev.data;
          setEntries((prev) => {
            const next = prev.slice();
            if (idx === -1) {
              next.push({ kind: "assistant", content: text });
              idx = next.length - 1;
            } else {
              next[idx] = { kind: "assistant", content: text };
            }
            return next;
          });
        } else if (ev.event === "tool") {
          const p = JSON.parse(ev.data) as { name: string; summary: string; ok: boolean; undo: boolean };
          setEntries((prev) => [...prev, { kind: "tool", ...p }]);
          idx = -1;
          text = "";
        } else if (ev.event === "confirm") {
          const p = JSON.parse(ev.data) as { id: string; name: string; summary: string };
          pending.current = { id: p.id, hist };
          setEntries((prev) => [...prev, { kind: "confirm", ...p }]);
        } else if (ev.event === "done") {
          history.current = [...hist, { role: "assistant", content: text }];
        } else if (ev.event === "error") {
          setEntries((prev) => [...prev, { kind: "error", content: ev.data }]);
          showToast(ev.data);
        }
      });
    } catch (err) {
      const msg = err instanceof Error ? err.message : t("request failed");
      showToast(msg);
      if (approve === null) {
        // The request never reached the server (or was rejected outright): drop the optimistic
        // turn and hand the text back so the user can retry without retyping it.
        const sent = hist[hist.length - 1];
        history.current = hist.slice(0, -1);
        setEntries((prev) => prev.slice(0, -1));
        if (sent) setDraft(sent.content);
      }
    } finally {
      setStreaming(false);
    }
  }

  function send() {
    const text = draft.trim();
    if (!text || streaming) return;
    const hist = [...history.current, { role: "user" as const, content: text }];
    history.current = hist;
    setEntries((prev) => [...prev, { kind: "user", content: text }]);
    setDraft("");
    void run(hist, null);
  }

  function approve(id: string) {
    const pc = pending.current;
    if (!pc || pc.id !== id) return;
    pending.current = null;
    setEntries((prev) => prev.map((e) => (e.kind === "confirm" && e.id === id ? { ...e, resolved: "approved" } : e)));
    void run(pc.hist, id);
  }

  function discard(id: string) {
    pending.current = null;
    setEntries((prev) => prev.map((e) => (e.kind === "confirm" && e.id === id ? { ...e, resolved: "discarded" } : e)));
  }

  async function undo(i: number) {
    setEntries((prev) => prev.map((e, j) => (j === i && e.kind === "tool" ? { ...e, undone: true } : e)));
    try {
      await doUndo();
    } catch {
      showToast(t("request failed"));
      setEntries((prev) => prev.map((e, j) => (j === i && e.kind === "tool" ? { ...e, undone: false } : e)));
    }
  }

  if (!ready) return null;

  return (
    <div class="chat">
      <div class="thread" aria-live="polite">
        {entries.length === 0 && <div class="muted">{t("Ask me to schedule, list, or change something.")}</div>}
        {entries.map((e, i) => {
          if (e.kind === "user") return <div class="bubble user" key={i}>{e.content}</div>;
          if (e.kind === "assistant") return <div class="bubble assistant" key={i}>{e.content}</div>;
          if (e.kind === "tool")
            return (
              <div class={`tool-line ${e.ok ? "" : "fail"}`} key={i}>
                <span>{e.name} → {e.summary}</span>
                {e.undo && !e.undone && <button onClick={() => undo(i)}>{t("Undo")}</button>}
                {e.undone && <span class="muted">{t("Undone")}</span>}
              </div>
            );
          if (e.kind === "confirm")
            return (
              <div class="confirm-card" key={i}>
                <div>{e.summary}</div>
                {!e.resolved && (
                  <div class="actions">
                    <button class="primary" onClick={() => approve(e.id)}>{t("Approve")}</button>
                    <button onClick={() => discard(e.id)}>{t("Discard")}</button>
                  </div>
                )}
                {e.resolved === "discarded" && <div class="muted">{t("Discarded")}</div>}
              </div>
            );
          return <div class="error-line" key={i}>{e.content}</div>;
        })}
        <div ref={bottomRef} />
      </div>
      <div class="composer">
        <textarea
          rows={1}
          placeholder={t("Message mgmt…")}
          value={draft}
          disabled={streaming}
          onInput={(ev) => setDraft((ev.target as HTMLTextAreaElement).value)}
          onKeyDown={(ev) => {
            if (ev.key === "Enter" && !ev.shiftKey) {
              ev.preventDefault();
              send();
            }
          }}
        />
        <button class="primary" disabled={streaming || !draft.trim()} onClick={send}>{t("Send")}</button>
      </div>
    </div>
  );
}
