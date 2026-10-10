# Chat UI: which libraries we took, and which we refused

Status: accepted, 2026-10-10. Scope: `web/` (the PWA's Chat view) and `crates/mgmt-web/src/chat/`.

**Stack correction up front:** `web/` is **Preact** 10 + Vite, not React. That is not a detail — most
"AI chat UI" packages are React-only or ship their own React runtime, which is why the survey below
ends where it does.

## What the review was answering

Five UX gaps in the Chat tab: streamed text arriving without spaces, no feedback that anything is
happening, tool results that name an item but can't open it, no way to clear a conversation, and no
in-app view of which model/endpoint is configured. The question asked of every one: **does a library
already do this better than we would?**

## Adopted

| Library | Version | Why it beat our own code |
|---|---|---|
| `eventsource-parser` | ^4.1.1 | The whitespace bug *was* a hand-rolled SSE parser. Our `line.slice(5).trim()` stripped the `data:` field separator and the token's own leading space with it, so streamed words ran together. This package implements the spec we got wrong — one-leading-space, CRLF, BOM, multi-line `data:`, comment frames — in zero dependencies, and is the parser the Vercel AI SDK uses. `web/src/lib/sse.ts` is now a ~10-line adapter over it. |
| `marked` | ^18.1.0 | GFM for assistant text: lists, code blocks, tables, and autolinked URLs. Replaced a planned hand-written URL linkifier, which would have been a worse regex than theirs. |
| `dompurify` | ^3.4.16 | Mandatory, not optional: chat text quotes the user's own vault, which carries ICS-subscription and shared-calendar text written by other people. Rendering markdown means producing HTML, and that HTML must be sanitized. Pairs with the `confirm`-before-delete gate described in `llm.md`. |

Adoption was decided on measured weekly downloads from the npm registry, not recollection:
`marked` 83.8M, `dompurify` 70.4M, `eventsource-parser` 84.8M.

### The cost, measured and then paid only where used

An esbuild probe importing just those three put them at **78.6 kB raw / 26.7 kB gzip**, and the vite
shell went 52.38 → 79.08 kB gzip — a ~51% increase on an offline-first PWA, for a feature that is
**off by default**. So `/chat` became a lazy route (`lazy(() => import("./pages/Chat")…)` in
`main.tsx`): the shell is back to 52.38 kB gzip and the Chat chunk (27.19 kB gzip) downloads on first
visit. An install with `llm.enabled: false` never fetches any of it.

## Rejected

- **A Preact/React chat-UI component library.** Surveyed `agentak`, `ai-ui`, `streamkit`,
  `derin-chat-ui`, `@makemore/agent-frontend`. All are either React-only or embeddable-widget SDKs
  (script tag, Shadow DOM, their own theming), at 15k–130k downloads/week. Each one *replaces* a chat
  view rather than supplying parts for one — and ours already exists, in three dependencies' worth of
  Preact, styled by `styles.css` tokens shared with the calendar. Adopting one means re-theming a
  foreign shell and giving up the tool-line/confirm-card entries that are specific to mgmt's
  server-side tool loop. The two libraries we did take are the ones that solve genuinely hard,
  standardized problems; bubbles in a flex column are not that.
- **`snarkdown` (129k/wk) and `streaming-markdown` (15k/wk).** Against marked's 83.8M. The
  incremental-DOM pitch of `streaming-markdown` buys nothing here: bubbles re-render from the
  accumulated string on each token anyway.
- **ACP (Agent Client Protocol).** Re-examined and still the wrong shape. Its transports are stdio
  (primary) plus a *draft* Streamable HTTP RFD, and it standardizes an editor driving a coding
  agent's session lifecycle. mgmt's agent already runs in-process over the caller's vault and the
  client is a browser; ACP would add a protocol hop and buy no UX. Where ACP or MCP *would* matter is
  the inverse direction — exposing mgmt's tools so pi/Zed/Claude can schedule — which
  `openspec/changes/llm-chat/proposal.md` already defers as a second surface.
- **A markdown dependency at all (the original plan).** Briefly rejected on bundle grounds, then
  reversed once the route was lazy-loaded and the real complaint turned out to be formatting, not
  only whitespace. Recorded here because the reversal is the interesting part: measure, then decide
  where the weight lands, rather than refusing the library outright.

## Known reinvention we did *not* remove

`crates/mgmt-web/src/chat/mod.rs` hand-assembles streaming tool-call deltas (`stream_completion`,
`PendingCall`). The `async-openai` crate does exactly that — SSE streaming, tool calls, custom
`api_base`, builder types. It was left alone deliberately: it would rewrite a working, tested loop in
a change whose subject was the UI. It is the next candidate for deletion, not an endorsement of the
current code.

## Where each of the five pieces lives

| Piece | Code |
|---|---|
| Spacing (the actual bug) | `web/src/lib/sse.ts` (`sseDecoder`), consumed by `streamChat` in `web/src/api.ts`; regression test `web/tests/sse.spec.ts` |
| Markdown rendering | `web/src/lib/md.ts` (`renderMarkdown`), applied to assistant bubbles in `web/src/pages/Chat.tsx`; `.bubble.md` rules in `web/src/styles.css` |
| Health indicator | `GET /api/chat/health` in `crates/mgmt-web/src/chat/mod.rs`; header dot + re-probe in `web/src/pages/Chat.tsx`; thinking row and Stop in the same file |
| Clickable item references | `send_tool` forwards the result's `uid` (`chat/mod.rs`); `openItem` opens the task/event form via `openModal` (`Chat.tsx`) |
| Clear chat | `clear()` in `web/src/pages/Chat.tsx` — drops transcript, upstream history, and any pending confirm |
| LLM settings in the main config view | `LlmSection` in `web/src/components/modals/Settings.tsx` (model, endpoint, Check connection), fed by `GET /api/chat/config` which now also reports `base_url` |

Behaviour and security notes for all of the above: `docs/llm.md`. Requirement scenarios:
`openspec/changes/web-chat-ux/specs/llm-chat/spec.md`.
