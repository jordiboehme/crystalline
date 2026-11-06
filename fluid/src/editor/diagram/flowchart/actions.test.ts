/**
 * The menus and connect mode on the real surface and the real verdict, with only
 * mermaid's render stubbed. Each item is compared with what the op compiler makes of
 * the same body, so a menu can only be as wrong as the op tests allow.
 * `actions.marks-hidden.test.ts` and `actions.marks-show.test.ts` run this again.
 */

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { EditorState, StateEffect } from "@codemirror/state";
import type { EditorView } from "@codemirror/view";

import { cellMarksMode } from "../../../test/cellMarksMode";
import type { MarkViewOptions } from "../../../test/markKit";
import {
  bytes,
  cleanupMarkViews,
  markView,
  setRaw,
  undoOnce,
  undoSteps,
} from "../../../test/markKit";
import F04 from "../../fixtures/diagrams/flowchart/f04-chains.mmd?raw";
import { diagramAnchors } from "../../diagramAnchors";
import { mainSelectionParked } from "../../parkedSelection";
import { applyEdits, NOTICES } from "../edits";
import { focusTarget } from "../focus";
import { diagramField } from "../field";
import type { TargetKey } from "../intent";
import { diagramIntent } from "../intent";
import { prefetchDiagrams } from "../loader";
import { requestVerdict } from "../verdict";
import { forgetDrawings, NOTICE_MS } from "../widget";
import { flowMenu } from "./actions";
import { resolveArrow } from "./heldArrow";
import type { EdgeKey } from "./model";
import type { FlowOp } from "./ops";
import { compileFlowOp } from "./ops";
import { parseFlowchart } from "./parse";

vi.mock("mermaid", async (original) => {
  const real = (await original<{ default: object }>()).default;
  return {
    default: {
      ...real,
      render: async (id: string, body: string) => {
        const verdict = await requestVerdict(body, false);
        const nodes =
          verdict.ok === true
            ? [...verdict.domIds.keys()]
                .map((dom) => `<g class="node default" id="${id}-${dom}"></g>`)
                .join("")
            : "";
        const edges = parseFlowchart(body)
          .edges.map(
            ({ mermaidId }) =>
              `<path d="M0,0L1,1" data-et="edge" data-id="${mermaidId}"></path>`,
          )
          .join("");
        return {
          svg: `<svg id="${id}"><g class="edgePaths">${edges}</g><g class="nodes">${nodes}</g></svg>`,
        };
      },
    },
  };
});

const SKELETON = "flowchart TD\n    A[First step] --> B[Next step]";
const fence = (body: string) => `\`\`\`mermaid\n${body}\n\`\`\``;
const page = (body: string) => `Above\n${fence(body)}\nBelow\n`;
const TWO_FENCES = `Above\n${fence(SKELETON)}\n\nMiddle\n\n${fence("flowchart TD\n    C --> D")}\n`;
const AB: EdgeKey = { start: "A", end: "B", occurrence: 0 };
const NODE_A: TargetKey = { kind: "node", id: "A" };
const NODE_B: TargetKey = { kind: "node", id: "B" };
const EDGE_AB: TargetKey = { kind: "edge", key: AB };
const CANVAS: TargetKey = { kind: "canvas" };

const flush = async () => {
  await new Promise((resolve) => setTimeout(resolve, 0));
};

function modeOptions(): MarkViewOptions {
  const mode = cellMarksMode();
  return mode === "off" ? {} : { mode };
}

async function drawn(doc = page(SKELETON), options: MarkViewOptions = {}) {
  const view = markView(doc, { ...modeOptions(), ...options });
  await prefetchDiagrams();
  await vi.waitFor(() => {
    expect(view.dom.querySelector(".cm-diagram svg")).not.toBeNull();
  });
  await flush();
  await flush();
  view.contentDOM.focus();
  return view;
}

const text = (view: EditorView) => bytes(view.state);
const active = () => document.activeElement as HTMLElement;
const fromOf = (view: EditorView) => text(view).indexOf("```");
const intentOf = (view: EditorView) => view.state.field(diagramIntent);
const menus = () => [...document.querySelectorAll('[role="menu"]')];
const noticeOf = (view: EditorView) =>
  view.dom.querySelector(".cm-diagram-notice")?.textContent ?? "";

function press(
  key: string,
  options: KeyboardEventInit = {},
  el: Element = active(),
): KeyboardEvent {
  const event = new KeyboardEvent("keydown", {
    key,
    bubbles: true,
    cancelable: true,
    ...options,
  });
  el.dispatchEvent(event);
  return event;
}

function openMenuOn(view: EditorView, target: TargetKey) {
  focusTarget(view, fromOf(view), target);
  press("F10", { shiftKey: true });
  const menu = menus()[0];
  if (menu === undefined) throw new Error("no menu");
  return menu;
}

function row(label: string): HTMLElement {
  const found = [...document.querySelectorAll('[role^="menuitem"]')].find(
    (el) => el.textContent === label,
  );
  if (found === undefined) throw new Error(`no item ${label}`);
  return found as HTMLElement;
}

function choose(...path: string[]) {
  for (const label of path) {
    row(label).click();
  }
}

const nodeOf = (view: EditorView, id: string) => {
  const found = view.dom.querySelector(`g.node[id*="flowchart-${id}-"]`);
  if (found === null) throw new Error(`no box ${id}`);
  return found;
};

const fieldOf = (view: EditorView) =>
  view.dom.querySelector<HTMLTextAreaElement>(".cm-diagram-overlay textarea");

const promptOf = (view: EditorView) =>
  view.dom.querySelector('.cm-diagram-overlay [role="status"]');

const click = (el: Element) =>
  el.dispatchEvent(new MouseEvent("click", { bubbles: true }));

function rightClick(el: Element): MouseEvent {
  const event = new MouseEvent("contextmenu", {
    bubbles: true,
    cancelable: true,
    clientX: 5,
    clientY: 5,
  });
  el.dispatchEvent(event);
  return event;
}

/** What the op compiler makes of `body`, on the page. */
function expected(body: string, op: FlowOp, doc = page): string {
  const result = compileFlowOp(parseFlowchart(body), op);
  if (!("edits" in result)) throw new Error(`refused ${result.refused}`);
  return doc(applyEdits(body, result.edits));
}

async function changed(view: EditorView, from: string) {
  await vi.waitFor(() => {
    expect(text(view)).not.toBe(from);
  });
}

beforeEach(() => {
  forgetDrawings();
});

afterEach(() => {
  cleanupMarkViews();
  document.body.replaceChildren();
});

