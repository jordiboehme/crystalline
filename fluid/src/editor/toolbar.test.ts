/**
 * The formatting commands, exercised on real views: what a toolbar button and
 * its shortcut both run.
 *
 * The keymap case dispatches an actual keydown rather than calling the command,
 * because what is under test there is precedence - whether this keymap beats
 * the one `baseExtensions` already installed - and calling the command proves
 * nothing about that.
 */

import { EditorSelection, EditorState } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { afterEach, describe, expect, test } from "vitest";
import { yCollab } from "y-codemirror.next";
import { Awareness } from "y-protocols/awareness";
import * as Y from "yjs";

import { frontmatterFold, unfoldEffect } from "./frontmatterFold";
import { baseExtensions, docText, lineSeparatorFor } from "./setup";
import {
  MERMAID_SKELETON,
  TABLE_SKELETON,
  cycleHeading,
  formattingKeymap,
  insertBlock,
  insertWikilink,
  toggleInline,
  toggleLinePrefix,
} from "./toolbar";

let view: EditorView | null = null;
afterEach(() => {
  view?.destroy();
  view = null;
});

function solo(doc: string, anchor: number, head = anchor): EditorView {
  view = new EditorView({
    state: EditorState.create({
      doc,
      selection: EditorSelection.single(anchor, head),
      extensions: [...lineSeparatorFor(doc)],
    }),
    parent: document.body,
  });
  return view;
}

describe("toggleInline", () => {
  test("wraps a selection and keeps it selected", () => {
    const v = solo("hello world", 0, 5);
    toggleInline(v, "**");
    expect(docText(v.state)).toBe("**hello** world");
    expect(
      v.state.sliceDoc(v.state.selection.main.from, v.state.selection.main.to),
    ).toBe("hello");
  });

  test("unwraps an already-wrapped selection", () => {
    const v = solo("**hello** world", 2, 7);
    toggleInline(v, "**");
    expect(docText(v.state)).toBe("hello world");
  });

  test("italic over a bold word nests instead of eating the bold", () => {
    // What a double-click inside `**hello**` selects. A neighbour sniff that
    // only looked one character out would see a `*` on each side, take it for
    // its own italic pair and unwrap it - the bold silently gone.
    const v = solo("**hello** world", 2, 7);
    toggleInline(v, "*");
    expect(docText(v.state)).toBe("***hello*** world");
    expect(
      v.state.sliceDoc(v.state.selection.main.from, v.state.selection.main.to),
    ).toBe("hello");
  });

  test("bold over an italic word nests too", () => {
    const v = solo("*hello* world", 1, 6);
    toggleInline(v, "**");
    expect(docText(v.state)).toBe("***hello*** world");
  });

  test("a fence beside inline code is not read as a code span", () => {
    const v = solo("``a`` b", 2, 3);
    toggleInline(v, "`");
    expect(docText(v.state)).toBe("```a``` b");
  });
});

describe("cycleHeading", () => {
  test("adds, switches and removes the mark", () => {
    const v = solo("Title line", 3);
    cycleHeading(v, 2);
    expect(docText(v.state)).toBe("## Title line");
    cycleHeading(v, 3);
    expect(docText(v.state)).toBe("### Title line");
    cycleHeading(v, 3);
    expect(docText(v.state)).toBe("Title line");
  });
});

describe("toggleLinePrefix", () => {
  test("prefixes every selected line once, then removes", () => {
    const v = solo("one\ntwo\n", 0, 7);
    toggleLinePrefix(v, "- ");
    expect(docText(v.state)).toBe("- one\n- two\n");
    toggleLinePrefix(v, "- ");
    expect(docText(v.state)).toBe("one\ntwo\n");
  });
});

describe("insertWikilink", () => {
  test("wraps the selection in brackets with the cursor inside", () => {
    const v = solo("see Target here", 4, 10);
    insertWikilink(v);
    expect(docText(v.state)).toBe("see [[Target]] here");
  });
});

