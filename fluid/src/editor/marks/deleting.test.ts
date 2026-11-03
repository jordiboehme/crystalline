/**
 * Backspace and Delete over hidden marks remove exactly the visible character
 * the person sees, plus the marks that would otherwise be left unpaired, and
 * a range deletion a person makes keeps every mark whose partner survives.
 * Every edit runs solo and in a room; show mode and Raw keep today's keys on
 * the line the caret touches.
 */

import {
  EditorSelection,
  EditorState,
  StateEffect,
  Transaction,
} from "@codemirror/state";
import {
  deleteGroupBackward,
  deleteGroupForward,
  deleteLineBoundaryBackward,
  deleteToLineEnd,
} from "@codemirror/commands";
import type { EditorView } from "@codemirror/view";
import { afterEach, beforeAll, describe, expect, it } from "vitest";

import { surfaceExtensions } from "../surface";
import { engramAdapter } from "../adapter";
import { QueryClient } from "@tanstack/react-query";
import { Compartment } from "@codemirror/state";
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
import { cleanupCells, openAt } from "../../test/cellKit";
import { parsedState } from "../../test/parse";
import { prefetchGrid } from "../grid/loader";
import { hiddenMarkLayerOn } from "./mode";
import { deleteVisible } from "./deleting";

afterEach(cleanupMarkViews);

/** A cut as the browser sends it; what reached the clipboard comes back. */
function cut(view: EditorView): string {
  const data = new Map<string, string>();
  const event = new Event("cut", { bubbles: true, cancelable: true });
  Object.defineProperty(event, "clipboardData", {
    value: {
      clearData: () => {
        data.clear();
      },
      setData: (type: string, value: string) => data.set(type, value),
      getData: (type: string) => data.get(type) ?? "",
    },
  });
  view.contentDOM.dispatchEvent(event);
  return data.get("text/plain") ?? "";
}

function select(view: EditorView, from: number, to: number): void {
  view.dispatch({
    selection: EditorSelection.range(from, to),
    userEvent: "select.pointer",
  });
}

