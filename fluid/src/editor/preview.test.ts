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
import { describe, expect, it } from "vitest";

import { parsedState } from "../test/parse";
import { frontmatterRegion } from "./frontmatterRegion";
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
    // And back again, off the freshly drawn box.
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
