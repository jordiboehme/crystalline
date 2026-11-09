/**
 * The keyboard and the pointer on a drawn flowchart, on the real surface and the
 * real verdict, with only mermaid's render stubbed. A key in the main view goes
 * through its keymaps; a key inside the diagram goes to the focused element, as
 * a browser sends it, because the widget's `ignoreEvent` keeps it from main.
 * `focus.marks-hidden.test.ts` and `focus.marks-show.test.ts` run this file again.
 */

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { EditorView } from "@codemirror/view";

import { cellMarksMode } from "../../test/cellMarksMode";
import type { MarkViewOptions } from "../../test/markKit";
import {
  cleanupMarkViews,
  keys,
  markView,
  remoteInsert,
  setCaret,
  typeText,
  undoOnce,
} from "../../test/markKit";
import { isSequenceBody, sequenceDrawing } from "../../test/sequenceDrawing";
import SKELETON from "../fixtures/diagrams/svg/f01-skeleton.svg?raw";
import { mainSelectionParked } from "../parkedSelection";
import { drawnFenceLines } from "./draws";
import { NOTICES } from "./edits";
import { flowKind, removeFence, runDiagramOp } from "./apply";
import { parseFlowchart } from "./flowchart/parse";
import { focusTarget, registerStopAction } from "./focus";
import { openLabelEditor } from "./labelEditor";
import { diagramIntent, intentStale } from "./intent";
import { prefetchDiagrams } from "./loader";
import { requestVerdict } from "./verdict";
import { forgetDrawings, NOTICE_MS } from "./widget";

const kit = vi.hoisted(() => ({
  /** Whether renders wait for the test to let them go. */
  hold: { value: false },
  held: [] as (() => void)[],
}));

const BODY = "flowchart TD\n    A[First step] --> B[Next step]";

/** A small drawing with the ids the real verdict gives, for a body not recorded. */
async function drawingOf(id: string, body: string): Promise<string> {
  if (body === BODY) {
    return SKELETON.replaceAll("rec-f01-skeleton", id);
  }
  if (isSequenceBody(body)) {
    return sequenceDrawing(id, body);
  }
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
        `<path d="M0,0L1,1" data-et="edge" data-id="${mermaidId}"></path>` +
        `<g class="label" data-id="${mermaidId}"></g>`,
    )
    .join("");
  return `<svg id="${id}"><g class="edgePaths">${edges}</g><g class="nodes">${nodes}</g></svg>`;
}

vi.mock("mermaid", async (original) => {
  const real = (await original<{ default: object }>()).default;
  return {
    default: {
      ...real,
      render: async (id: string, body: string) => {
        if (kit.hold.value) {
          await new Promise<void>((resolve) => {
            kit.held.push(resolve);
          });
        }
        return { svg: await drawingOf(id, body) };
      },
    },
  };
});

const fence = (body: string) => `\`\`\`mermaid\n${body}\n\`\`\``;
const DOC = `Above\n${fence(BODY)}\nBelow\n`;
const FROM = DOC.indexOf("```");

const flush = async () => {
  await new Promise((resolve) => setTimeout(resolve, 0));
};

function modeOptions(): MarkViewOptions {
  const mode = cellMarksMode();
  return mode === "off" ? {} : { mode };
}

async function drawn(doc = DOC, options: MarkViewOptions = {}) {
  const view = markView(doc, { ...modeOptions(), ...options });
  // A caret at the start of a fence first in the page would hold it as text.
  if (doc.startsWith("```")) {
    setCaret(view, doc.length, "select");
  }
  await prefetchDiagrams();
  await vi.waitFor(() => {
    expect(view.dom.querySelector(".cm-diagram svg")).not.toBeNull();
  });
  await flush();
  view.contentDOM.focus();
  return view;
}

const box = (view: EditorView) => {
  const found = view.dom.querySelector<HTMLElement>(".cm-diagram");
  if (found === null) throw new Error("no diagram box");
  return found;
};
const active = () => document.activeElement;
const text = (view: EditorView) => view.state.doc.toString();

/** A key on whatever holds the focus, as a browser sends it. */
function press(key: string, options: KeyboardEventInit = {}): void {
  active()?.dispatchEvent(
    new KeyboardEvent("keydown", {
      key,
      bubbles: true,
      cancelable: true,
      ...options,
    }),
  );
}

/** The verify step is real, so a press that writes takes a while to land. */
async function settled(view: EditorView, doc: string): Promise<void> {
  await vi.waitFor(() => {
    expect(text(view)).toBe(doc);
  });
  await flush();
}

beforeEach(async () => {
  // A click here is about the stop. What it opens has its own suite.
  await prefetchDiagrams();
  registerStopAction("rename", () => undefined);
  kit.hold.value = false;
  kit.held.length = 0;
  forgetDrawings();
});

afterEach(() => {
  vi.useRealTimers();
  for (const release of kit.held) release();
  cleanupMarkViews();
});

describe("going in from the main view", () => {
  it("takes ArrowDown from the line above to the canvas", async () => {
    const view = await drawn();
    setCaret(view, 2);
    expect(keys(view, "ArrowDown")).toBe(true);
    expect(active()).toBe(box(view));
    expect(text(view)).toBe(DOC);
  });

  it("takes ArrowRight at the end of the line above to the canvas", async () => {
    const view = await drawn();
    setCaret(view, 5);
    keys(view, "ArrowRight");
    expect(active()).toBe(box(view));
  });

  it("takes ArrowUp from the line below to the canvas", async () => {
    const view = await drawn();
    setCaret(view, DOC.indexOf("Below") + 2);
    keys(view, "ArrowUp");
    expect(active()).toBe(box(view));
  });

  it("takes ArrowLeft at the start of the line below to the canvas", async () => {
    const view = await drawn();
    setCaret(view, DOC.indexOf("Below"));
    keys(view, "ArrowLeft");
    expect(active()).toBe(box(view));
  });

  it("does not go in from the middle of a line", async () => {
    const view = await drawn();
    setCaret(view, 2);
    keys(view, "ArrowRight");
    expect(active()).toBe(view.contentDOM);
  });
});

