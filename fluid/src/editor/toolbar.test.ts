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
  CODE_SKELETON,
  MERMAID_SKELETON,
  ORDERED_ITEM,
  TABLE_SKELETON,
  cycleHeading,
  formattingKeymap,
  insertBlock,
  insertMarkdownLink,
  insertWikilink,
  selectToken,
  tableSkeleton,
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

  test("strikethrough wraps and unwraps the same words", () => {
    const v = solo("hello world", 0, 5);
    toggleInline(v, "~~");
    expect(docText(v.state)).toBe("~~hello~~ world");
    // The wrap keeps the words selected, so the second press is the same
    // gesture on the same text - the round trip a person actually makes.
    toggleInline(v, "~~");
    expect(docText(v.state)).toBe("hello world");
  });

  test("a bare cursor gets an empty strikethrough pair to type into", () => {
    const v = solo("hello world", 5);
    toggleInline(v, "~~");
    expect(docText(v.state)).toBe("hello~~~~ world");
    expect(v.state.selection.main.head).toBe(7);
  });

  test("strikethrough inside a longer tilde run nests rather than half-stripping", () => {
    // Three tildes on each side are not this command's pair. Taking two of
    // them would leave a stray `~` behind and break the markup that was
    // there; the exact-run rule nests instead, at the price of one undo.
    const v = solo("~~~hello~~~ world", 3, 8);
    toggleInline(v, "~~");
    expect(docText(v.state)).toBe("~~~~~hello~~~~~ world");
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

  test("quotes every selected line, then unquotes", () => {
    const v = solo("one\ntwo\n", 0, 7);
    toggleLinePrefix(v, "> ");
    expect(docText(v.state)).toBe("> one\n> two\n");
    toggleLinePrefix(v, "> ");
    expect(docText(v.state)).toBe("one\ntwo\n");
  });

  test("numbers every selected line with a literal 1., which markdown renumbers", () => {
    const v = solo("one\ntwo\n", 0, 7);
    toggleLinePrefix(v, "1. ", ORDERED_ITEM);
    expect(docText(v.state)).toBe("1. one\n1. two\n");
  });

  test("a renumbered list is still a list to the numbered toggle", () => {
    // What the author is looking at after markdown - or another editor - has
    // numbered the items in sequence. A toggle that only knew the literal
    // "1. " would strip the first line, prefix the rest and call it done.
    const v = solo("1. one\n2. two\n10. ten\n", 0, 21);
    expect(toggleLinePrefix(v, "1. ", ORDERED_ITEM)).toBe(true);
    expect(docText(v.state)).toBe("one\ntwo\nten\n");
  });
});

describe("insertWikilink", () => {
  test("wraps the selection in brackets with the cursor inside", () => {
    const v = solo("see Target here", 4, 10);
    insertWikilink(v);
    expect(docText(v.state)).toBe("see [[Target]] here");
  });
});