describe("what opens a menu", () => {
  it("Shift+F10 on box B opens Box actions with the focus on Rename", async () => {
    const view = await drawn();
    focusTarget(view, fromOf(view), NODE_B);
    const event = press("F10", { shiftKey: true });
    expect(event.defaultPrevented).toBe(true);
    expect(menus()).toHaveLength(1);
    expect(menus()[0]?.getAttribute("aria-label")).toBe("Box actions");
    expect(active().textContent).toBe("Rename");
    expect(intentOf(view)?.mode).toBe("menu");
    expect(intentOf(view)?.target).toEqual(NODE_B);
  });

  it("the ContextMenu key opens it too", async () => {
    const view = await drawn();
    focusTarget(view, fromOf(view), NODE_B);
    press("ContextMenu");
    expect(menus()[0]?.getAttribute("aria-label")).toBe("Box actions");
  });

  it("a held Shift+F10 opens nothing", async () => {
    const view = await drawn();
    focusTarget(view, fromOf(view), NODE_B);
    press("F10", { shiftKey: true, repeat: true });
    expect(menus()).toHaveLength(0);
  });

  it("right click on a box, an arrow and the canvas", async () => {
    const view = await drawn();
    expect(rightClick(nodeOf(view, "B")).defaultPrevented).toBe(true);
    expect(menus()[0]?.getAttribute("aria-label")).toBe("Box actions");
    expect(intentOf(view)?.target).toEqual(NODE_B);
    press("Escape");
    const arrow = view.dom.querySelector('path[data-et="edge"]');
    rightClick(arrow as Element);
    expect(menus()[0]?.getAttribute("aria-label")).toBe("Arrow actions");
    press("Escape");
    rightClick(view.dom.querySelector(".cm-diagram svg") as Element);
    expect(menus()[0]?.getAttribute("aria-label")).toBe("Diagram actions");
    expect(menus()).toHaveLength(1);
  });

  it("a right click on the wide hit path of an arrow is the arrow", async () => {
    const view = await drawn();
    rightClick(view.dom.querySelector("[data-hit-for]") as Element);
    expect(menus()[0]?.getAttribute("aria-label")).toBe("Arrow actions");
  });

  it("a second right click leaves one menu", async () => {
    const view = await drawn();
    rightClick(nodeOf(view, "B"));
    document.body.dispatchEvent(new MouseEvent("mousedown", { bubbles: true }));
    rightClick(nodeOf(view, "A"));
    expect(menus()).toHaveLength(1);
    expect(intentOf(view)?.target).toEqual(NODE_A);
  });

  it("names the items of each menu, in order", async () => {
    const view = await drawn();
    const labels = (menu: Element) =>
      [...menu.querySelectorAll('[role^="menuitem"]')].map(
        (el) => el.textContent,
      );
    expect(labels(openMenuOn(view, NODE_B))).toEqual([
      "Rename",
      "Add child",
      "Add sibling",
      "Connect to...",
      "Shape",
      "Colour",
      "Delete",
    ]);
    press("Escape");
    expect(labels(openMenuOn(view, EDGE_AB))).toEqual([
      "Edit label",
      "Line",
      "Arrowhead",
      "Reverse",
      "Delete",
    ]);
    press("Escape");
    expect(labels(openMenuOn(view, CANVAS))).toEqual([
      "Add box",
      "Direction",
      "Delete diagram",
    ]);
  });

  it("offers the fourteen shapes and the seven colours", async () => {
    const view = await drawn();
    openMenuOn(view, NODE_B);
    choose("Shape");
    expect(
      [...(menus()[1]?.querySelectorAll('[role^="menuitem"]') ?? [])].map(
        (el) => el.textContent,
      ),
    ).toEqual([
      "Rectangle",
      "Rounded",
      "Stadium",
      "Subroutine",
      "Database",
      "Circle",
      "Double circle",
      "Flag",
      "Decision",
      "Hexagon",
      "Slanted right",
      "Slanted left",
      "Trapezoid",
      "Upside-down trapezoid",
    ]);
    choose("Colour");
    expect(
      [...(menus()[1]?.querySelectorAll('[role^="menuitem"]') ?? [])].map(
        (el) => el.textContent,
      ),
    ).toEqual([
      "No colour",
      "Grey",
      "Blue",
      "Green",
      "Yellow",
      "Red",
      "Purple",
    ]);
  });
});

describe("what is checked", () => {
  it("a box drawn with its own class has no colour checked", async () => {
    const view = await drawn(
      page("flowchart TD\n    classDef hot fill:#f00\n    A --> B:::hot"),
    );
    openMenuOn(view, NODE_B);
    choose("Colour");
    expect(menus()[1]?.querySelectorAll('[aria-checked="true"]')).toHaveLength(
      0,
    );
  });

  const checkedIn = () =>
    [...(menus()[1]?.querySelectorAll('[aria-checked="true"]') ?? [])].map(
      (el) => el.textContent,
    );

  it("has Rectangle checked for B[Next step], and for a bare box", async () => {
    const view = await drawn();
    openMenuOn(view, NODE_B);
    choose("Shape");
    expect(checkedIn()).toEqual(["Rectangle"]);
    press("Escape");
    press("Escape");
    cleanupMarkViews();
    const bare = await drawn(page("flowchart TD\n    A --> B"));
    openMenuOn(bare, NODE_B);
    choose("Shape");
    expect(checkedIn()).toEqual(["Rectangle"]);
  });

  it("has the shape as written", async () => {
    const view = await drawn(page("flowchart TD\n    A --> B{Why}"));
    openMenuOn(view, NODE_B);
    choose("Shape");
    expect(checkedIn()).toEqual(["Decision"]);
  });

  it.each([
    ["no style at all", "", ["No colour"]],
    [
      "a preset",
      "\n    style B fill:#1d4ed8,stroke:#1e3a8a,color:#ffffff",
      ["Blue"],
    ],
    ["another colour", "\n    style B fill:#123456", []],
    ["a class line", "\n    classDef hot fill:#f00\n    class B hot", []],
    ["a default class", "\n    classDef default fill:#f00", []],
    [
      "another box's",
      "\n    style A fill:#1d4ed8,stroke:#1e3a8a,color:#ffffff",
      ["No colour"],
    ],
  ])("colour with %s", async (_name, line, want) => {
    const view = await drawn(page(`${SKELETON}${line}`));
    openMenuOn(view, NODE_B);
    choose("Colour");
    expect(checkedIn()).toEqual(want);
  });

  it.each([
    ["flowchart TD", ["Top to bottom"]],
    ["flowchart TB", ["Top to bottom"]],
    ["flowchart", ["Top to bottom"]],
    ["graph LR", ["Left to right"]],
    ["flowchart RL", []],
    ["flowchart BT", []],
  ])("direction for %s", async (header, want) => {
    const view = await drawn(page(`${header}\n    A --> B`));
    openMenuOn(view, CANVAS);
    choose("Direction");
    expect(checkedIn()).toEqual(want);
  });

  it.each([
    ["A --> B", ["Solid"], ["Arrow"]],
    ["A -.-> B", ["Dotted"], ["Arrow"]],
    ["A ==> B", ["Thick"], ["Arrow"]],
    ["A --- B", ["Solid"], ["None"]],
    ["A <--> B", ["Solid"], ["Both ends"]],
    ["A --o B", ["Solid"], []],
  ])("line and head of %s", async (line, lines, heads) => {
    const view = await drawn(page(`flowchart TD\n    ${line}`));
    openMenuOn(view, EDGE_AB);
    choose("Line");
    expect(checkedIn()).toEqual(lines);
    choose("Arrowhead");
    expect(checkedIn()).toEqual(heads);
  });
});