describe("moving through the stops", () => {
  async function onCanvas() {
    const view = await drawn();
    setCaret(view, 2);
    keys(view, "ArrowDown");
    return view;
  }

  it("reaches the first box with ArrowRight, by its label", async () => {
    await onCanvas();
    press("ArrowRight");
    expect(active()?.getAttribute("aria-label")).toBe("Box: First step");
  });

  it("goes back with ArrowLeft and ArrowUp, forward with ArrowDown", async () => {
    await onCanvas();
    press("ArrowDown");
    press("ArrowDown");
    expect(active()?.getAttribute("aria-label")).toBe("Box: Next step");
    press("ArrowUp");
    expect(active()?.getAttribute("aria-label")).toBe("Box: First step");
    press("ArrowLeft");
    expect(active()?.classList.contains("cm-diagram")).toBe(true);
  });

  it("reaches the last arrow with End and the first box with Home", async () => {
    await onCanvas();
    press("End");
    expect(active()?.getAttribute("aria-label")).toBe(
      "Arrow from First step to Next step",
    );
    press("Home");
    expect(active()?.getAttribute("aria-label")).toBe("Box: First step");
  });

  it("stays at the ends", async () => {
    const view = await onCanvas();
    press("ArrowLeft");
    expect(active()).toBe(box(view));
    press("End");
    const last = active();
    press("ArrowRight");
    expect(active()).toBe(last);
  });

  it("keeps exactly one stop in the tab order, the one with the focus", async () => {
    const view = await onCanvas();
    press("ArrowRight");
    const zero = box(view).querySelectorAll('[tabindex="0"]');
    expect(zero).toHaveLength(1);
    expect(zero[0]).toBe(active());
    expect(box(view).getAttribute("tabindex")).toBe("-1");
  });

  it("draws the focus look on that stop", async () => {
    const view = await onCanvas();
    press("ArrowRight");
    expect(active()?.classList.contains("cm-diagram-focus")).toBe(true);
    expect(box(view).querySelectorAll(".cm-diagram-focus")).toHaveLength(1);
    expect(box(view).querySelector(".cm-diagram-overlay > *")).not.toBeNull();
  });

  it("holds the stop in the intent", async () => {
    const view = await onCanvas();
    press("ArrowRight");
    expect(view.state.field(diagramIntent)).toEqual({
      anchor: FROM,
      target: { kind: "node", id: "A" },
      mode: "focus",
    });
  });

  it("does not handle Tab: the page's tab order takes it", async () => {
    await onCanvas();
    const event = new KeyboardEvent("keydown", {
      key: "Tab",
      bubbles: true,
      cancelable: true,
    });
    active()?.dispatchEvent(event);
    expect(event.defaultPrevented).toBe(false);
  });
});

describe("leaving", () => {
  it("leaves with Escape to the start of the line after the fence", async () => {
    const view = await drawn();
    setCaret(view, 2);
    keys(view, "ArrowDown");
    press("ArrowRight");
    press("Escape");
    expect(active()).toBe(view.contentDOM);
    expect(view.state.selection.main.head).toBe(DOC.indexOf("Below"));
    expect(view.state.field(diagramIntent)).toBeNull();
  });

  it("closes an open mode first, and leaves on the next Escape", async () => {
    const view = await drawn();
    setCaret(view, 2);
    keys(view, "ArrowDown");
    const { anchor, target } = view.state.field(diagramIntent) ?? {
      anchor: 0,
      target: { kind: "canvas" as const },
    };
    const { setDiagramIntent } = await import("./intent");
    view.dispatch({
      effects: setDiagramIntent.of({ anchor, target, mode: "menu" }),
    });
    press("Escape");
    expect(view.state.field(diagramIntent)?.mode).toBe("focus");
    expect(active()).toBe(box(view));
    press("Escape");
    expect(view.state.field(diagramIntent)).toBeNull();
  });
});

describe("deleting", () => {
  it("deletes box B of the skeleton as one undo step, and A stays needed", async () => {
    const view = await drawn();
    setCaret(view, 2);
    keys(view, "ArrowDown");
    press("End");
    press("ArrowLeft");
    expect(active()?.getAttribute("aria-label")).toBe("Box: Next step");
    press("Delete");
    await settled(
      view,
      `Above\n${fence("flowchart TD\n    A[First step]")}\nBelow\n`,
    );
    undoOnce(view);
    expect(text(view)).toBe(DOC);
  });

  it("moves to a neighbour after a clicked box goes, so a second Delete cannot take the diagram", async () => {
    const view = await drawn();
    const nodes = box(view).querySelectorAll("g.node");
    nodes[1]?.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    expect(active()?.getAttribute("aria-label")).toBe("Box: Next step");
    press("Delete");
    await settled(
      view,
      `Above\n${fence("flowchart TD\n    A[First step]")}\nBelow\n`,
    );
    await vi.waitFor(() => {
      expect(active()?.getAttribute("aria-label")).toBe("Box: First step");
    });
  });

  it("deletes the fence on a second Backspace from the line below, and undo gives the bytes back", async () => {
    const view = await drawn();
    setCaret(view, DOC.indexOf("Below"));
    keys(view, "Backspace");
    expect(text(view)).toBe(DOC);
    expect(active()).toBe(box(view));
    press("Backspace");
    expect(text(view)).toBe("Above\nBelow\n");
    undoOnce(view);
    expect(text(view)).toBe(DOC);
  });

  it("does the same in a room", async () => {
    const view = await drawn(DOC, { room: true });
    setCaret(view, DOC.indexOf("Below"));
    keys(view, "Backspace");
    press("Backspace");
    expect(text(view)).toBe("Above\nBelow\n");
    undoOnce(view);
    expect(text(view)).toBe(DOC);
  });

  it("leaves a last fence's line empty, so the next letter has a place", async () => {
    const doc = `Above\n${fence(BODY)}`;
    const view = await drawn(doc);
    setCaret(view, 5);
    keys(view, "Delete");
    expect(text(view)).toBe(doc);
    press("Delete");
    expect(text(view)).toBe("Above\n");
    expect(view.state.selection.main.head).toBe(6);
  });

  it("runs undo from the box, so a person is not stuck there", async () => {
    const view = await drawn();
    setCaret(view, 2);
    keys(view, "ArrowDown");
    press("ArrowRight");
    press("Delete");
    await vi.waitFor(() => {
      expect(text(view)).not.toBe(DOC);
    });
    box(view).focus();
    press("z", {
      ctrlKey: !/Mac/.test(navigator.platform),
      metaKey: /Mac/.test(navigator.platform),
    });
    expect(text(view)).toBe(DOC);
  });

  it("deletes nothing on a page that takes no writes", async () => {
    const view = await drawn();
    const { EditorState, StateEffect } = await import("@codemirror/state");
    view.dispatch({
      effects: StateEffect.appendConfig.of(EditorState.readOnly.of(true)),
    });
    box(view).focus();
    press("Delete");
    await flush();
    expect(text(view)).toBe(DOC);
  });
});

describe("an empty diagram (R60)", () => {
  it("leaves the canvas as the only stop after the last box went", async () => {
    const body = "flowchart TD\n    A[Only]";
    const doc = `Above\n${fence(body)}\nBelow\n`;
    const view = await drawn(doc);
    setCaret(view, 2);
    keys(view, "ArrowDown");
    press("ArrowRight");
    expect(active()?.getAttribute("aria-label")).toBe("Box: Only");
    press("Delete");
    await settled(view, `Above\n${fence("flowchart TD")}\nBelow\n`);
    await vi.waitFor(() => {
      expect(active()).toBe(box(view));
    });
    press("ArrowRight");
    press("End");
    press("Home");
    expect(active()).toBe(box(view));
  });

  it("maps an emptied subgraph's g.node to the canvas and gives it no stop", async () => {
    const body = "flowchart TD\n    subgraph s\n    end\n    A --> s";
    const doc = `Above\n${fence(body)}\nBelow\n`;
    const view = await drawn(doc);
    const svg = box(view).querySelector("svg");
    const ghost = document.createElementNS("http://www.w3.org/2000/svg", "g");
    ghost.setAttribute("class", "node");
    ghost.id = `${svg?.id ?? ""}-s`;
    svg?.append(ghost);
    ghost.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    expect(view.state.field(diagramIntent)?.target).toEqual({ kind: "canvas" });
    expect(ghost.getAttribute("tabindex")).toBeNull();
  });
});

