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
