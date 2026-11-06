/**
 * The rename field, on the real surface and the real verdict, with only mermaid's
 * render stubbed: solo and in a room. `labelEditor.marks-hidden.test.ts` and
 * `labelEditor.marks-show.test.ts` run this file again.
 */

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { EditorState } from "@codemirror/state";
import { EditorView } from "@codemirror/view";

import { cellMarksMode } from "../../test/cellMarksMode";
import type { MarkViewOptions } from "../../test/markKit";
import {
  bytes,
  cleanupMarkViews,
  markView,
  setCaret,
  setRaw,
  typeText,
  undoOnce,
  undoSteps,
} from "../../test/markKit";
import { mainSelectionParked } from "../parkedSelection";
import { mermaidFence, MERMAID_STARTER_GROUPS } from "../mermaidStarters";
import { insertBlock, selectToken } from "../toolbar";
import { drawnFenceLines, settleFieldDraft } from "./draws";
import { NOTICES } from "./edits";
import { flowKind, runDiagramOp } from "./apply";
import { parseFlowchart } from "./flowchart/parse";
import { focusTarget } from "./focus";
import { diagramField, pointerDown, showFenceSource } from "./field";
import { diagramIntent } from "./intent";
import {
  addWithName,
  cancelLabelEditor,
  commitLabelEditor,
  openLabelEditor,
} from "./labelEditor";
import { prefetchDiagrams } from "./loader";
import { requestVerdict } from "./verdict";
import { forgetDrawings, NOTICE_MS } from "./widget";

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
              `<path d="M0,0L1,1" data-et="edge" data-id="${mermaidId}"></path>` +
              `<g class="label" data-id="${mermaidId}"></g>`,
          )
          .join("");
        return {
          svg: `<svg id="${id}"><g class="edgePaths">${edges}</g><g class="nodes">${nodes}</g></svg>`,
        };
      },
    },
  };
});

const BODY = "flowchart TD\n    A[First step] --> B[Next step]";
const fence = (body: string) => `\`\`\`mermaid\n${body}\n\`\`\``;
const DOC = `Above\n${fence(BODY)}\nBelow\n`;
const FROM = DOC.indexOf("```");
const withB = (label: string) =>
  `Above\n${fence(`flowchart TD\n    A[First step] --> B[${label}]`)}\nBelow\n`;

const NODE_B = { kind: "node", id: "B" } as const;

const noticeOf = (view: EditorView) =>
  view.dom.querySelector(".cm-diagram-notice")?.textContent ?? "";

const flush = async () => {
  await new Promise((resolve) => setTimeout(resolve, 0));
};

function modeOptions(): MarkViewOptions {
  const mode = cellMarksMode();
  return mode === "off" ? {} : { mode };
}

async function drawn(doc = DOC, options: MarkViewOptions = {}) {
  const view = markView(doc, { ...modeOptions(), ...options });
  await prefetchDiagrams();
  await vi.waitFor(() => {
    expect(view.dom.querySelector(".cm-diagram svg g.node")).not.toBeNull();
  });
  await flush();
  view.contentDOM.focus();
  return view;
}

const text = (view: EditorView) => bytes(view.state);
const active = () => document.activeElement;

function nodeOf(view: EditorView, id: string): Element {
  const found = view.dom.querySelector(`g.node[id*="flowchart-${id}-"]`);
  if (found === null) throw new Error(`no box ${id}`);
  return found;
}

function clickNode(view: EditorView, id: string): void {
  nodeOf(view, id).dispatchEvent(new MouseEvent("click", { bubbles: true }));
}

function fieldOf(view: EditorView): HTMLTextAreaElement {
  const found = view.dom.querySelector<HTMLTextAreaElement>(
    ".cm-diagram-overlay textarea",
  );
  if (found === null) throw new Error("no field");
  return found;
}

const noField = (view: EditorView) =>
  view.dom.querySelector(".cm-diagram-overlay textarea") === null;

function type(field: HTMLTextAreaElement, value: string): void {
  field.value = value;
  field.dispatchEvent(new Event("input", { bubbles: true }));
}

function press(
  el: Element | null,
  key: string,
  options: KeyboardEventInit = {},
): KeyboardEvent {
  const event = new KeyboardEvent("keydown", {
    key,
    bubbles: true,
    cancelable: true,
    ...options,
  });
  el?.dispatchEvent(event);
  return event;
}

async function closed(view: EditorView): Promise<void> {
  await vi.waitFor(() => {
    expect(noField(view)).toBe(true);
  });
  await flush();
}

async function rename(view: EditorView, id: string, value: string) {
  clickNode(view, id);
  const field = fieldOf(view);
  type(field, value);
  press(field, "Enter");
  await closed(view);
}

beforeEach(async () => {
  forgetDrawings();
  // The last test's view went in `afterEach`, and a draft it held said so after.
  await flush();
  for (const note of document.querySelectorAll(".cm-diagram-lost-draft")) {
    note.remove();
  }
});

afterEach(() => {
  cleanupMarkViews();
  vi.useRealTimers();
  document.body.replaceChildren();
});