describe("a drawing behind the text", () => {
  it("answers a click after a remote change with the stale notice, and changes nothing", async () => {
    const view = await drawn(DOC, { room: true });
    kit.hold.value = true;
    remoteInsert(view, DOC.indexOf("Next"), "X");
    const doc = text(view);
    box(view)
      .querySelector("g.node")
      ?.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    expect(box(view).querySelector(".cm-diagram-notice")?.textContent).toBe(
      NOTICES.stale,
    );
    expect(text(view)).toBe(doc);
    expect(view.state.field(diagramIntent)).toBeNull();
  });

  it("answers a key the same way", async () => {
    const view = await drawn(DOC, { room: true });
    setCaret(view, 2);
    keys(view, "ArrowDown");
    kit.hold.value = true;
    remoteInsert(view, DOC.indexOf("Next"), "X");
    const doc = text(view);
    press("Delete");
    expect(box(view).querySelector(".cm-diagram-notice")?.textContent).toBe(
      NOTICES.stale,
    );
    await flush();
    expect(text(view)).toBe(doc);
  });

  it("still leaves with Escape", async () => {
    const view = await drawn(DOC, { room: true });
    setCaret(view, 2);
    keys(view, "ArrowDown");
    kit.hold.value = true;
    remoteInsert(view, DOC.indexOf("Next"), "X");
    press("Escape");
    expect(active()).toBe(view.contentDOM);
  });

  it("brings a parse that fell behind forward, so stale cannot stick (R79 N1)", async () => {
    const view = await drawn(DOC, { room: true });
    setCaret(view, 2);
    keys(view, "ArrowDown");
    let now = Date.now();
    const clock = vi.spyOn(Date, "now").mockImplementation(() => {
      now += 1_000;
      return now;
    });
    try {
      remoteInsert(view, DOC.indexOf("Next"), "X");
    } finally {
      clock.mockRestore();
    }
    expect(intentStale(view.state)).toBe(true);
    press("ArrowRight");
    expect(box(view).querySelector(".cm-diagram-notice")?.textContent).toBe(
      NOTICES.stale,
    );
    expect(intentStale(view.state)).toBe(false);
  });
});

describe("a selection inside a drawn fence", () => {
  it("draws the look on a search hit's box and leaves the focus where it was", async () => {
    const view = await drawn();
    const at = DOC.indexOf("Next step");
    view.dispatch({
      selection: { anchor: at, head: at + 4 },
      userEvent: "select.search",
    });
    await flush();
    expect(active()).toBe(view.contentDOM);
    expect(
      box(view).querySelector(".cm-diagram-focus")?.getAttribute("aria-label"),
    ).toBe("Box: Next step");
  });

  it("focuses the canvas for any other selection there", async () => {
    const view = await drawn();
    setCaret(view, DOC.indexOf("Next"), "select");
    await flush();
    expect(active()).toBe(box(view));
    expect(text(view)).toBe(DOC);
  });

  it("counts the fence's two ends as inside", async () => {
    const view = await drawn();
    setCaret(view, FROM, "select");
    await flush();
    expect(active()).toBe(box(view));
    setCaret(view, 2, "select");
    view.contentDOM.focus();
    await flush();
    setCaret(view, DOC.lastIndexOf("```") + 3, "select");
    await flush();
    expect(active()).toBe(box(view));
    expect(text(view)).toBe(DOC);
  });

  it("runs on undo: a caret an undo puts back in the fence takes no key", async () => {
    const view = await drawn();
    // A step whose undo restores a caret inside the fence, made with the focus
    // away so nothing has run on that caret yet.
    const inside = DOC.indexOf("Next");
    view.contentDOM.blur();
    view.dispatch({ selection: { anchor: inside } });
    await flush();
    view.dispatch({
      changes: { from: 0, insert: "x" },
      selection: { anchor: 1 },
      userEvent: "input.type",
    });
    view.contentDOM.focus();
    await flush();
    undoOnce(view);
    await flush();
    expect(view.state.selection.main.head).toBe(inside);
    expect(active()).toBe(box(view));
    press("Q");
    expect(text(view)).toBe(DOC);
  });

  it("leaves a selection across the fence to the main view", async () => {
    const view = await drawn();
    view.dispatch({
      selection: { anchor: 0, head: DOC.length },
      userEvent: "select",
    });
    await flush();
    expect(active()).toBe(view.contentDOM);
  });
});

describe("what the page around the box gets", () => {
  it("parks main's selection while the keyboard is on the drawing, so no bar writes", async () => {
    const view = await drawn();
    setCaret(view, 2);
    expect(view.state.facet(mainSelectionParked)).toBe(false);
    keys(view, "ArrowDown");
    expect(view.state.facet(mainSelectionParked)).toBe(true);
    press("Escape");
    expect(view.state.facet(mainSelectionParked)).toBe(false);
  });

  it("raises no floating bar for a search hit inside the fence", async () => {
    const contexts: unknown[] = [];
    const view = await drawn(DOC, {
      onFloatingContext: (context) => contexts.push(context),
    });
    const at = DOC.indexOf("Next step");
    view.dispatch({
      selection: { anchor: at, head: at + 4 },
      userEvent: "select.search",
    });
    await vi.waitFor(() => {
      expect(contexts.length).toBeGreaterThan(0);
    });
    await new Promise((resolve) => setTimeout(resolve, 50));
    expect(contexts.at(-1)).toBeNull();
    view.dispatch({
      selection: { anchor: 0, head: 3 },
      userEvent: "select",
    });
    expect(view.state.sliceDoc(0, 3)).toBe("Abo");
  });

  it("leaves main's text alone for a Backspace on a box", async () => {
    const view = await drawn();
    setCaret(view, 3);
    box(view).focus();
    press("x");
    press("Enter");
    expect(text(view)).toBe(DOC);
    expect(view.state.selection.main.head).toBe(3);
  });

  it("closes without a focus move when the focus goes elsewhere in the page", async () => {
    const view = await drawn();
    setCaret(view, 2);
    keys(view, "ArrowDown");
    const outside = document.createElement("input");
    document.body.append(outside);
    outside.focus();
    await flush();
    expect(view.state.field(diagramIntent)).toBeNull();
    expect(active()).toBe(outside);
    outside.remove();
  });

  it("leaves a click back into the text its canonical caret, so a letter lands as usual", async () => {
    if (cellMarksMode() === "show") {
      return;
    }
    const doc = `**Bold** text\n${fence(BODY)}\nBelow\n`;
    const view = await drawn(doc);
    setCaret(view, doc.indexOf("Below"));
    keys(view, "ArrowLeft");
    expect(active()).toBe(box(view));
    view.contentDOM.dispatchEvent(
      new MouseEvent("mousedown", { bubbles: true, cancelable: true }),
    );
    view.contentDOM.focus();
    setCaret(view, 1, "select.pointer");
    // Past the opening stars: the parked state must be over before the caret lands.
    expect(view.state.selection.main.head).toBe(2);
    await flush();
    typeText(view, "x");
    expect(text(view)).toBe(`**xBold** text\n${fence(BODY)}\nBelow\n`);
  });

  it("opens the intent when Tab brings the focus in", async () => {
    const view = await drawn();
    box(view).focus();
    await flush();
    expect(view.state.field(diagramIntent)?.target).toEqual({ kind: "canvas" });
  });
});

