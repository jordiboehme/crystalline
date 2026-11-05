/**
 * The decoration layer, driven through a real EditorView: what a reader sees,
 * what the cursor reveals, and the single click that is allowed to edit.
 *
 * Every assertion is on the rendered DOM rather than on a decoration set,
 * because "the marks fold away" is a claim about what somebody looking at the
 * screen reads. The fidelity test is the counterweight: decorations are a
 * read-model, so the buffer they decorate must come back out byte-identical.
 */

import { EditorSelection, EditorState, Text } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { afterEach, describe, expect, it, vi } from "vitest";

import {
  cleanupMarkViews,
  markView,
  setCaret,
  setShowMarks,
} from "../test/markKit";
import { parsedState, parsedView } from "../test/parse";
import { frontmatterRegion } from "./frontmatterRegion";
import { hiddenMarkLayer } from "./marks";
import { marksOnEditLine } from "./marks/mode";
import { livePreview } from "./preview";
import { baseExtensions, docText, lineSeparatorFor } from "./setup";

const DOC =
  "---\ntitle: Alpha\n---\n\n# Heading\n\nSome *emphasis* here.\n\n- [ ] a task\n";

function editor(doc: string, at?: number): EditorView {
  const view = new EditorView({
    // `parsedState`, because these decorations are read off the syntax tree and
    // a new state's first parse is cut off after 20ms of wall clock.
    // A named separator on every state this file builds, tests included: a
    // buffer that names none rewrites a CRLF document to LF on read-back.
    state: parsedState(
      EditorState.create({
        doc,
        extensions: [
          ...lineSeparatorFor(doc),
          ...baseExtensions(false),
          livePreview(),
          hiddenMarkLayer(),
          // Show mode: these tests pin the marks a touched line reveals.
          marksOnEditLine.of(true),
        ],
      }),
    ),
    parent: document.body,
  });
  // The end of the document is asked of the buffer, never of the file string:
  // positions are offsets into the buffer, and a CRLF document is one unit
  // shorter per line than the text it was built from.
  view.dispatch({
    selection: EditorSelection.cursor(at ?? view.state.doc.length),
  });
  return view;
}

afterEach(cleanupMarkViews);

describe("hidden marks", () => {
  it("draws no star on the caret line in hidden mode and the stars in show mode", () => {
    const hidden = markView("a **bold** c", { mode: "hidden" });
    setCaret(hidden, 6);
    expect(hidden.contentDOM.textContent).toBe("a bold c");
    expect(hidden.contentDOM.textContent).not.toContain("*");
    const show = markView("a **bold** c", { mode: "show" });
    setCaret(show, 6);
    expect(show.contentDOM.textContent).toContain("**bold**");
  });

  it("builds no new decoration set for a caret move in hidden mode", () => {
    const view = new EditorView({
      state: parsedState(
        EditorState.create({
          doc: "a **bold** c\nnext",
          extensions: [
            ...baseExtensions(false),
            livePreview(),
            hiddenMarkLayer(),
          ],
        }),
      ),
      parent: document.body,
    });
    const sets = () =>
      view.state.facet(EditorView.atomicRanges).map((ranges) => ranges(view));
    const before = sets();
    expect(before.some((set) => set.size > 0)).toBe(true);
    view.dispatch({ selection: EditorSelection.cursor(14) });
    const after = sets();
    expect(after).toHaveLength(before.length);
    after.forEach((set, index) => {
      expect(set).toBe(before[index]);
    });
    view.destroy();
  });

  it("shows every mark in the source view", () => {
    const raw = markView("a **bold** c", { raw: true });
    setCaret(raw, 6);
    expect(raw.contentDOM.textContent).toContain("**bold**");
  });

  it("redraws the caret line when the setting flips, with no selection change", () => {
    const view = markView("a **bold** c", { mode: "hidden" });
    setCaret(view, 6);
    expect(view.contentDOM.textContent).not.toContain("*");
    setShowMarks(view, true);
    expect(view.state.selection.main.head).toBe(6);
    expect(view.contentDOM.textContent).toContain("**bold**");
    setShowMarks(view, false);
    expect(view.contentDOM.textContent).not.toContain("*");
  });
});

