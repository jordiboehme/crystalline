/**
 * The picture verbs: each one change to exactly the bytes it names, one undo
 * step, re-read at dispatch, and refused where a picture is not this page's
 * to change (a link keeps its click, a cell is the grid's).
 *
 * Every case runs through `markView`, so the hidden-mark checks the screens
 * install run too, in LF and in CRLF.
 */

import { syntaxTree } from "@codemirror/language";
import { EditorSelection, EditorState, StateEffect } from "@codemirror/state";
import type { SyntaxNode } from "@lezer/common";
import type { EditorView } from "@codemirror/view";
import { afterEach, describe, expect, it } from "vitest";

import {
  bytes,
  cleanupMarkViews,
  markPeers,
  markView,
  undoOnce,
} from "../../test/markKit";
import { parsedState } from "../../test/parse";
import { blockInsertion } from "../toolbar";
import { dropLanding } from "./landing";
import { pictureAt, picturesIn, selectedPicture } from "./refs";
import { pictureRemoval } from "./removal";
import {
  movePicture,
  removePicture,
  setPictureAlign,
  setPictureAlt,
  setPictureSize,
  setPictureWidth,
} from "./verbs";

afterEach(cleanupMarkViews);

/** Each fixture: the picture, and its target token after a medium size. */
const FIXTURES: { picture: string; medium: string }[] = [
  {
    picture: "![a](./assets/a.png)",
    medium: "![a](./assets/a.png#w=50%)",
  },
  {
    picture: '![a](assets/a.png "Q3")',
    medium: '![a](assets/a.png#w=50% "Q3")',
  },
  {
    picture: "![a](assets/a(1).png)",
    medium: "![a](assets/a(1).png#w=50%)",
  },
  {
    picture: "![a](assets/a%20b.png)",
    medium: "![a](assets/a%20b.png#w=50%)",
  },
  {
    picture: "![a](assets/a.png#right,foo)",
    medium: "![a](assets/a.png#right,w=50%)",
  },
  {
    picture: "![a [b] c](assets/a.png)",
    medium: "![a [b] c](assets/a.png#w=50%)",
  },
];

const ENDINGS = [
  { name: "LF", br: "\n" },
  { name: "CRLF", br: "\r\n" },
] as const;

/** A page with `picture` alone in a paragraph between two others. */
function page(picture: string, br: string): string {
  return ["intro", "", picture, "", "later", ""].join(br);
}

function viewOf(text: string, br: string, room = false): EditorView {
  return markView(text, {
    room,
    lineSeparator: br === "\r\n" ? "\r\n" : "\n",
  });
}

/** The position of the picture's first character in `view`. */
function pictureStart(view: EditorView): number {
  const [ref] = picturesIn(parsedState(view.state));
  if (!ref) throw new Error("no picture");
  return ref.from;
}

/** The document with `[from, to)` (positions) replaced, as bytes. */
function replaced(
  view: EditorView,
  from: number,
  to: number,
  insert: string,
): string {
  const { state } = view;
  return bytes(
    state.update({ changes: { from, to, insert }, filter: false }).state,
  );
}