describe("what never reaches a fence's marks (R73)", () => {
  const refused = (view: EditorView, doc: string) => {
    expect(text(view)).toBe(doc);
    expect(view.dom.querySelector(".cm-diagram")).not.toBeNull();
  };

  it.each([
    ["Backspace", "Below"],
    ["Mod-Backspace", "Below"],
  ])("%s on the line below goes to the canvas", async (key, at) => {
    const view = await drawn();
    setCaret(view, DOC.indexOf(at));
    keys(view, key);
    refused(view, DOC);
    expect(active()).toBe(box(view));
  });

  it.each(["Delete", "Mod-Delete"])(
    "%s at the end of the line above goes to the canvas",
    async (key) => {
      const view = await drawn();
      setCaret(view, 5);
      keys(view, key);
      refused(view, DOC);
      expect(active()).toBe(box(view));
    },
  );

  it("Backspace at the visible start of a bold line below goes to the canvas", async () => {
    const doc = `Above\n${fence(BODY)}\n**Below**\n`;
    const view = await drawn(doc);
    setCaret(view, doc.indexOf("Below"));
    keys(view, "Backspace");
    if (cellMarksMode() === "show") {
      // The marks show on this line, so the key takes a star as it always has.
      expect(text(view)).not.toContain("```**");
    } else {
      refused(view, doc);
      expect(active()).toBe(box(view));
    }
  });

  it("Mod-Backspace at the visible start of a bold line below goes to the canvas", async () => {
    if (cellMarksMode() === "show") {
      return;
    }
    const doc = `Above\n${fence(BODY)}\n**Below**\n`;
    const view = await drawn(doc);
    setCaret(view, doc.indexOf("Below"));
    keys(view, "Mod-Backspace");
    refused(view, doc);
    expect(active()).toBe(box(view));
  });

  it("ArrowLeft at the visible start of a bold line below leaves the caret off the fence", async () => {
    if (cellMarksMode() === "show") {
      return;
    }
    const doc = `Above\n${fence(BODY)}\n**Below**\n`;
    const view = await drawn(doc);
    setCaret(view, doc.indexOf("Below"));
    const head = view.state.selection.main.head;
    keys(view, "ArrowLeft");
    expect(active()).toBe(box(view));
    expect(view.state.selection.main.head).toBe(head);
  });

  it("Alt-ArrowUp does not move the line below into the fence", async () => {
    const view = await drawn();
    setCaret(view, DOC.indexOf("Below"));
    keys(view, "Alt-ArrowUp");
    refused(view, DOC);
  });

  it("Alt-ArrowDown does not move the line above into the fence", async () => {
    const view = await drawn();
    setCaret(view, 2);
    keys(view, "Alt-ArrowDown");
    refused(view, DOC);
  });

  it("refuses a selection deleted up to the first mark, which would join them", async () => {
    const view = await drawn();
    view.dispatch({
      changes: { from: 2, to: FROM },
      userEvent: "delete.selection",
    });
    refused(view, DOC);
  });

  it("refuses a delete from the closing mark into the line below", async () => {
    const view = await drawn();
    const end = DOC.lastIndexOf("```") + 3;
    view.dispatch({
      changes: { from: end, to: end + 3 },
      userEvent: "delete.forward",
    });
    refused(view, DOC);
  });

  it("lets a line deleted whole above the fence through", async () => {
    const view = await drawn();
    view.dispatch({
      changes: { from: 0, to: FROM },
      userEvent: "delete.selection",
    });
    expect(text(view)).toBe(DOC.slice(FROM));
  });

  it("lets a selection across the whole fence through, and undo restores it", async () => {
    const view = await drawn();
    view.dispatch({
      changes: { from: 2, to: DOC.indexOf("Below") + 2 },
      userEvent: "delete.selection",
    });
    expect(text(view)).toBe("Ablow\n");
    undoOnce(view);
    expect(text(view)).toBe(DOC);
  });

  it("does not stop an edit from a peer, which is the room's to settle", async () => {
    const view = await drawn(DOC, { room: true });
    remoteInsert(view, FROM, "x");
    expect(text(view)).toBe(`Above\nx${DOC.slice(FROM)}`);
  });
});

describe("a re-render", () => {
  it.each([
    ["with the parse behind", true],
    ["with the parse caught up", false],
  ])(
    "keeps the person's stop through a line a peer inserts at the first mark, %s",
    async (_, spent) => {
      const view = await drawn(DOC, { room: true });
      setCaret(view, 2);
      keys(view, "ArrowDown");
      press("ArrowRight");
      let now = Date.now();
      const clock = spent
        ? vi.spyOn(Date, "now").mockImplementation(() => {
            now += 1_000;
            return now;
          })
        : null;
      try {
        remoteInsert(view, FROM, "New line\n");
      } finally {
        clock?.mockRestore();
      }
      await flush();
      await flush();
      expect(active()?.getAttribute("aria-label")).toBe("Box: First step");
      expect(box(view).querySelectorAll('[tabindex="0"]')).toHaveLength(1);
      expect(view.state.field(diagramIntent)?.target).toEqual({
        kind: "node",
        id: "A",
      });
    },
  );

  it("moves the focus to the same box's new element", async () => {
    const view = await drawn(DOC, { room: true });
    setCaret(view, 2);
    keys(view, "ArrowDown");
    press("ArrowRight");
    const old = active();
    remoteInsert(view, DOC.indexOf("Next"), "X");
    await vi.waitFor(() => {
      expect(active()).not.toBe(old);
      expect(active()?.getAttribute("aria-label")).toBe("Box: First step");
    });
    expect(box(view).querySelectorAll('[tabindex="0"]')).toHaveLength(1);
    expect(EditorView.findFromDOM(box(view))).toBe(view);
  });
});

/** A key as a browser sends it to the main view; `repeat` marks a held key. */
function pressMain(view: EditorView, key: string, repeat = false): void {
  view.contentDOM.dispatchEvent(
    new KeyboardEvent("keydown", {
      key,
      repeat,
      bubbles: true,
      cancelable: true,
    }),
  );
}

const noticeOf = (view: EditorView) =>
  box(view).querySelector(".cm-diagram-notice")?.textContent;

describe("main taking the focus with its selection in a drawn fence (R80)", () => {
  const LAST = `Above\n${fence(BODY)}`;

  it("shows the fence as text after Mod-End and a way back to main", async () => {
    const view = await drawn(LAST);
    setCaret(view, LAST.length, "select");
    await flush();
    expect(active()).toBe(box(view));
    view.contentDOM.focus();
    await flush();
    expect(view.dom.querySelector(".cm-diagram")).toBeNull();
    expect(view.contentDOM.textContent).toContain("flowchart TD");
    expect(text(view)).toBe(LAST);
  });

  it("shows the fence as text when a search hit is left for main", async () => {
    const view = await drawn();
    view.contentDOM.blur();
    const at = DOC.indexOf("Next step");
    view.dispatch({
      selection: { anchor: at, head: at + 4 },
      userEvent: "select.search",
    });
    await flush();
    view.contentDOM.focus();
    await flush();
    expect(view.dom.querySelector(".cm-diagram")).toBeNull();
    expect(view.state.sliceDoc(at, at + 4)).toBe("Next");
  });

  it("draws it again when the caret leaves", async () => {
    const view = await drawn(LAST);
    setCaret(view, LAST.length, "select");
    await flush();
    view.contentDOM.focus();
    await flush();
    setCaret(view, 2, "select");
    await flush();
    expect(view.dom.querySelector(".cm-diagram")).not.toBeNull();
  });

  describe("still drawn, with the selection inside", () => {
    /** A hit taken with main focused: Mod-d, which is no way back to main. */
    async function hit() {
      const view = await drawn();
      const at = DOC.indexOf("Next step");
      view.dispatch({
        selection: { anchor: at, head: at + 4 },
        userEvent: "select.search",
      });
      await flush();
      expect(active()).toBe(view.contentDOM);
      return { view, at };
    }

    it("Backspace takes nothing and hands the keyboard to the box", async () => {
      const { view } = await hit();
      keys(view, "Backspace");
      await flush();
      expect(text(view)).toBe(DOC);
      expect(active()).toBe(box(view));
    });

    it("a cut takes nothing and hands the keyboard to the box", async () => {
      const { view, at } = await hit();
      view.dispatch({
        changes: { from: at, to: at + 4 },
        userEvent: "delete.cut",
      });
      await flush();
      expect(text(view)).toBe(DOC);
      expect(active()).toBe(box(view));
    });

    it("a letter takes nothing", async () => {
      const { view } = await hit();
      typeText(view, "Q");
      await flush();
      expect(text(view)).toBe(DOC);
    });

    it("refuses an edit elsewhere too: the browser's caret is not the selection (N2)", async () => {
      const { view } = await hit();
      const below = DOC.indexOf("Below");
      view.dispatch({
        changes: { from: below, insert: "Q" },
        userEvent: "input.type",
      });
      await flush();
      expect(text(view)).toBe(DOC);
      expect(active()).toBe(box(view));
    });

    it("lets the search panel's Replace through: the person asked for that text", async () => {
      const { view, at } = await hit();
      view.dispatch({
        changes: { from: at, to: at + 4, insert: "Last" },
        userEvent: "input.replace",
      });
      expect(text(view)).toBe(DOC.replace("Next", "Last"));
    });
  });
});

