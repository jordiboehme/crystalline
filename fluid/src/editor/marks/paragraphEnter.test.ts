/**
 * Enter in a plain paragraph writes a new paragraph (a blank line), and
 * Shift+Enter a hard break (a backslash and the line break), so the page
 * reads as the preview shows it. Everywhere else Enter does what it did:
 * lists and quotes continue, and headings, code, tables, math, footnotes
 * and Raw get one line break. Every claim is on the bytes.
 */

import { completionStatus } from "@codemirror/autocomplete";
import { waitFor } from "@testing-library/react";
import { EditorSelection, EditorState, StateEffect } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { afterEach, describe, expect, it } from "vitest";

import {
  bytes,
  caret,
  cleanupMarkViews,
  keys,
  markView,
  setCaret,
  typeText,
  undoOnce,
} from "../../test/markKit";
import { parsedState } from "../../test/parse";
import { hiddenMarkLayer } from ".";
import { livePreview } from "../preview";
import { baseExtensions, lineSeparatorFor } from "../setup";

afterEach(cleanupMarkViews);

type Mode = "hidden" | "show";
const MODES: Mode[] = ["hidden", "show"];

function press(
  doc: string,
  at: number,
  key: string,
  options: { mode?: Mode; raw?: boolean } = {},
): string {
  const view = markView(doc, options);
  setCaret(view, at);
  keys(view, key);
  return bytes(view.state);
}

const enter = (doc: string, at: number, mode: Mode = "hidden") =>
  press(doc, at, "Enter", { mode });
const shiftEnter = (doc: string, at: number, mode: Mode = "hidden") =>
  press(doc, at, "Shift-Enter", { mode });

describe("Enter in a paragraph", () => {
  for (const mode of MODES) {
    it(`writes a new paragraph at the end of a line (${mode})`, () => {
      const view = markView("one", { mode });
      setCaret(view, 3);
      expect(keys(view, "Enter")).toBe(true);
      expect(bytes(view.state)).toBe("one\n\n");
      expect(caret(view)).toBe(5);
      typeText(view, "two");
      expect(bytes(view.state)).toBe("one\n\ntwo");
    });

    it(`splits a paragraph in the middle of a line (${mode})`, () => {
      expect(enter("one two", 4, mode)).toBe("one \n\ntwo");
    });

    it(`writes one break at the start of a line (${mode})`, () => {
      expect(enter("one\ntwo", 4, mode)).toBe("one\n\ntwo");
      expect(enter("one", 0, mode)).toBe("\none");
    });

    it(`keeps bold whole at the end of a bold line (${mode})`, () => {
      expect(enter("a **bold**", 10, mode)).toBe("a **bold**\n\n");
      expect(enter("a **bold**", 8, mode)).toBe("a **bold**\n\n");
    });

    it(`keeps a soft break inside a link, code or a bold word (${mode})`, () => {
      expect(enter("see [thepage](x) now", 8, mode)).toBe(
        "see [the\npage](x) now",
      );
      expect(enter("run `ab` now", 6, mode)).toBe("run `a\nb` now");
      expect(enter("**bold**", 4, mode)).toBe("**bo\nld**");
    });
  }

  it("leaves an empty line, a heading, code and a table to one break", () => {
    expect(enter("one\n\ntwo", 4)).toBe("one\n\n\ntwo");
    expect(enter("# Title", 7)).toBe("# Title\n");
    expect(enter("```\ncode\n```", 8)).toBe("```\ncode\n\n```");
    expect(enter("    code", 8)).toBe("    code\n    ");
    expect(enter("| a | b |\n| - | - |\n| 1 | 2 |", 29)).toBe(
      "| a | b |\n| - | - |\n| 1 | 2 |\n",
    );
  });

  it("leaves math and a footnote to one break", () => {
    expect(enter("$$\nx + y\n$$", 8)).toBe("$$\nx + y\n\n$$");
    expect(enter("[^1]: a note here", 17)).toBe("[^1]: a note here\n");
  });

  it("leaves lists and quotes to listEnter", () => {
    expect(enter("- a", 3)).toBe("- a\n- ");
    expect(enter("> q", 3)).toBe("> q\n> ");
  });

  it("writes at every caret, or leaves them all to one break", () => {
    const view = markView("one\n\ntwo");
    view.dispatch({
      effects: StateEffect.appendConfig.of(
        EditorState.allowMultipleSelections.of(true),
      ),
    });
    view.dispatch({
      selection: EditorSelection.create([
        EditorSelection.cursor(3),
        EditorSelection.cursor(8),
      ]),
    });
    keys(view, "Enter");
    expect(bytes(view.state)).toBe("one\n\n\n\ntwo\n\n");
    const mixed = markView("one\n\n# Two");
    mixed.dispatch({
      effects: StateEffect.appendConfig.of(
        EditorState.allowMultipleSelections.of(true),
      ),
    });
    mixed.dispatch({
      selection: EditorSelection.create([
        EditorSelection.cursor(3),
        EditorSelection.cursor(10),
      ]),
    });
    keys(mixed, "Enter");
    expect(bytes(mixed.state)).toBe("one\n\n\n# Two\n");
  });

  it("is one undo step", () => {
    const view = markView("one two");
    setCaret(view, 4);
    keys(view, "Enter");
    undoOnce(view);
    expect(bytes(view.state)).toBe("one two");
  });

  it("is one undo step in a room", () => {
    const view = markView("one two", { room: true });
    setCaret(view, 4);
    keys(view, "Enter");
    expect(bytes(view.state)).toBe("one \n\ntwo");
    undoOnce(view);
    expect(bytes(view.state)).toBe("one two");
  });

  it("keeps today's Enter in Raw", () => {
    expect(press("one", 3, "Enter", { raw: true })).toBe("one\n");
    expect(press("one", 3, "Shift-Enter", { raw: true })).toBe("one\n");
  });

  it("writes nothing on a read-only page", () => {
    const view = markView("one");
    view.dispatch({
      effects: StateEffect.appendConfig.of(EditorState.readOnly.of(true)),
    });
    setCaret(view, 3);
    keys(view, "Enter");
    keys(view, "Shift-Enter");
    expect(bytes(view.state)).toBe("one");
  });

  it("leaves Enter to an open completion", async () => {
    const view = markView("Intro.\n\n");
    setCaret(view, 8);
    typeText(view, "/war");
    await waitFor(() => {
      expect(completionStatus(view.state)).toBe("active");
    });
    // The open list takes keys after its interaction delay (75ms).
    await new Promise((resolve) => setTimeout(resolve, 120));
    expect(keys(view, "Enter")).toBe(true);
    expect(bytes(view.state)).toBe("Intro.\n\n> [!WARNING]\n> Text\n");
  });

  it("uses the document's line ending", () => {
    const view = new EditorView({
      state: parsedState(
        EditorState.create({
          doc: "one two\r\nx",
          extensions: [
            ...lineSeparatorFor("one two\r\nx"),
            ...baseExtensions(false),
            livePreview(),
            hiddenMarkLayer(),
          ],
        }),
      ),
      parent: document.body,
    });
    view.dispatch({ selection: EditorSelection.cursor(4) });
    keys(view, "Enter");
    expect(view.state.sliceDoc(0)).toBe("one \r\n\r\ntwo\r\nx");
    view.dispatch({ selection: EditorSelection.cursor(1) });
    keys(view, "Shift-Enter");
    expect(view.state.sliceDoc(0)).toBe("o\\\r\nne \r\n\r\ntwo\r\nx");
    view.destroy();
  });

  it("leaves a Reflection section's id line where it is", () => {
    const page = "## Night\n<!-- r:7f3a9c2e -->\n\nText.\n";
    const view = markView(page, { adapter: "reflection" });
    setCaret(view, page.indexOf("Text.") + 5);
    keys(view, "Enter");
    expect(bytes(view.state)).toBe(
      "## Night\n<!-- r:7f3a9c2e -->\n\nText.\n\n\n",
    );
  });
});

