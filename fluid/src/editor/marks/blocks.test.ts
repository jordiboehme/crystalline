/**
 * Bullets, numbers, task boxes, quotes and rules drawn in place of their
 * marks, the task gestures, and Enter in a list or a quote. Every drawing
 * claim is on the DOM; every gesture claim is on the bytes.
 */

import { StateEffect } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { afterEach, describe, expect, it } from "vitest";

import {
  bytes,
  cleanupMarkViews,
  keys,
  markView,
  setCaret,
  typeText,
  undoOnce,
} from "../../test/markKit";
import { parsedState } from "../../test/parse";
import { EditorState } from "@codemirror/state";
import { hiddenMarkLayer } from ".";
import { taskChanges } from "./blocks";
import { baseExtensions, lineSeparatorFor } from "../setup";
import { livePreview } from "../preview";

afterEach(cleanupMarkViews);

type Mode = "hidden" | "show";
const MODES: Mode[] = ["hidden", "show"];

function texts(view: EditorView, selector: string): string[] {
  return [...view.contentDOM.querySelectorAll(selector)].map(
    (el) => el.textContent ?? "",
  );
}

/** The box of the task line, a checkbox or the struck box. */
function box(view: EditorView): HTMLElement {
  const found = view.contentDOM.querySelector<HTMLElement>(
    "input.cm-task-toggle, .cm-task-na",
  );
  if (!found) {
    throw new Error("no task box is drawn");
  }
  return found;
}

function click(view: EditorView): void {
  box(view).dispatchEvent(
    new MouseEvent("mousedown", { bubbles: true, cancelable: true }),
  );
}

/** A solo view on `doc` with its own separator, for the CRLF cases. */
function crlfView(doc: string): EditorView {
  const view = new EditorView({
    state: parsedState(
      EditorState.create({
        doc,
        extensions: [
          ...lineSeparatorFor(doc),
          ...baseExtensions(false),
          livePreview(),
          hiddenMarkLayer(),
        ],
      }),
    ),
    parent: document.body,
  });
  return view;
}

describe("drawing, hidden mode", () => {
  it("draws a bullet for each depth", () => {
    const view = markView("- a\n  - b\n    - c");
    expect(texts(view, ".cm-list-bullet")).toEqual(["•", "◦", "▪"]);
    expect(
      view.contentDOM
        .querySelector(".cm-list-bullet")
        ?.getAttribute("aria-hidden"),
    ).toBe("true");
    expect(bytes(view.state)).toBe("- a\n  - b\n    - c");
  });

  it("draws the number the reading page shows and leaves the source", () => {
    const view = markView("3. a\n7. b");
    expect(texts(view, ".cm-list-number")).toEqual(["3.", "4."]);
    expect(bytes(view.state)).toBe("3. a\n7. b");
  });

  it("keeps the delimiter as written", () => {
    const view = markView("1) a\n1) b");
    expect(texts(view, ".cm-list-number")).toEqual(["1)", "2)"]);
  });

  it("draws one checkbox and no bullet for a task", () => {
    const view = markView("- [ ] t");
    expect(
      view.contentDOM.querySelectorAll("input.cm-task-toggle"),
    ).toHaveLength(1);
    expect(view.contentDOM.querySelectorAll(".cm-list-bullet")).toHaveLength(0);
    expect(view.contentDOM.textContent).toBe("t");
  });

  it("gives a quote line its class and hides the marks", () => {
    const view = markView("> q");
    expect(view.contentDOM.querySelectorAll(".cm-quote-line")).toHaveLength(1);
    expect(view.contentDOM.textContent).toBe("q");
  });

  it("draws one rule for a thematic break", () => {
    const view = markView("a\n\n---\n\nb");
    const rules = view.contentDOM.querySelectorAll("[role=separator]");
    expect(rules).toHaveLength(1);
    expect(view.contentDOM.textContent).not.toContain("---");
    expect(bytes(view.state)).toBe("a\n\n---\n\nb");
  });

  it("draws no rule over the frontmatter fences or a setext underline", () => {
    const front = markView("---\ntitle: x\n---\n\nb");
    expect(front.contentDOM.querySelectorAll("[role=separator]")).toHaveLength(
      0,
    );
    const setext = markView("Title\n---\n\nb");
    expect(setext.contentDOM.querySelectorAll("[role=separator]")).toHaveLength(
      0,
    );
  });

  it("keeps a callout's own look and draws no second bar", () => {
    const view = markView("> [!NOTE]\n> body");
    expect(
      view.contentDOM.querySelectorAll(".callout.cm-quote-line"),
    ).toHaveLength(2);
  });

  it("draws no bar for a > inside a multi-line comment", () => {
    const view = markView("<!--\n> x\n-->\n\ny");
    expect(view.contentDOM.querySelectorAll(".cm-quote-line")).toHaveLength(0);
  });
});