for (const { name, br } of ENDINGS) {
  describe(`the picture verbs in ${name}`, () => {
    for (const { picture, medium } of FIXTURES) {
      const original = page(picture, br);

      it(`sizes only the target of ${picture}`, () => {
        const view = viewOf(original, br);
        const at = pictureStart(view) + 1;
        expect(setPictureSize(view, "medium", at)).toBe(true);
        expect(bytes(view.state)).toBe(page(medium, br));
        const selected = selectedPicture(parsedState(view.state));
        expect(view.state.sliceDoc(selected?.from, selected?.to)).toBe(medium);
        undoOnce(view);
        expect(bytes(view.state)).toBe(original);
      });

      it(`changes only the alt middle of ${picture}`, () => {
        const view = viewOf(original, br);
        const ref = pictureAt(parsedState(view.state), pictureStart(view) + 1);
        if (!ref) throw new Error("no picture");
        const alt = view.state.sliceDoc(ref.altFrom, ref.altTo);
        const display = alt.replace(/\\(.)/g, "$1");
        expect(setPictureAlt(view, `${display}!`, ref.from)).toBe(true);
        expect(bytes(view.state)).toBe(
          page(
            `${picture.slice(0, 2 + alt.length)}!${picture.slice(2 + alt.length)}`,
            br,
          ),
        );
        expect(selectedPicture(parsedState(view.state))?.from).toBe(ref.from);
        undoOnce(view);
        expect(bytes(view.state)).toBe(original);
      });

      it(`removes ${picture} with its line`, () => {
        const view = viewOf(original, br);
        const state = parsedState(view.state);
        const ref = pictureAt(state, pictureStart(view) + 1);
        if (!ref) throw new Error("no picture");
        const range = pictureRemoval(state, ref);
        const expected = replaced(view, range.from, range.to, "");
        expect(removePicture(view, ref.from)).toBe(true);
        expect(bytes(view.state)).toBe(expected);
        expect(bytes(view.state)).toBe(["intro", "", "later", ""].join(br));
        expect(view.state.selection.main.empty).toBe(true);
        expect(view.state.selection.main.head).toBe(range.from);
        undoOnce(view);
        expect(bytes(view.state)).toBe(original);
      });

      it(`moves ${picture} below a later paragraph`, () => {
        const view = viewOf(original, br);
        const state = parsedState(view.state);
        const ref = pictureAt(state, pictureStart(view) + 1);
        if (!ref) throw new Error("no picture");
        const landing = dropLanding(state, state.doc.length - 2);
        if (!landing) throw new Error("no landing");
        const range = pictureRemoval(state, ref);
        const { change } = blockInsertion(state, state.doc.line(landing.line), [
          state.sliceDoc(ref.from, ref.to),
        ]);
        const expected = bytes(
          state.update({
            changes: [{ from: range.from, to: range.to }, change],
            filter: false,
          }).state,
        );
        expect(movePicture(view, ref.from, landing)).toBe(true);
        expect(bytes(view.state)).toBe(expected);
        expect(bytes(view.state)).toBe(
          ["intro", "", "later", "", picture, "", ""].join(br),
        );
        const selected = selectedPicture(parsedState(view.state));
        expect(view.state.sliceDoc(selected?.from, selected?.to)).toBe(picture);
        undoOnce(view);
        expect(bytes(view.state)).toBe(original);
      });

      it(`does not move ${picture} where it would land as it stands`, () => {
        const view = viewOf(original, br);
        const state = parsedState(view.state);
        const ref = pictureAt(state, pictureStart(view) + 1);
        if (!ref) throw new Error("no picture");
        // The blank line above the picture lands after "intro".
        const landing = dropLanding(state, ref.from - 1);
        expect(landing?.line).toBe(1);
        if (!landing) throw new Error("no landing");
        let dispatched = 0;
        const off = trackDispatches(view, () => (dispatched += 1));
        expect(movePicture(view, ref.from, landing)).toBe(false);
        off();
        expect(dispatched).toBe(0);
        expect(bytes(view.state)).toBe(original);
        // Onto the blank line itself: taken away with the picture.
        const blank = state.doc.lineAt(ref.from - 1);
        expect(
          movePicture(view, ref.from, {
            line: blank.number,
            pos: blank.to,
            caret: blank.to,
          }),
        ).toBe(false);
        expect(bytes(view.state)).toBe(original);
        // Onto its own line: inside the range the move takes away.
        const own = state.doc.lineAt(ref.from);
        expect(
          movePicture(view, ref.from, {
            line: own.number,
            pos: own.to,
            caret: own.to,
          }),
        ).toBe(false);
        expect(bytes(view.state)).toBe(original);
      });
    }

    it("refuses every format verb on an external picture and keeps the rest", () => {
      const picture = "![a](https://e.org/a.png)";
      const original = page(picture, br);
      const view = viewOf(original, br);
      const at = pictureStart(view) + 1;
      expect(setPictureSize(view, "small", at)).toBe(false);
      expect(setPictureAlign(view, "right", at)).toBe(false);
      expect(setPictureWidth(view, 40, at)).toBe(false);
      expect(bytes(view.state)).toBe(original);
      expect(setPictureAlt(view, "b", at)).toBe(true);
      expect(bytes(view.state)).toBe(page("![b](https://e.org/a.png)", br));
      undoOnce(view);
      const state = parsedState(view.state);
      const landing = dropLanding(state, state.doc.length - 2);
      if (!landing) throw new Error("no landing");
      expect(movePicture(view, at - 1, landing)).toBe(true);
      expect(bytes(view.state)).toBe(
        ["intro", "", "later", "", picture, "", ""].join(br),
      );
      undoOnce(view);
      expect(bytes(view.state)).toBe(original);
      expect(removePicture(view, at)).toBe(true);
      expect(bytes(view.state)).toBe(["intro", "", "later", ""].join(br));
    });

    for (const [where, text] of [
      ["a link", "[![a](assets/a.png)](https://e.org)"],
      ["a cell", "| h | i |\n| --- | --- |\n| ![a](assets/a.png) | c |"],
    ] as const) {
      it(`refuses everything on a picture in ${where}`, () => {
        const original = page(text.split("\n").join(br), br);
        const view = viewOf(original, br);
        const at = pictureStart(view) + 1;
        const state = parsedState(view.state);
        const landing = dropLanding(state, state.doc.length - 2);
        if (!landing) throw new Error("no landing");
        const ref = pictureAt(state, at);
        expect(where === "a link" ? ref?.inLink : ref?.inCell).toBe(true);
        expect(setPictureSize(view, "small", at)).toBe(false);
        expect(setPictureAlign(view, "left", at)).toBe(false);
        expect(setPictureWidth(view, 40, at)).toBe(false);
        expect(setPictureAlt(view, "b", at)).toBe(false);
        expect(removePicture(view, at)).toBe(false);
        expect(movePicture(view, at, landing)).toBe(false);
        expect(bytes(view.state)).toBe(original);
      });
    }
  });
}