describe.each<[string, MarkViewOptions]>([
  ["solo", {}],
  ["in a room", { room: true }],
])("the field %s", (_name, roomOptions) => {
  const open = (doc = DOC, options: MarkViewOptions = {}) =>
    drawn(doc, { ...roomOptions, ...options });

  it("writes a click, a name and Enter as one undo step, and the focus goes back to the box", async () => {
    const view = await open();
    const steps = undoSteps(view);
    await rename(view, "B", "Done");
    expect(text(view)).toBe(withB("Done"));
    expect(undoSteps(view)).toBe(steps + 1);
    expect(active()?.getAttribute("aria-label")).toBe("Box: Done");
    undoOnce(view);
    expect(text(view)).toBe(DOC);
  });

  it("names itself, shows the label selected and stands over the box", async () => {
    const view = await open();
    clickNode(view, "B");
    const field = fieldOf(view);
    await flush();
    expect(view.dom.querySelector(".cm-diagram-focus-ring")).toBeNull();
    expect(field.getAttribute("aria-label")).toBe("Box name");
    expect(field.value).toBe("Next step");
    expect(active()).toBe(field);
    expect([field.selectionStart, field.selectionEnd]).toEqual([0, 9]);
    expect(view.state.field(diagramIntent)).toMatchObject({
      target: NODE_B,
      mode: "rename",
    });
  });

  it("opens on an arrow, empty when it has no label, and writes the label", async () => {
    const view = await open();
    view.dom
      .querySelector('path[data-et="edge"]')
      ?.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    const field = fieldOf(view);
    expect(field.getAttribute("aria-label")).toBe("Arrow label");
    expect(field.value).toBe("");
    type(field, "yes");
    press(field, "Enter");
    await closed(view);
    expect(text(view)).toContain("A[First step] -->|yes| B[Next step]");
  });

  it("takes an arrow's label away when it is emptied", async () => {
    const doc = `Above\n${fence("flowchart TD\n    A -->|yes| B")}\nBelow\n`;
    const view = await open(doc);
    view.dom
      .querySelector('path[data-et="edge"]')
      ?.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    const field = fieldOf(view);
    expect(field.value).toBe("yes");
    type(field, "");
    press(field, "Enter");
    await closed(view);
    expect(text(view)).toContain("    A --> B");
  });

  it("says so at once on an arrow drawn with others, which takes no label of its own", async () => {
    const doc = `Above\n${fence("flowchart TD\n    A & B --> C")}\nBelow\n`;
    const view = await open(doc);
    view.dom
      .querySelector('path[data-et="edge"]')
      ?.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await flush();
    expect(noField(view)).toBe(true);
    expect(view.dom.querySelector(".cm-diagram-notice")?.textContent).toBe(
      NOTICES.group,
    );
  });

  it("shows a bare box's id, and writes a label for it", async () => {
    const doc = `Above\n${fence("flowchart TD\n    A --> B")}\nBelow\n`;
    const view = await open(doc);
    clickNode(view, "B");
    const field = fieldOf(view);
    expect(field.value).toBe("B");
    type(field, "Named");
    press(field, "Enter");
    await closed(view);
    expect(text(view)).toContain("A --> B[Named]");
  });

  it("cancels on Escape with no bytes and no step, and stays on the box", async () => {
    const view = await open();
    const steps = undoSteps(view);
    clickNode(view, "B");
    const field = fieldOf(view);
    type(field, "Done");
    press(field, "Escape");
    await closed(view);
    expect(text(view)).toBe(DOC);
    expect(undoSteps(view)).toBe(steps);
    expect(active()?.getAttribute("aria-label")).toBe("Box: Next step");
    expect(view.state.field(diagramIntent)?.mode).toBe("focus");
  });

  it("writes Shift+Enter as a line break", async () => {
    const view = await open();
    clickNode(view, "B");
    const field = fieldOf(view);
    type(field, "ab");
    field.setSelectionRange(1, 1);
    const event = press(field, "Enter", { shiftKey: true });
    expect(event.defaultPrevented).toBe(true);
    expect(field.value).toBe("a\nb");
    expect(field.selectionStart).toBe(2);
    expect(noField(view)).toBe(false);
    press(field, "Enter");
    await closed(view);
    expect(text(view)).toBe(withB("a<br>b"));
  });

  it("commits on a blur", async () => {
    const view = await open();
    const other = document.createElement("button");
    document.body.append(other);
    clickNode(view, "B");
    type(fieldOf(view), "Done");
    other.focus();
    await closed(view);
    expect(text(view)).toBe(withB("Done"));
    expect(active()).toBe(other);
    expect(view.state.field(diagramIntent)).toBeNull();
  });

  it("dispatches nothing for an unchanged text", async () => {
    const view = await open();
    const steps = undoSteps(view);
    clickNode(view, "B");
    const field = fieldOf(view);
    press(field, "Enter");
    await closed(view);
    expect(text(view)).toBe(DOC);
    expect(undoSteps(view)).toBe(steps);
  });

  it("cancels an empty box name, spaces too", async () => {
    const view = await open();
    const steps = undoSteps(view);
    clickNode(view, "B");
    const field = fieldOf(view);
    type(field, "  \n ");
    press(field, "Enter");
    await closed(view);
    expect(text(view)).toBe(DOC);
    expect(undoSteps(view)).toBe(steps);
  });

  it("add child, a name and Enter are two undo steps back to the exact bytes", async () => {
    const view = await open();
    const steps = undoSteps(view);
    await addWithName(view, FROM, { op: "addChild", id: "B" });
    const field = fieldOf(view);
    expect(field.value).toBe("New box");
    expect(active()).toBe(field);
    expect([field.selectionStart, field.selectionEnd]).toEqual([0, 7]);
    const added = text(view);
    expect(added).toContain("[New box]");
    type(field, "Check");
    press(field, "Enter");
    await closed(view);
    expect(text(view)).toBe(added.replace("New box", "Check"));
    expect(undoSteps(view)).toBe(steps + 2);
    undoOnce(view);
    expect(text(view)).toBe(added);
    undoOnce(view);
    expect(text(view)).toBe(DOC);
  });

  it.each(["addSibling", "addBox"] as const)(
    "%s opens the field on what it made",
    async (op) => {
      const doc = `Above\n${fence("flowchart TD\n    A[Root] --> B[Next step]")}\nBelow\n`;
      const view = await open(doc);
      await addWithName(view, FROM, op === "addBox" ? { op } : { op, id: "B" });
      expect(fieldOf(view).value).toBe("New box");
    },
  );

  it.each(["Enter", "F2", " "])(
    "opens with its label on %j on a focused box",
    async (key) => {
      const view = await open();
      focusTarget(view, FROM, NODE_B);
      press(active(), key);
      const field = fieldOf(view);
      expect(field.value).toBe("Next step");
      expect(active()).toBe(field);
    },
  );

  it("opens on a starter once its verdict settles, and typing goes into the field", async () => {
    const view = markView("Intro\n", { ...modeOptions(), ...roomOptions });
    await prefetchDiagrams();
    view.contentDOM.focus();
    const starter = MERMAID_STARTER_GROUPS[0]?.starters[0];
    if (starter === undefined) throw new Error("no starter");
    setCaret(view, 5);
    const { lines, select } = mermaidFence(starter);
    insertBlock(view, lines, select);
    await vi.waitFor(() => {
      expect(noField(view)).toBe(false);
    });
    const field = fieldOf(view);
    expect(field.value).toBe("First step");
    expect([field.selectionStart, field.selectionEnd]).toEqual([0, 10]);
    expect(active()).toBe(field);
    expect(view.state.facet(drawnFenceLines)).toHaveLength(1);
    type(field, "Go");
    press(field, "Enter");
    await closed(view);
    expect(text(view)).toContain("    A[Go] --> B[Next step]");
    undoOnce(view);
    expect(text(view)).toContain("    A[First step] --> B[Next step]");
    undoOnce(view);
    expect(text(view)).toBe("Intro\n");
  });

  it("leaves a starter as text when the person typed over the selection first", async () => {
    const view = markView("Intro\n", { ...modeOptions(), ...roomOptions });
    await prefetchDiagrams();
    view.contentDOM.focus();
    const starter = MERMAID_STARTER_GROUPS[0]?.starters[0];
    if (starter === undefined) throw new Error("no starter");
    setCaret(view, 5);
    const { lines, select } = mermaidFence(starter);
    insertBlock(view, lines, select);
    typeText(view, "Go");
    const written = text(view);
    await flush();
    await new Promise((resolve) => setTimeout(resolve, 50));
    expect(noField(view)).toBe(true);
    expect(view.state.facet(drawnFenceLines)).toHaveLength(0);
    typeText(view, "x");
    expect(text(view)).not.toBe(written);
  });

  it("commits the draft when Raw goes on, with the focus in the main view", async () => {
    const view = await open();
    clickNode(view, "B");
    type(fieldOf(view), "New");
    setRaw(view, true);
    await vi.waitFor(() => {
      expect(text(view)).toBe(withB("New"));
    });
    await flush();
    expect(active()).toBe(view.contentDOM);
  });

  it("drops the draft on setState", async () => {
    const view = await open();
    clickNode(view, "B");
    type(fieldOf(view), "New");
    view.setState(EditorState.create({ doc: "Other text" }));
    await flush();
    await new Promise((resolve) => setTimeout(resolve, 20));
    expect(view.state.doc.toString()).toBe("Other text");
    expect(noField(view)).toBe(true);
  });

  it("does not write a draft into the document a setState brought, even the same text", async () => {
    const view = await open();
    clickNode(view, "B");
    type(fieldOf(view), "New");
    const other = markView(DOC, { ...modeOptions(), ...roomOptions });
    await flush();
    view.setState(other.state);
    await new Promise((resolve) => setTimeout(resolve, 50));
    expect(view.state.doc.toString()).toBe(DOC);
    expect(noField(view)).toBe(true);
  });

  it("waits for the verdict of a starter it has not seen", async () => {
    const view = markView("Intro\n", { ...modeOptions(), ...roomOptions });
    await prefetchDiagrams();
    view.contentDOM.focus();
    setCaret(view, 5);
    const word = `Fresh ${roomOptions.room === true ? "room" : "solo"} ${cellMarksMode()}`;
    const lines = [
      "```mermaid",
      "flowchart TD",
      `    A[${word}] --> B[Next step]`,
      "```",
    ];
    insertBlock(view, lines, selectToken(lines, word));
    await Promise.resolve();
    await Promise.resolve();
    expect(noField(view)).toBe(true);
    expect(view.state.facet(drawnFenceLines)).toHaveLength(0);
    await vi.waitFor(() => {
      expect(fieldOf(view).value).toBe(word);
    });
  });

  it("stays open with its draft and says why on a refused text, and Escape then writes nothing", async () => {
    const view = await open();
    clickNode(view, "B");
    const field = fieldOf(view);
    type(field, "a %%{ b");
    press(field, "Enter");
    await vi.waitFor(() => {
      expect(noticeOf(view)).toContain(NOTICES.invalid);
    });
    expect(fieldOf(view)).toBe(field);
    expect(field.value).toBe("a %%{ b");
    expect(field.readOnly).toBe(false);
    expect(active()).toBe(field);
    press(field, "Escape");
    await closed(view);
    expect(text(view)).toBe(DOC);
  });

  it("keeps a refused blur's draft, and the person stays in the text", async () => {
    const view = await open();
    clickNode(view, "B");
    const field = fieldOf(view);
    type(field, "a %%{ b");
    view.contentDOM.focus();
    await vi.waitFor(() => {
      expect(noticeOf(view)).toContain(NOTICES.invalid);
    });
    expect(fieldOf(view)).toBe(field);
    expect(active()).toBe(view.contentDOM);
    expect(text(view)).toBe(DOC);
  });

  it("takes the draft along when another box is clicked, and opens there", async () => {
    const view = await open();
    clickNode(view, "B");
    type(fieldOf(view), "Done");
    clickNode(view, "A");
    await vi.waitFor(() => {
      expect(fieldOf(view).value).toBe("First step");
    });
    expect(text(view)).toBe(withB("Done"));
  });

  it("keeps the field for a click on its own box", async () => {
    const view = await open();
    clickNode(view, "B");
    const field = fieldOf(view);
    type(field, "Dra");
    clickNode(view, "B");
    await flush();
    expect(fieldOf(view)).toBe(field);
    expect(field.value).toBe("Dra");
    expect(text(view)).toBe(DOC);
  });

  it("commits a press in the text, and the caret there is no parked one", async () => {
    const view = await open();
    clickNode(view, "B");
    type(fieldOf(view), "Done");
    const line = view.contentDOM.querySelector(".cm-line");
    line?.dispatchEvent(
      new MouseEvent("mousedown", { bubbles: true, cancelable: true }),
    );
    expect(view.state.field(diagramIntent)).toBeNull();
    expect(view.state.facet(mainSelectionParked)).toBe(false);
    setCaret(view, 2, "select");
    view.contentDOM.focus();
    await closed(view);
    expect(text(view)).toBe(withB("Done"));
    expect(view.state.field(diagramIntent)).toBeNull();
    expect(active()).toBe(view.contentDOM);
  });

  it("takes no more typing while its text is being checked, and one Enter is one write", async () => {
    const view = await open();
    const steps = undoSteps(view);
    clickNode(view, "B");
    const field = fieldOf(view);
    type(field, "Done");
    press(field, "Enter");
    expect(field.readOnly).toBe(true);
    press(field, "Enter");
    await closed(view);
    expect(undoSteps(view)).toBe(steps + 1);
    expect(text(view)).toBe(withB("Done"));
  });

  it("offers nothing on a page that takes no writes", async () => {
    const view = await open();
    const { StateEffect } = await import("@codemirror/state");
    view.dispatch({
      effects: StateEffect.appendConfig.of(EditorState.readOnly.of(true)),
    });
    clickNode(view, "B");
    await flush();
    expect(noField(view)).toBe(true);
    expect(active()?.getAttribute("aria-label")).toBe("Box: Next step");
    openLabelEditor(view, FROM, NODE_B, { selectAll: true });
    expect(noField(view)).toBe(true);
  });

  it("closes with no bytes when the page stops taking writes under it", async () => {
    const view = await open();
    clickNode(view, "B");
    type(fieldOf(view), "Done");
    const { StateEffect } = await import("@codemirror/state");
    view.dispatch({
      effects: StateEffect.appendConfig.of(EditorState.readOnly.of(true)),
    });
    await closed(view);
    expect(text(view)).toBe(DOC);
  });

  it("does not move a composition on Enter or Escape", async () => {
    const view = await open();
    clickNode(view, "B");
    const field = fieldOf(view);
    field.dispatchEvent(new CompositionEvent("compositionstart"));
    type(field, "가");
    press(field, "Enter", { isComposing: true });
    press(field, "Escape", { isComposing: true });
    press(field, "Enter", { keyCode: 229 });
    await flush();
    expect(fieldOf(view)).toBe(field);
    expect(text(view)).toBe(DOC);
    field.dispatchEvent(new CompositionEvent("compositionend"));
    type(field, "가나");
    press(field, "Enter");
    await closed(view);
    expect(text(view)).toBe(withB("가나"));
  });

  it("opens with a given text", async () => {
    const view = await open();
    openLabelEditor(view, FROM, NODE_B, { selectAll: false, initial: "Mine" });
    const field = fieldOf(view);
    expect(field.value).toBe("Mine");
    expect(field.selectionStart).toBe(4);
    await commitLabelEditor(view);
    expect(text(view)).toBe(withB("Mine"));
    expect(noField(view)).toBe(true);
  });

  it("does nothing for a commit or a cancel with no field", async () => {
    const view = await open();
    await commitLabelEditor(view);
    cancelLabelEditor(view);
    expect(text(view)).toBe(DOC);
  });

  it.each([
    ['a "quoted" word'],
    ["a [bracket] (and) {more}"],
    ["#1 of #35; things"],
    ["&amp; and #quot; as text"],
    ["one\ntwo"],
    ["party 🎉 time"],
    ["größer als <b> & co"],
    ["x|y"],
    ["a -- b --> c"],
  ])("shows %j as typed when it opens again", async (typed) => {
    const view = await open();
    const steps = undoSteps(view);
    clickNode(view, "B");
    const field = fieldOf(view);
    type(field, typed);
    press(field, "Enter");
    await vi.waitFor(() => {
      expect(noField(view)).toBe(true);
    });
    await vi.waitFor(() => {
      expect(nodeOf(view, "B").getAttribute("aria-label")).not.toBe(
        "Box: Next step",
      );
    });
    expect(undoSteps(view)).toBe(steps + 1);
    press(active(), "Enter");
    expect(fieldOf(view).value).toBe(typed);
    press(fieldOf(view), "Escape");
    await closed(view);
    undoOnce(view);
    expect(text(view)).toBe(DOC);
  });

  it("takes a very long label as one line in the source", async () => {
    const view = await open();
    const long = "word ".repeat(400).trim();
    await rename(view, "B", long);
    expect(text(view)).toBe(withB(long));
  });

  it("leaves a later op on the same body to the op layer, with the field's draft kept", async () => {
    const view = await open();
    clickNode(view, "B");
    const field = fieldOf(view);
    type(field, "Dra");
    await runDiagramOp(view, FROM, flowKind, {
      op: "shape",
      id: "A",
      shape: "rounded",
    });
    await flush();
    expect(fieldOf(view)).toBe(field);
    expect(field.value).toBe("Dra");
    press(field, "Enter");
    await closed(view);
    expect(text(view)).toContain("A(First step) --> B[Dra]");
  });
});

