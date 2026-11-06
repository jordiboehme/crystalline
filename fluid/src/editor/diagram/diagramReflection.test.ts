/**
 * A drawn flowchart on a Reflection page, right below a section heading and its
 * hidden id line: the diagram layer leaves what the page reads section by section
 * alone. Built the way the Reflection editor builds its buffer.
 */

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

// R73: today's bytes, wrong on purpose. Task 10 binds these keys, on this branch
// and before the PR, and flips each of these.
describe("Backspace on the line right after a drawn fence, until Task 10", () => {
  const ID_NEXT = "## Next\n<!-- r:0b1c2d3e -->\n\nMore.\n";
  const mode = () => (cellMarksMode() === "show" ? "show" : "hidden");

  async function press(doc: string, at: string) {
    const view = await drawn(doc);
    setCaret(view, doc.indexOf(at));
    keys(view, "Backspace");
    return view;
  }

  it("a blank line goes, and one undo gives it back", async () => {
    const view = await press(DOC, "\nDeploys");
    expect(view.state.doc.toString()).toBe(
      DOC.replace("\n\nDeploys", "\nDeploys"),
    );
    expect(hidesIdLines(view.state)).toBe(true);
    undoOnce(view);
    expect(view.state.doc.toString()).toBe(DOC);
  });

  it("a section heading: joins in show mode, nothing in hidden mode", async () => {
    const doc = `${HEAD}${fence(BODY)}\n${ID_NEXT}`;
    const view = await press(doc, "## Next");
    expect(view.state.doc.toString()).toBe(
      mode() === "show" ? `${HEAD}${fence(BODY)}${ID_NEXT}` : doc,
    );
    expect(hidesIdLines(view.state)).toBe(true);
    if (mode() === "show") {
      undoOnce(view);
    }
    expect(view.state.doc.toString()).toBe(doc);
  });

  it("text: joins the closing mark, and one undo gives the bytes back", async () => {
    const doc = `${HEAD}${fence(BODY)}\nBelow\n`;
    const view = await press(doc, "Below");
    expect(view.state.doc.toString()).toBe(`${HEAD}${fence(BODY)}Below\n`);
    undoOnce(view);
    expect(view.state.doc.toString()).toBe(doc);
  });
});