describe("frontmatterRegion", () => {
  it("finds the opening block and only the opening block", () => {
    const region = frontmatterRegion(Text.of(DOC.split("\n")));
    expect(region).toEqual({ from: 0, to: DOC.indexOf("---\n\n") + 3 });
    expect(frontmatterRegion(Text.of(["no frontmatter", "---"]))).toBeNull();
  });

  it("ignores a fence that is not on the first line and an unclosed one", () => {
    expect(frontmatterRegion(Text.of(["", "---", "title: Alpha", "---"]))).toBe(
      null,
    );
    expect(frontmatterRegion(Text.of(["---", "title: Alpha"]))).toBeNull();
    expect(frontmatterRegion(Text.of(["---"]))).toBeNull();
  });
});

describe("live preview", () => {
  it("folds syntax markers away from the cursor and shows them on the active line", () => {
    const view = editor(DOC);
    // Cursor is at the end: the heading line is inactive, its mark is hidden.
    expect(view.contentDOM.textContent).toContain("Heading");
    expect(view.contentDOM.textContent).not.toContain("# Heading");
    expect(view.contentDOM.textContent).not.toContain("*emphasis*");
    // Move onto the heading line: the marks materialize.
    view.dispatch({
      selection: EditorSelection.cursor(DOC.indexOf("Heading")),
    });
    expect(view.contentDOM.textContent).toContain("# Heading");
    // And the line the cursor left folds back up.
    expect(view.contentDOM.textContent).not.toContain("*emphasis*");
    view.destroy();
  });

  it("hides an HTML comment off the lines being edited, as the page does", () => {
    const doc = [
      "# Notes",
      "",
      "<!-- sources: the log -->",
      "",
      "A line <!-- an aside --> goes on.",
      "",
      "<!--",
      "a note over",
      "three lines",
      "-->",
      "",
      "<!-- before --> <b>after</b>",
      "",
      "```",
      "<!-- in a fence -->",
      "```",
      "",
      "<div>not a comment</div>",
      "",
      "End.",
    ].join("\n");
    const view = editor(doc);
    const text = () => view.contentDOM.textContent;
    for (const gone of [
      "sources:",
      "an aside",
      "a note over",
      "three lines",
      "before",
    ]) {
      expect(text()).not.toContain(gone);
    }
    expect(text()).toContain("A line  goes on.");
    expect(text()).toContain("<b>after</b>");
    expect(text()).toContain("<!-- in a fence -->");
    expect(text()).toContain("<div>not a comment</div>");
    // On the line being edited a comment shows as written, and the bytes
    // never changed.
    view.dispatch({
      selection: EditorSelection.cursor(doc.indexOf("an aside")),
    });
    expect(text()).toContain("A line <!-- an aside --> goes on.");
    view.dispatch({
      selection: EditorSelection.cursor(doc.indexOf("a note over")),
    });
    expect(text()).toContain("a note over");
    expect(text()).not.toContain("three lines");
    expect(docText(view.state)).toBe(doc);
    view.destroy();
  });

  it("leaves a section id line to the hidden id lines", () => {
    const doc = "## Night\n<!-- r:7f3a9c2e -->\n\nText.\n";
    const view = editor(doc);
    expect(view.contentDOM.textContent).toContain("<!-- r:7f3a9c2e -->");
    view.destroy();
  });

  it("leaves the frontmatter block undecorated", () => {
    const view = editor(DOC);
    expect(view.contentDOM.textContent).toContain("---");
    expect(view.contentDOM.textContent).toContain("title: Alpha");
    view.destroy();
  });

  it("renders a clickable checkbox that edits the marker text", () => {
    const view = editor(DOC, 0);
    const box = view.dom.querySelector<HTMLInputElement>(
      "input.cm-task-toggle",
    );
    expect(box).not.toBeNull();
    expect(box?.checked).toBe(false);
    box?.dispatchEvent(new MouseEvent("mousedown", { bubbles: true }));
    // `docText`, never `doc.toString()`: the read-back is the file's bytes.
    expect(docText(view.state)).toContain("- [x] a task");
    // And back again, off the freshly drawn box. The box is drawn off the
    // tree, and the reparse of that edit stops after 20 ms of wall clock: on a
    // loaded machine the box comes with the next parse, which this asks for.
    parsedView(view);
    const checked = view.dom.querySelector<HTMLInputElement>(
      "input.cm-task-toggle",
    );
    expect(checked?.checked).toBe(true);
    checked?.dispatchEvent(new MouseEvent("mousedown", { bubbles: true }));
    expect(docText(view.state)).toContain("- [ ] a task");
    view.destroy();
  });

  it("toggles a task on a CRLF document without disturbing its endings", () => {
    const crlf = DOC.replace(/\n/g, "\r\n");
    const view = editor(crlf, 0);
    view.dom
      .querySelector<HTMLInputElement>("input.cm-task-toggle")
      ?.dispatchEvent(new MouseEvent("mousedown", { bubbles: true }));
    // The edit is the three marker characters and nothing else: every other
    // byte of the file, line endings included, is where it was.
    expect(docText(view.state)).toBe(crlf.replace("- [ ]", "- [x]"));
    view.destroy();
  });

  it("keeps a fenced block's delimiters and info string visible", () => {
    const doc = "# Doc\n\n`inline` code.\n\n```ts\nconst a = 1;\n```\n\ntail\n";
    const view = editor(doc);
    // The fence is the only thing saying "this is a code block" until a
    // widget draws one, and folding it would strand `ts` as a paragraph.
    expect(view.contentDOM.textContent).toContain("```ts");
    expect(view.contentDOM.textContent).toContain("const a = 1;");
    // Inline backticks are a different CodeMark and still fold away.
    expect(view.contentDOM.textContent).toContain("inline");
    expect(view.contentDOM.textContent).not.toContain("`inline`");
    view.destroy();
  });

  it("decorates without touching the document, line endings included", () => {
    for (const doc of [DOC, DOC.replace(/\n/g, "\r\n")]) {
      const view = editor(doc, 0);
      expect(docText(view.state)).toBe(doc);
      // Every cursor position in turn: the decorations rebuild on each
      // selection change and none of them is allowed to be an edit.
      for (let at = 0; at <= view.state.doc.length; at += 1) {
        view.dispatch({ selection: EditorSelection.cursor(at) });
      }
      expect(docText(view.state)).toBe(doc);
      view.destroy();
    }
  });
});

