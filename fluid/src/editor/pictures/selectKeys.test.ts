/**
 * The keys around a picture (rulings 3, 4, 5 and 8): an arrow, Backspace or
 * Delete onto a picture selects it first, and the next press passes it or
 * removes it; Enter adds a line after a selected picture, Escape leaves it;
 * a cut takes the picture with its line and a paste gives the reference back
 * byte for byte.
 *
 * Every view is built through the surface with the picture layer and a
 * finished parse (`markView`), solo and as a room peer (`markPeers`), in the
 * hidden-marks and the show-marks mode. The tools chunk is loaded before any
 * view is built, so every view starts with the keys in.
 */

import type { Extension } from "@codemirror/state";
import { EditorSelection, EditorState, StateEffect } from "@codemirror/state";
import type { EditorView } from "@codemirror/view";
import { afterEach, beforeAll, describe, expect, it, vi } from "vitest";

import { engramAssetHome } from "../imageFormat";
import { imagePreviews } from "../imagePreviews";
import type { MarkViewOptions } from "../../test/markKit";
import {
  bytes,
  cleanupMarkViews,
  keys,
  markPeers,
  markView,
  setCaret,
  typeText,
  undoOnce,
  undoSteps,
} from "../../test/markKit";
import { pictureForKeys, pictureKeysOn, picturesDrawn } from "../marks/mode";
import { baseExtensions } from "../setup";
import { parsedState } from "../../test/parse";
import { prefetchPictureTools } from "./loader";

beforeAll(async () => {
  await prefetchPictureTools();
});

afterEach(cleanupMarkViews);

const INLINE = "a ![x](assets/p.png) b";
const ALONE = "a\n\n![x](assets/p.png)\n\nb";

/** Where each document's picture stands, and what removing it leaves. */
const CASES = [
  { name: "inside a sentence", doc: INLINE, from: 2, to: 20, removed: "a  b" },
  { name: "alone on its line", doc: ALONE, from: 3, to: 21, removed: "a\n\nb" },
] as const;

type Mode = "hidden" | "show";
type Where = "solo" | "room";

const SETUPS: [Mode, Where][] = [
  ["hidden", "solo"],
  ["hidden", "room"],
  ["show", "solo"],
  ["show", "room"],
];

interface Built {
  view: EditorView;
  /** The other peer's bytes after a sync, or the view's own solo. */
  other: () => string;
}

function build(
  doc: string,
  mode: Mode,
  where: Where,
  options: MarkViewOptions = {},
): Built {
  const images = options.images ?? imagePreviews("eng");
  if (where === "solo") {
    const view = markView(doc, { mode, images, ...options });
    return { view, other: () => bytes(view.state) };
  }
  const { local, peer, sync } = markPeers(doc, { mode, images, ...options });
  return {
    view: local,
    other: () => {
      sync();
      return bytes(peer.state);
    },
  };
}

/** The main selection as `[from, to]`. */
function selection(view: EditorView): [number, number] {
  const { main } = view.state.selection;
  return [main.from, main.to];
}

function select(view: EditorView, from: number, to: number): void {
  view.dispatch({
    selection: EditorSelection.single(from, to),
    userEvent: "select.pointer",
  });
}