describe.each([
  { name: "solo", room: false },
  { name: "in a room", room: true },
])("hidden mode, $name", ({ room }) => {
  const view = (text: string) => markView(text, { room });

  it("Backspace after a bold word's last letter removes the letter", () => {
    const v = view("a **bold** c");
    setCaret(v, 8);
    keys(v, "Backspace");
    expect(bytes(v.state)).toBe("a **bol** c");
    expect(caret(v)).toBe(7);
  });

  it("removes a pair whose last visible letter goes", () => {
    const v = view("a **b** c");
    setCaret(v, 5);
    keys(v, "Backspace");
    expect(bytes(v.state)).toBe("a  c");
    expect(caret(v)).toBe(2);
    const w = view("a **b** c");
    setCaret(w, 2);
    keys(w, "Delete");
    expect(bytes(w.state)).toBe("a  c");
    expect(caret(w)).toBe(2);
  });

  it("Backspace before an opening mark removes the letter before it", () => {
    const v = view("a **bold** c");
    setCaret(v, 2);
    keys(v, "Backspace");
    expect(bytes(v.state)).toBe("a**bold** c");
  });

  it("Delete before a closing mark removes the letter after it", () => {
    const v = view("a **bold** c");
    setCaret(v, 8);
    keys(v, "Delete");
    expect(bytes(v.state)).toBe("a **bold**c");
  });

  it("Backspace at a heading's visible start removes the prefix, one undo restores it", () => {
    const v = view("## Night\n\nText.");
    setCaret(v, 3);
    keys(v, "Backspace");
    expect(bytes(v.state)).toBe("Night\n\nText.");
    expect(caret(v)).toBe(0);
    undoOnce(v);
    expect(bytes(v.state)).toBe("## Night\n\nText.");
  });

  it("the prefix is its own undo step, apart from the typing before it", () => {
    const v = view("## Night");
    setCaret(v, 8);
    typeText(v, "s");
    setCaret(v, 3);
    keys(v, "Backspace");
    expect(bytes(v.state)).toBe("Nights");
    undoOnce(v);
    expect(bytes(v.state)).toBe("## Nights");
  });

  it("Backspace at a list item's visible start removes the bullet and the box", () => {
    const task = view("- [ ] task");
    setCaret(task, 6);
    keys(task, "Backspace");
    expect(bytes(task.state)).toBe("task");
    const item = view("  - item");
    setCaret(item, 4);
    keys(item, "Backspace");
    expect(bytes(item.state)).toBe("  item");
    const quote = view("> > q");
    setCaret(quote, 4);
    keys(quote, "Backspace");
    expect(bytes(quote.state)).toBe("> q");
  });

  it("Backspace at a visible line start with no prefix joins the lines", () => {
    const v = view("a\n**b**");
    setCaret(v, 4);
    expect(caret(v)).toBe(4);
    keys(v, "Backspace");
    expect(bytes(v.state)).toBe("a**b**");
  });

  it("Delete at a visible line end joins the lines and keeps the marks", () => {
    const v = view("x **b**\ny");
    setCaret(v, 5);
    keys(v, "Delete");
    expect(bytes(v.state)).toBe("x **b**y");
  });

  it("Backspace right after a wikilink chip removes the link whole", () => {
    const v = view("x [[Target]] y");
    setCaret(v, 12);
    keys(v, "Backspace");
    expect(bytes(v.state)).toBe("x  y");
  });

  it("a selection deleted, cut and typed over keeps the surviving partner", () => {
    const deleted = view("ab **cd** ef");
    select(deleted, 1, 6);
    keys(deleted, "Backspace");
    expect(bytes(deleted.state)).toBe("a**d** ef");
    const cutOne = view("ab **cd** ef");
    select(cutOne, 1, 6);
    expect(cut(cutOne)).toBe("b **c");
    expect(bytes(cutOne.state)).toBe("a**d** ef");
    const typed = view("ab **cd** ef");
    select(typed, 1, 6);
    typeText(typed, "X");
    expect(bytes(typed.state)).toBe("aX**d** ef");
    expect(caret(typed)).toBe(2);
  });

  it("an autocorrect replacement keeps the closing mark", () => {
    const v = view("ab **cd** ef");
    v.dispatch({
      changes: { from: 5, to: 9, insert: "the" },
      userEvent: "input.type",
    });
    expect(bytes(v.state)).toBe("ab **the** ef");
  });

  it("a command's bare input deletion passes as it is", () => {
    const v = view("ab **cd** ef");
    v.dispatch({ changes: { from: 3, to: 5 }, userEvent: "input" });
    expect(bytes(v.state)).toBe("ab cd** ef");
  });

  it("a deleted heading prefix goes with the selection", () => {
    const v = view("## Title");
    v.dispatch({
      changes: { from: 0, to: 3 },
      userEvent: "delete.selection",
    });
    expect(bytes(v.state)).toBe("Title");
  });

  it("one undo takes a split deletion back", () => {
    const v = view("ab **cd** ef");
    select(v, 1, 6);
    keys(v, "Backspace");
    undoOnce(v);
    expect(bytes(v.state)).toBe("ab **cd** ef");
  });
});

it("deletes as at the drawn place when the caret was never normalized", () => {
  const view = markView("a **bold** c");
  // A state with the raw selection at 10 that no filter has seen.
  const state = parsedState(
    EditorState.create({
      doc: "a **bold** c",
      selection: EditorSelection.cursor(10),
      extensions: [
        EditorState.lineSeparator.of("\n"),
        ...surfaceExtensions({
          adapter: engramAdapter({
            domain: "eng",
            client: new QueryClient(),
            vocab: () => null,
          }),
          dark: false,
          history: true,
          screen: [],
          preview: new Compartment(),
          raw: false,
          images: [],
          uploads: [],
          onTableContext: () => {},
          onImageContext: () => {},
          onElementContext: () => {},
          onFloatingContext: () => {},
        }),
      ],
    }),
  );
  view.setState(state);
  expect(caret(view)).toBe(10);
  expect(deleteVisible(view, false)).toBe(true);
  expect(bytes(view.state)).toBe("a **bol** c");
  expect(caret(view)).toBe(7);
});

describe("show mode, on the line the caret touches", () => {
  it("Backspace after the stars removes one star, as today", () => {
    const v = markView("a **bold** c", { mode: "show" });
    setCaret(v, 10);
    keys(v, "Backspace");
    expect(bytes(v.state)).toBe("a **bold* c");
  });

  it("a selection deleted there is today's deletion", () => {
    const v = markView("ab **cd** ef", { mode: "show" });
    select(v, 1, 6);
    keys(v, "Backspace");
    expect(bytes(v.state)).toBe("ad** ef");
  });
});