/** Counts the view's dispatches until the returned function is called. */
function trackDispatches(view: EditorView, onDispatch: () => void): () => void {
  const own = view.dispatch.bind(view);
  view.dispatch = ((...args: Parameters<EditorView["dispatch"]>) => {
    onDispatch();
    (own as (...a: unknown[]) => void)(...args);
  }) as EditorView["dispatch"];
  return () => {
    view.dispatch = own;
  };
}

/** A page with one picture `picture`, the caret on it. */
function on(picture: string, room = false): EditorView {
  const view = markView(`${picture}\n`, { room });
  view.dispatch({ selection: EditorSelection.cursor(1) });
  return view;
}

/** The single picture's token. */
function token(view: EditorView): string {
  const [ref] = picturesIn(parsedState(view.state));
  return ref?.token ?? "";
}

describe("setPictureAlign", () => {
  it("keeps the width", () => {
    const view = on("![a](assets/a.png#w=40%)");
    expect(setPictureAlign(view, "left")).toBe(true);
    expect(token(view)).toBe("assets/a.png#left,w=40%");
    expect(setPictureAlign(view, "center")).toBe(true);
    expect(token(view)).toBe("assets/a.png#w=40%");
  });

  it("turns a full picture into that placement with no width", () => {
    const view = on("![a](assets/a.png#full,w=40%)");
    expect(setPictureAlign(view, "right")).toBe(true);
    expect(token(view)).toBe("assets/a.png#right");
  });

  it("dispatches nothing for the placement a picture already has", () => {
    const view = on("![a](assets/a.png#left)");
    const before = view.state;
    expect(setPictureAlign(view, "left")).toBe(true);
    expect(view.state.doc).toBe(before.doc);
  });

  it("leaves the picture selected", () => {
    const view = on("x ![a](assets/a.png) y");
    view.dispatch({ selection: EditorSelection.cursor(3) });
    expect(setPictureAlign(view, "right")).toBe(true);
    const selected = selectedPicture(parsedState(view.state));
    expect(selected?.token).toBe("assets/a.png#right");
  });
});