describe.each(SETUPS)(
  "the keys around a picture (%s mode, %s)",
  (mode, where) => {
    describe.each(CASES)("$name", ({ doc, from, to, removed }) => {
      it("ArrowRight at its start selects it, and the next one goes past it", () => {
        const { view } = build(doc, mode, where);
        setCaret(view, from);
        expect(keys(view, "ArrowRight")).toBe(true);
        expect(selection(view)).toEqual([from, to]);
        keys(view, "ArrowRight");
        expect(selection(view)).toEqual([to, to]);
        expect(bytes(view.state)).toBe(doc);
      });

      it("ArrowLeft at its end selects it, and the next one goes past it", () => {
        const { view } = build(doc, mode, where);
        setCaret(view, to);
        expect(keys(view, "ArrowLeft")).toBe(true);
        expect(selection(view)).toEqual([from, to]);
        keys(view, "ArrowLeft");
        expect(selection(view)).toEqual([from, from]);
        expect(bytes(view.state)).toBe(doc);
      });

      it("Backspace after it selects it, and the next one removes it", () => {
        const built = build(doc, mode, where);
        const { view } = built;
        setCaret(view, to);
        keys(view, "Backspace");
        expect(selection(view)).toEqual([from, to]);
        expect(bytes(view.state)).toBe(doc);
        keys(view, "Backspace");
        expect(bytes(view.state)).toBe(removed);
        expect(built.other()).toBe(removed);
      });

      it("Delete before it selects it, and the next one removes it", () => {
        const { view } = build(doc, mode, where);
        setCaret(view, from);
        keys(view, "Delete");
        expect(selection(view)).toEqual([from, to]);
        expect(bytes(view.state)).toBe(doc);
        keys(view, "Delete");
        expect(bytes(view.state)).toBe(removed);
      });

      it("removes it in one undo step, and one undo brings the exact bytes back", () => {
        const { view } = build(doc, mode, where);
        setCaret(view, to);
        const before = undoSteps(view);
        keys(view, "Backspace", "Backspace");
        expect(bytes(view.state)).toBe(removed);
        expect(undoSteps(view)).toBe(before + 1);
        undoOnce(view);
        expect(bytes(view.state)).toBe(doc);
      });

      it("Enter on a selected picture adds a line after it and keeps it", () => {
        const built = build(doc, mode, where);
        const { view } = built;
        select(view, from, to);
        expect(keys(view, "Enter")).toBe(true);
        const after = `${doc.slice(0, to)}\n${doc.slice(to)}`;
        expect(bytes(view.state)).toBe(after);
        expect(selection(view)).toEqual([to + 1, to + 1]);
        expect(built.other()).toBe(after);
        undoOnce(view);
        expect(bytes(view.state)).toBe(doc);
      });

      it("Escape on a selected picture puts the caret after it", () => {
        const { view } = build(doc, mode, where);
        select(view, from, to);
        expect(keys(view, "Escape")).toBe(true);
        expect(selection(view)).toEqual([to, to]);
        expect(bytes(view.state)).toBe(doc);
      });

      it("typing over a selected picture replaces it", () => {
        const { view } = build(doc, mode, where);
        select(view, from, to);
        typeText(view, "z");
        expect(bytes(view.state)).toBe(
          `${doc.slice(0, from)}z${doc.slice(to)}`,
        );
      });
    });

    it("Shift+ArrowRight takes a picture whole where it is drawn in place", () => {
      if (mode === "show") return; // the edited line shows its source
      const { view } = build(INLINE, mode, where);
      setCaret(view, 2);
      keys(view, "Shift-ArrowRight");
      expect(selection(view)).toEqual([2, 20]);
    });

    it("does not act on a picture inside a link", () => {
      const doc = "a [![x](assets/p.png)](https://h) b";
      const { view } = build(doc, mode, where);
      // Enter is the editor's there: it replaces the selection.
      select(view, 3, 21);
      keys(view, "Enter");
      expect(bytes(view.state)).not.toContain("![x]");
    });

    it("does not select a picture inside a link from the caret before it", () => {
      const doc = "a [![x](assets/p.png)](https://h) b";
      const { view } = build(doc, mode, where);
      setCaret(view, 3);
      keys(view, "ArrowRight");
      expect(view.state.selection.main.empty).toBe(true);
    });
  },
);

describe("the canonical caret beside hidden marks", () => {
  it("selects a picture after a bold word from the caret inside the bold", () => {
    const doc = "**a**![x](assets/p.png) c";
    const view = markView(doc, { images: imagePreviews("eng") });
    // Right after `a`, before the closing marks the caret steps over.
    setCaret(view, 3);
    keys(view, "ArrowRight");
    expect(selection(view)).toEqual([5, 23]);
    setCaret(view, 3);
    keys(view, "Delete");
    expect(selection(view)).toEqual([5, 23]);
  });

  it("selects a picture before a bold word from the caret inside the bold", () => {
    const doc = "c ![x](assets/p.png)**b**";
    const view = markView(doc, { images: imagePreviews("eng") });
    // Right before `b`, after the opening marks.
    setCaret(view, 22);
    keys(view, "ArrowLeft");
    expect(selection(view)).toEqual([2, 20]);
    setCaret(view, 22);
    keys(view, "Backspace");
    expect(selection(view)).toEqual([2, 20]);
    keys(view, "Backspace");
    expect(bytes(view.state)).toBe("c **b**");
  });

  it("keeps both bolds when a picture between them goes", () => {
    const doc = "**a**![x](assets/p.png)**b**\n\nend";
    const view = markView(doc, { images: imagePreviews("eng") });
    setCaret(view, 23);
    keys(view, "Backspace", "Backspace");
    expect(bytes(view.state)).toBe("**ab**\n\nend");
  });
});

describe("the keys without the picture layer", () => {
  it("leave an image to the hidden-mark layer's own keys", () => {
    const view = markView(INLINE);
    setCaret(view, 2);
    keys(view, "ArrowRight");
    expect(view.state.selection.main.empty).toBe(true);
  });

  it("leave a selected image's keys to the editor in Raw", () => {
    const view = markView(INLINE, { raw: true, images: imagePreviews("eng") });
    select(view, 2, 20);
    keys(view, "Enter");
    expect(bytes(view.state)).toBe("a \nb");
  });
});

interface Copied {
  data: Map<string, string>;
  prevented: boolean;
}

