// Chat: a message list fed by the server's tool-calling loop over SSE (api.ts's `streamChat`,
// since EventSource can't POST). The LLM-facing conversation (role/content only) lives in a ref
// so it survives renders without becoming the render model itself; `entries` is the render model
// (user/assistant bubbles, tool lines, confirm cards, errors) and can hold things — tool runs,
// confirm prompts — that are never sent back upstream as "messages".
import { useEffect, useRef, useState } from "preact/hooks";
import { useLocation } from "preact-iso";
import { api, chatEnabled, streamChat, type ChatConfig, type ChatMessage } from "../api";
import { showToast } from "../lib/cache";
import { t } from "../lib/i18n";
import { renderMarkdown } from "../lib/md";
import { openModal } from "../state/ui";
import { doUndo } from "../app";

type Entry =
  | { kind: "user"; content: string }
  | { kind: "assistant"; content: string }
  | { kind: "tool"; name: string; summary: string; ok: boolean; undo: boolean; uid?: string; undone?: boolean }
  | { kind: "confirm"; id: string; name: string; summary: string; resolved?: "approved" | "discarded" }
  | { kind: "error"; content: string };

type Health = "checking" | "ok" | "down";

export function Chat() {
  const loc = useLocation();
  const [cfg, setCfg] = useState<ChatConfig | null>(null);
  const [entries, setEntries] = useState<Entry[]>([]);
  const [draft, setDraft] = useState("");
  const [streaming, setStreaming] = useState(false);
  // Distinct from `streaming`: the gap between Send and the first token/tool event is the one a
  // slow local model makes look like a dead UI.
  const [waiting, setWaiting] = useState(false);
  const [health, setHealth] = useState<Health>("checking");
  const history = useRef<ChatMessage[]>([]);
  const pending = useRef<{ id: string; hist: ChatMessage[] } | null>(null);
  const abort = useRef<AbortController | null>(null);
  const bottomRef = useRef<HTMLDivElement>(null);

  // The feature probe doubles as the route guard: chat has no UI at all while disabled, and a
  // direct hit on /chat bounces to the calendar instead of showing a dead composer.
  useEffect(() => {
    api
      .chatConfig()
      .then((c) => {
        chatEnabled.value = c.enabled;
        if (c.enabled) {
          setCfg(c);
          void probe();
        } else loc.route("/", true);
      })
      .catch(() => {
        chatEnabled.value = false;
        loc.route("/", true);
      });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  useEffect(() => {
    bottomRef.current?.scrollIntoView({ block: "end" });
  }, [entries, waiting]);

  async function probe() {
    setHealth("checking");
    try {
      setHealth((await api.chatHealth()).ok ? "ok" : "down");
    } catch {
      setHealth("down");
    }
  }

  async function run(hist: ChatMessage[], approve: string | null) {
    const ctl = new AbortController();
    abort.current = ctl;
    setStreaming(true);
    setWaiting(true);
    let text = "";
    let idx = -1;
    try {
      await streamChat(
        hist,
        approve,
        (ev) => {
          if (ev.event === "token") {
            setWaiting(false);
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
            setWaiting(false);
            const p = JSON.parse(ev.data) as Omit<Entry & { kind: "tool" }, "kind">;
            setEntries((prev) => [...prev, { kind: "tool", ...p }]);
            idx = -1;
            text = "";
          } else if (ev.event === "confirm") {
            setWaiting(false);
            const p = JSON.parse(ev.data) as { id: string; name: string; summary: string };
            pending.current = { id: p.id, hist };
            setEntries((prev) => [...prev, { kind: "confirm", ...p }]);
          } else if (ev.event === "done") {
            history.current = [...hist, { role: "assistant", content: text }];
          } else if (ev.event === "error") {
            setEntries((prev) => [...prev, { kind: "error", content: ev.data }]);
            showToast(ev.data);
          }
        },
        ctl.signal,
      );
    } catch (err) {
      // Stop is a user action, not a failure: keep what already streamed and say nothing.
      if (ctl.signal.aborted) {
        if (text) history.current = [...hist, { role: "assistant", content: text }];
        return;
      }
      const msg = err instanceof Error ? err.message : t("request failed");
      showToast(msg);
      void probe();
      if (approve === null) {
        // The request never reached the server (or was rejected outright): drop the optimistic
        // turn and hand the text back so the user can retry without retyping it.
        const sent = hist[hist.length - 1];
        history.current = hist.slice(0, -1);
        setEntries((prev) => prev.slice(0, -1));
        if (sent) setDraft(sent.content);
      }
    } finally {
      abort.current = null;
      setStreaming(false);
      setWaiting(false);
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

  function clear() {
    abort.current?.abort();
    pending.current = null;
    history.current = [];
    setEntries([]);
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

  /** Open what a tool touched. The tool name carries the kind; its result carried the uid. */
  async function openItem(name: string, uid: string) {
    try {
      if (name.endsWith("_task")) openModal({ kind: "taskForm", task: await api.task(uid) });
      else openModal({ kind: "eventForm", event: await api.event(uid) });
    } catch {
      showToast(t("item is no longer there"));
    }
  }

  if (!cfg) return null;

  return (
    <div class="chat">
      <div class="chat-head">
        <button class={`health ${health}`} title={t("Check the model endpoint")} onClick={() => void probe()}>
          <span class="dot" />
          {cfg.model || t("model")}
        </button>
        <button class="link" disabled={!entries.length && !streaming} onClick={clear}>{t("Clear")}</button>
      </div>
      <div class="thread" aria-live="polite">
        {entries.length === 0 && !waiting && (
          <div class="muted">{t("Ask me to schedule, list, or change something.")}</div>
        )}
        {entries.map((e, i) => {
          if (e.kind === "user") return <div class="bubble user" key={i}>{e.content}</div>;
          if (e.kind === "assistant")
            return <div class="bubble assistant md" key={i} dangerouslySetInnerHTML={{ __html: renderMarkdown(e.content) }} />;
          if (e.kind === "tool")
            return (
              <div class={`tool-line ${e.ok ? "" : "fail"}`} key={i}>
                {e.uid ? (
                  <button class="goto" onClick={() => void openItem(e.name, e.uid!)} title={t("Open")}>
                    {e.name} → {e.summary}
                  </button>
                ) : (
                  <span>{e.name} → {e.summary}</span>
                )}
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
        {waiting && (
          <div class="thinking" aria-label={t("Thinking…")}>
            <span /><span /><span />
          </div>
        )}
        <div ref={bottomRef} />
      </div>
      <div class="composer">
        <textarea
          rows={1}
          placeholder={t("Message mgmt…")}
          disabled={streaming}
          value={draft}
          onInput={(ev) => setDraft((ev.target as HTMLTextAreaElement).value)}
          onKeyDown={(ev) => {
            if (ev.key === "Enter" && !ev.shiftKey) {
              ev.preventDefault();
              send();
            }
          }}
        />
        {streaming ? (
          <button onClick={() => abort.current?.abort()}>{t("Stop")}</button>
        ) : (
          <button class="primary" disabled={!draft.trim()} onClick={send}>{t("Send")}</button>
        )}
      </div>
    </div>
  );
}