describe("drawing, show mode and Raw", () => {
  it("shows the source on the line the caret is on and draws the others", () => {
    const view = markView("- a\n- b\n> q\n---", { mode: "show" });
    setCaret(view, 1);
    expect(view.contentDOM.textContent).toContain("- a");
    expect(texts(view, ".cm-list-bullet")).toEqual(["•"]);
    setCaret(view, view.state.doc.length);
    expect(view.contentDOM.querySelectorAll("[role=separator]")).toHaveLength(
      0,
    );
    expect(view.contentDOM.textContent).toContain("---");
    expect(view.contentDOM.querySelectorAll(".cm-quote-line")).toHaveLength(1);
  });

  it("shows the source in Raw", () => {
    const view = markView("- [ ] a\n> q\n---\n1. x", { raw: true });
    expect(view.contentDOM.querySelectorAll(".cm-list-bullet")).toHaveLength(0);
    expect(view.contentDOM.querySelectorAll(".cm-list-number")).toHaveLength(0);
    expect(view.contentDOM.querySelectorAll("input")).toHaveLength(0);
    expect(view.contentDOM.querySelectorAll("[role=separator]")).toHaveLength(
      0,
    );
    expect(view.contentDOM.textContent).toContain("- [ ] a");
  });
});

describe("typing a bullet", () => {
  it("draws a bullet once the dash has its space, and the dash alone stays", () => {
    const view = markView("", { mode: "hidden" });
    typeText(view, "-");
    expect(view.contentDOM.querySelectorAll(".cm-list-bullet")).toHaveLength(0);
    expect(view.contentDOM.textContent).toBe("-");
    typeText(view, " ");
    expect(texts(view, ".cm-list-bullet")).toEqual(["•"]);
  });
});

describe("the rule atom", () => {
  it("Backspace after a rule removes its text whole", () => {
    const view = markView("a\n\n---\n\nb");
    setCaret(view, 6);
    expect(keys(view, "Backspace")).toBe(true);
    expect(bytes(view.state)).toBe("a\n\n\n\nb");
  });
});

describe("task gestures", () => {
  for (const mode of MODES) {
    for (const room of [false, true]) {
      const where = `${mode}, ${room ? "room" : "solo"}`;
      it(`a click on the box writes the middle character and is one undo step (${where})`, () => {
        const view = markView("- [ ] t\nz", { mode, room });
        setCaret(view, view.state.doc.length);
        click(view);
        expect(bytes(view.state)).toBe("- [x] t\nz");
        undoOnce(view);
        expect(bytes(view.state)).toBe("- [ ] t\nz");
      });
    }
  }

  it("a click on a done box writes a space and leaves [X] bytes elsewhere alone", () => {
    const view = markView("- [X] t\n- [x] u");
    box(view).dispatchEvent(new MouseEvent("mousedown", { bubbles: true }));
    expect(bytes(view.state)).toBe("- [ ] t\n- [x] u");
  });

  it("a click on a [~] box writes a space", () => {
    const view = markView("- [~] t");
    click(view);
    expect(bytes(view.state)).toBe("- [ ] t");
  });

  it("a click on a CRLF document changes one character", () => {
    const view = crlfView("- [ ] t\r\n- [ ] u");
    click(view);
    expect(view.state.sliceDoc(0)).toBe("- [x] t\r\n- [ ] u");
    view.destroy();
  });

  it("a click is one undo step in a room even right after typing", () => {
    const view = markView("- [ ] t\n\nq", { room: true });
    setCaret(view, view.state.doc.length);
    typeText(view, "w");
    click(view);
    undoOnce(view);
    expect(bytes(view.state)).toBe("- [ ] t\n\nqw");
  });

  it("a click writes nothing on a page that is not writable", () => {
    const view = markView("- [ ] t");
    view.dispatch({
      effects: StateEffect.appendConfig.of(EditorView.editable.of(false)),
    });
    click(view);
    expect(bytes(view.state)).toBe("- [ ] t");
    const na = markView("- [~] t");
    na.dispatch({
      effects: StateEffect.appendConfig.of(EditorView.editable.of(false)),
    });
    click(na);
    expect(bytes(na.state)).toBe("- [~] t");
  });

  it("Mod-Enter cycles an open task to done and back, and [~] to open", () => {
    for (const mode of MODES) {
      const view = markView("- [x] t", { mode });
      setCaret(view, 7);
      expect(keys(view, "Mod-Enter")).toBe(true);
      expect(bytes(view.state)).toBe("- [ ] t");
      keys(view, "Mod-Enter");
      expect(bytes(view.state)).toBe("- [x] t");
      const na = markView("- [~] t", { mode });
      setCaret(na, 7);
      keys(na, "Mod-Enter");
      expect(bytes(na.state)).toBe("- [ ] t");
    }
  });

  it("Mod-Enter off a task line flips nothing, and refuses on a read-only page", () => {
    const plain = markView("text\n- [ ] t");
    setCaret(plain, 2);
    keys(plain, "Mod-Enter");
    expect(bytes(plain.state)).not.toContain("[x]");
    const view = markView("- [ ] t");
    view.dispatch({
      effects: StateEffect.appendConfig.of(EditorView.editable.of(false)),
    });
    setCaret(view, 7);
    keys(view, "Mod-Enter");
    expect(bytes(view.state)).toBe("- [ ] t");
  });
});