describe("what each item writes", () => {
  const cases: [string, TargetKey, string[], FlowOp][] = [
    [
      "Shape",
      NODE_B,
      ["Shape", "Circle"],
      { op: "shape", id: "B", shape: "circle" },
    ],
    [
      "Shape again",
      NODE_A,
      ["Shape", "Decision"],
      { op: "shape", id: "A", shape: "diamond" },
    ],
    [
      "Colour",
      NODE_B,
      ["Colour", "Blue"],
      { op: "colour", id: "B", colour: "blue" },
    ],
    ["Delete a box", NODE_B, ["Delete"], { op: "deleteNode", id: "B" }],
    [
      "Line",
      EDGE_AB,
      ["Line", "Dotted"],
      { op: "lineStyle", edge: AB, line: "dotted" },
    ],
    [
      "Line thick",
      EDGE_AB,
      ["Line", "Thick"],
      { op: "lineStyle", edge: AB, line: "thick" },
    ],
    [
      "Head none",
      EDGE_AB,
      ["Arrowhead", "None"],
      { op: "arrowhead", edge: AB, head: "none" },
    ],
    [
      "Head both",
      EDGE_AB,
      ["Arrowhead", "Both ends"],
      { op: "arrowhead", edge: AB, head: "both" },
    ],
    ["Reverse", EDGE_AB, ["Reverse"], { op: "reverse", edge: AB }],
    ["Delete an arrow", EDGE_AB, ["Delete"], { op: "deleteEdge", edge: AB }],
    [
      "Direction",
      CANVAS,
      ["Direction", "Left to right"],
      { op: "direction", value: "LR" },
    ],
  ];

  it.each(cases)(
    "%s is the op's text and one undo step",
    async (_n, target, path, op) => {
      const view = await drawn();
      const before = text(view);
      const steps = undoSteps(view);
      openMenuOn(view, target);
      choose(...path);
      await changed(view, before);
      expect(text(view)).toBe(expected(SKELETON, op));
      expect(undoSteps(view)).toBe(steps + 1);
      undoOnce(view);
      expect(text(view)).toBe(before);
      expect(menus()).toHaveLength(0);
    },
  );

  it("No colour takes a preset off", async () => {
    const body = `${SKELETON}\n    style B fill:#1d4ed8,stroke:#1e3a8a,color:#ffffff`;
    const view = await drawn(page(body));
    openMenuOn(view, NODE_B);
    choose("Colour", "No colour");
    await changed(view, page(body));
    expect(text(view)).toBe(
      expected(body, { op: "colour", id: "B", colour: null }),
    );
  });

  it("Top to bottom on a page that reads left to right", async () => {
    const body = "flowchart LR\n    A --> B";
    const view = await drawn(page(body));
    openMenuOn(view, CANVAS);
    choose("Direction", "Top to bottom");
    await changed(view, page(body));
    expect(text(view)).toBe(expected(body, { op: "direction", value: "TD" }));
  });

  it.each<[string, TargetKey, string, FlowOp]>([
    ["Add child", NODE_A, "Add child", { op: "addChild", id: "A" }],
    ["Add sibling", NODE_B, "Add sibling", { op: "addSibling", id: "B" }],
    ["Add box", CANVAS, "Add box", { op: "addBox" }],
  ])(
    "%s writes the box and opens the field on its name",
    async (_n, target, label, op) => {
      const view = await drawn();
      openMenuOn(view, target);
      choose(label);
      await changed(view, page(SKELETON));
      expect(text(view)).toBe(expected(SKELETON, op));
      await vi.waitFor(() => {
        expect(fieldOf(view)).not.toBeNull();
      });
      expect(active()).toBe(fieldOf(view));
      press("Escape");
      await flush();
      undoOnce(view);
      expect(text(view)).toBe(page(SKELETON));
    },
  );

  it("Delete diagram takes the whole fence, and the focus goes to the text", async () => {
    const view = await drawn();
    openMenuOn(view, CANVAS);
    choose("Delete diagram");
    await flush();
    expect(text(view)).toBe("Above\nBelow\n");
    expect(active()).toBe(view.contentDOM);
    undoOnce(view);
    expect(text(view)).toBe(page(SKELETON));
  });

  it("Rename opens the field with its name selected", async () => {
    const view = await drawn();
    openMenuOn(view, NODE_B);
    press("Enter");
    const field = fieldOf(view);
    expect(field?.value).toBe("Next step");
    expect(active()).toBe(field);
    expect(field?.selectionStart).toBe(0);
    expect(field?.selectionEnd).toBe(9);
    expect(text(view)).toBe(page(SKELETON));
    expect(intentOf(view)?.mode).toBe("rename");
  });

  it("Edit label opens the arrow's field", async () => {
    const view = await drawn(page("flowchart TD\n    A -->|old| B"));
    openMenuOn(view, EDGE_AB);
    press("Enter");
    expect(fieldOf(view)?.value).toBe("old");
    expect(active()).toBe(fieldOf(view));
  });

  it("a shape already checked writes nothing and says nothing", async () => {
    const view = await drawn();
    openMenuOn(view, NODE_B);
    choose("Shape", "Rectangle");
    await flush();
    await flush();
    expect(text(view)).toBe(page(SKELETON));
    expect(noticeOf(view)).toBe("");
  });

  it("works on f04's chains, where only some arrows can reverse", async () => {
    const body = F04.trimEnd();
    const view = await drawn(page(body));
    openMenuOn(view, { kind: "edge", key: AB });
    const reverse = row("Reverse");
    expect(reverse.getAttribute("aria-disabled")).toBe("true");
    expect(reverse.getAttribute("aria-description")).toBe(NOTICES.chain);
    expect(row("Delete").hasAttribute("aria-disabled")).toBe(false);
    choose("Delete");
    await changed(view, page(body));
    expect(text(view)).toBe(expected(body, { op: "deleteEdge", edge: AB }));
    undoOnce(view);
    expect(text(view)).toBe(page(body));
  });

  it("in a CRLF document writes CRLF and undoes to its exact bytes", async () => {
    const crlf = (s: string) => s.replaceAll("\n", "\r\n");
    const doc = crlf(page(SKELETON));
    const view = await drawn(doc, { lineSeparator: "\r\n" });
    openMenuOn(view, NODE_A);
    choose("Connect to...");
    press("ArrowRight");
    press("Enter");
    await vi.waitFor(() => {
      expect(view.state.doc.toString()).toContain("A --> B");
    });
    const written = expected(SKELETON, { op: "connect", from: "A", to: "B" });
    expect(view.state.sliceDoc(0, view.state.doc.length)).toBe(crlf(written));
    undoOnce(view);
    expect(view.state.sliceDoc(0, view.state.doc.length)).toBe(doc);
  });
});