describe("the field in a CRLF document", () => {
  it("writes into a CRLF document and undoes to its exact bytes", async () => {
    const doc = `Above\r\n\`\`\`mermaid\r\nflowchart TD\r\n    A[First step] --> B[Next step]\r\n\`\`\`\r\nBelow\r\n`;
    const view = await drawn(doc, { lineSeparator: "\r\n" });
    await rename(view, "B", "a\nb");
    expect(text(view)).toBe(doc.replace("Next step", "a<br>b"));
    undoOnce(view);
    expect(text(view)).toBe(doc);
  });
});

/** Runs CodeMirror's pending measure now, as a frame would. */
function measured(view: EditorView): void {
  (view as unknown as { measure(): void }).measure();
}

const panelOf = (view: EditorView) =>
  view.dom.querySelector(".cm-diagram-refusal")?.textContent ?? "";

describe("a starter on a page that has the chunk (C2)", () => {
  const starter = () => {
    const found = MERMAID_STARTER_GROUPS[0]?.starters[0];
    if (found === undefined) throw new Error("no starter");
    return mermaidFence(found);
  };

  it("opens the field under a running pointer hold, as after a menu's mouse up", async () => {
    const view = await drawn(`${DOC}Tail\n`);
    view.dispatch({ effects: pointerDown.of(true) });
    setCaret(view, view.state.doc.length - 1);
    const { lines, select } = starter();
    insertBlock(view, lines, select);
    await vi.waitFor(() => {
      expect(noField(view)).toBe(false);
    });
    const field = fieldOf(view);
    expect(field.value).toBe("First step");
    expect(active()).toBe(field);
  });

  it("opens a second starter on the same page", async () => {
    const view = markView("Intro\n", modeOptions());
    await prefetchDiagrams();
    view.contentDOM.focus();
    setCaret(view, 5);
    const { lines, select } = starter();
    insertBlock(view, lines, select);
    await vi.waitFor(() => {
      expect(noField(view)).toBe(false);
    });
    press(fieldOf(view), "Escape");
    await closed(view);
    setCaret(view, view.state.doc.length);
    view.contentDOM.focus();
    view.dispatch({ effects: pointerDown.of(true) });
    insertBlock(view, lines, select);
    await vi.waitFor(() => {
      expect(noField(view)).toBe(false);
    });
    expect(view.state.facet(drawnFenceLines)).toHaveLength(2);
  });

  it("does not open a field on a fence that is not drawn: no record, and it says so", async () => {
    const doc = `Above\n${fence(BODY)}\nBelow\n`;
    const view = await drawn(doc);
    setCaret(view, doc.indexOf("Next"), "select");
    view.dispatch({ effects: showFenceSource.of(FROM) });
    view.contentDOM.focus();
    await flush();
    expect(view.state.facet(drawnFenceLines)).toHaveLength(0);
    openLabelEditor(view, FROM, NODE_B, { selectAll: true });
    expect(noField(view)).toBe(true);
    expect(view.state.field(diagramIntent)).toBeNull();
    expect(view.state.field(diagramField).kept).toBeNull();
    await vi.waitFor(() => {
      expect(panelOf(view)).toBe(NOTICES.stale);
    });
    typeText(view, "x");
    expect(text(view)).not.toBe(doc);
  });
});