/** A copy or cut as the browser sends it; what reached the clipboard comes back. */
function send(view: EditorView, type: "copy" | "cut"): Copied {
  const data = new Map<string, string>();
  const event = new Event(type, { bubbles: true, cancelable: true });
  Object.defineProperty(event, "clipboardData", {
    configurable: true,
    value: {
      clearData: () => {
        data.clear();
      },
      setData: (kind: string, value: string) => {
        data.set(kind, value);
      },
      getData: (kind: string) => data.get(kind) ?? "",
    },
  });
  view.contentDOM.dispatchEvent(event);
  return { data, prevented: event.defaultPrevented };
}

/** A paste as the browser sends it, with the flavours `copied` holds. */
function pasteInto(view: EditorView, copied: Copied): void {
  const event = new Event("paste", { bubbles: true, cancelable: true });
  Object.defineProperty(event, "clipboardData", {
    value: {
      getData: (kind: string) => copied.data.get(kind) ?? "",
      types: [...copied.data.keys()],
      files: [],
      items: [],
    },
  });
  view.contentDOM.dispatchEvent(event);
}

const OWN = '<div data-crystalline-copy="1">';

describe.each(SETUPS)(
  "cut and paste of a picture (%s mode, %s)",
  (mode, where) => {
    const REF = '![a [b] c](assets/p.png#right,w=50% "A title")';

    it("cuts a picture in a sentence and pastes it back byte for byte", () => {
      const doc = `one ${REF} two\n\nend`;
      const built = build(doc, mode, where);
      const { view } = built;
      expect(view.state.sliceDoc(4, 4 + REF.length)).toBe(REF);
      select(view, 4, 4 + REF.length);
      const cut = send(view, "cut");
      expect(cut.prevented).toBe(true);
      expect(cut.data.get("text/plain")).toBe(REF);
      // Part 2's marked HTML stays beside it (P9): a paste in Fluid takes the
      // Markdown from text/plain.
      expect(cut.data.get("text/html")?.startsWith(OWN)).toBe(true);
      expect(bytes(view.state)).toBe("one  two\n\nend");
      setCaret(view, view.state.doc.length);
      pasteInto(view, cut);
      expect(bytes(view.state)).toBe(`one  two\n\nend${REF}`);
      expect(built.other()).toBe(`one  two\n\nend${REF}`);
    });

    it("cuts a picture alone on its line with its line", () => {
      const doc = `a\n\n${REF}\n\nb\n\n`;
      const { view } = build(doc, mode, where);
      select(view, 3, 3 + REF.length);
      const cut = send(view, "cut");
      expect(cut.data.get("text/plain")).toBe(REF);
      expect(bytes(view.state)).toBe("a\n\nb\n\n");
      setCaret(view, view.state.doc.length);
      pasteInto(view, cut);
      expect(bytes(view.state)).toBe(`a\n\nb\n\n${REF}`);
    });

    it("copies a picture's reference text and leaves it", () => {
      const { view } = build(INLINE, mode, where);
      select(view, 2, 20);
      const copied = send(view, "copy");
      expect(copied.data.get("text/plain")).toBe("![x](assets/p.png)");
      expect(bytes(view.state)).toBe(INLINE);
    });
  },
);

describe("removing a picture", () => {
  it("never removes its file", () => {
    const remove = vi.fn(() => Promise.resolve());
    const home = { ...engramAssetHome("eng"), remove };
    const view = markView(ALONE, {
      images: imagePreviews(home),
    });
    setCaret(view, 21);
    keys(view, "Backspace", "Backspace");
    expect(bytes(view.state)).toBe("a\n\nb");
    select(view, 0, 0);
    expect(remove).not.toHaveBeenCalled();
  });

  it("refuses a removal, Enter and a cut on a page that is not writable", () => {
    const view = markView(INLINE, { images: imagePreviews("eng") });
    view.dispatch({
      effects: StateEffect.appendConfig.of(EditorState.readOnly.of(true)),
    });
    select(view, 2, 20);
    keys(view, "Backspace");
    keys(view, "Enter");
    send(view, "cut");
    expect(bytes(view.state)).toBe(INLINE);
  });
});

describe("pictureForKeys", () => {
  const state = (extensions: Extension[]) =>
    parsedState(
      EditorState.create({
        doc: INLINE,
        extensions: [...baseExtensions(false), ...extensions],
      }),
    );

  it("leaves the keys to the hidden-mark layer until the tools are in", () => {
    expect(pictureForKeys(state([picturesDrawn.of(true)]), 2, true)).toBeNull();
    const on = state([picturesDrawn.of(true), pictureKeysOn.of(true)]);
    expect(pictureForKeys(on, 2, true)).toMatchObject({ from: 2, to: 20 });
    expect(pictureForKeys(on, 20, false)).toMatchObject({ from: 2, to: 20 });
    expect(pictureForKeys(on, 20, true)).toBeNull();
    expect(pictureForKeys(on, 2, false)).toBeNull();
  });
});