function press(view: EditorView, key: string): void {
  const target = box(view);
  target.dispatchEvent(
    new KeyboardEvent("keydown", { key, bubbles: true, cancelable: true }),
  );
  target.dispatchEvent(new KeyboardEvent("keyup", { key, bubbles: true }));
  // The click the browser sends for Space on a checkbox.
  if (key === " ") {
    target.dispatchEvent(new MouseEvent("click", { bubbles: true }));
  }
}

describe("task box accessibility and input", () => {
  it("names an open and a done box and reports their state", () => {
    const open = box(markView("- [ ] t"));
    expect(open.getAttribute("aria-label")).toBe("Mark task done");
    expect((open as HTMLInputElement).checked).toBe(false);
    const done = box(markView("- [x] t"));
    expect(done.getAttribute("aria-label")).toBe("Mark task open");
    expect((done as HTMLInputElement).checked).toBe(true);
  });

  it("a right or middle press writes nothing and is not cancelled", () => {
    for (const button of [1, 2]) {
      const view = markView("- [ ] t");
      const event = new MouseEvent("mousedown", {
        bubbles: true,
        cancelable: true,
        button,
      });
      box(view).dispatchEvent(event);
      expect(bytes(view.state)).toBe("- [ ] t");
      expect(event.defaultPrevented).toBe(false);
    }
  });

  for (const key of [" ", "Enter"]) {
    it(`one ${key === " " ? "Space" : "Enter"} press writes exactly once`, () => {
      const view = markView("- [ ] t\nz");
      press(view, key);
      expect(bytes(view.state)).toBe("- [x] t\nz");
      const done = markView("- [x] t\nz");
      press(done, key);
      expect(bytes(done.state)).toBe("- [ ] t\nz");
    });
  }
});

describe("block drawing in nesting and order", () => {
  it("draws a list inside a quote", () => {
    const view = markView("> - a\n>   - b");
    expect(texts(view, ".cm-list-bullet")).toEqual(["•", "◦"]);
    expect(view.contentDOM.querySelectorAll(".cm-quote-line")).toHaveLength(2);
    expect(view.contentDOM.textContent).toBe("•a  ◦b");
  });

  it("draws a quote inside a list item", () => {
    const view = markView("- a\n  > q");
    expect(texts(view, ".cm-list-bullet")).toEqual(["•"]);
    expect(view.contentDOM.querySelectorAll(".cm-quote-line")).toHaveLength(1);
    expect(view.contentDOM.textContent).toBe("•a  q");
  });

  it("numbers on across a blank line between items", () => {
    const view = markView("3. a\n\n7. b\n\n9. c");
    expect(texts(view, ".cm-list-number")).toEqual(["3.", "4.", "5."]);
  });

  it("starts a new count after a list is interrupted by another block", () => {
    const comment = markView("1. a\n2. b\n\n<!-- c -->\n\n5. d\n6. e");
    expect(texts(comment, ".cm-list-number")).toEqual(["1.", "2.", "5.", "6."]);
    const rule = markView("1. a\n2. b\n\n# H\n\n4. c\n5. d");
    expect(texts(rule, ".cm-list-number")).toEqual(["1.", "2.", "4.", "5."]);
  });
});