describe("leaving with a field open (C3, R95)", () => {
  function linkOnPage(onClick: () => void): HTMLElement {
    const link = document.createElement("a");
    link.href = "#elsewhere";
    link.addEventListener("click", (event) => {
      event.preventDefault();
      onClick();
    });
    document.body.append(link);
    return link;
  }

  function click(el: Element, options: MouseEventInit = {}): void {
    el.dispatchEvent(
      new MouseEvent("click", { bubbles: true, cancelable: true, ...options }),
    );
  }

  it("holds a link at 0 ms until the draft is written, then lets it go once", async () => {
    const view = await drawn();
    const went = vi.fn();
    const link = linkOnPage(() => {
      expect(text(view)).toBe(withB("Done"));
      went();
    });
    clickNode(view, "B");
    type(fieldOf(view), "Done");
    click(link);
    expect(went).not.toHaveBeenCalled();
    await vi.waitFor(() => {
      expect(went).toHaveBeenCalledTimes(1);
    });
    expect(noField(view)).toBe(true);
  });

  it("holds a marked Done button the same way", async () => {
    const view = await drawn();
    const went = vi.fn();
    const done = document.createElement("button");
    done.setAttribute("data-leaves-editor", "");
    done.addEventListener("click", went);
    document.body.append(done);
    clickNode(view, "B");
    type(fieldOf(view), "Done");
    click(done);
    expect(went).not.toHaveBeenCalled();
    await vi.waitFor(() => {
      expect(went).toHaveBeenCalledTimes(1);
    });
    expect(text(view)).toBe(withB("Done"));
  });

  it("keeps a refused draft on the page, with the sentence, and the link does not go", async () => {
    const view = await drawn();
    const went = vi.fn();
    const link = linkOnPage(went);
    clickNode(view, "B");
    const field = fieldOf(view);
    type(field, "a %%{ b");
    click(link);
    await vi.waitFor(() => {
      expect(noticeOf(view)).toContain(NOTICES.invalid);
    });
    await flush();
    expect(went).not.toHaveBeenCalled();
    expect(fieldOf(view)).toBe(field);
    expect(field.value).toBe("a %%{ b");
    expect(field.getAttribute("aria-invalid")).toBe("true");
  });

  it("lets a link go at once with nothing typed, and a click that opens a new tab", async () => {
    const view = await drawn();
    const went = vi.fn();
    const link = linkOnPage(went);
    clickNode(view, "B");
    click(link);
    expect(went).toHaveBeenCalledTimes(1);
    type(fieldOf(view), "Done");
    click(link, { metaKey: true });
    expect(went).toHaveBeenCalledTimes(2);
  });

  it("leaves other buttons alone while nothing is being checked", async () => {
    const view = await drawn();
    const pressed = vi.fn();
    const other = document.createElement("button");
    other.addEventListener("click", pressed);
    document.body.append(other);
    clickNode(view, "B");
    type(fieldOf(view), "Dra");
    click(other);
    expect(pressed).toHaveBeenCalledTimes(1);
  });

  it("asks before the page goes while a draft is unsaved, and not otherwise", async () => {
    const view = await drawn();
    const ask = () => {
      const event = new Event("beforeunload", { cancelable: true });
      window.dispatchEvent(event);
      return event.defaultPrevented;
    };
    expect(ask()).toBe(false);
    clickNode(view, "B");
    expect(ask()).toBe(false);
    type(fieldOf(view), "Done");
    expect(ask()).toBe(true);
    press(fieldOf(view), "Escape");
    await closed(view);
    expect(ask()).toBe(false);
  });

  it("holds Back until the draft is written, then the router hears it", async () => {
    const router = vi.fn();
    window.addEventListener("popstate", router);
    try {
      const view = await drawn();
      clickNode(view, "B");
      type(fieldOf(view), "Done");
      window.dispatchEvent(new PopStateEvent("popstate"));
      expect(router).not.toHaveBeenCalled();
      await vi.waitFor(() => {
        expect(router).toHaveBeenCalledTimes(1);
      });
      expect(text(view)).toBe(withB("Done"));
    } finally {
      window.removeEventListener("popstate", router);
    }
  });

  it("undoes Back for a refused draft, and the router never hears it", async () => {
    const router = vi.fn();
    const forward = vi
      .spyOn(window.history, "forward")
      .mockImplementation(() => undefined);
    window.addEventListener("popstate", router);
    try {
      const view = await drawn();
      clickNode(view, "B");
      type(fieldOf(view), "a %%{ b");
      window.dispatchEvent(new PopStateEvent("popstate"));
      await vi.waitFor(() => {
        expect(noticeOf(view)).toContain(NOTICES.invalid);
      });
      expect(forward).toHaveBeenCalledTimes(1);
      // The browser's own answer to that forward: not a person leaving.
      window.dispatchEvent(new PopStateEvent("popstate"));
      await flush();
      expect(router).not.toHaveBeenCalled();
      expect(noField(view)).toBe(false);
    } finally {
      window.removeEventListener("popstate", router);
      forward.mockRestore();
    }
  });
});