describe("Shift+Enter in a paragraph", () => {
  for (const mode of MODES) {
    it(`writes a hard break (${mode})`, () => {
      const view = markView("one", { mode });
      setCaret(view, 3);
      expect(keys(view, "Shift-Enter")).toBe(true);
      expect(bytes(view.state)).toBe("one\\\n");
      typeText(view, "two");
      expect(bytes(view.state)).toBe("one\\\ntwo");
      expect(shiftEnter("one two", 4, mode)).toBe("one \\\ntwo");
    });
  }

  it("writes only the break after a backslash that escapes nothing", () => {
    expect(shiftEnter("one\\", 4)).toBe("one\\\n");
    expect(shiftEnter("one\\\\", 5)).toBe("one\\\\\\\n");
  });

  it("is not handled at the start of a line or outside a paragraph", () => {
    expect(shiftEnter("one", 0)).toBe("\none");
    expect(shiftEnter("# Title", 7)).toBe("# Title\n");
    expect(shiftEnter("- a", 3)).toBe("- a\n");
  });

  it("is one undo step", () => {
    const view = markView("one two");
    setCaret(view, 4);
    keys(view, "Shift-Enter");
    undoOnce(view);
    expect(bytes(view.state)).toBe("one two");
  });
});

describe("the backslash of a hard break", () => {
  it("is faded where marks hide and plain where they show", () => {
    const view = markView("one\\\ntwo\n\nend");
    setCaret(view, 13);
    expect(
      [...view.contentDOM.querySelectorAll(".cm-hard-break")].map(
        (el) => el.textContent,
      ),
    ).toEqual(["\\"]);
    const shown = markView("one\\\ntwo\n\nend", { mode: "show" });
    setCaret(shown, 2);
    expect(shown.contentDOM.querySelectorAll(".cm-hard-break")).toHaveLength(0);
  });
});