describe("what cannot be done", () => {
  const GROUP = "flowchart TD\n    A --> B & C";

  it("Reverse and Delete on a group arrow are disabled and say why", async () => {
    const view = await drawn(page(GROUP));
    openMenuOn(view, EDGE_AB);
    for (const label of ["Reverse", "Delete"]) {
      expect(row(label).getAttribute("aria-disabled")).toBe("true");
      expect(row(label).getAttribute("aria-description")).toBe(NOTICES.group);
    }
  });

  it("a disabled item runs nothing, the menu stays, and the eye gets the reason", async () => {
    const view = await drawn(page(GROUP));
    openMenuOn(view, EDGE_AB);
    choose("Delete");
    await flush();
    expect(text(view)).toBe(page(GROUP));
    expect(menus()).toHaveLength(1);
    expect(document.querySelector(".cm-diagram-menu-hint")?.textContent).toBe(
      NOTICES.group,
    );
  });

  it("a refused item shows its notice and changes nothing", async () => {
    const view = await drawn(page(GROUP));
    openMenuOn(view, EDGE_AB);
    choose("Line", "Dotted");
    await vi.waitFor(() => {
      expect(noticeOf(view)).toBe(NOTICES.group);
    });
    expect(text(view)).toBe(page(GROUP));
    expect(menus()).toHaveLength(0);
  });

  it("says a repeated refusal again to a screen reader", async () => {
    const view = await drawn(page(GROUP));
    for (let i = 0; i < 2; i += 1) {
      openMenuOn(view, EDGE_AB);
      choose("Line", "Dotted");
      await vi.waitFor(() => {
        expect(noticeOf(view)).toBe(NOTICES.group);
      });
    }
    expect(view.dom.querySelector(".cm-announced")?.textContent).toBe(
      NOTICES.group,
    );
  });

  it("the Edit label of a group arrow says why it opens nothing", async () => {
    const view = await drawn(page(GROUP));
    openMenuOn(view, EDGE_AB);
    choose("Edit label");
    expect(fieldOf(view)).toBeNull();
    expect(noticeOf(view)).toBe(NOTICES.group);
    expect(active().tagName.toLowerCase()).not.toBe("body");
  });

  it("an item while an IME composes refuses with its sentence", async () => {
    const view = await drawn();
    openMenuOn(view, NODE_B);
    vi.spyOn(view, "composing", "get").mockReturnValue(true);
    choose("Delete");
    await vi.waitFor(() => {
      expect(noticeOf(view)).toBe(NOTICES.stale);
    });
    expect(text(view)).toBe(page(SKELETON));
  });

  it("the notice goes after its time", async () => {
    const view = await drawn(page(GROUP));
    openMenuOn(view, EDGE_AB);
    const timers = vi.spyOn(globalThis, "setTimeout");
    choose("Line", "Dotted");
    await vi.waitFor(() => {
      expect(noticeOf(view)).toBe(NOTICES.group);
    });
    expect(timers.mock.calls.some(([, ms]) => ms === NOTICE_MS)).toBe(true);
    timers.mockRestore();
  });
});

describe("a person's own undo, solo", () => {
  it("keeps a stop on its arrow through Line, undo and redo", async () => {
    const view = await drawn();
    openMenuOn(view, EDGE_AB);
    choose("Line", "Dotted");
    await changed(view, page(SKELETON));
    await flush();
    undoOnce(view);
    await flush();
    expect(text(view)).toBe(page(SKELETON));
    expect(intentOf(view)?.target).toEqual(EDGE_AB);
    expect(view.dom.querySelector(".cm-diagram-refusal")).toBeNull();
    expect(noticeOf(view)).toBe("");
  });

  it("follows its arrow when an undo brings a twin back above it", async () => {
    const body = "flowchart TD\n    A -->|one| B\n    A -->|two| B";
    const view = await drawn(page(body));
    openMenuOn(view, EDGE_AB);
    choose("Delete");
    await changed(view, page(body));
    await flush();
    focusTarget(view, fromOf(view), EDGE_AB);
    await flush();
    await new Promise((resolve) => setTimeout(resolve, 20));
    undoOnce(view);
    await new Promise((resolve) => setTimeout(resolve, 20));
    expect(text(view)).toBe(page(body));
    expect(intentOf(view)?.target).toEqual({
      kind: "edge",
      key: { start: "A", end: "B", occurrence: 1 },
    });
  });
});

describe("resolveArrow with no ticket asked for", () => {
  const key = AB;
  const noTicket = () => ({ ticket: null, key, sole: true });

  it("lets a sole arrow answer, and a duplicate seen once ends that for good", async () => {
    const view = await drawn();
    const hold = noTicket();
    const one = parseFlowchart("flowchart TD\n    A -->|x| B");
    const two = parseFlowchart("flowchart TD\n    A -->|x| B\n    A -->|y| B");
    const none = parseFlowchart("flowchart TD\n    A --> C");
    expect(resolveArrow(view, hold, one, 0)).toEqual(key);
    expect(resolveArrow(view, hold, two, 0)).toBe("stale");
    expect(resolveArrow(view, hold, one, 0)).toBe("stale");
    expect(resolveArrow(view, hold, none, 0)).toBe("gone");
  });

  it("does not let an arrow answer once a ticket was asked for and lost", async () => {
    const view = await drawn();
    const one = parseFlowchart("flowchart TD\n    A -->|x| B");
    const hold = { ticket: 987_654, key, sole: true };
    expect(resolveArrow(view, hold, one, 0)).toBe("stale");
  });
});