describe("an edit a search hit in a fence has nothing to do with (R91)", () => {
  /** A hit in the fence, with the focus in another field of the page. */
  async function hitAndField() {
    const view = await drawn();
    const at = DOC.indexOf("Next step");
    view.dispatch({
      selection: { anchor: at, head: at + 4 },
      userEvent: "select.search",
    });
    await flush();
    const field = document.createElement("input");
    document.body.append(field);
    field.focus();
    await flush();
    return { view, at, field };
  }

  it.each([
    "input.type",
    "input.paste",
    "input.alt",
    "input.format",
    "delete.backward",
  ])(
    "%s elsewhere from another field passes, and the focus stays",
    async (event) => {
      const { view, field } = await hitAndField();
      view.dispatch({
        changes: { from: 0, to: 1, insert: "Z" },
        userEvent: event,
      });
      await flush();
      expect(text(view)).toBe(`Z${DOC.slice(1)}`);
      expect(active()).toBe(field);
      expect(noticeOf(view)).toBe("");
      field.remove();
    },
  );

  it("a verb writing elsewhere with main focused passes: only a letter goes astray", async () => {
    const view = await drawn();
    const at = DOC.indexOf("Next step");
    view.dispatch({
      selection: { anchor: at, head: at + 4 },
      userEvent: "select.search",
    });
    await flush();
    expect(active()).toBe(view.contentDOM);
    view.dispatch({
      changes: { from: 0, to: 1, insert: "Z" },
      userEvent: "input.format",
    });
    await flush();
    expect(text(view)).toBe(`Z${DOC.slice(1)}`);
    expect(active()).toBe(view.contentDOM);
  });

  it("an edit at the fence from that selection is still refused, wherever the focus is", async () => {
    const { view, at, field } = await hitAndField();
    view.dispatch({
      changes: { from: at, to: at + 4 },
      userEvent: "delete.cut",
    });
    await flush();
    expect(text(view)).toBe(DOC);
    field.remove();
  });

  it("the panel's Replace at the hit passes", async () => {
    const { view, at, field } = await hitAndField();
    view.dispatch({
      changes: { from: at, to: at + 4, insert: "Last" },
      userEvent: "input.replace",
    });
    expect(text(view)).toBe(DOC.replace("Next", "Last"));
    expect(active()).toBe(field);
    field.remove();
  });

  it("an undo and a peer's edit pass", async () => {
    const view = await drawn(DOC, { room: true });
    const at = DOC.indexOf("Next step");
    view.dispatch({
      selection: { anchor: at, head: at + 4 },
      userEvent: "select.search",
    });
    await flush();
    remoteInsert(view, 0, "P");
    expect(text(view)).toBe(`P${DOC}`);
  });

  it("an undo with the selection in the fence passes", async () => {
    const view = await drawn();
    view.contentDOM.blur();
    view.dispatch({
      changes: { from: 0, to: 1, insert: "Z" },
      userEvent: "input.type",
    });
    const at = DOC.indexOf("Next step");
    view.dispatch({
      selection: { anchor: at, head: at + 4 },
      userEvent: "select.search",
    });
    await flush();
    undoOnce(view);
    expect(text(view)).toBe(DOC);
  });
});

describe("a held key never crosses a boundary (R81)", () => {
  it("stops Backspace at the fence", async () => {
    const view = await drawn();
    setCaret(view, DOC.indexOf("Below"));
    pressMain(view, "Backspace", true);
    expect(text(view)).toBe(DOC);
    expect(active()).toBe(view.contentDOM);
    expect(view.state.field(diagramIntent)).toBeNull();
  });

  it("stops Delete at the fence from the line above", async () => {
    const view = await drawn();
    setCaret(view, 5);
    pressMain(view, "Delete", true);
    expect(text(view)).toBe(DOC);
    expect(active()).toBe(view.contentDOM);
  });

  it.each([
    ["ArrowDown", 2],
    ["ArrowRight", 5],
  ])("does not take %s from the text into the diagram", async (key, at) => {
    const view = await drawn();
    setCaret(view, at);
    pressMain(view, key, true);
    expect(active()).toBe(view.contentDOM);
    expect(view.state.selection.main.head).toBe(at);
  });

  it("goes in on a real press after a held one", async () => {
    const view = await drawn();
    setCaret(view, DOC.indexOf("Below"));
    pressMain(view, "Backspace", true);
    pressMain(view, "Backspace");
    expect(active()).toBe(box(view));
    expect(text(view)).toBe(DOC);
  });

  it.each(["Delete", "Backspace"])("does not fire %s on a box", async (key) => {
    const view = await drawn();
    setCaret(view, 2);
    keys(view, "ArrowDown");
    press("ArrowRight");
    press(key, { repeat: true });
    await flush();
    expect(text(view)).toBe(DOC);
    expect(active()?.getAttribute("aria-label")).toBe("Box: First step");
  });

  it("does not take the fence from the canvas", async () => {
    const view = await drawn();
    setCaret(view, 2);
    keys(view, "ArrowDown");
    press("Delete", { repeat: true });
    expect(text(view)).toBe(DOC);
  });

  it("leaves a selection a held key put in the fence to the key's release", async () => {
    const view = await drawn();
    setCaret(view, 2);
    pressMain(view, "PageDown", true);
    setCaret(view, DOC.indexOf("Next"), "select");
    await flush();
    expect(active()).toBe(view.contentDOM);
    view.contentDOM.dispatchEvent(
      new KeyboardEvent("keyup", { key: "PageDown", bubbles: true }),
    );
    await flush();
    expect(active()).toBe(box(view));
  });
});