describe("a checkbox redrawn after a short reparse", () => {
  it("comes back when the parse catches up, and the bytes are right", () => {
    const view = editor(DOC, 0);
    const now = Date.now.bind(Date);
    let ahead = 0;
    const spy = vi.spyOn(Date, "now").mockImplementation(() => {
      ahead += 1_000;
      return now() + ahead;
    });
    view.dom
      .querySelector<HTMLInputElement>("input.cm-task-toggle")
      ?.dispatchEvent(new MouseEvent("mousedown", { bubbles: true }));
    spy.mockImplementation(() => now() + ahead);
    expect(docText(view.state)).toContain("- [x] a task");
    expect(view.dom.querySelector("input.cm-task-toggle")).toBeNull();
    parsedView(view);
    expect(
      view.dom.querySelector<HTMLInputElement>("input.cm-task-toggle")?.checked,
    ).toBe(true);
    spy.mockRestore();
    view.destroy();
  });
});

describe("the not-applicable task state", () => {
  it("draws [~] as a struck box that a click marks open", () => {
    const doc = "# Doc\n\n- [~] skipped\n- [ ] open\n";
    const view = editor(doc, 0);
    const box = view.dom.querySelector<HTMLElement>(".cm-task-na");
    expect(box).not.toBeNull();
    expect(box?.getAttribute("aria-label")).toBe("Not applicable");
    expect(box?.tagName).not.toBe("INPUT");
    expect(view.contentDOM.textContent).not.toContain("[~]");
    // The open task keeps its own checkbox.
    expect(view.dom.querySelectorAll("input.cm-task-toggle").length).toBe(1);
    // A click writes the one character: the box becomes an open task.
    box?.dispatchEvent(new MouseEvent("mousedown", { bubbles: true }));
    expect(docText(view.state)).toBe("# Doc\n\n- [ ] skipped\n- [ ] open\n");
    expect(view.dom.querySelector(".cm-task-na")).toBeNull();
    view.destroy();
  });

  it("shows [~] as written on the line the cursor is on", () => {
    const doc = "# Doc\n\n- [~] skipped\n";
    const view = editor(doc, doc.indexOf("skipped"));
    expect(view.dom.querySelector(".cm-task-na")).toBeNull();
    expect(view.contentDOM.textContent).toContain("- [~] skipped");
    view.destroy();
  });
});