describe("what an arrow's menu holds", () => {
  /** Beyond the one the stop itself holds, when the keyboard stands on an arrow. */
  const held = (view: EditorView) => {
    const size = view.state.field(diagramAnchors).size;
    return intentOf(view)?.mode === "focus" &&
      intentOf(view)?.target.kind === "edge"
      ? size - 1
      : size;
  };

  it("lets go when it closes unused", async () => {
    const view = await drawn();
    openMenuOn(view, EDGE_AB);
    expect(held(view)).toBe(1);
    press("Escape");
    await flush();
    expect(held(view)).toBe(0);
  });

  it("holds through its item's verify, and lets go after", async () => {
    const view = await drawn();
    openMenuOn(view, EDGE_AB);
    choose("Delete");
    expect(held(view)).toBeGreaterThan(0);
    await changed(view, page(SKELETON));
    await flush();
    expect(held(view)).toBe(0);
  });

  it("lets go after an item that leaves the arrow's token alone", async () => {
    const view = await drawn();
    openMenuOn(view, EDGE_AB);
    choose("Reverse");
    await changed(view, page(SKELETON));
    await flush();
    expect(held(view)).toBe(0);
  });

  it("lets go after a refused item", async () => {
    const view = await drawn(page("flowchart TD\n    A --> B & C"));
    openMenuOn(view, EDGE_AB);
    choose("Line", "Dotted");
    await vi.waitFor(() => {
      expect(noticeOf(view)).toBe(NOTICES.group);
    });
    expect(held(view)).toBe(0);
  });

  it("lets go when an item opens the field, which holds its own", async () => {
    const view = await drawn();
    openMenuOn(view, EDGE_AB);
    choose("Edit label");
    expect(fieldOf(view)).not.toBeNull();
    expect(held(view)).toBe(1);
    press("Escape");
    await flush();
    expect(held(view)).toBe(0);
  });
});

describe("the focus around a menu", () => {
  it("Escape goes back to the stop, in focus mode, and writes nothing", async () => {
    const view = await drawn();
    openMenuOn(view, NODE_B);
    press("Escape");
    expect(menus()).toHaveLength(0);
    expect(active()).toBe(nodeOf(view, "B"));
    expect(intentOf(view)?.mode).toBe("focus");
    expect(intentOf(view)?.target).toEqual(NODE_B);
    expect(text(view)).toBe(page(SKELETON));
  });

  it("a right click's menu gives the focus to the box it was on", async () => {
    const view = await drawn();
    rightClick(nodeOf(view, "A"));
    press("Escape");
    expect(active()).toBe(nodeOf(view, "A"));
  });

  it("a canvas menu goes back to the canvas", async () => {
    const view = await drawn();
    openMenuOn(view, CANVAS);
    press("Escape");
    expect(active()).toBe(view.dom.querySelector(".cm-diagram"));
  });

  it("Tab closes it on the stop, and the intent is not stuck in the menu", async () => {
    const view = await drawn();
    openMenuOn(view, NODE_B);
    press("Tab");
    expect(menus()).toHaveLength(0);
    expect(active()).toBe(nodeOf(view, "B"));
    expect(intentOf(view)?.mode).toBe("focus");
  });

  it("the focus leaving the diagram closes the menu and the intent", async () => {
    const view = await drawn();
    openMenuOn(view, NODE_B);
    const other = document.createElement("input");
    document.body.append(other);
    other.focus();
    expect(menus()).toHaveLength(0);
    expect(intentOf(view)).toBeNull();
    expect(view.state.facet(mainSelectionParked)).toBe(false);
    expect(active()).toBe(other);
    await flush();
    expect(view.dom.querySelector(".cm-diagram-focus")).toBeNull();
    expect(view.dom.querySelector(".cm-diagram-focus-ring")).toBeNull();
  });

  it("a press in the text closes the menu and the intent", async () => {
    const view = await drawn();
    openMenuOn(view, NODE_B);
    view.contentDOM
      .querySelector(".cm-line")
      ?.dispatchEvent(new MouseEvent("mousedown", { bubbles: true }));
    expect(menus()).toHaveLength(0);
    expect(intentOf(view)).toBeNull();
  });

  it("a press that only dismisses the menu opens no field under it", async () => {
    const view = await drawn();
    openMenuOn(view, NODE_B);
    const a = nodeOf(view, "A");
    a.dispatchEvent(new MouseEvent("mousedown", { bubbles: true }));
    a.dispatchEvent(new MouseEvent("mouseup", { bubbles: true }));
    click(a);
    expect(menus()).toHaveLength(0);
    expect(fieldOf(view)).toBeNull();
    await flush();
    // The next click is an ordinary one.
    click(nodeOf(view, "A"));
    expect(fieldOf(view)).not.toBeNull();
  });

  it("a dismissing press with no mouse up does not eat a later click", async () => {
    const view = await drawn();
    openMenuOn(view, NODE_B);
    const a = nodeOf(view, "A");
    a.dispatchEvent(new MouseEvent("mousedown", { bubbles: true }));
    await flush();
    expect(menus()).toHaveLength(0);
    // A native drag took the mouse up. The next press is a click of its own.
    a.dispatchEvent(new MouseEvent("mousedown", { bubbles: true }));
    a.dispatchEvent(new MouseEvent("mouseup", { bubbles: true }));
    click(a);
    expect(fieldOf(view)).not.toBeNull();
  });

  it("a held Enter that ran Delete takes nothing further", async () => {
    const view = await drawn(page("flowchart TD\n    A --> B --> C"));
    openMenuOn(view, NODE_B);
    press("End");
    press("Enter");
    press("Enter", { repeat: true });
    await vi.waitFor(() => {
      expect(text(view)).not.toContain(" B ");
    });
    press("Enter", { repeat: true });
    press("Delete", { repeat: true });
    await flush();
    expect(text(view)).toBe(
      expected("flowchart TD\n    A --> B --> C", {
        op: "deleteNode",
        id: "B",
      }),
    );
    expect(fieldOf(view)).toBeNull();
  });

  it("a held Enter that ran Delete diagram does not reach the text", async () => {
    const view = await drawn();
    openMenuOn(view, CANVAS);
    press("End");
    press("Enter");
    await flush();
    expect(active()).toBe(view.contentDOM);
    const repeat = press("Enter", { repeat: true });
    expect(repeat.defaultPrevented).toBe(true);
    expect(text(view)).toBe("Above\nBelow\n");
  });

  it("a held Escape stops at the stop, and does not leave the diagram", async () => {
    const view = await drawn();
    openMenuOn(view, NODE_B);
    press("Escape");
    press("Escape", { repeat: true });
    expect(active()).toBe(nodeOf(view, "B"));
    expect(intentOf(view)?.mode).toBe("focus");
  });

  it("the keys of a menu do not walk the drawing or delete a stop", async () => {
    const view = await drawn();
    openMenuOn(view, NODE_B);
    press("Delete");
    press("Backspace");
    await flush();
    expect(text(view)).toBe(page(SKELETON));
    expect(menus()).toHaveLength(1);
  });

  it("a box deleted through its menu gives the stop to its neighbour", async () => {
    const view = await drawn(page("flowchart TD\n    A --> B\n    A --> C"));
    openMenuOn(view, NODE_B);
    choose("Delete");
    await vi.waitFor(() => {
      expect(text(view)).not.toContain("--> B");
    });
    await vi.waitFor(() => {
      expect(view.dom.querySelector('g.node[id*="flowchart-B-"]')).toBeNull();
    });
    await flush();
    expect(active().tagName.toLowerCase()).not.toBe("body");
    expect(view.dom.contains(active())).toBe(true);
  });

  it("the selection stays parked while a menu is open, and the fence stays held", async () => {
    const view = await drawn();
    openMenuOn(view, NODE_B);
    expect(view.state.facet(mainSelectionParked)).toBe(true);
    expect(view.state.field(diagramField).kept).toBe(fromOf(view));
    press("Escape");
    expect(view.state.field(diagramField).kept).toBeNull();
  });
});

