/**
 * Element edit requests: what asks for a popover (Mod-k, Shift+F10, a press
 * on a link or a chip), where the request goes (main's listener, from main or
 * from the open cell), when it closes without writing, and what a commit
 * writes: only the element's span, as one undo step, solo and in a room, on
 * the view that asked.
 */

import { EditorSelection, EditorState, StateEffect } from "@codemirror/state";
import type { EditorView } from "@codemirror/view";
import { afterEach, describe, expect, it, vi } from "vitest";

import { cleanupCells, openAt, roomMain, soloMain } from "../test/cellKit";
import {
  bytes,
  caret,
  cleanupMarkViews,
  keys,
  markPeers,
  markView,
  setCaret,
  setRaw,
  setShowMarks,
  typeText,
  undoOnce,
} from "../test/markKit";
import type { ElementEdit } from "./elementEdit";
import {
  commitElementEdit,
  discardElementEdit,
  CANNOT_LINK,
  elementAtCaret,
  elementEditCell,
  elementEditListener,
  linkVerb,
  openElementEdit,
  requestElementEdit,
} from "./elementEdit";
import { linkChanges, newLinkChanges, wikilinkChanges } from "./elementVerbs";
import { cellForwarding } from "./grid/cellEditor";
import { marksOnEditLine } from "./marks/mode";
import { linkPressed } from "./preview";

afterEach(() => {
  cleanupMarkViews();
  cleanupCells();
  vi.restoreAllMocks();
});

type Mode = "hidden" | "show";
const MODES: Mode[] = ["hidden", "show"];

interface Heard {
  edit: ElementEdit | null;
  view: EditorView;
}

/** A view whose element edits are written down, newest last. */
function listened(
  text: string,
  options: Parameters<typeof markView>[1] = {},
): { view: EditorView; heard: Heard[] } {
  const heard: Heard[] = [];
  const view = markView(text, {
    ...options,
    onElementEdit: (edit, from) => {
      heard.push({ edit, view: from });
    },
  });
  return { view, heard };
}

function last(heard: Heard[]): ElementEdit | null {
  return heard.at(-1)?.edit ?? null;
}

function select(view: EditorView, from: number, to: number): void {
  view.dispatch({
    selection: EditorSelection.range(from, to),
    userEvent: "select",
  });
}

function press(el: Element, init: MouseEventInit = {}): MouseEvent {
  const event = new MouseEvent("mousedown", {
    bubbles: true,
    cancelable: true,
    button: 0,
    ...init,
  });
  el.dispatchEvent(event);
  return event;
}

function linkText(view: EditorView): Element {
  const el = view.contentDOM.querySelector(".cm-link-text");
  if (!el) throw new Error("no link text is drawn");
  return el;
}

const address = (value: string) => (state: EditorState, edit: ElementEdit) =>
  edit.kind === "link" ? linkChanges(state, edit.at, { address: value }) : null;

const newLink =
  (value: string, text = "") =>
  (state: EditorState, edit: ElementEdit) =>
    edit.kind === "new-link"
      ? newLinkChanges(state, edit.range, { text, address: value })
      : null;

describe.each(MODES)("Mod-k (%s mode)", (mode) => {
  it("asks for a new link over a selection", () => {
    const { view, heard } = listened("go there", { mode });
    select(view, 3, 8);
    expect(keys(view, "Mod-k")).toBe(true);
    expect(last(heard)).toMatchObject({
      kind: "new-link",
      range: { from: 3, to: 8 },
    });
    expect(heard.at(-1)?.view).toBe(view);
  });

  it("asks for the link the caret is in", () => {
    const { view, heard } = listened("a [t](u) b", { mode });
    setCaret(view, 3);
    keys(view, "Mod-k");
    expect(last(heard)).toMatchObject({
      kind: "link",
      at: 2,
      text: { from: 3, to: 4 },
      address: { from: 6, to: 7 },
      reference: false,
    });
  });

  it("asks for a new link over the word the caret is in", () => {
    const { view, heard } = listened("go there", { mode });
    setCaret(view, 5);
    keys(view, "Mod-k");
    expect(last(heard)).toMatchObject({
      kind: "new-link",
      range: { from: 3, to: 8 },
    });
  });

  it("asks for a new link at a caret between words", () => {
    const { view, heard } = listened("a  b", { mode });
    setCaret(view, 2);
    keys(view, "Mod-k");
    expect(last(heard)).toMatchObject({
      kind: "new-link",
      range: { from: 2, to: 2 },
    });
  });

  it("asks nothing inside code, writes nothing and keeps the key", () => {
    const { view, heard } = listened("a `code` b", { mode });
    setCaret(view, 4);
    const dispatch = vi.spyOn(view, "dispatch");
    // Consumed, so the browser's own Ctrl-K does not open.
    expect(keys(view, "Mod-k")).toBe(true);
    expect(heard).toEqual([]);
    expect(bytes(view.state)).toBe("a `code` b");
    // Said, as a refused format is.
    expect(JSON.stringify(dispatch.mock.calls)).toContain(CANNOT_LINK);
  });

  it("asks nothing across lines, writes nothing and keeps the key", () => {
    const { view, heard } = listened("a\nb", { mode });
    select(view, 0, 3);
    expect(keys(view, "Mod-k")).toBe(true);
    expect(heard).toEqual([]);
    expect(bytes(view.state)).toBe("a\nb");
  });

  it("does nothing from the Link button in code", () => {
    const { view, heard } = listened("a `code` b", { mode });
    setCaret(view, 4);
    linkVerb(view);
    expect(heard).toEqual([]);
    expect(bytes(view.state)).toBe("a `code` b");
  });
});