describe("a refused blur (I1, R96)", () => {
  it("never takes the focus back, marks the field and keeps the sentence", async () => {
    const view = await drawn();
    const tags = document.createElement("input");
    document.body.append(tags);
    clickNode(view, "B");
    const field = fieldOf(view);
    type(field, "a %%{ b");
    tags.focus();
    await vi.waitFor(() => {
      expect(noticeOf(view)).toContain(NOTICES.invalid);
    });
    await flush();
    expect(active()).toBe(tags);
    expect(field.getAttribute("aria-invalid")).toBe("true");
    const status = view.dom.querySelector(".cm-diagram-notice");
    expect(status?.id).not.toBe("");
    expect(field.getAttribute("aria-describedby")).toBe(status?.id);
    expect(noticeOf(view)).toContain("Escape");
    await new Promise((resolve) => setTimeout(resolve, NOTICE_MS + 100));
    expect(noticeOf(view)).toContain("Escape");
    expect(fieldOf(view)).toBe(field);
  }, 10_000);

  it("takes the mark and the sentence away when the text changes", async () => {
    const view = await drawn();
    clickNode(view, "B");
    const field = fieldOf(view);
    type(field, "a %%{ b");
    view.contentDOM.focus();
    await vi.waitFor(() => {
      expect(noticeOf(view)).toContain("Escape");
    });
    field.focus();
    type(field, "fine");
    expect(field.getAttribute("aria-invalid")).toBeNull();
    expect(noticeOf(view)).toBe("");
  });

  it("does not park main's selection for a person gone on to the text", async () => {
    const view = await drawn();
    clickNode(view, "B");
    type(fieldOf(view), "a %%{ b");
    view.contentDOM.focus();
    await vi.waitFor(() => {
      expect(noticeOf(view)).toContain("Escape");
    });
    expect(active()).toBe(view.contentDOM);
    expect(view.state.facet(mainSelectionParked)).toBe(false);
  });

  it("keeps the field on a window that lost the focus, with nothing written", async () => {
    const view = await drawn();
    const steps = undoSteps(view);
    clickNode(view, "B");
    const field = fieldOf(view);
    type(field, "Dra");
    const has = vi.spyOn(document, "hasFocus").mockReturnValue(false);
    try {
      field.blur();
      await flush();
      await flush();
    } finally {
      has.mockRestore();
    }
    expect(fieldOf(view)).toBe(field);
    expect(undoSteps(view)).toBe(steps);
  });

  it("holds Enter's refusal in the field, marked too", async () => {
    const view = await drawn();
    clickNode(view, "B");
    const field = fieldOf(view);
    type(field, "a %%{ b");
    press(field, "Enter");
    await vi.waitFor(() => {
      expect(field.getAttribute("aria-invalid")).toBe("true");
    });
    expect(active()).toBe(field);
  });
});