describe("insertBlock", () => {
  test("a table lands on its own lines below the cursor line", () => {
    const v = solo("prose line", 5);
    insertBlock(v, TABLE_SKELETON);
    expect(docText(v.state)).toBe(
      "prose line\n\n| Column | Column |\n| --- | --- |\n|  |  |\n",
    );
  });

  test("a CRLF document keeps its endings through an insertion", () => {
    const v = solo("top\r\nbottom\r\n", 3);
    insertBlock(v, MERMAID_SKELETON);
    const text = docText(v.state);
    expect(text).toContain("```mermaid\r\n");
    expect(text).not.toMatch(/[^\r]\n```/);
  });
});

describe("formattingKeymap", () => {
  test("a real Mod-i keydown lands italic, beating defaultKeymap's Mod-i", () => {
    // baseExtensions carries defaultKeymap, whose own Mod-i binding
    // (selectParentSyntax) would swallow the key at equal precedence; this
    // dispatches through the real keymap machinery, not the command directly.
    view = new EditorView({
      state: EditorState.create({
        doc: "hello world",
        selection: EditorSelection.single(0, 5),
        extensions: [...baseExtensions(false), formattingKeymap],
      }),
      parent: document.body,
    });
    view.contentDOM.dispatchEvent(
      new KeyboardEvent("keydown", {
        key: "i",
        ctrlKey: true, // Mod is Ctrl on jsdom's non-Mac userAgent
        bubbles: true,
        cancelable: true,
      }),
    );
    expect(docText(view.state)).toBe("*hello* world");
  });
});

/**
 * The buffer as the engram editor draws it in preview mode: a frontmatter
 * block behind its summary chip, and - because `CmEditor` sets no initial
 * selection - a caret sitting at position 0, INSIDE that folded block.
 */
const FOLDED_DOC = "---\ntitle: T\nstatus: draft\n---\n\nBody line\n";
const YAML = "---\ntitle: T\nstatus: draft\n---\n";

function folded(doc: string, anchor: number, head = anchor): EditorView {
  view = new EditorView({
    state: EditorState.create({
      doc,
      selection: EditorSelection.single(anchor, head),
      extensions: [...lineSeparatorFor(doc), frontmatterFold()],
    }),
    parent: document.body,
  });
  return view;
}

describe("the folded frontmatter", () => {
  test("a verb run from the mount-time caret lands in the body", () => {
    const v = folded(FOLDED_DOC, 0);
    cycleHeading(v, 2);
    // The yaml is untouched and the mark went onto the first line the author
    // can actually see.
    expect(docText(v.state)).toBe(`${YAML}## \nBody line\n`);
  });

  test("a table from the mount-time caret lands after the block", () => {
    const v = folded(FOLDED_DOC, 0);
    insertBlock(v, TABLE_SKELETON);
    expect(docText(v.state)).toBe(
      `${YAML}| Column | Column |\n| --- | --- |\n|  |  |\n\nBody line\n`,
    );
  });

  test("a selection spanning the block is refused rather than moved", () => {
    // Select-all then bold. Silently relocating a selection somebody made on
    // purpose would be worse than doing nothing.
    const v = folded(FOLDED_DOC, 0, FOLDED_DOC.length);
    expect(toggleInline(v, "**")).toBe(false);
    expect(docText(v.state)).toBe(FOLDED_DOC);
  });

  test("nothing is guarded once the block is unfolded", () => {
    const v = folded(FOLDED_DOC, 0);
    v.dispatch({ effects: unfoldEffect.of(true) });
    cycleHeading(v, 2);
    // The yaml is on screen now, so it is ordinary text to format.
    expect(docText(v.state)).toBe(`## ${FOLDED_DOC}`);
  });

  test("Raw mode has no fold and formats the frontmatter like any text", () => {
    // Raw is the same buffer with the preview layers - the fold among them -
    // reconfigured away, so the guard must not fire from mode assumptions.
    const v = solo(FOLDED_DOC, 4, 9);
    expect(toggleInline(v, "**")).toBe(true);
    expect(docText(v.state)).toBe(
      "---\n**title**: T\nstatus: draft\n---\n\nBody line\n",
    );
  });
});

describe("in a room", () => {
  test("a toolbar insertion reaches the shared text exactly once", () => {
    const ydoc = new Y.Doc();
    const ytext = ydoc.getText("content");
    ytext.insert(0, "---\ntitle: T\n---\n\nBody\n");
    const awareness = new Awareness(ydoc);
    view = new EditorView({
      state: EditorState.create({
        doc: ytext.toJSON(),
        extensions: [
          EditorState.lineSeparator.of("\n"),
          yCollab(ytext, awareness),
        ],
      }),
      parent: document.body,
    });
    view.dispatch({ selection: { anchor: view.state.doc.length } });
    insertBlock(view, TABLE_SKELETON);
    const shared = ytext.toJSON(); // Y.Text read: sanctioned, LF space by design
    expect(shared.split("| Column | Column |").length - 1).toBe(1);
    expect(shared).toBe(docText(view.state));
  });
});
