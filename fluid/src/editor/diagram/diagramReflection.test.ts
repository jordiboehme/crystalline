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

async function drawn() {
  kit.settled.set(BODY, OK);
  const mode = cellMarksMode();
  const view = markView(DOC, {
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

  it("changes nothing but one line break on Backspace on the blank line after it", async () => {
    const view = await drawn();
    const below = HEAD.length + fence(BODY).length + 1;
    setCaret(view, below);
    keys(view, "Backspace");
    const text = view.state.doc.toString();
    expect(text.startsWith(`${HEAD}${fence(BODY)}`)).toBe(true);
    expect(text.endsWith(REST)).toBe(true);
    expect(hidesIdLines(view.state)).toBe(true);
  });
});