describe("a rename field and a menu", () => {
  it("writes the field's draft first, and then opens on the other box", async () => {
    const view = await drawn();
    click(nodeOf(view, "B"));
    const field = fieldOf(view);
    if (field === null) throw new Error("no field");
    field.value = "Done";
    field.dispatchEvent(new Event("input", { bubbles: true }));
    rightClick(nodeOf(view, "A"));
    await vi.waitFor(() => {
      expect(menus()).toHaveLength(1);
    });
    expect(text(view)).toContain("B[Done]");
    expect(fieldOf(view)).toBeNull();
    expect(menus()[0]?.getAttribute("aria-label")).toBe("Box actions");
    expect(intentOf(view)?.target).toEqual(NODE_A);
  });

  it("opens no menu over a draft that is refused", async () => {
    const view = await drawn();
    click(nodeOf(view, "B"));
    const field = fieldOf(view);
    if (field === null) throw new Error("no field");
    field.value = "%%{x";
    field.dispatchEvent(new Event("input", { bubbles: true }));
    rightClick(nodeOf(view, "A"));
    await vi.waitFor(() => {
      expect(noticeOf(view)).toContain(NOTICES.invalid);
    });
    expect(menus()).toHaveLength(0);
    expect(intentOf(view)?.mode).toBe("rename");
    expect(fieldOf(view)?.value).toBe("%%{x");
  });
});

describe("with two diagrams on the page", () => {
  it("a click and a right click on the second reach it, with a stop on the first", async () => {
    const view = await drawn(TWO_FENCES);
    focusTarget(view, fromOf(view), NODE_A);
    rightClick(nodeOf(view, "C"));
    await flush();
    expect(menus()[0]?.getAttribute("aria-label")).toBe("Box actions");
    expect(intentOf(view)?.anchor).toBe(text(view).lastIndexOf("```mermaid"));
    press("Escape");
    expect(active()).toBe(nodeOf(view, "C"));
    focusTarget(view, fromOf(view), NODE_A);
    click(nodeOf(view, "D"));
    await flush();
    expect(fieldOf(view)?.value).toBe("D");
  });

  it("a menu on the second ends connect mode on the first, and holds its own fence", async () => {
    const view = await drawn(TWO_FENCES);
    openMenuOn(view, NODE_A);
    choose("Connect to...");
    rightClick(nodeOf(view, "C"));
    await flush();
    expect(promptOf(view)).toBeNull();
    expect(menus()).toHaveLength(1);
    expect(intentOf(view)?.target).toEqual({ kind: "node", id: "C" });
    expect(view.state.field(diagramField).kept).toBe(
      text(view).lastIndexOf("```mermaid"),
    );
  });

  it("a click on the second's box ends it, and the field there keeps its hold", async () => {
    const view = await drawn(TWO_FENCES);
    openMenuOn(view, NODE_A);
    choose("Connect to...");
    click(nodeOf(view, "C"));
    await flush();
    expect(promptOf(view)).toBeNull();
    expect(fieldOf(view)).not.toBeNull();
    expect(view.state.field(diagramField).kept).toBe(
      text(view).lastIndexOf("```mermaid"),
    );
    expect(text(view)).toBe(TWO_FENCES);
  });
});

describe("a page that takes no writes", () => {
  async function readOnly() {
    const view = await drawn();
    view.dispatch({
      effects: StateEffect.appendConfig.of(EditorState.readOnly.of(true)),
    });
    await flush();
    return view;
  }

  it("leaves a right click to the browser, and no aria-haspopup stands", async () => {
    const view = await readOnly();
    expect(rightClick(nodeOf(view, "B")).defaultPrevented).toBe(false);
    expect(menus()).toHaveLength(0);
    expect(view.dom.querySelectorAll("[aria-haspopup]")).toHaveLength(0);
  });

  it("leaves Shift+F10 and the ContextMenu key to the browser", async () => {
    const view = await readOnly();
    focusTarget(view, fromOf(view), NODE_B);
    expect(press("F10", { shiftKey: true }).defaultPrevented).toBe(false);
    expect(press("ContextMenu").defaultPrevented).toBe(false);
    expect(menus()).toHaveLength(0);
  });

  it("opens nothing on Shift+F10, and the walk still goes", async () => {
    const view = await readOnly();
    focusTarget(view, fromOf(view), NODE_B);
    press("F10", { shiftKey: true });
    expect(menus()).toHaveLength(0);
    expect(active()).toBe(nodeOf(view, "B"));
    press("ArrowLeft");
    expect(active()).toBe(nodeOf(view, "A"));
  });

  it("closes an open menu when the page stops taking writes", async () => {
    const view = await drawn();
    openMenuOn(view, NODE_B);
    view.dispatch({
      effects: StateEffect.appendConfig.of(EditorState.readOnly.of(true)),
    });
    await flush();
    expect(menus()).toHaveLength(0);
    expect(intentOf(view)?.mode).toBe("focus");
    expect(active()).toBe(nodeOf(view, "B"));
    expect(text(view)).toBe(page(SKELETON));
  });
});