describe("the Link verb", () => {
  it("asks for the popover where marks are hidden", () => {
    const { view, heard } = listened("go there");
    select(view, 3, 8);
    linkVerb(view);
    expect(last(heard)?.kind).toBe("new-link");
    expect(bytes(view.state)).toBe("go there");
  });

  it("keeps insertMarkdownLink in Raw, for the button and Mod-k", () => {
    const { view, heard } = listened("go there", { raw: true });
    select(view, 3, 8);
    linkVerb(view);
    expect(heard).toEqual([]);
    expect(bytes(view.state)).toBe("go [there](url)");
    const keyed = listened("go there", { raw: true });
    select(keyed.view, 3, 8);
    expect(keys(keyed.view, "Mod-k")).toBe(true);
    expect(keyed.heard).toEqual([]);
    expect(bytes(keyed.view.state)).toBe("go [there](url)");
  });

  it("keeps insertMarkdownLink on a surface with no popover host", () => {
    const view = markView("go there");
    select(view, 3, 8);
    linkVerb(view);
    expect(bytes(view.state)).toBe("go [there](url)");
    const keyed = markView("");
    keys(keyed, "Mod-k");
    expect(bytes(keyed.state)).toBe("[text](url)");
  });

  it("writes no link with Mod-k on a page that takes no writes", () => {
    const view = markView("go there", { raw: true });
    view.dispatch({
      effects: StateEffect.appendConfig.of(EditorState.readOnly.of(true)),
    });
    select(view, 3, 8);
    keys(view, "Mod-k");
    expect(bytes(view.state)).toBe("go there");
  });
});

describe.each(MODES)("Shift+F10 (%s mode)", (mode) => {
  it("opens the link at the caret", () => {
    const { view, heard } = listened("a [t](u) b", { mode });
    setCaret(view, 3);
    expect(elementAtCaret(view.state)).toBe("link");
    expect(keys(view, "Shift-F10")).toBe(true);
    expect(last(heard)?.kind).toBe("link");
  });

  it("opens the wikilink at the caret on an engram", () => {
    const { view, heard } = listened("a [[Old|w]] b", { mode });
    setCaret(view, 4);
    expect(elementAtCaret(view.state)).toBe("wikilink");
    keys(view, "ContextMenu");
    expect(last(heard)).toMatchObject({
      kind: "wikilink",
      at: 2,
      target: { from: 4, to: 7 },
    });
  });

  it("lets the key go on where there is no element", () => {
    const { view, heard } = listened("plain words", { mode });
    setCaret(view, 3);
    expect(elementAtCaret(view.state)).toBeNull();
    expect(keys(view, "Shift-F10")).toBe(false);
    expect(heard).toEqual([]);
  });

  it("finds no wikilink on a Reflection page", () => {
    const { view } = listened("a [[Old]] b\n", { mode, adapter: "reflection" });
    setCaret(view, 4);
    expect(elementAtCaret(view.state)).toBeNull();
  });
});