describe("a refused Raw commit (I2, R97)", () => {
  it("says so under Raw, with the draft, and brings the field back when Raw goes off", async () => {
    const view = await drawn();
    clickNode(view, "B");
    type(fieldOf(view), "a %%{ b");
    setRaw(view, true);
    await vi.waitFor(() => {
      expect(
        view.dom.querySelector(".cm-diagram-raw-note")?.textContent,
      ).toContain(NOTICES.invalid);
    });
    expect(
      view.dom.querySelector(".cm-diagram-raw-note")?.textContent,
    ).toContain("a %%{ b");
    expect(text(view)).toBe(DOC);
    setRaw(view, false);
    await vi.waitFor(() => {
      expect(fieldOf(view).value).toBe("a %%{ b");
    });
    expect(view.dom.querySelector(".cm-diagram-raw-note")).toBeNull();
    expect(active()).toBe(fieldOf(view));
    press(fieldOf(view), "Escape");
    await closed(view);
    expect(text(view)).toBe(DOC);
  });
});

describe("a box the person scrolled away (I3, R98)", () => {
  const LONG = `${DOC}${"Line of text\n\n".repeat(8000)}`;

  async function scrolledAway() {
    const view = await drawn(LONG);
    clickNode(view, "B");
    const field = fieldOf(view);
    type(field, "Dra");
    field.setSelectionRange(3, 3);
    view.dispatch({
      effects: EditorView.scrollIntoView(view.state.doc.length),
    });
    measured(view);
    expect(view.dom.querySelector(".cm-diagram")).toBeNull();
    expect(field.isConnected).toBe(false);
    (document.activeElement as HTMLElement | null)?.blur();
    return { view, field };
  }

  function keyOnBody(key: string): KeyboardEvent {
    const event = new KeyboardEvent("keydown", {
      key,
      bubbles: true,
      cancelable: true,
    });
    document.body.dispatchEvent(event);
    return event;
  }

  it("takes a letter back to the field, which comes back with it", async () => {
    const { view, field } = await scrolledAway();
    const event = keyOnBody("f");
    expect(event.defaultPrevented).toBe(true);
    keyOnBody("t");
    measured(view);
    await vi.waitFor(() => {
      expect(field.isConnected).toBe(true);
    });
    expect(field.value).toBe("Draft");
    expect(active()).toBe(field);
    expect(view.state.field(diagramIntent)?.draft).toBe("Draft");
  });

  it("leaves a shortcut and a scrolling key alone", async () => {
    const { field } = await scrolledAway();
    expect(keyOnBody("PageDown").defaultPrevented).toBe(false);
    document.body.dispatchEvent(
      new KeyboardEvent("keydown", {
        key: "s",
        metaKey: true,
        bubbles: true,
      }),
    );
    expect(field.value).toBe("Dra");
  });

  it("leaves a key in another field alone", async () => {
    const { field } = await scrolledAway();
    const other = document.createElement("input");
    document.body.append(other);
    other.focus();
    const event = new KeyboardEvent("keydown", {
      key: "f",
      bubbles: true,
      cancelable: true,
    });
    other.dispatchEvent(event);
    expect(event.defaultPrevented).toBe(false);
    expect(field.value).toBe("Dra");
  });

  it("says a refusal in the panel while the box is away, and keeps the draft", async () => {
    const { view, field } = await scrolledAway();
    type(field, "a %%{ b");
    await commitLabelEditor(view);
    await vi.waitFor(() => {
      expect(panelOf(view)).toContain(NOTICES.invalid);
    });
    expect(field.value).toBe("a %%{ b");
  });
});

