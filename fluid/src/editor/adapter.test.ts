/**
 * The two document adapters, through the shared block verbs.
 *
 * The verbs themselves are `toolbar.test.ts`'s; what is under test here is
 * what the Reflection adapter adds to them: the hidden section id lines stay
 * under their headings whatever a block verb does around them, and a heading
 * change that makes a section vanish asks first.
 */

import { history, undo } from "@codemirror/commands";
import type { Extension } from "@codemirror/state";
import { EditorSelection, EditorState } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { QueryClient } from "@tanstack/react-query";
import { afterEach, describe, expect, it } from "vitest";

import { engramAdapter, JOIN_SECTIONS, reflectionAdapter } from "./adapter";
import {
  CODE_SKELETON,
  cycleHeading,
  headingLevelAfter,
  insertBlock,
  insertMarkdownLink,
  setHeadingLevel,
  toggleInline,
  toggleLinePrefix,
} from "./toolbar";

const DOC =
  "# P\n\n## A\n<!-- r:aaaa0001 -->\n\none\n\n## B\n<!-- r:bbbb0002 -->\n\ntwo\n";

let view: EditorView | null = null;
afterEach(() => {
  view?.destroy();
  view = null;
});

function mount(
  doc: string,
  extensions: Extension[],
  anchor: number,
  head = anchor,
) {
  view = new EditorView({
    state: EditorState.create({
      doc,
      selection: EditorSelection.single(anchor, head),
      extensions: [EditorState.lineSeparator.of("\n"), ...extensions],
    }),
    parent: document.body,
  });
  return view;
}

/** The number of the line the main caret is on. */
function caretLine(v: EditorView): number {
  return v.state.doc.lineAt(v.state.selection.main.head).number;
}

describe("the reflection adapter", () => {
  const adapter = reflectionAdapter();

  it("puts a block inserted from a heading after its id line", () => {
    const v = mount(DOC, adapter.extensions, DOC.indexOf("## A") + 2);
    insertBlock(v, CODE_SKELETON);
    expect(v.state.doc.toString()).toBe(
      "# P\n\n## A\n<!-- r:aaaa0001 -->\n\n```\n\n```\n\n\none\n\n## B\n<!-- r:bbbb0002 -->\n\ntwo\n",
    );
  });

  it("leaves the id lines and their headings alone when a range is made a list", () => {
    const v = mount(
      DOC,
      adapter.extensions,
      DOC.indexOf("one"),
      DOC.indexOf("two") + 3,
    );
    toggleLinePrefix(v, "- ");
    expect(v.state.doc.toString()).toBe(
      "# P\n\n## A\n<!-- r:aaaa0001 -->\n\n- one\n- \n## B\n<!-- r:bbbb0002 -->\n- \n- two\n",
    );
    // And off again across the same range: the lines it skipped do not
    // count as lines without the prefix.
    v.dispatch({
      selection: EditorSelection.single(
        v.state.doc.toString().indexOf("one"),
        v.state.doc.length - 1,
      ),
    });
    toggleLinePrefix(v, "- ");
    expect(v.state.doc.toString()).toBe(DOC);
  });

  it("asks before a section heading becomes a level 3 heading, and drops its id once confirmed", () => {
    const v = mount(DOC, adapter.extensions, DOC.indexOf("## B") + 3);
    const line = caretLine(v);
    expect(
      adapter.confirmHeadingChange(
        v.state,
        line,
        headingLevelAfter(v.state, line, 3),
      ),
    ).toBe(JOIN_SECTIONS);
    expect(JOIN_SECTIONS).toBe(
      "This joins the section with the one above. The agent will see the change and rebuild its sources.",
    );
    cycleHeading(v, 3, { joinSections: true });
    expect(v.state.doc.toString()).toBe(
      "# P\n\n## A\n<!-- r:aaaa0001 -->\n\none\n\n### B\n\ntwo\n",
    );
  });

  it("asks before a section heading is taken off or made a title", () => {
    const v = mount(DOC, adapter.extensions, DOC.indexOf("## B") + 3);
    const line = caretLine(v);
    // Heading 2 on a level 2 heading takes the heading off.
    expect(headingLevelAfter(v.state, line, 2)).toBe(0);
    expect(adapter.confirmHeadingChange(v.state, line, 0)).toBe(JOIN_SECTIONS);
    expect(adapter.confirmHeadingChange(v.state, line, 1)).toBe(JOIN_SECTIONS);
    expect(adapter.confirmHeadingChange(v.state, line, 2)).toBeNull();
    cycleHeading(v, 2, { joinSections: true });
    expect(v.state.doc.toString()).toBe(
      "# P\n\n## A\n<!-- r:aaaa0001 -->\n\none\n\nB\n\ntwo\n",
    );
  });

  it("goes ahead without asking when a paragraph becomes a section heading", () => {
    const v = mount(DOC, adapter.extensions, DOC.indexOf("one"));
    const line = caretLine(v);
    expect(
      adapter.confirmHeadingChange(
        v.state,
        line,
        headingLevelAfter(v.state, line, 2),
      ),
    ).toBeNull();
    cycleHeading(v, 2);
    // No id line: the server mints the new section's id on save.
    expect(v.state.doc.toString()).toBe(DOC.replace("\none\n", "\n## one\n"));
  });

  it("setHeadingLevel joins a section with its id line in one transaction, one undo restores both", () => {
    const v = mount(
      DOC,
      [...adapter.extensions, history()],
      DOC.indexOf("## B") + 3,
    );
    expect(adapter.confirmHeadingChange(v.state, caretLine(v), 3)).toBe(
      JOIN_SECTIONS,
    );
    setHeadingLevel(v, 3, { joinSections: true });
    expect(v.state.doc.toString()).toBe(
      "# P\n\n## A\n<!-- r:aaaa0001 -->\n\none\n\n### B\n\ntwo\n",
    );
    undo(v);
    expect(v.state.doc.toString()).toBe(DOC);
  });

  it("setHeadingLevel on a non-section line asks nothing and writes ##", () => {
    const v = mount(DOC, adapter.extensions, DOC.indexOf("one"));
    expect(adapter.confirmHeadingChange(v.state, caretLine(v), 2)).toBeNull();
    setHeadingLevel(v, 2);
    expect(v.state.doc.toString()).toBe(DOC.replace("\none\n", "\n## one\n"));
  });

  it("keeps the id under the heading when the change was not confirmed", () => {
    const v = mount(DOC, adapter.extensions, DOC.indexOf("## B") + 3);
    cycleHeading(v, 3);
    expect(v.state.doc.toString()).toContain("### B\n<!-- r:bbbb0002 -->\n");
  });
});

