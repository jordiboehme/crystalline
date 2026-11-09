/**
 * A drawn flowchart on a Reflection page, right below a section heading and its
 * hidden id line: the diagram layer leaves what the page reads section by section
 * alone. Built the way the Reflection editor builds its buffer.
 */

import type { EditorView } from "@codemirror/view";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { cellMarksMode } from "../../test/cellMarksMode";
import {
  cleanupMarkViews,
  keys,
  markView,
  setCaret,
  undoOnce,
} from "../../test/markKit";
import { hidesIdLines } from "../hiddenIdLines";
import { flowKind, runDiagramOp } from "./apply";
import { prefetchDiagrams } from "./loader";
import type { Verdict } from "./verdict";
import { forgetDrawings } from "./widget";

const kit = vi.hoisted(() => ({ settled: new Map<string, unknown>() }));

vi.mock("mermaid", () => ({
  default: {
    initialize: vi.fn(),
    render: (id: string) => Promise.resolve({ svg: `<svg id="${id}"></svg>` }),
  },
}));

vi.mock("./verdict", async (original) => ({
  ...(await original<typeof import("./verdict")>()),
  verdictFor: (body: string) => kit.settled.get(body),
  requestVerdict: () =>
    Promise.resolve({ ok: true, kind: "flowchart", domIds: new Map() }),
}));

const OK: Verdict = { ok: true, kind: "flowchart", domIds: new Map() };
const BODY = "flowchart TD\n    A[First step] --> B[Next step]";
const HEAD = "## Night deploys\n<!-- r:7f3a9c2e -->\n";
const REST =
  "\nDeploys run at night.\n\n## Next\n<!-- r:0b1c2d3e -->\n\nMore.\n";
const fence = (body: string) => `\`\`\`mermaid\n${body}\n\`\`\``;
const DOC = `${HEAD}${fence(BODY)}\n${REST}`;

async function drawn(doc = DOC) {
  kit.settled.set(BODY, OK);
  const mode = cellMarksMode();
  const view = markView(doc, {
    adapter: "reflection",
    ...(mode === "off" ? {} : { mode }),
  });
  await prefetchDiagrams();
  await vi.waitFor(() => {
    expect(view.dom.querySelector(".cm-diagram svg")).not.toBeNull();
  });
  return view;
}

beforeEach(() => {
  kit.settled.clear();
  forgetDrawings();
});

afterEach(() => {
  cleanupMarkViews();
});

describe("a Reflection", () => {
  it("still hides its id lines with the diagram drawn", async () => {
    const view = await drawn();
    expect(hidesIdLines(view.state)).toBe(true);
    expect(view.contentDOM.textContent).not.toContain("r:7f3a9c2e");
    expect(view.contentDOM.textContent).toContain("Night deploys");
    expect(view.state.doc.toString()).toBe(DOC);
  });

  it("leaves the id line byte for byte on an addChild", async () => {
    const view = await drawn();
    const outcome = await runDiagramOp(view, HEAD.length, flowKind, {
      op: "addChild",
      id: "B",
    });
    expect(outcome).toMatchObject({ applied: true });
    const text = view.state.doc.toString();
    expect(text.startsWith(HEAD)).toBe(true);
    expect(text.endsWith(`\n${REST}`)).toBe(true);
    expect(text).not.toBe(DOC);
    undoOnce(view);
    expect(view.state.doc.toString()).toBe(DOC);
  });
});

// R73: Backspace on the line right below a drawn fence never joins it to the
// closing mark. It takes the keyboard to the canvas and changes nothing.
describe("Backspace on the line right after a drawn fence", () => {
  const ID_NEXT = "## Next\n<!-- r:0b1c2d3e -->\n\nMore.\n";
  const mode = () => (cellMarksMode() === "show" ? "show" : "hidden");
  const box = (view: EditorView) => view.dom.querySelector(".cm-diagram");

  async function press(doc: string, at: string, key = "Backspace") {
    const view = await drawn(doc);
    setCaret(view, doc.indexOf(at));
    keys(view, key);
    return view;
  }

  it("a blank line stays, and the keyboard goes to the canvas", async () => {
    const view = await press(DOC, "\nDeploys");
    expect(view.state.doc.toString()).toBe(DOC);
    expect(hidesIdLines(view.state)).toBe(true);
    expect(document.activeElement).toBe(box(view));
  });

  it("a section heading: unchanged in both modes, and show mode goes to the canvas", async () => {
    const doc = `${HEAD}${fence(BODY)}\n${ID_NEXT}`;
    const view = await press(doc, "## Next");
    expect(view.state.doc.toString()).toBe(doc);
    expect(hidesIdLines(view.state)).toBe(true);
    // Hidden mode keeps its own answer: the caret stands past the heading's
    // marks, and the key that would change a section heading is refused there.
    if (mode() === "show") {
      expect(document.activeElement).toBe(box(view));
    }
  });

  it("text: stays on its own line, and the keyboard goes to the canvas", async () => {
    const doc = `${HEAD}${fence(BODY)}\nBelow\n`;
    const view = await press(doc, "Below");
    expect(view.state.doc.toString()).toBe(doc);
    expect(document.activeElement).toBe(box(view));
  });

  it("Mod-Backspace: the same", async () => {
    const doc = `${HEAD}${fence(BODY)}\nBelow\n`;
    const view = await press(doc, "Below", "Mod-Backspace");
    expect(view.state.doc.toString()).toBe(doc);
    expect(document.activeElement).toBe(box(view));
  });

  it.each(["Alt-ArrowUp"])(
    "%s with an id line in reach does not move a line into the fence",
    async (key) => {
      const doc = `${HEAD}${fence(BODY)}\n${ID_NEXT}`;
      const view = await press(doc, "## Next", key);
      expect(view.state.doc.toString()).toBe(doc);
    },
  );

  it("a second press on the canvas deletes the fence, and one undo gives every byte back", async () => {
    const doc = `${HEAD}${fence(BODY)}\nBelow\n`;
    const view = await press(doc, "Below");
    document.activeElement?.dispatchEvent(
      new KeyboardEvent("keydown", {
        key: "Backspace",
        bubbles: true,
        cancelable: true,
      }),
    );
    expect(view.state.doc.toString()).toBe(`${HEAD}Below\n`);
    undoOnce(view);
    expect(view.state.doc.toString()).toBe(doc);
  });
});