describe("taskChanges", () => {
  it("writes one character of the task on the line, and nothing off a task", () => {
    const view = markView("- [ ] t\ntext\n1. [~] u");
    expect(taskChanges(view.state, 2, "done")).toEqual({
      from: 3,
      to: 4,
      insert: "x",
    });
    expect(taskChanges(view.state, 2, "na")).toEqual({
      from: 3,
      to: 4,
      insert: "~",
    });
    expect(taskChanges(view.state, 9, "open")).toBeNull();
    expect(taskChanges(view.state, 17, "open")).toEqual({
      from: 17,
      to: 18,
      insert: " ",
    });
  });
});

function enter(doc: string, at: number, mode: Mode = "hidden"): string {
  const view = markView(doc, { mode });
  setCaret(view, at);
  keys(view, "Enter");
  return bytes(view.state);
}

describe("Enter in a list or a quote", () => {
  for (const mode of MODES) {
    it(`continues the markers (${mode})`, () => {
      expect(enter("1. a\n2. b", 4, mode)).toBe("1. a\n2. \n2. b");
      expect(enter("- [x] a", 7, mode)).toBe("- [x] a\n- [ ] ");
      expect(enter("> q", 3, mode)).toBe("> q\n> ");
      expect(enter("  - a", 5, mode)).toBe("  - a\n  - ");
      expect(enter("> > q", 5, mode)).toBe("> > q\n> > ");
      expect(enter("1) a", 4, mode)).toBe("1) a\n2) ");
      expect(enter("* a", 3, mode)).toBe("* a\n* ");
      expect(enter("- [~] a", 7, mode)).toBe("- [~] a\n- [ ] ");
    });

    it(`removes the marker of an empty item (${mode})`, () => {
      expect(enter("- ", 2, mode)).toBe("");
      expect(enter("- a\n- ", 6, mode)).toBe("- a\n");
      expect(enter("  - ", 4, mode)).toBe("");
      expect(enter("- [ ] ", 6, mode)).toBe("");
      expect(enter("> q\n> ", 6, mode)).toBe("> q\n");
      expect(enter("> q\n> > ", 8, mode)).toBe("> q\n");
    });
  }

  it("splits an item in the middle and moves the rest after the marker", () => {
    expect(enter("- ab", 3)).toBe("- a\n- b");
  });

  it("leaves other lines to the default Enter", () => {
    const view = markView("plain");
    setCaret(view, 5);
    expect(keys(view, "Enter")).toBe(true);
    expect(bytes(view.state)).toBe("plain\n");
  });

  it("uses the document's line ending", () => {
    const view = crlfView("1. a\r\n2. b");
    setCaret(view, 4);
    keys(view, "Enter");
    expect(view.state.sliceDoc(0)).toBe("1. a\r\n2. \r\n2. b");
    view.destroy();
    const task = crlfView("- [x] a\r\nb");
    setCaret(task, 7);
    keys(task, "Enter");
    expect(task.state.sliceDoc(0)).toBe("- [x] a\r\n- [ ] \r\nb");
    task.destroy();
    const empty = crlfView("> q\r\n> ");
    setCaret(empty, 6);
    keys(empty, "Enter");
    expect(empty.state.sliceDoc(0)).toBe("> q\r\n");
    empty.destroy();
  });

  it("is one undo step", () => {
    const view = markView("1. a\n2. b");
    setCaret(view, 4);
    keys(view, "Enter");
    undoOnce(view);
    expect(bytes(view.state)).toBe("1. a\n2. b");
  });

  it("keeps today's Enter in Raw, renumbering included", () => {
    const view = markView("1. a\n2. b", { raw: true });
    setCaret(view, 4);
    keys(view, "Enter");
    expect(bytes(view.state)).toBe("1. a\n2. \n3. b");
  });
});