describe("setPictureSize", () => {
  it("writes small and medium as shares, keeping a float", () => {
    const view = on("![a](assets/a.png#left)");
    expect(setPictureSize(view, "small")).toBe(true);
    expect(token(view)).toBe("assets/a.png#left,w=25%");
    expect(setPictureSize(view, "medium")).toBe(true);
    expect(token(view)).toBe("assets/a.png#left,w=50%");
  });

  it("turns full into center for a share", () => {
    const view = on("![a](assets/a.png#full)");
    expect(setPictureSize(view, "small")).toBe(true);
    expect(token(view)).toBe("assets/a.png#w=25%");
  });

  it("writes full with no width", () => {
    const view = on("![a](assets/a.png#right,w=30%)");
    expect(setPictureSize(view, "full")).toBe(true);
    expect(token(view)).toBe("assets/a.png#full");
  });

  it("drops the width for original and turns full into center", () => {
    const right = on("![a](assets/a.png#right,w=30%)");
    expect(setPictureSize(right, "original")).toBe(true);
    expect(token(right)).toBe("assets/a.png#right");
    const full = on("![a](assets/a.png#full)");
    expect(setPictureSize(full, "original")).toBe(true);
    expect(token(full)).toBe("assets/a.png");
  });

  it("refuses where there is no picture", () => {
    const view = markView("just prose\n");
    expect(setPictureSize(view, "small", 3)).toBe(false);
    expect(bytes(view.state)).toBe("just prose\n");
  });

  it("refuses on a page that takes no writes", () => {
    const view = on("![a](assets/a.png)");
    view.dispatch({
      effects: StateEffect.appendConfig.of(EditorState.readOnly.of(true)),
    });
    expect(setPictureSize(view, "small")).toBe(false);
    expect(setPictureAlt(view, "b")).toBe(false);
    expect(removePicture(view)).toBe(false);
    expect(bytes(view.state)).toBe("![a](assets/a.png)\n");
  });
});

describe("setPictureWidth", () => {
  it("writes an integer share clamped to 10 through 100", () => {
    const view = on("![a](assets/a.png#right)");
    expect(setPictureWidth(view, 33.4)).toBe(true);
    expect(token(view)).toBe("assets/a.png#right,w=33%");
    expect(setPictureWidth(view, 4)).toBe(true);
    expect(token(view)).toBe("assets/a.png#right,w=10%");
    expect(setPictureWidth(view, 250)).toBe(true);
    expect(token(view)).toBe("assets/a.png#right,w=100%");
  });
});

describe("setPictureAlt", () => {
  it("dispatches nothing for the alt a picture already has", () => {
    const view = on("![a \\[b\\]](assets/a.png)");
    const before = view.state;
    expect(setPictureAlt(view, "a [b]")).toBe(true);
    expect(view.state.doc).toBe(before.doc);
  });

  it("writes brackets escaped", () => {
    const view = on("![a](assets/a.png)");
    expect(setPictureAlt(view, "a]")).toBe(true);
    expect(bytes(view.state)).toBe("![a\\]](assets/a.png)\n");
  });
});

describe("the picture verbs re-read at dispatch", () => {
  it("acts on the picture at `at`, not at the caret", () => {
    const view = markView("![a](assets/a.png) ![b](assets/b.png)\n");
    view.dispatch({ selection: EditorSelection.cursor(1) });
    const second = view.state.doc.toString().indexOf("![b]") + 2;
    expect(setPictureSize(view, "small", second)).toBe(true);
    expect(bytes(view.state)).toBe(
      "![a](assets/a.png) ![b](assets/b.png#w=25%)\n",
    );
  });
});

/** Whether `word` reads bold in `view`'s document. */
function bold(view: EditorView, word: string): boolean {
  const state = parsedState(view.state);
  const doc = state.doc.toString();
  const at = doc.indexOf(word);
  let node = syntaxAt(state, at);
  while (node) {
    if (node.name === "StrongEmphasis") return true;
    node = node.parent;
  }
  return false;
}