describe("a press held after the canvas delete (N6)", () => {
  it("takes nothing more from the text, and a new press works again", async () => {
    const view = await drawn();
    setCaret(view, DOC.indexOf("Below"));
    keys(view, "Backspace");
    press("Backspace");
    expect(text(view)).toBe("Above\nBelow\n");
    await flush();
    expect(active()).toBe(view.contentDOM);
    setCaret(view, 5);
    pressMain(view, "Backspace", true);
    expect(text(view)).toBe("Above\nBelow\n");
    view.contentDOM.dispatchEvent(
      new KeyboardEvent("keyup", { key: "Backspace", bubbles: true }),
    );
    pressMain(view, "Backspace");
    expect(text(view)).toBe("Abov\nBelow\n");
  });
});

describe("a refused edit says so (R82)", () => {
  const SAID = NOTICES.breaksFence;

  it("Replace all with its matches only in a body passes: it names that text, and the verdict decides", async () => {
    const view = await drawn();
    const at = DOC.indexOf("Next");
    view.dispatch({
      changes: { from: at, to: at + 4, insert: "Last" },
      userEvent: "input.replace.all",
    });
    expect(text(view)).toBe(DOC.replace("Next", "Last"));
  });
  const REPLACED = `mermaid\nAbove\n${fence(BODY)}\nBelow\n`;

  function replaceAll(view: EditorView, doc: string) {
    const changes = [...doc.matchAll(/mermaid/g)].map((match) => ({
      from: match.index,
      to: match.index + 7,
      insert: "chart",
    }));
    view.dispatch({ changes, userEvent: "input.replace.all" });
  }

  it("Replace all over a fence's mark: nothing written, and a sentence", async () => {
    const view = await drawn(REPLACED);
    replaceAll(view, REPLACED);
    await flush();
    expect(text(view)).toBe(REPLACED);
    expect(noticeOf(view)).toBe(SAID);
  });

  it("a toolbar prefix over a fence: nothing written, and a sentence", async () => {
    const view = await drawn();
    const changes = [];
    for (let n = 1; n <= view.state.doc.lines; n += 1) {
      changes.push({ from: view.state.doc.line(n).from, insert: "> " });
    }
    view.dispatch({ changes, userEvent: "input" });
    await flush();
    expect(text(view)).toBe(DOC);
    expect(noticeOf(view)).toBe(SAID);
  });

  it("a paste over a selection up to the first mark: a sentence", async () => {
    const view = await drawn();
    view.dispatch({
      changes: { from: 2, to: FROM, insert: "x" },
      userEvent: "input.paste",
    });
    await flush();
    expect(text(view)).toBe(DOC);
    expect(noticeOf(view)).toBe(SAID);
  });

  it("stays silent for a lone key at the line's edge", async () => {
    const view = await drawn();
    setCaret(view, DOC.indexOf("Below"));
    keys(view, "Alt-ArrowUp");
    const end = DOC.lastIndexOf("```") + 3;
    setCaret(view, 5);
    view.dispatch({
      changes: { from: 5, to: FROM },
      userEvent: "delete.forward",
    });
    await flush();
    expect(text(view)).toBe(DOC);
    expect(end).toBeGreaterThan(FROM);
    expect(noticeOf(view)).toBe("");
  });

  const panelText = (view: EditorView) =>
    view.dom.querySelector(".cm-diagram-refusal")?.textContent;

  it("says it in a panel for a box not mounted, and does not scroll", async () => {
    const doc = `${"Line of text\n\n".repeat(8000)}${REPLACED}`;
    const view = markView(doc, modeOptions());
    await prefetchDiagrams();
    await vi.waitFor(() => {
      expect(view.state.facet(drawnFenceLines)).toHaveLength(1);
    });
    expect(view.dom.querySelector(".cm-diagram")).toBeNull();
    const dispatch = vi.spyOn(view, "dispatch");
    replaceAll(view, doc);
    await flush();
    expect(text(view)).toBe(doc);
    expect(panelText(view)).toBe(SAID);
    expect(
      dispatch.mock.calls
        .flat()
        .some((spec) => JSON.stringify(spec).includes("scrollIntoView")),
    ).toBe(false);
    expect(view.dom.querySelector(".cm-diagram")).toBeNull();
  });

  it("says it in a panel for a box mounted outside the window", async () => {
    const view = await drawn(REPLACED);
    const notice = box(view).querySelector(".cm-diagram-notice");
    vi.spyOn(notice as Element, "getBoundingClientRect").mockReturnValue(
      new DOMRect(0, window.innerHeight + 500, 100, 20),
    );
    replaceAll(view, REPLACED);
    await flush();
    expect(panelText(view)).toBe(SAID);
    expect(noticeOf(view)).toBe("");
  });

  it("takes the panel away after the notice's time", async () => {
    const view = await drawn(REPLACED);
    const notice = box(view).querySelector(".cm-diagram-notice");
    vi.spyOn(notice as Element, "getBoundingClientRect").mockReturnValue(
      new DOMRect(0, -500, 100, 20),
    );
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });
    replaceAll(view, REPLACED);
    await vi.advanceTimersByTimeAsync(0);
    expect(panelText(view)).toBe(SAID);
    await vi.advanceTimersByTimeAsync(NOTICE_MS);
    expect(panelText(view)).toBeUndefined();
    vi.useRealTimers();
  });

  it("announces each refusal in the panel, a second one again (R92)", async () => {
    const heard: string[] = [];
    const view = await drawn(REPLACED);
    const { StateEffect } = await import("@codemirror/state");
    view.dispatch({
      effects: StateEffect.appendConfig.of(
        EditorView.updateListener.of((update) => {
          for (const tr of update.transactions) {
            for (const effect of tr.effects) {
              if (effect.is(EditorView.announce)) {
                heard.push(effect.value);
              }
            }
          }
        }),
      ),
    });
    const notice = box(view).querySelector(".cm-diagram-notice");
    vi.spyOn(notice as Element, "getBoundingClientRect").mockReturnValue(
      new DOMRect(0, window.innerHeight + 500, 100, 20),
    );
    replaceAll(view, REPLACED);
    await flush();
    expect(heard).toEqual([SAID]);
    replaceAll(view, REPLACED);
    await flush();
    expect(heard).toEqual([SAID, SAID]);
    expect(
      view.dom.querySelectorAll(".cm-announced > div").length,
    ).toBeGreaterThan(0);
  });

  it("refuses a Replace that would close the fence early (N5)", async () => {
    const view = await drawn();
    const at = DOC.indexOf("Next");
    view.dispatch({
      changes: { from: at, to: at + 4, insert: "x]\n```\n" },
      userEvent: "input.replace",
    });
    await flush();
    expect(text(view)).toBe(DOC);
    expect(noticeOf(view)).toBe(SAID);
  });
});

