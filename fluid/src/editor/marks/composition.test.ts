/**
 * An IME composition, judged when it ends (I3, D33, D34). jsdom has no IME,
 * so a composition is played the way CodeMirror delivers one: the view
 * reports `composing` (stubbed, as caret.test.ts and deleteCheck.test.ts do),
 * the composed changes arrive as `input.type.compose` transactions, and the
 * `compositionend` event starts the 60 ms wait for the last change (fake
 * timers).
 */

import { EditorSelection, StateEffect } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import {
  afterEach,
  beforeAll,
  beforeEach,
  describe,
  expect,
  it,
  vi,
} from "vitest";

import { cleanupCells, openAt } from "../../test/cellKit";
import {
  bytes,
  caret,
  cleanupMarkViews,
  keys,
  markView,
  remoteInsert,
  setCaret,
  typeText,
  undoOnce,
} from "../../test/markKit";
import { prefetchGrid } from "../grid/loader";
import { gridTables } from "../grid/structure";
import { CANNOT_JOIN } from "./deleteCheck";
import { formatsAt } from "./formatting";

beforeAll(async () => {
  await prefetchGrid();
});

beforeEach(() => {
  vi.useFakeTimers();
});

afterEach(() => {
  cleanupCells();
  cleanupMarkViews();
  vi.useRealTimers();
});

/** An IME on `view`: start, composed changes, end. */
function ime(view: EditorView) {
  const state = { on: false };
  Object.defineProperty(view, "composing", {
    configurable: true,
    get: () => state.on,
  });
  Object.defineProperty(view, "compositionStarted", {
    configurable: true,
    get: () => state.on,
  });
  let first = true;
  return {
    /**
     * Starts one in CodeMirror's order: its own observer marks the view as
     * composing before any plugin hears the event.
     */
    start() {
      state.on = true;
      first = true;
      view.contentDOM.dispatchEvent(
        new Event("compositionstart", { bubbles: true }),
      );
    },
    /** One composed change, the caret after its text. */
    change(from: number, to: number, insert: string) {
      view.dispatch({
        changes: { from, to, insert },
        selection: EditorSelection.cursor(from + insert.length),
        userEvent: first ? "input.type.compose.start" : "input.type.compose",
      });
      first = false;
    },
    /**
     * Ends it the way Chrome's EditContext does: no `compositionend` on the
     * content, only the view's next update with the composition over.
     */
    endQuietly() {
      state.on = false;
      view.dispatch({});
      vi.advanceTimersByTime(60);
    },
    /** Ends it; with `settle`, the 60 ms for its last change pass too. */
    end(settle = true) {
      state.on = false;
      view.contentDOM.dispatchEvent(
        new Event("compositionend", { bubbles: true }),
      );
      if (settle) vi.advanceTimersByTime(60);
    },
  };
}

/** What the view says, from now on. */
function hear(view: EditorView): string[] {
  const said: string[] = [];
  view.dispatch({
    effects: StateEffect.appendConfig.of(
      EditorView.updateListener.of((update) => {
        for (const tr of update.transactions) {
          for (const effect of tr.effects) {
            if (effect.is(EditorView.announce)) said.push(effect.value);
          }
        }
      }),
    ),
  });
  return said;
}

const CASES = (["hidden", "show"] as const).flatMap((mode) =>
  [false, true].map((room) => ({ mode, room })),
);

describe.each(CASES)(
  "a format waiting for a composition, $mode mode, room $room",
  ({ mode, room }) => {
    it("writes Bold around the composed word at its end, and one undo takes it", () => {
      const view = markView("ab", { mode, room });
      setCaret(view, 2);
      keys(view, "Mod-b");
      const keyboard = ime(view);
      keyboard.start();
      keyboard.change(2, 2, "c");
      keyboard.change(2, 3, "cd");
      // Nothing is written beside the composition while it runs.
      expect(bytes(view.state)).toBe("abcd");
      expect(formatsAt(view.state, 2).has("strong")).toBe(true);
      keyboard.end();
      expect(bytes(view.state)).toBe("ab**cd**");
      expect(caret(view)).toBe(6);
      undoOnce(view);
      expect(bytes(view.state)).toBe("ab");
    });
  },
);

describe("a composition that ends with no compositionend event", () => {
  it("is judged at the view's next update", () => {
    const view = markView("ab");
    setCaret(view, 2);
    keys(view, "Mod-b");
    const keyboard = ime(view);
    keyboard.start();
    keyboard.change(2, 2, "cd");
    keyboard.endQuietly();
    expect(bytes(view.state)).toBe("ab**cd**");
  });
});