describe("the verbs from a caret on a hidden id line", () => {
  const adapter = reflectionAdapter();
  const ID_FROM = DOC.indexOf("<!-- r:aaaa0001");
  const ID_TO = DOC.indexOf("-->", ID_FROM) + "-->".length;
  const ID_LINE = "\n<!-- r:aaaa0001 -->\n";

  // The buffer opened with the caret there, which no selection filter saw:
  // the verbs step off the id line themselves.
  const verbs: [string, (v: EditorView) => boolean, string, string][] = [
    ["Bold", (v) => toggleInline(v, "**"), "## A****", "\n****\none"],
    ["Italic", (v) => toggleInline(v, "*"), "## A**", "\n**\none"],
    ["Strikethrough", (v) => toggleInline(v, "~~"), "## A~~~~", "\n~~~~\none"],
    ["Inline code", (v) => toggleInline(v, "`"), "## A``", "\n``\none"],
    ["Link", insertMarkdownLink, "## A[text](url)", "\n[text](url)\none"],
  ];
  for (const [name, run, atStart, atEnd] of verbs) {
    it(`${name} leaves the id line as it was, from either end`, () => {
      const one = mount(DOC, adapter.extensions, ID_FROM);
      run(one);
      expect(one.state.doc.toString()).toContain(`${atStart}${ID_LINE}`);
      one.destroy();
      const two = mount(DOC, adapter.extensions, ID_TO);
      run(two);
      expect(two.state.doc.toString()).toContain(
        `## A${ID_LINE.slice(0, -1)}${atEnd}`,
      );
    });
  }

  it("a heading change from the id line's end makes the line below a heading", () => {
    const v = mount(DOC, adapter.extensions, ID_TO);
    cycleHeading(v, 2);
    expect(v.state.doc.toString()).toContain(`## A${ID_LINE}## \none`);
  });

  it("a heading change from the id line's start is about the heading above", () => {
    const v = mount(DOC, adapter.extensions, ID_FROM);
    cycleHeading(v, 3);
    expect(v.state.doc.toString()).toContain(`### A${ID_LINE}`);
  });
});

describe("the engram adapter", () => {
  const adapter = engramAdapter({
    domain: "eng",
    client: new QueryClient(),
    vocab: () => null,
  });

  it("never asks before a heading change", () => {
    const v = mount(DOC, adapter.extensions, DOC.indexOf("## B") + 3);
    expect(adapter.confirmHeadingChange(v.state, caretLine(v), 3)).toBeNull();
    expect(adapter.confirmHeadingChange(v.state, caretLine(v), 0)).toBeNull();
  });

  it("treats an id-shaped comment as ordinary text", () => {
    const v = mount(
      DOC,
      adapter.extensions,
      DOC.indexOf("one"),
      DOC.length - 1,
    );
    toggleLinePrefix(v, "> ");
    expect(v.state.doc.toString()).toContain("> <!-- r:bbbb0002 -->\n");
  });
});