describe("what ends a menu from outside", () => {
  it("Raw takes it along", async () => {
    const view = await drawn();
    openMenuOn(view, NODE_B);
    setRaw(view, true);
    await flush();
    expect(menus()).toHaveLength(0);
    expect(active()).toBe(view.contentDOM);
    expect(text(view)).toBe(page(SKELETON));
  });

  it("another document takes it along", async () => {
    const view = await drawn();
    openMenuOn(view, EDGE_AB);
    view.setState(EditorState.create({ doc: "other" }));
    await flush();
    expect(menus()).toHaveLength(0);
    expect(view.state.doc.toString()).toBe("other");
  });

  it("a destroyed view takes it along, and a later press does nothing", async () => {
    const view = await drawn();
    openMenuOn(view, NODE_B);
    view.destroy();
    await flush();
    expect(document.querySelectorAll('[role="menu"]')).toHaveLength(0);
    document.body.dispatchEvent(new MouseEvent("mousedown", { bubbles: true }));
  });

  it("the fence taken away closes it with the notice", async () => {
    const view = await drawn();
    openMenuOn(view, NODE_B);
    const at = fromOf(view);
    view.dispatch({ changes: { from: at, to: text(view).indexOf("Below") } });
    await flush();
    await flush();
    expect(menus()).toHaveLength(0);
    expect(view.dom.querySelector(".cm-diagram-refusal")?.textContent).toBe(
      NOTICES.gone,
    );
    expect(active().tagName.toLowerCase()).not.toBe("body");
  });
});