describe.each([false, true])("the phone path of D34, room %s", (room) => {
  it("keeps a composed word after a bold word and a space bold", () => {
    const view = markView("**red**", { room });
    setCaret(view, 7);
    typeText(view, " ");
    expect(bytes(view.state)).toBe("**red** ");
    const keyboard = ime(view);
    keyboard.start();
    keyboard.change(8, 8, "s");
    keyboard.change(8, 9, "sea");
    keyboard.end();
    expect(bytes(view.state)).toBe("**red** **sea**");
  });

  it("moves a space composed inside the closing marks after them", () => {
    const view = markView("**bold**", { room });
    setCaret(view, 8);
    expect(caret(view)).toBe(6);
    const keyboard = ime(view);
    keyboard.start();
    keyboard.change(6, 6, " ");
    keyboard.end();
    expect(bytes(view.state)).toBe("**bold** ");
    keyboard.start();
    keyboard.change(9, 9, "x");
    keyboard.end();
    expect(bytes(view.state)).toBe("**bold** **x**");
  });
});

describe.each([false, true])("a composed deletion, room %s", (room) => {
  it("leaves an exact one as it is (Gboard writes `ab` as `a`)", () => {
    const view = markView("**ab** c", { room });
    const keyboard = ime(view);
    keyboard.start();
    keyboard.change(2, 4, "a");
    keyboard.end();
    expect(bytes(view.state)).toBe("**a** c");
  });

  it("moves the closing mark across the space it leaves", () => {
    const view = markView("**a b**", { room });
    const keyboard = ime(view);
    keyboard.start();
    keyboard.change(4, 5, "");
    keyboard.end();
    expect(bytes(view.state)).toBe("**a** ");
    undoOnce(view);
    expect(bytes(view.state)).toBe("**a b**");
  });

  it("merges two bold words whose space it takes", () => {
    const view = markView("**a** **b**", { room });
    const keyboard = ime(view);
    keyboard.start();
    keyboard.change(5, 6, "");
    keyboard.end();
    expect(bytes(view.state)).toBe("**ab**");
    undoOnce(view);
    expect(bytes(view.state)).toBe("**a** **b**");
  });

  it("puts back what it took when no repair keeps the pair, and says so", () => {
    const view = markView("**a.** b", { room });
    const said = hear(view);
    setCaret(view, 7);
    const keyboard = ime(view);
    keyboard.start();
    keyboard.change(6, 7, "");
    expect(bytes(view.state)).toBe("**a.**b");
    keyboard.end();
    expect(bytes(view.state)).toBe("**a.** b");
    expect(said).toContain(CANNOT_JOIN);
    undoOnce(view);
    expect(bytes(view.state)).toBe("**a.** b");
  });

  it("judges the change that arrives just after compositionend too", () => {
    const view = markView("**a.** b", { room });
    const keyboard = ime(view);
    keyboard.start();
    keyboard.change(8, 8, "b");
    keyboard.end(false);
    // Before the 60 ms are up, the space goes as plain typing: one change
    // with the composition, a space written over as `b`.
    view.dispatch({ changes: { from: 6, to: 7 }, userEvent: "input.type" });
    vi.advanceTimersByTime(60);
    expect(bytes(view.state)).toBe("**a.b**b");
  });

  it("judges it on a Reflection page the same way", () => {
    const view = markView("## Notes\n\n**a.** b", {
      room,
      adapter: "reflection",
    });
    const keyboard = ime(view);
    keyboard.start();
    keyboard.change(16, 17, "");
    keyboard.end();
    expect(bytes(view.state)).toBe("## Notes\n\n**a.** b");
  });
});

describe("a composition a peer's edit interrupts", () => {
  it("goes unjudged: the peer sees the result", () => {
    const view = markView("**a.** b", { room: true });
    const keyboard = ime(view);
    keyboard.start();
    keyboard.change(6, 7, "");
    remoteInsert(view, 0, "Z");
    keyboard.end();
    expect(bytes(view.state)).toBe("Z**a.**b");
  });
});