describe("Shift+Tab onto the box (I4)", () => {
  it("keeps the focus on the stop through the commit and the redraw", async () => {
    const view = await drawn();
    clickNode(view, "B");
    type(fieldOf(view), "Good");
    await flush();
    const stop = nodeOf(view, "B") as SVGElement;
    stop.focus();
    await vi.waitFor(() => {
      expect(text(view)).toBe(withB("Good"));
    });
    await vi.waitFor(() => {
      expect(active()?.getAttribute("aria-label")).toBe("Box: Good");
    });
    expect(view.state.field(diagramIntent)).toMatchObject({
      target: NODE_B,
      mode: "focus",
    });
  });
});

describe("what the field holds to", () => {
  it("does not take an unchanged name's Enter for a repeat of a held key", async () => {
    const view = await drawn();
    focusTarget(view, FROM, NODE_B);
    press(active(), "Enter");
    const field = fieldOf(view);
    press(field, "Enter", { repeat: true });
    await flush();
    expect(fieldOf(view)).toBe(field);
    press(field, "Escape", { repeat: true });
    await flush();
    expect(fieldOf(view)).toBe(field);
    press(field, "Escape");
    await closed(view);
  });
});

const lostNote = () =>
  document.querySelector(".cm-diagram-lost-draft")?.textContent ?? "";

describe("a view going away with a draft (N4)", () => {
  it("names the text when the view goes by a route nothing held", async () => {
    const view = await drawn();
    clickNode(view, "B");
    type(fieldOf(view), "My word");
    view.destroy();
    await flush();
    expect(lostNote()).toContain("My word");
    document
      .querySelector<HTMLElement>(".cm-diagram-lost-draft button")
      ?.click();
    expect(lostNote()).toBe("");
  });

  it("names a refused one too, and says nothing for a field nobody typed in", async () => {
    const view = await drawn();
    clickNode(view, "B");
    view.destroy();
    await flush();
    expect(lostNote()).toBe("");
    const other = await drawn();
    clickNode(other, "B");
    type(fieldOf(other), "a %%{ b");
    other.contentDOM.focus();
    await vi.waitFor(() => {
      expect(noticeOf(other)).toContain("Escape");
    });
    other.destroy();
    await flush();
    expect(lostNote()).toContain("a %%{ b");
  });

  it("names it on a setState", async () => {
    const view = await drawn();
    clickNode(view, "B");
    type(fieldOf(view), "New");
    view.setState(EditorState.create({ doc: "Other text" }));
    await flush();
    await new Promise((resolve) => setTimeout(resolve, 20));
    expect(lostNote()).toContain("New");
  });

  it("names it when the page stops taking writes under the field", async () => {
    const view = await drawn();
    clickNode(view, "B");
    type(fieldOf(view), "Done");
    const { StateEffect } = await import("@codemirror/state");
    view.dispatch({
      effects: StateEffect.appendConfig.of(EditorState.readOnly.of(true)),
    });
    await closed(view);
    expect(lostNote()).toContain("Done");
  });
});