describe("after a canvas delete, the next letter is on a line of its own (R83)", () => {
  const CODE = "```js\nx\n```\n";
  const OTHER = "flowchart TD\n    C --> D";

  async function deleteAndType(
    view: EditorView,
    target: HTMLElement,
    expected: string,
    doc: string,
  ) {
    target.focus();
    await flush();
    press("Delete");
    await vi.waitFor(() => {
      expect(active()).toBe(view.contentDOM);
    });
    typeText(view, "Q");
    expect(text(view)).toBe(expected);
    undoOnce(view);
    undoOnce(view);
    expect(text(view)).toBe(doc);
  }

  it.each([
    [
      "the fence first, a code block below",
      `${fence(BODY)}\n${CODE}`,
      `Q\n${CODE}`,
    ],
    [
      "frontmatter with no blank line, a code block below",
      `---\ntype: x\n---\n${fence(BODY)}\n${CODE}`,
      `---\ntype: x\n---\nQ\n${CODE}`,
    ],
    [
      "a table above, no final line break",
      `| a | b |\n|---|---|\n| 1 | 2 |\n${fence(BODY)}`,
      `| a | b |\n|---|---|\n| 1 | 2 |\nQ`,
    ],
    [
      "a table below",
      `Top\n${fence(BODY)}\n| a |\n|---|\n| 1 |\n`,
      `Top\nQ\n| a |\n|---|\n| 1 |\n`,
    ],
    ["a rule below", `Top\n${fence(BODY)}\n---\nEnd\n`, `Top\nQ\n---\nEnd\n`],
    ["math below", `Top\n${fence(BODY)}\n$$\nx\n$$\n`, `Top\nQ\n$$\nx\n$$\n`],
    [
      "an HTML block below",
      `Top\n${fence(BODY)}\n<div>\nx\n</div>\n`,
      `Top\nQ\n<div>\nx\n</div>\n`,
    ],
    ["a quote below", `Top\n${fence(BODY)}\n> q\n`, `Top\nQ\n> q\n`],
    ["a heading below", `Top\n${fence(BODY)}\n## H\n`, `Top\nQ\n## H\n`],
    [
      "a horizontal rule above, no final line break",
      `Top\n\n---\n${fence(BODY)}`,
      `Top\n\n---\nQ`,
    ],
    [
      "frontmatter above, no final line break",
      `---\ntype: x\n---\n${fence(BODY)}`,
      `---\ntype: x\n---\nQ`,
    ],
  ])("%s", async (_, doc, expected) => {
    const view = await drawn(doc);
    await deleteAndType(view, box(view), expected, doc);
  });

  it("a Reflection id line above, a code block below", async () => {
    const doc = `## Head\n<!-- r:0a1b2c3d -->\n${fence(BODY)}\n${CODE}`;
    const view = await drawn(doc, { adapter: "reflection" });
    await deleteAndType(
      view,
      box(view),
      `## Head\n<!-- r:0a1b2c3d -->\nQ\n${CODE}`,
      doc,
    );
  });

  it("another drawn fence right above, no final line break", async () => {
    const doc = `Top\n${fence(BODY)}\n${fence(OTHER)}`;
    const view = await drawn(doc);
    await vi.waitFor(() => {
      expect(view.dom.querySelectorAll(".cm-diagram svg")).toHaveLength(2);
    });
    const second = view.dom.querySelectorAll<HTMLElement>(".cm-diagram")[1];
    if (second === undefined) throw new Error("no second box");
    await deleteAndType(view, second, `Top\n${fence(BODY)}\nQ`, doc);
  });

  it.each(["===", "--", "  ==="])(
    "a paragraph line %j below, which would make the line above a heading",
    async (below) => {
      const doc = `Above\n${fence(BODY)}\n${below}\n`;
      const view = await drawn(doc);
      await deleteAndType(view, box(view), `Above\nQ\n${below}\n`, doc);
    },
  );

  it("a table's delimiter line below, which would make the line above a table (M2)", async () => {
    const doc = `a | b\n${fence(BODY)}\n---|---\n`;
    const view = await drawn(doc);
    await deleteAndType(view, box(view), "a | b\nQ\n---|---\n", doc);
  });

  it.each([["- item"], ["> quote"]])(
    "a paragraph below %j, which would run into it with no byte changed (N7)",
    async (above) => {
      const doc = `${above}\n${fence(BODY)}\nfoo\n`;
      const view = await drawn(doc);
      await deleteAndType(view, box(view), `${above}\nQ\nfoo\n`, doc);
    },
  );

  it("a paragraph below a table's last row, which would become a row (N7)", async () => {
    const table = "| a | b |\n|---|---|\n| 1 | 2 |";
    const view = await drawn(`${table}\n${fence(BODY)}\nfoo\n`);
    expect(removeFence(view, `${table}\n`.length)).toEqual({ applied: true });
    expect(text(view)).toBe(`${table}\n\nfoo\n`);
  });

  it("an ordinary paragraph below joins as before, with `===` harmless under a heading", async () => {
    const doc = `# Head\n${fence(BODY)}\n===\n`;
    const view = await drawn(doc);
    await deleteAndType(view, box(view), "# Head\nQ===\n", doc);
  });

  it("a plain line above, no final line break", async () => {
    const doc = `Above\n${fence(BODY)}`;
    const view = await drawn(doc);
    await deleteAndType(view, box(view), "Above\nQ", doc);
  });
});

describe("an arrow as a stop (R84)", () => {
  const DUPLICATES =
    "flowchart LR\n    A -->|one| B\n    A -->|two| B\n    A -->|three| B";

  it.each(['[data-hit-for="L_A_B_0"]', 'g.label[data-id="L_A_B_0"]'])(
    "a click on %s selects that arrow",
    async (selector) => {
      const view = await drawn();
      box(view)
        .querySelector(selector)
        ?.dispatchEvent(new MouseEvent("click", { bubbles: true }));
      expect(view.state.field(diagramIntent)?.target).toEqual({
        kind: "edge",
        key: { start: "A", end: "B", occurrence: 0 },
      });
      expect(active()?.getAttribute("aria-label")).toBe(
        "Arrow from First step to Next step",
      );
    },
  );

  it.each([
    [1, "A -->|two| B", "A -->|one| B\n    A -->|three| B"],
    [2, "A -->|three| B", "A -->|one| B\n    A -->|two| B"],
  ])(
    "Delete on occurrence %i of a duplicate takes that one, by its bytes",
    async (occurrence, _, rest) => {
      const doc = `Above\n${fence(DUPLICATES)}\nBelow\n`;
      const view = await drawn(doc);
      box(view).focus();
      await flush();
      press("End");
      await flush();
      if (occurrence === 1) {
        press("ArrowLeft");
        await flush();
      }
      expect(view.state.field(diagramIntent)?.target).toEqual({
        kind: "edge",
        key: { start: "A", end: "B", occurrence },
      });
      press("Delete");
      await settled(
        view,
        `Above\n${fence(`flowchart LR\n    ${rest}`)}\nBelow\n`,
      );
      undoOnce(view);
      expect(text(view)).toBe(doc);
    },
  );
});

describe("a click on one of duplicate arrows", () => {
  const DUPLICATES =
    "flowchart LR\n    A -->|one| B\n    A -->|two| B\n    A -->|three| B";

  it.each([
    ['g.label[data-id="L_A_B_2"]', 1, "A -->|one| B\n    A -->|three| B"],
    ['[data-hit-for="L_A_B_2"]', 1, "A -->|one| B\n    A -->|three| B"],
    ['g.label[data-id="L_A_B_3"]', 2, "A -->|one| B\n    A -->|two| B"],
    ['[data-hit-for="L_A_B_3"]', 2, "A -->|one| B\n    A -->|two| B"],
  ])(
    "%s holds occurrence %i, and Delete takes that one",
    async (selector, occurrence, rest) => {
      const doc = `Above\n${fence(DUPLICATES)}\nBelow\n`;
      const view = await drawn(doc);
      box(view)
        .querySelector(selector)
        ?.dispatchEvent(new MouseEvent("click", { bubbles: true }));
      await flush();
      expect(view.state.field(diagramIntent)?.target).toEqual({
        kind: "edge",
        key: { start: "A", end: "B", occurrence },
      });
      press("Delete");
      await settled(
        view,
        `Above\n${fence(`flowchart LR\n    ${rest}`)}\nBelow\n`,
      );
    },
  );
});