it("Raw: Backspace after the stars removes one star", () => {
  const v = markView("a **bold** c", { raw: true });
  setCaret(v, 10);
  keys(v, "Backspace");
  expect(bytes(v.state)).toBe("a **bold* c");
});

it("a remote deletion is never rewritten", () => {
  const v = markView("ab **cd** ef");
  v.dispatch({
    changes: { from: 1, to: 6 },
    userEvent: "delete.selection",
    annotations: Transaction.remote.of(true),
  });
  expect(bytes(v.state)).toBe("ad** ef");
});

describe("in a table cell, a hidden escape is left to the cell's own rule", () => {
  beforeAll(async () => {
    await prefetchGrid();
  });
  afterEach(cleanupCells);

  it.each([
    { name: "an escaped pipe", text: "| a\\|b | c |\n| - | - |\n" },
    { name: "a backslash pair", text: "| a\\\\b | c |\n| - | - |\n" },
  ])("$name", ({ text }) => {
    const main = markView(text);
    const cell = openAt(main, 5).view;
    // The cell has no hidden-mark layer until it is given one; with it, the
    // key still defers on the escape the cell keeps whole.
    cell.dispatch({
      effects: StateEffect.appendConfig.of(hiddenMarkLayerOn.of(true)),
    });
    cell.dispatch({ selection: EditorSelection.cursor(5) });
    expect(deleteVisible(cell, false)).toBe(false);
    cell.dispatch({ selection: EditorSelection.cursor(3) });
    expect(deleteVisible(cell, true)).toBe(false);
    expect(bytes(main.state)).toBe(text);
  });
});

describe.each([
  { name: "solo", room: false },
  { name: "in a room", room: true },
])("word and line deletes keep the partner, $name", ({ room }) => {
  const view = (text: string, at: number) => {
    const v = markView(text, { room });
    setCaret(v, at);
    return v;
  };

  it.each([
    {
      name: "deleteGroupBackward",
      command: deleteGroupBackward,
      text: "a **bold** c",
      at: 11,
      // The space goes, the closing mark stays.
      bytes: "a **bold**c",
    },
    {
      name: "deleteGroupForward",
      command: deleteGroupForward,
      text: "a **bold** c",
      at: 2,
      // Only the hidden opening mark lay in reach: nothing is left to take.
      bytes: "a **bold** c",
    },
    {
      name: "deleteToLineEnd",
      command: deleteToLineEnd,
      text: "a **bold** c",
      at: 5,
      bytes: "a **b**",
    },
    {
      name: "deleteLineBoundaryBackward",
      command: deleteLineBoundaryBackward,
      text: "a **bold**",
      at: 8,
      // The opening mark stays for its partner, then the pair is empty.
      bytes: "",
    },
  ])("$name", ({ command, text, at, bytes: expected }) => {
    const v = view(text, at);
    command(v);
    expect(bytes(v.state)).toBe(expected);
  });

  it("Mod+Backspace and Mod+Delete through the keymap", () => {
    const back = view("a **bold** c", 11);
    keys(back, "Mod-Backspace");
    expect(bytes(back.state)).toBe("a **bold**c");
    const forward = view("a **bold** c", 2);
    keys(forward, "Mod-Delete");
    // Only the hidden opening mark lay in reach, so the word after it goes
    // too, and the emptied pair with it.
    expect(bytes(forward.state)).toBe("a  c");
  });

  it("a selection delete that empties a pair takes both marks", () => {
    const bold = markView("a **bold** c", { room });
    select(bold, 4, 8);
    keys(bold, "Backspace");
    expect(bytes(bold.state)).toBe("a  c");
    const link = markView("a [x](http://x) c", { room });
    select(link, 3, 4);
    keys(link, "Delete");
    expect(bytes(link.state)).toBe("a  c");
    const typed = markView("a **bold** c", { room });
    select(typed, 4, 8);
    typeText(typed, "X");
    expect(bytes(typed.state)).toBe("a **X** c");
  });

  it("a pair across two lines keeps its partner", () => {
    const v = markView("**a\nb** c", { room });
    select(v, 1, 3);
    keys(v, "Backspace");
    expect(bytes(v.state)).toBe("**\nb** c");
    // Everything between the two marks gone: the pair goes with it.
    const all = markView("**a\nb** c", { room });
    select(all, 1, 5);
    keys(all, "Backspace");
    expect(bytes(all.state)).toBe(" c");
  });
});