describe("a composition in a table cell", () => {
  it("is judged by the cell, with the cell's marks", () => {
    const main = markView("| a | b |\n| - | - |\n| x | **a.** b |\n");
    const cell = gridTables(main.state)[0]?.rows[1]?.[1];
    if (!cell) throw new Error("no cell");
    const view = openAt(main, cell.content.from, false).view;
    const space = cell.content.from + 6;
    expect(view.state.sliceDoc(space, space + 1)).toBe(" ");
    const keyboard = ime(view);
    keyboard.start();
    keyboard.change(space, space + 1, "");
    keyboard.end();
    expect(main.state.doc.lineAt(cell.content.from).text).toBe(
      "| x | **a.** b |",
    );
  });
});

/**
 * The person acts in the 60 ms after a composition, before it is judged
 * (D47, D48, D50, D51): the composition is judged first, on the state before
 * the action, and the action follows.
 */
describe.each([false, true])(
  "an action in the settle window, room %s",
  (room) => {
    it("judges a composed deletion before the Backspace that follows it", () => {
      const view = markView("**a.** bc", { room });
      const said = hear(view);
      setCaret(view, 7);
      const keyboard = ime(view);
      keyboard.start();
      keyboard.change(6, 7, "");
      keyboard.end(false);
      keys(view, "Backspace");
      vi.advanceTimersByTime(60);
      // The space comes back first, with the caret after it, so the
      // Backspace is judged on that text: refused, as on a desktop (D5 (c)).
      expect(bytes(view.state)).toBe("**a.** bc");
      expect(said.filter((each) => each === CANNOT_JOIN)).toHaveLength(2);
    });

    it("writes Bold around the composed word before an Enter that follows it", () => {
      const view = markView("ab", { room });
      setCaret(view, 2);
      keys(view, "Mod-b");
      const keyboard = ime(view);
      keyboard.start();
      keyboard.change(2, 2, "cd");
      keyboard.end(false);
      keys(view, "Enter");
      vi.advanceTimersByTime(60);
      expect(bytes(view.state)).toBe("ab**cd**\n");
    });

    it("keeps Bold for a Korean syllable whose space comes with the commit", () => {
      const view = markView("ab", { room });
      setCaret(view, 2);
      keys(view, "Mod-b");
      const keyboard = ime(view);
      keyboard.start();
      keyboard.change(2, 2, "\ud55c");
      keyboard.end(false);
      // The key that commits the syllable types the space too, as plain input.
      typeText(view, " ");
      vi.advanceTimersByTime(60);
      expect(bytes(view.state)).toBe("ab**\ud55c** ");
      keyboard.start();
      keyboard.change(8, 8, "\uad6d");
      keyboard.end();
      expect(bytes(view.state)).toBe("ab**\ud55c** **\uad6d**");
    });

    it("judges the last composition before the next one begins (CodeMirror's event order)", () => {
      const view = markView("**a.** b", { room });
      const keyboard = ime(view);
      keyboard.start();
      keyboard.change(6, 7, "");
      keyboard.end(false);
      expect(view.compositionStarted).toBe(false);
      keyboard.start();
      expect(view.compositionStarted).toBe(true);
      // The new composition types after the `b` the person sees.
      keyboard.change(7, 7, "x");
      keyboard.end();
      expect(bytes(view.state)).toBe("**a.** bx");
    });

    it("keeps a caret the person moved before the judge ran", () => {
      const view = markView("xyz ab", { room });
      setCaret(view, 6);
      keys(view, "Mod-b");
      const keyboard = ime(view);
      keyboard.start();
      keyboard.change(6, 6, "cd");
      keyboard.end(false);
      setCaret(view, 1);
      vi.advanceTimersByTime(60);
      expect(bytes(view.state)).toBe("xyz ab**cd**");
      expect(caret(view)).toBe(1);
    });
  },
);

describe.each([false, true])(
  "a composition beside a Reflection id line (D33), room %s",
  (room) => {
    const ID = "<!-- r:7f3a9c2e -->";

    it("refuses a composed deletion on the line below the id and keeps the id line", () => {
      const text = `## Night\n${ID}\n**a.** b\n`;
      const view = markView(text, { room, adapter: "reflection" });
      const at = text.indexOf(" b");
      const keyboard = ime(view);
      keyboard.start();
      keyboard.change(at, at + 1, "");
      keyboard.end();
      expect(bytes(view.state)).toBe(text);
    });

    it("never joins the line below onto the id line", () => {
      const text = `## Night\n${ID}\nText\n`;
      const view = markView(text, { room, adapter: "reflection" });
      const at = text.indexOf("\nText");
      const keyboard = ime(view);
      keyboard.start();
      keyboard.change(at, at + 1, "");
      keyboard.end();
      expect(bytes(view.state)).toContain(`\n${ID}\n`);
    });
  },
);
