/**
 * The rename field, on the real surface and the real verdict, with only mermaid's
 * render stubbed: solo and in a room. `labelEditor.marks-hidden.test.ts` and
 * `labelEditor.marks-show.test.ts` run this file again.
 */

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { EditorState } from "@codemirror/state";
import type { EditorView } from "@codemirror/view";

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
import { drawnFenceLines } from "./draws";
import { NOTICES } from "./edits";
import { flowKind, runDiagramOp } from "./apply";
import { parseFlowchart } from "./flowchart/parse";
import { focusTarget } from "./focus";
import { diagramIntent } from "./intent";
import {
  addWithName,
  cancelLabelEditor,
  commitLabelEditor,
  openLabelEditor,
} from "./labelEditor";
import { prefetchDiagrams } from "./loader";
import { requestVerdict } from "./verdict";
import { forgetDrawings } from "./widget";

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

beforeEach(() => {
  forgetDrawings();
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
      expect(view.dom.querySelector(".cm-diagram-notice")?.textContent).toBe(
        NOTICES.invalid,
      );
    });
    expect(fieldOf(view)).toBe(field);
    expect(field.value).toBe("a %%{ b");
    expect(field.readOnly).toBe(false);
    expect(active()).toBe(field);
    press(field, "Escape");
    await closed(view);
    expect(text(view)).toBe(DOC);
  });

  it("keeps a refused blur's draft and takes the focus back from the text", async () => {
    const view = await open();
    clickNode(view, "B");
    const field = fieldOf(view);
    type(field, "a %%{ b");
    view.contentDOM.focus();
    await vi.waitFor(() => {
      expect(view.dom.querySelector(".cm-diagram-notice")?.textContent).toBe(
        NOTICES.invalid,
      );
    });
    expect(fieldOf(view)).toBe(field);
    expect(active()).toBe(field);
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
