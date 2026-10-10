// SSE decoding for `POST /api/chat` (EventSource can't POST, so the response body is read by hand
// — but the *framing* is `eventsource-parser`'s job, not ours).
//
// Why the library: the separator after a field name is part of the wire format, `data: hello`
// carries "hello" and `data:  hello` carries " hello". A hand-rolled `.trim()` ate the token's own
// leading space, so streamed words ran together ("movedstandupto10:00"). The parser also handles
// CRLF, BOM, multi-line data, and keep-alive comments.

import { createParser, type EventSourceParser } from "eventsource-parser";

export interface SseFrame {
  event: string;
  data: string;
}

/** A push decoder: feed response-body chunks, get whole SSE events back. */
export function sseDecoder(onFrame: (f: SseFrame) => void): EventSourceParser {
  return createParser({ onEvent: (e) => onFrame({ event: e.event ?? "message", data: e.data }) });
}

/** Decode a complete SSE text blob in one shot (tests, and any non-streamed body). */
export function decodeSse(text: string): SseFrame[] {
  const out: SseFrame[] = [];
  const parser = sseDecoder((f) => out.push(f));
  parser.feed(text);
  return out;
}