describe("a press on a link", () => {
  it.each(MODES)(
    "is handled here only where marks hide, so no caret lands in it (%s)",
    (mode) => {
      // The caret on the link's line: in show mode its marks show there.
      const { view } = listened("x\na [t](u) b", { mode });
      setCaret(view, 2);
      const event = new MouseEvent("mousedown", {
        bubbles: true,
        cancelable: true,
        button: 0,
      });
      Object.defineProperty(event, "target", { value: linkText(view) });
      expect(linkPressed(event, view)).toBe(mode === "hidden");
    },
  );

  it("asks for the popover and places no caret inside it (hidden mode)", () => {
    const { view, heard } = listened("a [t](u) b");
    setCaret(view, 0);
    const event = press(linkText(view));
    expect(last(heard)).toMatchObject({ kind: "link", at: 2 });
    expect(event.defaultPrevented).toBe(true);
    const head = caret(view);
    expect(head >= 1 && head < 8).toBe(false);
  });

  it("asks for the popover and lets the caret in (show mode)", () => {
    const { view, heard } = listened("x\na [t](u) b", { mode: "show" });
    setCaret(view, 0);
    press(linkText(view));
    expect(last(heard)).toMatchObject({ kind: "link", at: 4 });
  });

  it("opens a web address in a new tab with Cmd or Ctrl and asks nothing", () => {
    const open = vi.spyOn(window, "open").mockReturnValue(null);
    const { view, heard } = listened("a [t](https://x.y) b");
    press(linkText(view), { ctrlKey: true });
    expect(open).toHaveBeenCalledWith(
      "https://x.y",
      "_blank",
      "noopener,noreferrer",
    );
    expect(heard).toEqual([]);
    expect(bytes(view.state)).toBe("a [t](https://x.y) b");
  });

  it("opens no script address", () => {
    const open = vi.spyOn(window, "open").mockReturnValue(null);
    const { view } = listened("a [t](javascript:alert(1)) b");
    press(linkText(view), { metaKey: true });
    expect(open).not.toHaveBeenCalled();
  });

  it("asks nothing with a right press", () => {
    const { view, heard } = listened("a [t](u) b");
    press(linkText(view), { button: 2 });
    expect(heard).toEqual([]);
  });

  it("asks nothing on a page that takes no writes (C12)", () => {
    const { view, heard } = listened("a [t](u) b");
    view.dispatch({
      effects: StateEffect.appendConfig.of(EditorState.readOnly.of(true)),
    });
    press(linkText(view));
    keys(view, "Mod-k");
    expect(heard).toEqual([]);
  });

  it("draws no link press in Raw", () => {
    const { view } = listened("a [t](u) b", { raw: true });
    expect(view.contentDOM.querySelector(".cm-link-text")).toBeNull();
  });
});

describe("a press on a chip", () => {
  it("asks for the wikilink popover with no caret inside (hidden mode)", () => {
    const { view, heard } = listened("a [[Old]] b");
    setCaret(view, 0);
    const chip = view.contentDOM.querySelector(".cm-wikilink");
    if (!chip) throw new Error("no chip is drawn");
    const event = press(chip);
    expect(last(heard)).toMatchObject({
      kind: "wikilink",
      at: 2,
      target: { from: 4, to: 7 },
    });
    expect(event.defaultPrevented).toBe(true);
    expect(caret(view)).toBe(0);
  });

  it("asks for it in show mode too", () => {
    const { view, heard } = listened("x\na [[Old]] b", { mode: "show" });
    setCaret(view, 0);
    const chip = view.contentDOM.querySelector(".cm-wikilink");
    if (!chip) throw new Error("no chip is drawn");
    press(chip);
    expect(last(heard)?.kind).toBe("wikilink");
  });
});

describe.each([false, true])("a commit (room: %s)", (room) => {
  it.each(MODES)("writes only the address, as one undo step (%s)", (mode) => {
    const { view } = listened("[t](https://a.b) after", { mode, room });
    setCaret(view, 1);
    keys(view, "Mod-k");
    const events: string[] = [];
    const spy = vi.spyOn(view, "dispatch");
    expect(commitElementEdit(view, address("https://c.d"))).toBe(true);
    for (const call of spy.mock.calls) {
      const spec = call[0] as { userEvent?: string };
      if (spec.userEvent) events.push(spec.userEvent);
    }
    expect(events).toEqual(["input"]);
    expect(bytes(view.state)).toBe("[t](https://c.d) after");
    // The caret after the element.
    expect(caret(view)).toBeGreaterThanOrEqual(16);
    undoOnce(view);
    expect(bytes(view.state)).toBe("[t](https://a.b) after");
  });

  it.each(MODES)("is its own undo step after typing (%s)", (mode) => {
    const { view } = listened("go there", { mode, room });
    setCaret(view, 8);
    typeText(view, "x");
    select(view, 3, 9);
    keys(view, "Mod-k");
    commitElementEdit(view, newLink("https://x"));
    expect(bytes(view.state)).toBe("go [therex](https://x)");
    undoOnce(view);
    expect(bytes(view.state)).toBe("go therex");
  });

  it("writes a new link over the word at the caret", () => {
    const { view } = listened("go there", { room });
    setCaret(view, 5);
    keys(view, "Mod-k");
    commitElementEdit(view, newLink("https://x"));
    expect(bytes(view.state)).toBe("go [there](https://x)");
  });

  it("writes nothing and closes when the page stopped taking writes", () => {
    const { view, heard } = listened("[t](u)", { room });
    setCaret(view, 1);
    keys(view, "Mod-k");
    view.dispatch({
      effects: StateEffect.appendConfig.of(EditorState.readOnly.of(true)),
    });
    expect(last(heard)).toBeNull();
    expect(commitElementEdit(view, address("v"))).toBe(false);
    expect(bytes(view.state)).toBe("[t](u)");
  });
});

