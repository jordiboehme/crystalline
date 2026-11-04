/**
 * Where a dropped file or a moved picture lands: after the whole block under
 * the pointer, never inside it.
 *
 * Every case checks the landing and then the upload path itself: `insertBlock`
 * run from a caret at the landing's `caret`, against a document written out by
 * hand, so a landing that is wrong in the same way as the insertion cannot
 * pass by agreeing with it.
 */

import type { Extension } from "@codemirror/state";
import { EditorSelection, EditorState } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { afterEach, describe, expect, it } from "vitest";

import { parsedState } from "../../test/parse";
import { frontmatterFold } from "../frontmatterFold";
import type { GridTable } from "../grid/structure";
import { drawnGridTables } from "../grid/draws";
import { hiddenIdLines } from "../hiddenIdLines";
import { baseExtensions, docText } from "../setup";
import { insertBlock } from "../toolbar";
import { dropLanding } from "./landing";

const PICTURE = "![x](assets/p.png)";

let view: EditorView | null = null;
afterEach(() => {
  view?.destroy();
  view = null;
});

function stateOf(doc: string, extra: Extension[] = []): EditorState {
  return parsedState(
    EditorState.create({
      doc,
      extensions: [
        EditorState.lineSeparator.of("\n"),
        ...baseExtensions(false),
        ...extra,
      ],
    }),
  );
}

/** Runs today's upload insertion from `caret` and answers the document. */
function uploadFrom(state: EditorState, caret: number): string {
  view = new EditorView({
    state: state.update({ selection: EditorSelection.cursor(caret) }).state,
    parent: document.body,
  });
  expect(insertBlock(view, [PICTURE])).toBe(true);
  return docText(view.state);
}