describe("connect mode", () => {
  function startOn(view: EditorView, target: TargetKey = NODE_A) {
    openMenuOn(view, target);
    choose("Connect to...");
  }

  const WROTE = expected(SKELETON, { op: "connect", from: "A", to: "B" });

  it("says what it waits for, in status role, with the intent in connect", async () => {
    const view = await drawn();
    startOn(view);
    expect(intentOf(view)?.mode).toBe("connect");
    expect(promptOf(view)?.textContent).toBe(
      "Choose the box to connect to. Press Escape to cancel.",
    );
    expect(active()).toBe(nodeOf(view, "A"));
    expect(view.dom.querySelector(".cm-diagram-connecting")).not.toBeNull();
    expect(view.dom.querySelector(".cm-announced")?.textContent).toBe(
      promptOf(view)?.textContent,
    );
  });

  it("ArrowRight to B and Enter appends A --> B, one undo step", async () => {
    const view = await drawn();
    const steps = undoSteps(view);
    startOn(view);
    press("ArrowRight");
    expect(active()).toBe(nodeOf(view, "B"));
    expect(intentOf(view)?.mode).toBe("connect");
    press("Enter");
    await changed(view, page(SKELETON));
    expect(text(view)).toBe(WROTE);
    expect(text(view)).toContain("    A --> B\n```");
    expect(undoSteps(view)).toBe(steps + 1);
    await flush();
    expect(promptOf(view)).toBeNull();
    expect(intentOf(view)?.mode).toBe("focus");
    undoOnce(view);
    expect(text(view)).toBe(page(SKELETON));
  });

  it("walks only boxes, and Home and End stay on boxes", async () => {
    const body = "flowchart TD\n    A --> B\n    B --> C";
    const view = await drawn(page(body));
    startOn(view);
    press("End");
    expect(active()).toBe(nodeOf(view, "C"));
    press("ArrowRight");
    expect(active()).toBe(nodeOf(view, "C"));
    expect(intentOf(view)?.target).toEqual({ kind: "node", id: "C" });
    press("Home");
    expect(active()).toBe(nodeOf(view, "A"));
    press("ArrowLeft");
    expect(active()).toBe(nodeOf(view, "A"));
    expect(intentOf(view)?.mode).toBe("connect");
  });

  it("a click on a box connects", async () => {
    const view = await drawn();
    startOn(view);
    click(nodeOf(view, "B"));
    await changed(view, page(SKELETON));
    expect(text(view)).toBe(WROTE);
    expect(fieldOf(view)).toBeNull();
  });

  it("Escape changes nothing and goes back to the box it began from", async () => {
    const view = await drawn();
    startOn(view);
    press("ArrowRight");
    press("Escape");
    await flush();
    expect(text(view)).toBe(page(SKELETON));
    expect(promptOf(view)).toBeNull();
    expect(intentOf(view)?.mode).toBe("focus");
    expect(active()).toBe(nodeOf(view, "A"));
    expect(view.state.field(diagramField).kept).toBeNull();
  });

  it("a click on the canvas cancels", async () => {
    const view = await drawn();
    startOn(view);
    click(view.dom.querySelector(".cm-diagram svg") as Element);
    await flush();
    expect(text(view)).toBe(page(SKELETON));
    expect(promptOf(view)).toBeNull();
    expect(active()).toBe(nodeOf(view, "A"));
  });

  it("its Cancel button cancels, and takes no place in the tab order", async () => {
    const view = await drawn();
    startOn(view);
    const cancel = view.dom.querySelector<HTMLButtonElement>(
      ".cm-diagram-overlay button",
    );
    expect(cancel?.tabIndex).toBe(-1);
    cancel?.click();
    await flush();
    expect(promptOf(view)).toBeNull();
    expect(active()).toBe(nodeOf(view, "A"));
    expect(text(view)).toBe(page(SKELETON));
  });

  it("a right click cancels and opens no menu", async () => {
    const view = await drawn();
    startOn(view);
    expect(rightClick(nodeOf(view, "B")).defaultPrevented).toBe(true);
    await flush();
    expect(menus()).toHaveLength(0);
    expect(promptOf(view)).toBeNull();
    expect(text(view)).toBe(page(SKELETON));
  });

  it("a click on an arrow is no choice, and it keeps waiting", async () => {
    const view = await drawn();
    startOn(view);
    click(view.dom.querySelector('path[data-et="edge"]') as Element);
    await flush();
    expect(intentOf(view)?.mode).toBe("connect");
    expect(text(view)).toBe(page(SKELETON));
    expect(promptOf(view)).not.toBeNull();
  });

  it("no other key on a stop takes anything while it waits", async () => {
    const view = await drawn();
    startOn(view);
    press("Delete");
    press("Backspace");
    press("F2");
    press("F10", { shiftKey: true });
    press("ContextMenu");
    await flush();
    expect(text(view)).toBe(page(SKELETON));
    expect(menus()).toHaveLength(0);
    expect(fieldOf(view)).toBeNull();
    expect(intentOf(view)?.mode).toBe("connect");
  });

  it("a held Enter does not choose, and two presses write once", async () => {
    const view = await drawn();
    startOn(view);
    press("ArrowRight");
    press("Enter", { repeat: true });
    await flush();
    expect(text(view)).toBe(page(SKELETON));
    press("Enter");
    press("Enter");
    await changed(view, page(SKELETON));
    await flush();
    await flush();
    expect(text(view)).toBe(WROTE);
  });

  it("the Enter that started it, still held, chooses nothing", async () => {
    const view = await drawn();
    openMenuOn(view, NODE_A);
    press("ArrowDown");
    press("ArrowDown");
    press("ArrowDown");
    press("Enter");
    expect(intentOf(view)?.mode).toBe("connect");
    press("Enter", { repeat: true });
    await flush();
    expect(text(view)).toBe(page(SKELETON));
    expect(intentOf(view)?.mode).toBe("connect");
  });

  it("Escape during the check writes nothing", async () => {
    const view = await drawn();
    startOn(view);
    press("ArrowRight");
    press("Enter");
    press("Escape");
    await flush();
    await new Promise((resolve) => setTimeout(resolve, 30));
    expect(text(view)).toBe(page(SKELETON));
    expect(noticeOf(view)).toBe("");
    expect(promptOf(view)).toBeNull();
  });

  it("leaving by a focus move during the check writes nothing", async () => {
    // A body of its own: a verdict already known would leave no time to leave in.
    const body = "flowchart TD\n    A[Only here] --> B[Next step]";
    const view = await drawn(page(body));
    startOn(view);
    press("ArrowRight");
    press("Enter");
    const other = document.createElement("input");
    document.body.append(other);
    const from = active();
    other.focus();
    from.dispatchEvent(
      new FocusEvent("blur", { bubbles: false, relatedTarget: other }),
    );
    await new Promise((resolve) => setTimeout(resolve, 30));
    expect(text(view)).toBe(page(body));
    expect(promptOf(view)).toBeNull();
    expect(active()).toBe(other);
  });

  it("leaving by a click on another diagram during the check writes nothing", async () => {
    const view = await drawn(TWO_FENCES);
    startOn(view);
    press("ArrowRight");
    press("Enter");
    click(nodeOf(view, "C"));
    await new Promise((resolve) => setTimeout(resolve, 30));
    expect(text(view)).toBe(TWO_FENCES);
    expect(promptOf(view)).toBeNull();
  });

  it("does not end on the ContextMenu key's own contextmenu event", async () => {
    const view = await drawn();
    startOn(view);
    press("ContextMenu");
    const event = new MouseEvent("contextmenu", {
      bubbles: true,
      cancelable: true,
    });
    active().dispatchEvent(event);
    await flush();
    expect(event.defaultPrevented).toBe(true);
    expect(intentOf(view)?.mode).toBe("connect");
    // The key may have sent none, so a real right click still gets out.
    nodeOf(view, "B").dispatchEvent(
      new MouseEvent("mousedown", { bubbles: true, button: 2 }),
    );
    rightClick(nodeOf(view, "B"));
    await flush();
    expect(promptOf(view)).toBeNull();
  });

  it("a key that sent no contextmenu of its own does not eat a later right click", async () => {
    const view = await drawn();
    startOn(view);
    press("ContextMenu");
    nodeOf(view, "B").dispatchEvent(
      new MouseEvent("mousedown", { bubbles: true, button: 2 }),
    );
    rightClick(nodeOf(view, "B"));
    await flush();
    expect(promptOf(view)).toBeNull();
  });

  it("a destroyed editor takes its Escape guard along", async () => {
    const view = await drawn();
    startOn(view);
    view.destroy();
    await flush();
    const event = new KeyboardEvent("keydown", {
      key: "Escape",
      bubbles: true,
      cancelable: true,
    });
    let reached = false;
    const listener = () => {
      reached = true;
    };
    document.addEventListener("keydown", listener);
    document.body.dispatchEvent(event);
    document.removeEventListener("keydown", listener);
    expect(event.defaultPrevented).toBe(false);
    expect(reached).toBe(true);
  });

  it("the focus leaving the diagram ends it, and nothing stays parked", async () => {
    const view = await drawn();
    startOn(view);
    const other = document.createElement("input");
    document.body.append(other);
    const from = active();
    other.focus();
    from.dispatchEvent(
      new FocusEvent("blur", { bubbles: false, relatedTarget: other }),
    );
    await flush();
    expect(promptOf(view)).toBeNull();
    expect(intentOf(view)).toBeNull();
    expect(view.state.facet(mainSelectionParked)).toBe(false);
    expect(active()).toBe(other);
  });

  it("a stop that takes the focus some other way keeps the mode and is the choice", async () => {
    const view = await drawn();
    startOn(view);
    (nodeOf(view, "B") as unknown as HTMLElement).focus();
    await flush();
    expect(intentOf(view)?.mode).toBe("connect");
    expect(intentOf(view)?.target).toEqual(NODE_B);
    press("Enter");
    await changed(view, page(SKELETON));
    expect(text(view)).toBe(WROTE);
  });

  it("the page ceasing to take writes ends it, on the stop", async () => {
    const view = await drawn();
    startOn(view);
    view.dispatch({
      effects: StateEffect.appendConfig.of(EditorState.readOnly.of(true)),
    });
    await flush();
    expect(promptOf(view)).toBeNull();
    expect(intentOf(view)?.mode).toBe("focus");
    expect(active()).toBe(nodeOf(view, "A"));
    expect(text(view)).toBe(page(SKELETON));
  });

  it("Raw ends it", async () => {
    const view = await drawn();
    startOn(view);
    setRaw(view, true);
    await flush();
    expect(promptOf(view)).toBeNull();
    expect(active()).toBe(view.contentDOM);
    expect(text(view)).toBe(page(SKELETON));
  });

  it("Escape with the focus on the body still gets out", async () => {
    const view = await drawn();
    startOn(view);
    active().blur();
    expect(active()).toBe(document.body);
    press("Escape", {}, document.body);
    await flush();
    expect(promptOf(view)).toBeNull();
    expect(intentOf(view)?.mode).toBe("focus");
  });

  it("a refused connect ends the mode with its notice", async () => {
    const body = "flowchart TD\n    A --> A\n    A --> B";
    const view = await drawn(page(body));
    startOn(view);
    press("Enter");
    await vi.waitFor(() => {
      expect(noticeOf(view)).toBe(NOTICES.invalid);
    });
    expect(text(view)).toBe(page(body));
    expect(promptOf(view)).toBeNull();
    expect(active()).toBe(nodeOf(view, "A"));
  });

  it("flowMenu can be asked by itself, and acts as the menu does", async () => {
    const view = await drawn();
    const { name, items } = flowMenu(
      view,
      fromOf(view),
      NODE_B,
      parseFlowchart(SKELETON),
    );
    expect(name).toBe("Box actions");
    items.find((item) => item.label === "Delete")?.run?.();
    await changed(view, page(SKELETON));
    expect(text(view)).toBe(expected(SKELETON, { op: "deleteNode", id: "B" }));
  });
});