describe("a request closes without writing", () => {
  it.each(MODES)("when the setting flips (%s)", (mode) => {
    const { view, heard } = listened("[t](u)", { mode });
    setCaret(view, 1);
    keys(view, "Mod-k");
    expect(last(heard)?.kind).toBe("link");
    setShowMarks(view, mode === "hidden");
    expect(last(heard)).toBeNull();
    expect(openElementEdit(view)).toBeNull();
    expect(commitElementEdit(view, address("v"))).toBe(false);
    expect(bytes(view.state)).toBe("[t](u)");
  });

  it("when Raw flips", () => {
    const { view, heard } = listened("[t](u)");
    setCaret(view, 1);
    keys(view, "Mod-k");
    setRaw(view, true);
    expect(last(heard)).toBeNull();
    expect(bytes(view.state)).toBe("[t](u)");
  });

  it("when the buffer is rebuilt", () => {
    const { view, heard } = listened("[t](u)");
    setCaret(view, 1);
    keys(view, "Mod-k");
    view.setState(EditorState.create({ doc: "[t](u)" }));
    expect(last(heard)).toBeNull();
  });

  it("when a peer deleted the link", () => {
    const heard: Heard[] = [];
    const { local, peer, sync } = markPeers("a [t](u) b", {
      onElementEdit: (edit, from) => {
        heard.push({ edit, view: from });
      },
    });
    setCaret(local, 3);
    keys(local, "Mod-k");
    expect(last(heard)?.kind).toBe("link");
    peer.dispatch({ changes: { from: 2, to: 8 }, userEvent: "delete" });
    sync();
    expect(bytes(local.state)).toBe("a  b");
    expect(last(heard)).toBeNull();
    expect(commitElementEdit(local, address("v"))).toBe(false);
    expect(bytes(local.state)).toBe("a  b");
  });

  it.each([
    ["the link after it takes its place", "[t](u) [v](w)", 1, 0, 7, "[v](w)"],
    [
      "the link before it ends where it began",
      "[a](b)[t](u)",
      7,
      6,
      12,
      "[a](b)",
    ],
    ["the wikilink after it takes its place", "[[A]] [[B]]", 3, 0, 6, "[[B]]"],
    [
      "the wikilink before it ends where it began",
      "[[A]][[B]]",
      7,
      5,
      10,
      "[[A]]",
    ],
  ])("when a peer deleted it and %s", (_name, text, at, from, to, after) => {
    const heard: Heard[] = [];
    const { local, peer, sync } = markPeers(text, {
      onElementEdit: (edit, view) => {
        heard.push({ edit, view });
      },
    });
    setCaret(local, at);
    keys(local, "Shift-F10");
    expect(last(heard)).not.toBeNull();
    peer.dispatch({ changes: { from, to }, userEvent: "delete" });
    sync();
    expect(bytes(local.state)).toBe(after);
    expect(last(heard)).toBeNull();
    expect(
      commitElementEdit(local, (state, edit) =>
        edit.kind === "link"
          ? linkChanges(state, edit.at, { address: "x" })
          : edit.kind === "wikilink"
            ? wikilinkChanges(state, edit.at, { target: "X" })
            : null,
      ),
    ).toBe(false);
    expect(bytes(local.state)).toBe(after);
  });

  it("when a peer deleted the selection a new link was for", () => {
    const heard: Heard[] = [];
    const { local, peer, sync } = markPeers("go there now", {
      onElementEdit: (edit, view) => {
        heard.push({ edit, view });
      },
    });
    select(local, 3, 8);
    keys(local, "Mod-k");
    expect(last(heard)?.kind).toBe("new-link");
    peer.dispatch({ changes: { from: 3, to: 8 }, userEvent: "delete" });
    sync();
    expect(last(heard)).toBeNull();
    expect(commitElementEdit(local, newLink("https://x"))).toBe(false);
    expect(bytes(local.state)).toBe("go  now");
  });

  it("follows the link when a peer types before it", () => {
    const heard: Heard[] = [];
    const { local, peer, sync } = markPeers("a [t](u) b", {
      onElementEdit: (edit, from) => {
        heard.push({ edit, view: from });
      },
    });
    setCaret(local, 3);
    keys(local, "Mod-k");
    peer.dispatch({ changes: { from: 0, insert: "zz " }, userEvent: "input" });
    sync();
    expect(commitElementEdit(local, address("v"))).toBe(true);
    expect(bytes(local.state)).toBe("zz a [t](v) b");
    expect(bytes(peer.state)).toBe("zz a [t](u) b");
    sync();
    expect(bytes(peer.state)).toBe("zz a [t](v) b");
  });

  it("on Escape, which hands the focus back with the caret where it was", () => {
    const { view, heard } = listened("a [t](u) b");
    setCaret(view, 3);
    const before = caret(view);
    keys(view, "Shift-F10");
    const focus = vi.spyOn(view, "focus");
    discardElementEdit(view);
    expect(last(heard)).toBeNull();
    expect(focus).toHaveBeenCalled();
    expect(caret(view)).toBe(before);
    expect(bytes(view.state)).toBe("a [t](u) b");
  });
});