describe("what a refusal reports (R85)", () => {
  async function refusing() {
    const view = await drawn();
    const { EditorState, StateEffect } = await import("@codemirror/state");
    view.dispatch({
      effects: StateEffect.appendConfig.of(
        EditorState.transactionFilter.of((tr) =>
          tr.isUserEvent("delete.diagram") || tr.isUserEvent("input.diagram")
            ? []
            : tr,
        ),
      ),
    });
    return view;
  }

  it("removeFence does not say applied when a filter stopped it", async () => {
    const view = await refusing();
    expect(removeFence(view, FROM)).toEqual({
      applied: false,
      refused: "invalid",
    });
    expect(text(view)).toBe(DOC);
  });

  it("an op does not say applied when a filter stopped it", async () => {
    const view = await refusing();
    expect(
      await runDiagramOp(view, FROM, flowKind, { op: "deleteNode", id: "B" }),
    ).toEqual({ applied: false, refused: "invalid" });
    expect(text(view)).toBe(DOC);
  });

  it("a canvas delete goes through with main's caret on a hidden code fence line", async () => {
    const code = "```js\nx\n```\n";
    const doc = `Top\n${fence(BODY)}\n\n${code}`;
    const view = await drawn(doc);
    box(view).focus();
    await flush();
    // A peer's edit can leave it there; a person's caret is steered off it.
    view.dispatch({ selection: { anchor: doc.indexOf("```js") } });
    expect(view.state.selection.main.head).toBe(doc.indexOf("```js"));
    press("Delete");
    expect(text(view)).toBe(`Top\n\n${code}`);
  });
});

describe("a paste over exactly the fence (R86)", () => {
  it("passes like one over a larger selection", async () => {
    const view = await drawn();
    view.dispatch({
      changes: {
        from: FROM,
        to: DOC.lastIndexOf("```") + 3,
        insert: "pasted",
      },
      userEvent: "input.paste",
    });
    expect(text(view)).toBe("Above\npasted\nBelow\n");
  });
});

describe("for a screen reader (R87)", () => {
  it("takes Space on a stop as it takes Enter, and keeps the page from scrolling", async () => {
    const rename = vi.fn();
    registerStopAction("rename", rename);
    const view = await drawn();
    setCaret(view, 2);
    keys(view, "ArrowDown");
    press("ArrowRight");
    const event = new KeyboardEvent("keydown", {
      key: " ",
      bubbles: true,
      cancelable: true,
    });
    active()?.dispatchEvent(event);
    expect(event.defaultPrevented).toBe(true);
    expect(rename).toHaveBeenCalledTimes(1);
  });

  it("says a box and an arrow open a menu", async () => {
    const view = await drawn();
    expect(
      box(view).querySelector("g.node")?.getAttribute("aria-haspopup"),
    ).toBe("menu");
    expect(
      box(view)
        .querySelector('path[data-et="edge"]')
        ?.getAttribute("aria-haspopup"),
    ).toBe("menu");
  });

  it("says no menu on a page that takes no writes", async () => {
    const view = await drawn();
    const { EditorState, StateEffect } = await import("@codemirror/state");
    view.dispatch({
      effects: StateEffect.appendConfig.of(EditorState.readOnly.of(true)),
    });
    await flush();
    expect(box(view).querySelectorAll("[aria-haspopup]")).toHaveLength(0);
  });

  it("reads a subgraph's title on an arrow, not its id", async () => {
    const body =
      "flowchart TD\n    Start[Begin] --> sub\n    subgraph sub[Group]\n    B[Inner]\n    end";
    const view = await drawn(`Above\n${fence(body)}\nBelow\n`);
    await vi.waitFor(() => {
      expect(
        box(view)
          .querySelector('path[data-et="edge"]')
          ?.getAttribute("aria-label"),
      ).toBe("Arrow from Begin to Group");
    });
  });
});

describe("a sequence diagram", () => {
  const STARTER =
    "sequenceDiagram\n    participant caller as Caller\n    participant service as Service\n    caller->>service: Request\n    service-->>caller: Reply";
  const SEQ_DOC = `Above\n${fence(STARTER)}\nBelow\n`;
  const SEQ_FROM = SEQ_DOC.indexOf("```");

  beforeEach(() => {
    // What Enter opens is the real field here.
    registerStopAction("rename", (main, anchor, target) => {
      openLabelEditor(main, anchor, target, { selectAll: true });
    });
  });

  const menus = () => [...document.querySelectorAll('[role="menu"]')];
  const field = (view: EditorView) =>
    view.dom.querySelector<HTMLTextAreaElement>(".cm-diagram-overlay textarea");

  it("is one group named Sequence diagram", async () => {
    const view = await drawn(SEQ_DOC);
    expect(box(view).getAttribute("aria-label")).toBe("Sequence diagram");
  });

  it("walks the canvas, the participants and the messages, by their names", async () => {
    const view = await drawn(SEQ_DOC);
    focusTarget(view, SEQ_FROM, { kind: "canvas" });
    const names: (string | null | undefined)[] = [];
    for (let i = 0; i < 4; i++) {
      press("ArrowRight");
      names.push(active()?.getAttribute("aria-label"));
    }
    expect(names).toEqual([
      "Participant: Caller",
      "Participant: Service",
      "Message from Caller to Service: Request",
      "Message from Service to Caller: Reply",
    ]);
    press("ArrowRight");
    expect(active()?.getAttribute("aria-label")).toBe(
      "Message from Service to Caller: Reply",
    );
  });

  it("Shift+F10 on a message opens Message actions", async () => {
    const view = await drawn(SEQ_DOC);
    focusTarget(view, SEQ_FROM, {
      kind: "message",
      from: "caller",
      to: "service",
      text: "Request",
      occurrence: 0,
    });
    press("F10", { shiftKey: true });
    expect(menus()).toHaveLength(1);
    expect(menus()[0]?.getAttribute("aria-label")).toBe("Message actions");
    expect(active()?.textContent).toBe("Edit text");
  });

  it("Enter on a participant opens Participant name with its label", async () => {
    const view = await drawn(SEQ_DOC);
    focusTarget(view, SEQ_FROM, { kind: "participant", name: "service" });
    press("Enter");
    await flush();
    const opened = field(view);
    expect(opened?.getAttribute("aria-label")).toBe("Participant name");
    expect(opened?.value).toBe("Service");
    expect(active()).toBe(opened);
    expect(text(view)).toBe(SEQ_DOC);
  });

  it("Delete on a message takes that line as one undo step", async () => {
    const view = await drawn(SEQ_DOC);
    focusTarget(view, SEQ_FROM, {
      kind: "message",
      from: "service",
      to: "caller",
      text: "Reply",
      occurrence: 0,
    });
    press("Delete");
    const after = SEQ_DOC.replace("\n    service-->>caller: Reply", "");
    await settled(view, after);
    undoOnce(view);
    expect(text(view)).toBe(SEQ_DOC);
  });

  it("a click on a message's text puts the stop on that message", async () => {
    const view = await drawn(SEQ_DOC);
    const texts = [...box(view).querySelectorAll(".messageText")];
    texts[1]?.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await flush();
    expect(view.state.field(diagramIntent)?.target).toEqual({
      kind: "message",
      from: "service",
      to: "caller",
      text: "Reply",
      occurrence: 0,
    });
  });
});
