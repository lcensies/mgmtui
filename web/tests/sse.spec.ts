// Regression guard for the streamed-text bug: tokens arrive as `data: <token>` and the assistant's
// own spaces must survive decoding. The SSE spec itself is eventsource-parser's problem; this only
// pins mgmt's use of it (and the shape `streamChat` relies on).

import { test, expect } from "@playwright/test";
import { decodeSse, sseDecoder } from "../src/lib/sse";

test("a streamed sentence keeps its spaces", () => {
  const wire = ["event: token\ndata:  standup", "event: token\ndata:  to 10:00"].join("\n\n") + "\n\n";
  expect(decodeSse("event: token\ndata: moved\n\n" + wire).map((f) => f.data).join("")).toBe("moved standup to 10:00");
});

test("a token that is only a space stays a space", () => {
  expect(decodeSse("event: token\ndata:  \n\n")[0].data).toBe(" ");
});

test("event name, multi-line data, and keep-alive comments", () => {
  const frames = decodeSse(":ping\n\nevent: error\ndata: line one\ndata: line two\n\n");
  expect(frames).toHaveLength(1);
  expect(frames[0]).toEqual({ event: "error", data: "line one\nline two" });
});

test("frames survive arbitrary chunk boundaries", () => {
  const out: string[] = [];
  const parser = sseDecoder((f) => out.push(f.data));
  for (const chunk of ["event: tok", "en\ndata: he", "llo there\n", "\n"]) parser.feed(chunk);
  expect(out).toEqual(["hello there"]);
});