describe.each([false, true])(
  "a request from the open cell (room: %s)",
  (room) => {
    const TABLE = "| h |\n| - |\n| [t](u) |\n";

    function mainWith(heard: Heard[]): EditorView {
      const listener = elementEditListener((edit, from) => {
        heard.push({ edit, view: from });
      });
      return room
        ? roomMain(TABLE, [cellForwarding, listener]).main
        : soloMain(TABLE, [cellForwarding, listener]);
    }

    it("reaches main's listener with the cell's view, and commits there", () => {
      const heard: Heard[] = [];
      const main = mainWith(heard);
      const at = TABLE.indexOf("[t]");
      const cell = openAt(main, at + 1, true, [elementEditCell(main)]);
      requestElementEdit(cell.view, {
        kind: "link",
        at,
        rect: new DOMRect(),
        text: { from: at + 1, to: at + 2 },
        address: { from: at + 4, to: at + 5 },
        reference: false,
      });
      expect(heard.at(-1)?.view).toBe(cell.view);
      expect(commitElementEdit(main, address("v"))).toBe(true);
      expect(bytes(main.state)).toBe("| h |\n| - |\n| [t](v) |\n");
    });

    it("closes without writing when main stops taking writes (C12)", () => {
      const heard: Heard[] = [];
      const main = mainWith(heard);
      const at = TABLE.indexOf("[t]");
      const cell = openAt(main, at + 1, true, [elementEditCell(main)]);
      requestElementEdit(cell.view, {
        kind: "link",
        at,
        rect: new DOMRect(),
        text: { from: at + 1, to: at + 2 },
        address: { from: at + 4, to: at + 5 },
        reference: false,
      });
      expect(heard.at(-1)?.view).toBe(cell.view);
      main.dispatch({
        effects: StateEffect.appendConfig.of(EditorState.readOnly.of(true)),
      });
      expect(last(heard)).toBeNull();
      expect(commitElementEdit(main, address("v"))).toBe(false);
      expect(bytes(main.state)).toBe(TABLE);
    });

    it("closes without writing when main's setting flips (Review Focus 5)", () => {
      const heard: Heard[] = [];
      const main = mainWith(heard);
      const at = TABLE.indexOf("[t]");
      const cell = openAt(main, at + 1, true, [elementEditCell(main)]);
      requestElementEdit(cell.view, {
        kind: "link",
        at,
        rect: new DOMRect(),
        text: { from: at + 1, to: at + 2 },
        address: { from: at + 4, to: at + 5 },
        reference: false,
      });
      main.dispatch({
        effects: StateEffect.appendConfig.of(marksOnEditLine.of(true)),
      });
      expect(last(heard)).toBeNull();
      expect(commitElementEdit(main, address("v"))).toBe(false);
      expect(bytes(main.state)).toBe(TABLE);
    });

    it("closes when the cell goes", () => {
      const heard: Heard[] = [];
      const main = mainWith(heard);
      const at = TABLE.indexOf("[t]");
      const cell = openAt(main, at + 1, true, [elementEditCell(main)]);
      requestElementEdit(cell.view, {
        kind: "link",
        at,
        rect: new DOMRect(),
        text: { from: at + 1, to: at + 2 },
        address: { from: at + 4, to: at + 5 },
        reference: false,
      });
      cell.destroy();
      expect(last(heard)).toBeNull();
      expect(commitElementEdit(main, address("v"))).toBe(false);
      expect(bytes(main.state)).toBe(TABLE);
    });
  },
);