describe("a room closing with a field open (N2, R100)", () => {
  it("answers at once with no field", async () => {
    const view = await drawn();
    expect(settleFieldDraft(view)).toBeNull();
  });

  it("writes a good draft into the text, for the draft the banner promises", async () => {
    const view = await drawn();
    clickNode(view, "B");
    type(fieldOf(view), "Kept");
    await expect(settleFieldDraft(view)).resolves.toBeNull();
    expect(text(view)).toBe(withB("Kept"));
    expect(noField(view)).toBe(true);
    expect(view.state.field(diagramIntent)).toBeNull();
  });

  it("names a text it cannot write, and holds nothing after", async () => {
    const view = await drawn();
    const went = vi.fn();
    const link = document.createElement("a");
    link.href = "#x";
    link.addEventListener("click", (event) => {
      event.preventDefault();
      went();
    });
    document.body.append(link);
    clickNode(view, "B");
    type(fieldOf(view), "a %%{ b");
    await expect(settleFieldDraft(view)).resolves.toBe("a %%{ b");
    expect(text(view)).toBe(DOC);
    expect(noField(view)).toBe(true);
    link.click();
    expect(went).toHaveBeenCalledTimes(1);
    view.destroy();
    await flush();
    expect(lostNote()).toBe("");
  });

  it("answers null for a field nobody typed in, and closes it", async () => {
    const view = await drawn();
    clickNode(view, "B");
    await expect(settleFieldDraft(view)).resolves.toBeNull();
    expect(noField(view)).toBe(true);
  });
});

describe("a refused draft under Raw has an end (N3)", () => {
  const under = async () => {
    const view = await drawn();
    clickNode(view, "B");
    type(fieldOf(view), "a %%{ b");
    setRaw(view, true);
    await vi.waitFor(() => {
      expect(
        view.dom.querySelector(".cm-diagram-raw-note")?.textContent,
      ).toContain("a %%{ b");
    });
    return view;
  };
  const note = (view: EditorView) =>
    view.dom.querySelector(".cm-diagram-raw-note")?.textContent ?? "";

  it("comes back when the caret leaves a fence that Raw off left as text", async () => {
    const view = await under();
    setCaret(view, DOC.indexOf("Next"), "select");
    setRaw(view, false);
    await flush();
    await flush();
    expect(noField(view)).toBe(true);
    expect(note(view)).toContain("caret");
    setCaret(view, 2, "select");
    await vi.waitFor(() => {
      expect(fieldOf(view).value).toBe("a %%{ b");
    });
    expect(note(view)).toBe("");
  });

  it("comes back when the verdict of a changed fence lands", async () => {
    const view = await under();
    const at = DOC.indexOf("    A[First");
    view.dispatch({
      changes: { from: at, insert: `    %% n${String(Date.now())}\n` },
    });
    setRaw(view, false);
    await vi.waitFor(() => {
      expect(fieldOf(view).value).toBe("a %%{ b");
    });
  });

  it("says how to leave, and a second try leaves, naming the text", async () => {
    const view = await under();
    expect(note(view)).toContain("again");
    expect(note(view)).toContain("stopped once");
    expect(note(view)).not.toContain("asks");
    const went = vi.fn();
    const link = document.createElement("a");
    link.href = "#x";
    link.addEventListener("click", (event) => {
      event.preventDefault();
      went();
    });
    document.body.append(link);
    link.click();
    expect(went).not.toHaveBeenCalled();
    link.click();
    expect(went).toHaveBeenCalledTimes(1);
    expect(lostNote()).toContain("a %%{ b");
    expect(note(view)).toBe("");
    setRaw(view, false);
    await flush();
    await flush();
    expect(noField(view)).toBe(true);
  });

  it("a second Back goes as well", async () => {
    const router = vi.fn();
    const forward = vi
      .spyOn(window.history, "forward")
      .mockImplementation(() => undefined);
    window.addEventListener("popstate", router);
    try {
      await under();
      window.dispatchEvent(new PopStateEvent("popstate"));
      expect(forward).toHaveBeenCalledTimes(1);
      window.dispatchEvent(new PopStateEvent("popstate"));
      expect(router).not.toHaveBeenCalled();
      window.dispatchEvent(new PopStateEvent("popstate"));
      expect(router).toHaveBeenCalledTimes(1);
      expect(lostNote()).toContain("a %%{ b");
    } finally {
      window.removeEventListener("popstate", router);
      forward.mockRestore();
    }
  });

  it("names the text when the view goes while Raw holds it", async () => {
    const view = await under();
    view.destroy();
    await flush();
    await flush();
    expect(lostNote()).toContain("a %%{ b");
  });

  it("ends with the text named when the fence cannot read again", async () => {
    const view = await under();
    const at = DOC.indexOf("    A[First");
    view.dispatch({ changes: { from: at, insert: "    C -->\n" } });
    setRaw(view, false);
    await vi.waitFor(() => {
      expect(lostNote()).toContain("a %%{ b");
    });
    expect(note(view)).toBe("");
  });
});

describe("two Backs in one tick (N6)", () => {
  it("writes the draft, and the router hears one", async () => {
    const router = vi.fn();
    window.addEventListener("popstate", router);
    try {
      const view = await drawn();
      clickNode(view, "B");
      type(fieldOf(view), "Twice");
      window.dispatchEvent(new PopStateEvent("popstate"));
      window.dispatchEvent(new PopStateEvent("popstate"));
      await vi.waitFor(() => {
        expect(router).toHaveBeenCalled();
      });
      expect(text(view)).toBe(withB("Twice"));
    } finally {
      window.removeEventListener("popstate", router);
    }
  });
});