describe("dropLanding", () => {
  it("lands a plain line after itself", () => {
    const doc = "one\n\ntwo\n";
    const state = stateOf(doc);
    expect(dropLanding(state, 1)).toEqual({ line: 1, pos: 3, caret: 3 });
    const after = uploadFrom(state, 3);
    expect(after).toBe(`one\n\n${PICTURE}\n\n\ntwo\n`);
    expect(after.indexOf(PICTURE)).toBe(3 + 2);
  });

  it("lands after the whole paragraph, so a bold over two lines stays whole", () => {
    const doc = "**a\nb**\n\nc\n";
    const state = stateOf(doc);
    expect(dropLanding(state, 1)).toEqual({ line: 2, pos: 7, caret: 7 });
    expect(uploadFrom(state, 7)).toBe(`**a\nb**\n\n${PICTURE}\n\n\nc\n`);
  });

  it("lands after a whole list, never inside it", () => {
    const doc = "- a\n- b\n- c\n\nend\n";
    const state = stateOf(doc);
    expect(dropLanding(state, doc.indexOf("b"))).toEqual({
      line: 3,
      pos: 11,
      caret: 11,
    });
  });

  it("lands a blank line right below a block after that block", () => {
    // Written into the blank line itself, the picture would join the
    // paragraph above it.
    const doc = "one\n\ntwo\n";
    const state = stateOf(doc);
    expect(dropLanding(state, 4)).toEqual({ line: 1, pos: 3, caret: 3 });
  });

  it("lands a second blank line on itself", () => {
    const doc = "one\n\n\ntwo\n";
    const state = stateOf(doc);
    expect(dropLanding(state, 5)).toEqual({ line: 3, pos: 5, caret: 5 });
    expect(uploadFrom(state, 5)).toBe(`one\n\n${PICTURE}\n\ntwo\n`);
  });

  it("lands a table after its last line", () => {
    const doc = "intro\n\n| a | b |\n| --- | --- |\n| 1 | 2 |\n\nend\n";
    const state = stateOf(doc);
    const last = doc.indexOf("| 1 | 2 |") + "| 1 | 2 |".length;
    expect(dropLanding(state, doc.indexOf("| a"))).toEqual({
      line: 5,
      pos: last,
      caret: last,
    });
    const after = uploadFrom(state, last);
    expect(after).toBe(
      `intro\n\n| a | b |\n| --- | --- |\n| 1 | 2 |\n\n${PICTURE}\n\n\nend\n`,
    );
    expect(after.indexOf(PICTURE)).toBe(last + 2);
  });

  it("lands a table the grid still draws from its own lines", () => {
    // The tree reads three lines of one paragraph; the grid says it draws the
    // first two as a table (D58). The drawn table wins.
    const doc = "a\nb\nc\n\nd\n";
    const fake = { from: 0, to: 3 } as unknown as GridTable;
    const state = stateOf(doc, [drawnGridTables.of(() => [fake])]);
    expect(dropLanding(state, 0)).toEqual({ line: 2, pos: 3, caret: 3 });
    expect(dropLanding(stateOf(doc), 0)).toEqual({ line: 3, pos: 5, caret: 5 });
  });

  it("lands a fence after its closing line", () => {
    const doc = "a\n\n```\ncode\n\nmore\n```\n\nb\n";
    const state = stateOf(doc);
    const close = doc.lastIndexOf("```") + 3;
    expect(dropLanding(state, doc.indexOf("code"))).toEqual({
      line: 7,
      pos: close,
      caret: close,
    });
    expect(uploadFrom(state, close)).toBe(
      `a\n\n\`\`\`\ncode\n\nmore\n\`\`\`\n\n${PICTURE}\n\n\nb\n`,
    );
  });

  it("lands a Reflection heading below its id line", () => {
    const doc = "## H\n<!-- r:7f3a9c2e -->\n\nBody.\n";
    const state = stateOf(doc, [hiddenIdLines()]);
    const idEnd = doc.indexOf("-->") + 3;
    const landing = { line: 2, pos: idEnd, caret: 4 };
    expect(dropLanding(state, 1)).toEqual(landing);
    // A position on the hidden id line itself lands there too.
    expect(dropLanding(state, idEnd - 2)).toEqual(landing);
    const after = uploadFrom(state, landing.caret);
    expect(after).toBe(`## H\n<!-- r:7f3a9c2e -->\n\n${PICTURE}\n\n\nBody.\n`);
    expect(after.indexOf(PICTURE)).toBe(idEnd + 2);
  });

  it("lands an id line that ends the page below it", () => {
    const doc = "## H\n<!-- r:7f3a9c2e -->";
    const state = stateOf(doc, [hiddenIdLines()]);
    expect(dropLanding(state, 1)).toEqual({
      line: 2,
      pos: doc.length,
      caret: 4,
    });
    expect(uploadFrom(state, 4)).toBe(`${doc}\n\n${PICTURE}\n`);
  });

  it("lands folded frontmatter after the first body block", () => {
    const doc = "---\ntitle: T\n---\nBody line\nmore\n\nend\n";
    const state = stateOf(doc, [frontmatterFold()]);
    const end = doc.indexOf("more") + 4;
    expect(dropLanding(state, 5)).toEqual({ line: 5, pos: end, caret: end });
    expect(uploadFrom(state, end)).toBe(
      `---\ntitle: T\n---\nBody line\nmore\n\n${PICTURE}\n\n\nend\n`,
    );
  });

  it("lands a blank first body line on itself", () => {
    const doc = "---\ntitle: T\n---\n\nBody line\n";
    const state = stateOf(doc, [frontmatterFold()]);
    const blank = doc.indexOf("\n\n") + 1;
    expect(dropLanding(state, 2)).toEqual({
      line: 4,
      pos: blank,
      caret: blank,
    });
    expect(uploadFrom(state, blank)).toBe(
      `---\ntitle: T\n---\n${PICTURE}\n\nBody line\n`,
    );
  });

  it("refuses folded frontmatter with no body line", () => {
    const state = stateOf("---\ntitle: T\n---", [frontmatterFold()]);
    expect(dropLanding(state, 5)).toBeNull();
  });

  it("reads frontmatter that is not folded as text", () => {
    const doc = "---\ntitle: T\n---\n\nBody\n";
    expect(dropLanding(stateOf(doc), doc.indexOf("Body"))?.line).toBe(5);
  });
});