describe("insertMarkdownLink", () => {
  test("a bare cursor gets a whole link with its text selected", () => {
    const v = solo("see here", 4);
    insertMarkdownLink(v);
    expect(docText(v.state)).toBe("see [text](url)here");
    expect(
      v.state.sliceDoc(v.state.selection.main.from, v.state.selection.main.to),
    ).toBe("text");
  });

  test("a selection becomes the link text and the url is what is selected", () => {
    // The words are already written; the address is the thing still missing,
    // so that is where the next keystroke belongs.
    const v = solo("see Target here", 4, 10);
    insertMarkdownLink(v);
    expect(docText(v.state)).toBe("see [Target](url) here");
    expect(
      v.state.sliceDoc(v.state.selection.main.from, v.state.selection.main.to),
    ).toBe("url");
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

  test("a code block lands with the caret in its language slot", () => {
    const v = solo("prose line", 5);
    insertBlock(v, CODE_SKELETON);
    expect(docText(v.state)).toBe("prose line\n\n```\n\n```\n");
    // Right after the opening fence: the block's own first line is where
    // `insertBlock` leaves the caret, and on a bare fence that spot is the
    // language name - the one token a fresh code block is missing.
    expect(v.state.selection.main.head).toBe("prose line\n\n```".length);
  });

  test("a CRLF document keeps its endings through an insertion", () => {
    const v = solo("top\r\nbottom\r\n", 3);
    insertBlock(v, MERMAID_SKELETON);
    const text = docText(v.state);
    expect(text).toContain("```mermaid\r\n");
    expect(text).not.toMatch(/[^\r]\n```/);
  });

  test("a CRLF document leaves the caret at the end of the first line", () => {
    // A line break is ONE document position however many characters it is
    // written with, so a caret computed from the separator's length would sit
    // two positions past the header row in a CRLF buffer - inside the rule
    // line - while every LF test stayed green.
    const v = solo("top\r\nbottom\r\n", 3);
    insertBlock(v, TABLE_SKELETON);
    const { head } = v.state.selection.main;
    const line = v.state.doc.lineAt(head);
    expect(line.text).toBe("| Column | Column |");
    expect(head).toBe(line.to);
  });
});

describe("tableSkeleton and selection", () => {
  test("2x2 is byte-identical to the historical skeleton", () => {
    expect(tableSkeleton(2, 2)).toEqual([...TABLE_SKELETON]);
  });

  test("4x3 is four columns, header plus two data rows", () => {
    const lines = tableSkeleton(4, 3);
    expect(lines).toHaveLength(4); // header, rule, 2 data rows
    expect(lines[0]?.split("|").filter((c) => c.trim() !== "")).toHaveLength(4);
    expect(lines[1]).toBe("| --- | --- | --- | --- |");
    expect(lines[3]).toBe("|  |  |  |  |");
  });

  test("one column by one row is a header and its rule, nothing else", () => {
    // The grid's smallest cell: a table with no data row yet is still a
    // table, and refusing to draw one would make the corner cell a lie.
    expect(tableSkeleton(1, 1)).toEqual(["| Column |", "| --- |"]);
  });

  test("selectToken takes the first occurrence and nothing when absent", () => {
    const lines = tableSkeleton(2, 2);
    expect(selectToken(lines, "Column")).toEqual({ line: 0, from: 2, to: 8 });
    expect(selectToken(lines, "nowhere")).toBeNull();
  });

  test("insertBlock with a selection lands it on the token", () => {
    const v = solo("Text", 4);
    const lines = tableSkeleton(2, 2);
    insertBlock(v, lines, selectToken(lines, "Column"));
    const { main } = v.state.selection;
    expect(v.state.sliceDoc(main.from, main.to)).toBe("Column");
  });

  test("a selection on a later line lands there too", () => {
    // The line term of the mapping, which a first-line-only token cannot
    // exercise: every line before the selected one counts its own length and
    // its own break.
    const v = solo("Text", 4);
    const lines = ["```mermaid", "flowchart TD", "    A[First step]", "```"];
    insertBlock(v, lines, selectToken(lines, "First step"));
    const { main } = v.state.selection;
    expect(v.state.sliceDoc(main.from, main.to)).toBe("First step");
  });

  test("a CRLF document still selects the token", () => {
    const v = solo("Text\r\nMore", 4);
    const lines = tableSkeleton(3, 2);
    insertBlock(v, lines, selectToken(lines, "Column"));
    expect(
      v.state.sliceDoc(v.state.selection.main.from, v.state.selection.main.to),
    ).toBe("Column");
    expect(docText(v.state)).toContain("\r\n| Column | Column | Column |");
  });

  test("a token below the first line survives CRLF too", () => {
    const v = solo("Text\r\nMore", 4);
    const lines = tableSkeleton(2, 3);
    // Nothing in the skeleton repeats below the header, so this walks the
    // mapping over two breaks written with two characters each.
    insertBlock(v, lines, { line: 1, from: 2, to: 5 });
    expect(
      v.state.sliceDoc(v.state.selection.main.from, v.state.selection.main.to),
    ).toBe("---");
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

  test("a link from the mount-time caret lands in the body", () => {
    const v = folded(FOLDED_DOC, 0);
    insertMarkdownLink(v);
    expect(docText(v.state)).toBe(`${YAML}[text](url)\nBody line\n`);
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
