// Assistant text → HTML. `marked` for GFM (lists, code, tables, autolinked URLs), DOMPurify for
// the sanitize step, because chat text is model output over the user's own vault data — event
// titles and descriptions arrive from ICS subscriptions and other people's shared calendars.

import DOMPurify from "dompurify";
import { marked } from "marked";

// `breaks` matters here: a model's single newlines are intentional line breaks, not soft wraps.
marked.setOptions({ gfm: true, breaks: true });

DOMPurify.addHook("afterSanitizeAttributes", (node) => {
  if ("target" in node) {
    node.setAttribute("target", "_blank");
    node.setAttribute("rel", "noopener noreferrer");
  }
});

export function renderMarkdown(src: string): string {
  return DOMPurify.sanitize(marked.parse(src, { async: false }) as string, { ADD_ATTR: ["target", "rel"] });
}
