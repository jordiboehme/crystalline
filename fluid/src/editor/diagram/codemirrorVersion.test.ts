// The pin on the CodeMirror view whose measure pass the drawn diagram's scroll
// step was written against.
//
// `widget.ts` keeps the text still when a drawing lands above the window, and
// it leans on how `@codemirror/view` behaves inside `measure()`, none of which
// is a promise CodeMirror makes. A version change has to re-check the list in
// `RECHECK` against the new `dist/index.js`. This test is what makes a bump stop
// and say so.

import { EditorView } from "@codemirror/view";
import { afterEach, describe, expect, test } from "vitest";

import { ScrollRequest } from "./widget";

const PINNED = "6.43.15";

const RECHECK = [
  `@codemirror/view is no longer ${PINNED}: re-check in its dist/index.js`,
  "1. scrollAnchorAt still looks 12 px under the scroll offset (ANCHOR_REACH_PX in widget.ts)",
  "2. in measure(), after the writes: the scroll target branch, then the anchor step with its gate (scrollDOM, focus, a wheel or touch under 100 ms)",
  "3. isScrolledToBottom still allows 4 px",
  "4. scrollIntoView and scrollSnapshot effects still carry a ScrollTarget value (ScrollRequest in widget.ts)",
].join("\n");

interface Fs {
  readFileSync: (path: string, encoding: "utf8") => string;
}

/** The installed view's version, read off disk (no Node types here). */
function installedView(): string {
  const node = globalThis as unknown as {
    process: { getBuiltinModule(id: string): unknown; cwd(): string };
  };
  const { readFileSync } = node.process.getBuiltinModule("node:fs") as Fs;
  const pkg = readFileSync(
    `${node.process.cwd()}/node_modules/@codemirror/view/package.json`,
    "utf8",
  );
  return (JSON.parse(pkg) as { version: string }).version;
}

let view: EditorView | null = null;

afterEach(() => {
  view?.destroy();
  view = null;
});

describe("the CodeMirror view the scroll step was written against", () => {
  test("is the installed one", () => {
    expect(installedView(), RECHECK).toBe(PINNED);
  });

  test("still tells its scroll requests by their value's class", () => {
    // A plain object would make every effect with an object value a request.
    expect(ScrollRequest, RECHECK).not.toBe(Object);
    view = new EditorView({ doc: "text", parent: document.body });
    expect(view.scrollSnapshot().value, RECHECK).toBeInstanceOf(ScrollRequest);
    expect(EditorView.scrollIntoView(0).value, RECHECK).toBeInstanceOf(
      ScrollRequest,
    );
  });
});