describe.each([
  { name: "solo", room: false },
  { name: "in a room", room: true },
])(
  "a word delete that reaches only hidden marks goes on, $name",
  ({ room }) => {
    it("Mod+Delete before an opening mark takes the word and the empty pair", () => {
      const v = markView("a **bold** c", { room });
      setCaret(v, 2);
      keys(v, "Mod-Delete");
      expect(bytes(v.state)).toBe("a  c");
    });

    it("Mod+Backspace after a link's hidden address takes the link text", () => {
      const v = markView("x [a](u)", { room });
      setCaret(v, 8);
      expect(caret(v)).toBe(8);
      keys(v, "Mod-Backspace");
      expect(bytes(v.state)).toBe("x ");
    });
  },
);

describe.each([
  { name: "solo", room: false },
  { name: "in a room", room: true },
])("an escaped character goes with its backslash, $name", ({ room }) => {
  it.each([
    { name: "\\*", text: "a **\\*** c" },
    { name: "\\`", text: "a **\\`** c" },
    { name: "\\\\", text: "a **\\\\** c" },
  ])("$name", ({ text }) => {
    const back = markView(text, { room });
    setCaret(back, 6);
    expect(caret(back)).toBe(6);
    keys(back, "Backspace");
    expect(bytes(back.state)).toBe("a  c");
    const forward = markView(text, { room });
    setCaret(forward, 4);
    keys(forward, "Delete");
    expect(bytes(forward.state)).toBe("a  c");
  });
});

describe.each([
  { name: "solo", room: false },
  { name: "in a room", room: true },
])("a hidden fence line is never joined, $name", ({ room }) => {
  // a=0, ```ts=2..7, x=8, y=10, ```=12..15, b=16
  const TEXT = "a\n```ts\nx\ny\n```\nb";

  it.each([
    ["Backspace at the first code line's start", 8, "Backspace"],
    ["Delete at the last code line's end", 11, "Delete"],
    ["Delete at the end of the line above", 1, "Delete"],
    ["Backspace at the start of the line below", 16, "Backspace"],
  ])("%s changes nothing", (_name, at, key) => {
    const v = markView(TEXT, { room });
    setCaret(v, at);
    keys(v, key);
    expect(bytes(v.state)).toBe(TEXT);
    expect(caret(v)).toBe(at);
  });

  it("Backspace at the visible start of a bold line below changes nothing", () => {
    const text = "a\n```ts\nx\n```\n**b**";
    const v = markView(text, { room });
    setCaret(v, text.indexOf("**"));
    expect(caret(v)).toBe(text.indexOf("b**"));
    keys(v, "Backspace");
    expect(bytes(v.state)).toBe(text);
  });

  it("Backspace inside a code line still deletes", () => {
    const v = markView(TEXT, { room });
    setCaret(v, 9);
    keys(v, "Backspace");
    expect(bytes(v.state)).toBe("a\n```ts\n\ny\n```\nb");
  });
});

describe("a fence line the caret shows (show mode)", () => {
  it("Backspace at the first code line's start runs today's key", () => {
    const text = "a\n```ts\nx\ny\n```\nb";
    const v = markView(text, { mode: "show" });
    setCaret(v, 8);
    expect(v.contentDOM.textContent).toContain("```ts");
    keys(v, "Backspace");
    expect(bytes(v.state)).toBe("a\n```tsx\ny\n```\nb");
  });

  it("Backspace below a fence the caret does not show changes nothing", () => {
    const text = "a\n```ts\nx\ny\n```\nb";
    const v = markView(text, { mode: "show" });
    setCaret(v, 16);
    keys(v, "Backspace");
    expect(bytes(v.state)).toBe(text);
  });
});

it("Raw: Backspace at the first code line's start joins the lines, as today", () => {
  const v = markView("a\n```ts\nx\n```", { raw: true });
  setCaret(v, 8);
  keys(v, "Backspace");
  expect(bytes(v.state)).toBe("a\n```tsx\n```");
});
