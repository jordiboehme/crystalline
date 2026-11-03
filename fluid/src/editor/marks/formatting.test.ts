/**
 * The format commands over hidden marks: Bold, Italic, Strikethrough and
 * Inline code on a selection change the format of exactly the visible
 * characters picked, with no selection they wait for the next typed text,
 * and typing at the edge of a format continues it. Each case runs solo and in
 * a room; show mode and Raw keep the source view's own behaviour where the
 * marks show.
 */

import { EditorSelection, StateEffect } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { afterEach, describe, expect, it, vi } from "vitest";

import {
  bytes,
  caret,
  cleanupMarkViews,
  keys,
  markView,
  setCaret,
  setShowMarks,
  typeText,
  undoOnce,
} from "../../test/markKit";
import { marksOnLine } from "./hiddenMarks";
import type { Format } from "./formatting";
import {
  charFormats,
  formatsAt,
  pendingFormat,
  toggleFormat,
} from "./formatting";

afterEach(cleanupMarkViews);

function select(view: EditorView, from: number, to: number): void {
  view.dispatch({
    selection: EditorSelection.range(from, to),
    userEvent: "select.pointer",
  });
}

/** Every sentence the view announces from now on. */
function announcements(view: EditorView): string[] {
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

/** The formats of the visible character at `pos` (a character, not a caret). */
function formatsOf(view: EditorView, pos: number): Format[] {
  return [...charFormats(view.state, pos)].sort();
}

describe.each([
  { name: "solo", room: false },
  { name: "in a room", room: true },
])("hidden mode, $name", ({ room }) => {
  const view = (text: string) => markView(text, { room });

  describe("a selection", () => {
    it.each([
      ["ab **cd** ef", 1, 6, "strong", "a**b cd** ef"],
      ["**hello**", 4, 6, "strong", "**he**ll**o**"],
      ["**the big dog**", 6, 9, "strong", "**the** big **dog**"],
      ["a b c", 1, 4, "strong", "a **b** c"],
      ["x **y** z", 0, 9, "strong", "**x y z**"],
      ["**bold**", 2, 6, "strong", "bold"],
      ["**bold**", 2, 6, "em", "***bold***"],
      ["a**b**", 0, 1, "strong", "**ab**"],
      ["**ab**cd", 6, 8, "strong", "**abcd**"],
      ["a ~~b~~ c", 0, 9, "strike", "~~a b c~~"],
      ["a b c", 2, 3, "code", "a `b` c"],
      ["a `b` c", 3, 4, "code", "a b c"],
    ] as const)(
      "%j [%i,%i) %s gives %j, one undo gives it back",
      (text, from, to, format, expected) => {
        const v = view(text);
        select(v, from, to);
        expect(toggleFormat(v, format)).toBe(true);
        expect(bytes(v.state)).toBe(expected);
        undoOnce(v);
        expect(bytes(v.state)).toBe(text);
      },
    );

    it("keeps the same visible characters selected", () => {
      const v = view("**hello**");
      select(v, 4, 6);
      toggleFormat(v, "strong");
      const { from, to } = v.state.selection.main;
      expect(v.state.sliceDoc(from, to)).toBe("ll");
    });

    it("a selection of only spaces changes nothing", () => {
      const v = view("a   b");
      select(v, 1, 4);
      expect(toggleFormat(v, "strong")).toBe(true);
      expect(bytes(v.state)).toBe("a   b");
    });

    it("code over another mark is refused and says why", () => {
      const v = view("a **b** c");
      const said = announcements(v);
      select(v, 0, 9);
      expect(toggleFormat(v, "code")).toBe(true);
      expect(bytes(v.state)).toBe("a **b** c");
      expect(said).toEqual(["This text cannot be formatted here."]);
    });

    it("a format the parser would not read as asked is refused and says why", () => {
      // `**a.**b` is no bold: a closing `**` after a full stop and before a
      // letter does not close. Nothing is written.
      const v = view("a.b");
      const said = announcements(v);
      select(v, 0, 2);
      expect(toggleFormat(v, "strong")).toBe(true);
      expect(bytes(v.state)).toBe("a.b");
      expect(said).toEqual(["This text cannot be formatted here."]);
    });

    it("is its own undo step, apart from the typing before it", () => {
      const v = view("ab");
      setCaret(v, 2);
      typeText(v, "c");
      select(v, 0, 3);
      toggleFormat(v, "strong");
      expect(bytes(v.state)).toBe("**abc**");
      undoOnce(v);
      expect(bytes(v.state)).toBe("abc");
    });

    it("Mod-b and Mod-i run it", () => {
      const v = view("a b c");
      select(v, 2, 3);
      keys(v, "Mod-b");
      expect(bytes(v.state)).toBe("a **b** c");
      select(v, 0, 1);
      keys(v, "Mod-i");
      expect(bytes(v.state)).toBe("*a* **b** c");
    });
  });

  describe("a pair across a soft line break", () => {
    const WRAPPED = "**a b\nc d**";

    it("its characters on both lines are bold", () => {
      const v = view(WRAPPED);
      expect(formatsOf(v, 4)).toEqual(["strong"]);
      expect(formatsOf(v, 6)).toEqual(["strong"]);
      expect([...formatsAt(v.state, 5)]).toEqual(["strong"]);
    });

    it("Bold on a word inside it takes the bold off that word only", () => {
      const v = view(WRAPPED);
      select(v, 4, 5);
      toggleFormat(v, "strong");
      expect(bytes(v.state)).toBe("**a** b\n**c d**");
      expect(formatsOf(v, 2)).toEqual(["strong"]);
      undoOnce(v);
      expect(bytes(v.state)).toBe(WRAPPED);
    });

    it("Bold on its first line's start keeps the pair whole", () => {
      const v = view(WRAPPED);
      select(v, 6, 7);
      toggleFormat(v, "strong");
      expect(bytes(v.state)).toBe("**a b**\nc **d**");
    });
  });

  describe("no selection", () => {
    it("Mod-b, then typing, writes the typed text bold; one undo takes it all", () => {
      const v = view("ab");
      setCaret(v, 2);
      keys(v, "Mod-b");
      expect(bytes(v.state)).toBe("ab");
      typeText(v, "cd");
      expect(bytes(v.state)).toBe("ab**cd**");
      undoOnce(v);
      expect(bytes(v.state)).toBe("ab");
    });

    it("Mod-b, then a click elsewhere, writes nothing and forgets it", () => {
      const v = view("ab");
      setCaret(v, 2);
      keys(v, "Mod-b");
      setCaret(v, 0);
      expect(bytes(v.state)).toBe("ab");
      expect(v.state.field(pendingFormat)).toBeNull();
      setCaret(v, 2);
      typeText(v, "c");
      expect(bytes(v.state)).toBe("abc");
    });

    it("Mod-b twice cancels", () => {
      const v = view("ab");
      setCaret(v, 2);
      keys(v, "Mod-b", "Mod-b");
      typeText(v, "c");
      expect(bytes(v.state)).toBe("abc");
    });

    it("Mod-b inside bold at its end types plain after it", () => {
      const v = view("**ab**");
      setCaret(v, 4);
      keys(v, "Mod-b");
      typeText(v, "c");
      expect(bytes(v.state)).toBe("**ab**c");
    });

    it("Mod-b in the middle of bold splits it around the typed text", () => {
      const v = view("**abcd**");
      setCaret(v, 4);
      keys(v, "Mod-b");
      typeText(v, "x");
      expect(bytes(v.state)).toBe("**ab**x**cd**");
      expect(formatsOf(v, 6)).toEqual([]);
    });

    it("a space with bold waiting is written plain and bold still waits", () => {
      const v = view("ab");
      setCaret(v, 2);
      keys(v, "Mod-b");
      typeText(v, " c");
      expect(bytes(v.state)).toBe("ab **c**");
    });

    it("the Bold verb with no selection writes nothing", () => {
      const v = view("ab");
      setCaret(v, 2);
      toggleFormat(v, "strong");
      expect(bytes(v.state)).toBe("ab");
      expect(formatsAt(v.state, 2).has("strong")).toBe(true);
    });

    it("a blur forgets what waits", async () => {
      const v = view("ab");
      v.focus();
      await vi.waitFor(() => {
        expect(v.hasFocus).toBe(true);
      });
      setCaret(v, 2);
      keys(v, "Mod-b");
      expect(v.state.field(pendingFormat)).not.toBeNull();
      v.contentDOM.blur();
      await vi.waitFor(() => {
        expect(v.state.field(pendingFormat)).toBeNull();
      });
    });

    it("a flip of the setting forgets what waits (the next c is plain)", () => {
      const v = view("ab");
      setCaret(v, 2);
      keys(v, "Mod-b");
      setShowMarks(v, true);
      typeText(v, "c");
      expect(bytes(v.state)).toBe("abc");
    });

    it("refuses on a page that is not writable", () => {
      const v = view("a b");
      v.dispatch({
        effects: StateEffect.appendConfig.of(EditorView.editable.of(false)),
      });
      select(v, 0, 1);
      toggleFormat(v, "strong");
      expect(bytes(v.state)).toBe("a b");
      setCaret(v, 1);
      toggleFormat(v, "strong");
      expect(v.state.field(pendingFormat)).toBeNull();
    });
  });

  describe("typing at the edge of a format continues it", () => {
    it("a letter at the end of bold is bold", () => {
      const v = view("a **bold** c");
      setCaret(v, 8);
      typeText(v, "x");
      expect(bytes(v.state)).toBe("a **boldx** c");
    });

    it("a space at the end of bold goes after the marks, and the next letter is bold", () => {
      const v = view("a **bold** c");
      setCaret(v, 8);
      typeText(v, " n");
      expect(bytes(v.state)).toBe("a **bold** **n** c");
      expect(formatsOf(v, 13)).toEqual(["strong"]);
    });

    it("left wins at the line end: ` more` after bold is bold, the space is not", () => {
      // The formatting on the caret's left wins, so the words typed after a
      // bold word at the end of a line stay bold. The space goes outside the
      // marks (a `**` after a space does not close), so the word after it
      // gets a pair of its own.
      const v = view("a **bold**");
      setCaret(v, 10);
      typeText(v, " more");
      expect(bytes(v.state)).toBe("a **bold** **more**");
      expect(formatsOf(v, 13)).toEqual(["strong"]);
    });

    it("a full stop stays inside where the bold still closes after it", () => {
      const v = view("a **bold** c");
      setCaret(v, 8);
      typeText(v, ".");
      expect(bytes(v.state)).toBe("a **bold.** c");
      typeText(v, "x");
      expect(bytes(v.state)).toBe("a **bold.x** c");
    });

    it("a full stop goes after the marks where inside it would end the bold", () => {
      const v = view("**bold**x");
      setCaret(v, 6);
      typeText(v, ".");
      expect(bytes(v.state)).toBe("**bold**.x");
      expect(formatsOf(v, 2)).toEqual(["strong"]);
    });
  });

  describe("Markdown typed by hand", () => {
    it("`**word** next` is written as typed, `next` plain", () => {
      const v = view("");
      typeText(v, "**word** next");
      expect(bytes(v.state)).toBe("**word** next");
      expect(formatsOf(v, 2)).toEqual(["strong"]);
      expect(formatsOf(v, 10)).toEqual([]);
    });

    it("`*it* next` leaves `next` plain", () => {
      const v = view("a ");
      setCaret(v, 2);
      typeText(v, "*it* next");
      expect(bytes(v.state)).toBe("a *it* next");
      expect(formatsOf(v, 9)).toEqual([]);
    });

    it("`- ` at a line start is a list mark", () => {
      const v = view("");
      typeText(v, "- ");
      expect(bytes(v.state)).toBe("- ");
      expect(marksOnLine(v.state, 1).map((mark) => mark.kind)).toContain(
        "list",
      );
    });
  });
});

describe.each([
  { name: "solo", room: false },
  { name: "in a room", room: true },
])("show mode, $name", ({ room }) => {
  it("Mod-b with no selection writes nothing; the typed text is bold", () => {
    const v = markView("ab", { mode: "show", room });
    setCaret(v, 2);
    keys(v, "Mod-b");
    expect(bytes(v.state)).toBe("ab");
    typeText(v, "c");
    expect(bytes(v.state)).toBe("ab**c**");
    // The marks show on the caret's line.
    expect(marksOnLine(v.state, 1)).toEqual([]);
    expect(caret(v)).toBe(5);
  });

  it("a selection keeps the source view's wrap", () => {
    const v = markView("**hello**", { mode: "show", room });
    select(v, 2, 7);
    keys(v, "Mod-b");
    expect(bytes(v.state)).toBe("hello");
  });
});

describe("Raw", () => {
  it("Mod-b with no selection writes a pair with the caret between", () => {
    const v = markView("ab", { raw: true });
    setCaret(v, 2);
    keys(v, "Mod-b");
    expect(bytes(v.state)).toBe("ab****");
    expect(caret(v)).toBe(4);
  });
});

describe("the format state", () => {
  it("reports bold at the caret inside bold, not outside, and after Mod-b", () => {
    const seen: ReadonlySet<string>[] = [];
    const v = markView("a **bold** c", {
      onFormatState: (formats) => seen.push(formats),
    });
    const last = () => [...(seen.at(-1) ?? [])];
    setCaret(v, 5);
    expect(last()).toEqual(["strong"]);
    setCaret(v, 1);
    expect(last()).toEqual([]);
    keys(v, "Mod-b");
    expect(last()).toEqual(["strong"]);
  });

  it("reports what every visible character of a selection has", () => {
    const seen: ReadonlySet<string>[] = [];
    const v = markView("**a *b* c**", {
      onFormatState: (formats) => seen.push(formats),
    });
    select(v, 2, 10);
    expect([...(seen.at(-1) ?? [])]).toEqual(["strong"]);
    select(v, 5, 6);
    expect([...(seen.at(-1) ?? [])].sort()).toEqual(["em", "strong"]);
  });
});