function syntaxAt(state: EditorState, at: number): SyntaxNode | null {
  return syntaxTree(state).resolveInner(at, 1);
}

describe("movePicture", () => {
  it("moves a picture in prose below its own paragraph", () => {
    const doc = "text ![x](assets/a.png) more\n\nlater\n";
    const view = markView(doc);
    const state = parsedState(view.state);
    const landing = dropLanding(state, 1);
    expect(landing?.line).toBe(1);
    if (!landing) throw new Error("no landing");
    expect(movePicture(view, 5, landing)).toBe(true);
    expect(bytes(view.state)).toBe(
      "text  more\n\n![x](assets/a.png)\n\n\nlater\n",
    );
    expect(selectedPicture(parsedState(view.state))?.from).toBe(12);
  });

  it("moves the picture that starts at `from` where two meet", () => {
    const doc = "![a](assets/a.png)![b](assets/b.png)\n\nlater\n";
    const view = markView(doc);
    const state = parsedState(view.state);
    const landing = dropLanding(state, state.doc.length - 2);
    if (!landing) throw new Error("no landing");
    const b = doc.indexOf("![b]");
    expect(movePicture(view, b + 1, landing)).toBe(false);
    expect(movePicture(view, b, landing)).toBe(true);
    expect(bytes(view.state)).toBe(
      "![a](assets/a.png)\n\nlater\n\n![b](assets/b.png)\n\n",
    );
  });

  it("refuses a landing past the end of the page", () => {
    const view = markView("![x](assets/a.png)\n");
    expect(movePicture(view, 0, { line: 9, pos: 0, caret: 0 })).toBe(false);
  });
});

describe("a picture between two bold words", () => {
  const BOLD = "**a**![x](assets/p.png)**b**";

  for (const room of [false, true]) {
    it(`keeps both bolds when it is removed ${room ? "in a room" : "solo"}`, () => {
      const view = markView(`${BOLD}\n`, { room });
      const twin = markView(`${BOLD}\n`, { room });
      const ref = pictureAt(parsedState(view.state), 6);
      if (!ref) throw new Error("no picture");
      twin.dispatch({
        changes: { from: ref.from, to: ref.to },
        selection: EditorSelection.cursor(ref.from),
        userEvent: "delete.selection",
      });
      expect(removePicture(view, 6)).toBe(true);
      expect(bytes(view.state)).toBe(bytes(twin.state));
      expect(bytes(view.state)).not.toBe("**a****b**\n");
      expect(bold(view, "a")).toBe(true);
      expect(bold(view, "b")).toBe(true);
      undoOnce(view);
      expect(bytes(view.state)).toBe(`${BOLD}\n`);
    });

    it(`keeps both bolds when it is moved away ${room ? "in a room" : "solo"}`, () => {
      const view = markView(`${BOLD}\n\nlater\n`, { room });
      const state = parsedState(view.state);
      const landing = dropLanding(state, state.doc.length - 2);
      if (!landing) throw new Error("no landing");
      expect(movePicture(view, 5, landing)).toBe(true);
      expect(bold(view, "a")).toBe(true);
      expect(bold(view, "b")).toBe(true);
      expect(bytes(view.state)).toContain("later\n\n![x](assets/p.png)\n");
      const selected = selectedPicture(parsedState(view.state));
      expect(selected?.token).toBe("assets/p.png");
      undoOnce(view);
      expect(bytes(view.state)).toBe(`${BOLD}\n\nlater\n`);
    });
  }

  it("reaches the other peer of a room as the same bytes", () => {
    const { local, peer, sync } = markPeers(`${BOLD}\n`);
    expect(removePicture(local, 6)).toBe(true);
    sync();
    expect(bytes(peer.state)).toBe(bytes(local.state));
    expect(bold(peer, "a")).toBe(true);
    expect(bold(peer, "b")).toBe(true);
  });
});
