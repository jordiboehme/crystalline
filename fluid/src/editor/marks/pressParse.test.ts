/**
 * What a press judges after a reparse that stopped short, beside the keys in
 * `shortReparse.test.ts`: a replaced selection, a new link, a picture, a rich
 * paste and the `/` menu. Each of them read "nothing here" off a short tree
 * as "allowed". The clock is stubbed, so this holds at any load: while
 * starved, each look finds a second gone and a parse takes one step and stops
 * (@codemirror/language 6.13.1, D26).
 */

import { CompletionContext } from "@codemirror/autocomplete";
import { syntaxTree } from "@codemirror/language";
import type { EditorState } from "@codemirror/state";
import { EditorSelection, StateEffect } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import {
  bytes,
  cleanupMarkViews,
  keys,
  markView,
  setCaret,
  typeText,
} from "../../test/markKit";
import { parsedView } from "../../test/parse";
import type { ElementEdit } from "../elementEdit";
import { CANNOT_LINK, commitElementEdit } from "../elementEdit";
import { newLinkChanges } from "../elementVerbs";
import { imagePreviews } from "../imagePreviews";
import { slashCompletions } from "../slashMenu";
import { PICTURE_NOT_FORMATTED } from "./formatting";
import { PARSE_BUDGET, deadlineIn, parsedTo } from "./parse";

/** `starved`: every look finds a second gone. `stall`: so many looks do. */
const clock = { starved: false, stall: 0, pauseAt: 0, seen: 0 };

beforeEach(() => {
  // Time moves only when a test moves it. Real time under the stubbed steps
  // would let a busy machine run out a budget the test means to be reached.
  const start = Date.now();
  let ahead = 0;
  vi.spyOn(Date, "now").mockImplementation(() => {
    if (clock.starved) ahead += 1_000;
    clock.seen += 1;
    if (clock.seen === clock.pauseAt) ahead += 25;
    if (clock.stall > 0) {
      clock.stall -= 1;
      ahead += 1_000;
    }
    return start + ahead;
  });
});

afterEach(() => {
  clock.starved = false;
  clock.stall = 0;
  clock.pauseAt = 0;
  vi.restoreAllMocks();
  cleanupMarkViews();
});

const MODES = [
  { mode: "hidden" as const, room: false },
  { mode: "show" as const, room: false },
  { mode: "hidden" as const, room: true },
  { mode: "show" as const, room: true },
];

const page = (target: string) =>
  `# Title\n\nA first paragraph.\n\n${target}\n\nThe end.\n`;

/** An edit above `pos` whose reparse stops short of it. */
function cut(view: EditorView, pos: number): void {
  clock.starved = true;
  view.dispatch({ changes: { from: 9, insert: "Z" } });
  view.dispatch({ changes: { from: 9, to: 10 } });
  clock.starved = false;
  expect(syntaxTree(view.state).length).toBeLessThan(pos);
}

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

const newLink = (state: EditorState, edit: ElementEdit) =>
  edit.kind === "new-link"
    ? newLinkChanges(state, edit.range, { text: "", address: "https://x" })
    : null;

/** A cut as the browser sends it: a real event on the view's content. */
function cutEvent(view: EditorView): void {
  const event = new Event("cut", { bubbles: true, cancelable: true });
  Object.defineProperty(event, "clipboardData", {
    value: { setData: () => {}, getData: () => "", clearData: () => {} },
  });
  view.focus();
  view.contentDOM.dispatchEvent(event);
}

describe.each(MODES)("a selection replaced, $mode, room $room", (options) => {
  const text = page("a **bold** b `code` e");

  it("cut with a real event reads as on a whole tree", () => {
    const cut = (view: EditorView) => {
      cutEvent(view);
    };
    const whole = replaced(false, cut);
    expect(whole).not.toBe(text);
    expect(replaced(true, cut)).toBe(whole);
  });

  it.each(["delete.selection", "delete.cut", "delete.picture", "move.drop"])(
    "%s reads as on a whole tree",
    (userEvent) => {
      const remove = (view: EditorView) => {
        const { from, to } = view.state.selection.main;
        view.dispatch({ changes: { from, to }, userEvent });
      };
      expect(replaced(true, remove)).toBe(replaced(false, remove));
    },
  );

  it.each(["input.drop", "input.complete"])(
    "%s reads as on a whole tree",
    (userEvent) => {
      const put = (view: EditorView) => {
        const { from, to } = view.state.selection.main;
        view.dispatch({ changes: { from, to, insert: "Y" }, userEvent });
      };
      expect(replaced(true, put)).toBe(replaced(false, put));
    },
  );

  function replaced(short: boolean, write: (view: EditorView) => void) {
    const view = parsedView(markView(text, options));
    const from = text.indexOf("a **bold");
    const to = text.indexOf("old** b");
    view.dispatch({
      selection: EditorSelection.range(from, to),
      userEvent: "select.pointer",
    });
    if (short) cut(view, from);
    write(view);
    const result = bytes(view.state);
    cleanupMarkViews();
    return result;
  }

  it("typed over keeps the pair it does not empty, as on a whole tree", () => {
    const type = (view: EditorView) => {
      typeText(view, "Y");
    };
    expect(replaced(true, type)).toBe(replaced(false, type));
  });

  it("pasted over keeps it too", () => {
    const paste = (view: EditorView) => {
      const { from, to } = view.state.selection.main;
      view.dispatch({
        changes: { from, to, insert: "Y" },
        userEvent: "input.paste",
      });
    };
    expect(replaced(true, paste)).toBe(replaced(false, paste));
  });
});

describe.each(MODES)("a new link, $mode, room $room", (options) => {
  function linked(target: string, word: string, length: number) {
    const text = page(target);
    const heard: { edit: ElementEdit | null }[] = [];
    const view = parsedView(
      markView(text, {
        ...options,
        onElementEdit: (edit) => {
          heard.push({ edit });
        },
      }),
    );
    const from = text.indexOf(word);
    view.dispatch({
      selection: EditorSelection.range(from, from + length),
      userEvent: "select.pointer",
    });
    cut(view, from);
    const said = hear(view);
    keys(view, "Mod-k");
    commitElementEdit(view, newLink);
    return { text, view, said };
  }

  it.each([
    ["across a pair of marks", "Some **bold word** here.", "word** h", 8],
    ["into a code span", "Some `code word` here.", "word", 4],
    ["into a fence", "```sh\necho word\n```", "word", 4],
    ["into a link", "See [the word](https://a.b) now.", "word", 4],
  ])("is refused %s", (_name, target, word, length) => {
    const { text, view, said } = linked(target, word, length);
    expect(bytes(view.state)).toBe(text);
    expect(said).toEqual([CANNOT_LINK]);
  });

  it("is written on plain words", () => {
    const { text, view, said } = linked("Some plain words.", "plain", 5);
    expect(bytes(view.state)).toBe(text.replace("plain", "[plain](https://x)"));
    expect(said).toEqual([]);
  });
});

describe.each(MODES)("Mod-b on a picture, $mode, room $room", (options) => {
  it("says a picture has no text to format", () => {
    const text = page("Some ![alt](assets/a.png) text.");
    const view = parsedView(
      markView(text, { ...options, images: imagePreviews("eng") }),
    );
    const from = text.indexOf("![alt]");
    view.dispatch({
      selection: EditorSelection.range(
        from,
        from + "![alt](assets/a.png)".length,
      ),
      userEvent: "select.pointer",
    });
    cut(view, from);
    const said = hear(view);
    keys(view, "Mod-b");
    expect(bytes(view.state)).toBe(text);
    expect(said).toEqual([PICTURE_NOT_FORMATTED]);
  });
});

describe("Mod-b on a parsed page", () => {
  it("looks at no clock for its parse and dispatches no empty update", () => {
    const text = page("Some plain words.");
    const view = parsedView(markView(text, { mode: "hidden" }));
    const from = text.indexOf("plain");
    view.dispatch({
      selection: EditorSelection.range(from, from + 5),
      userEvent: "select.pointer",
    });
    const dispatch = vi.spyOn(view, "dispatch");
    keys(view, "Mod-b");
    expect(bytes(view.state)).toBe(text.replace("plain", "**plain**"));
    // The write itself, and nothing before it.
    expect(dispatch).toHaveBeenCalledTimes(1);
  });
});

describe("a rich paste", () => {
  function pasteInto(view: EditorView, html: string, plain: string) {
    const event = new Event("paste", { bubbles: true, cancelable: true });
    Object.defineProperty(event, "clipboardData", {
      value: {
        getData: (type: string) =>
          type === "text/html" ? html : type === "text/plain" ? plain : "",
        types: ["text/html", "text/plain"],
        files: [],
        items: [],
      },
    });
    view.contentDOM.dispatchEvent(event);
  }

  it("into a fence goes in as plain text on a short tree", async () => {
    const text = page("```sh\necho one\n```");
    const view = parsedView(markView(text, { mode: "hidden" }));
    const at = text.indexOf("echo one") + 8;
    setCaret(view, at);
    cut(view, at);
    pasteInto(view, "<p><b>bold</b> word</p>", "bold word");
    const want = text.replace("echo one", "echo onebold word");
    await vi.waitFor(() => {
      expect(bytes(view.state)).not.toBe(text);
    });
    expect(bytes(view.state)).toBe(want);
  });
});

describe("a rich paste past its budget", () => {
  it("into an unread fence goes in as plain text", () => {
    const above = Array.from(
      { length: 4000 },
      (_, i) => `Paragraph ${String(i)} with **bold** text.`,
    ).join("\n\n");
    const text = `${above}\n\n\`\`\`sh\necho one\n\`\`\`\n`;
    const view = markView(text, { mode: "hidden", parsed: false });
    const at = text.indexOf("echo one") + 8;
    setCaret(view, at);
    clock.starved = true;
    const event = new Event("paste", { bubbles: true, cancelable: true });
    Object.defineProperty(event, "clipboardData", {
      value: {
        getData: (type: string) =>
          type === "text/html"
            ? "<p><b>bold</b> word</p>"
            : type === "text/plain"
              ? "bold word"
              : "",
        types: ["text/html", "text/plain"],
        files: [],
        items: [],
      },
    });
    view.contentDOM.dispatchEvent(event);
    clock.starved = false;
    expect(bytes(view.state)).toBe(
      text.replace("echo one", "echo onebold word"),
    );
  });
});

describe("the / menu", () => {
  it.each([
    ["a fence", "```sh\n/\n```"],
    ["an HTML block", "<div>\n/\n</div>"],
    ["a lazy quote line", "> a quote\n/"],
  ])("does not open in %s on a short tree", (_name, target) => {
    const text = page(target);
    const view = parsedView(markView(text, { mode: "hidden" }));
    const at = text.indexOf("\n/") + 2;
    setCaret(view, at);
    cut(view, at);
    const found = slashCompletions(
      new CompletionContext(view.state, at, false),
    );
    expect(found).toBeNull();
  });

  it("opens in prose on a short tree", () => {
    const text = page("/");
    const view = parsedView(markView(text, { mode: "hidden" }));
    const at = text.indexOf("/") + 1;
    setCaret(view, at);
    cut(view, at);
    expect(
      slashCompletions(new CompletionContext(view.state, at, false)),
    ).not.toBeNull();
  });
});

describe("the rest of the page", () => {
  const filler = Array.from(
    { length: 300 },
    (_, i) => `Filler ${String(i)} with **bold** text.`,
  ).join("\n\n");

  /** A state cut short at `line`, with the filler below it. */
  function short(line: string): EditorState {
    const text = page(`${line}\nA second line.\n\n${filler}`);
    const view = parsedView(markView(text, { mode: "hidden" }));
    cut(view, text.indexOf(line));
    return view.state;
  }

  const lineEnd = (state: EditorState, word: string) =>
    state.doc.lineAt(state.doc.toString().indexOf(word)).to;

  it("is not read for a line that nothing below it can change", () => {
    const state = short("Plain words only.");
    const parsed = parsedTo(
      state,
      lineEnd(state, "Plain words"),
      deadlineIn(PARSE_BUDGET.key),
    );
    expect(parsed).not.toBeNull();
    expect(syntaxTree(parsed as EditorState).length).toBeLessThan(
      state.doc.length,
    );
  });

  it.each([
    ["on the line", "A [ref] here."],
    ["on the paragraph's next line", "Plain words.\nA [ref] here."],
  ])("is read for a reference %s", (_name, line) => {
    const state = short(line);
    const parsed = parsedTo(state, lineEnd(state, line.split("\n")[0] ?? ""), {
      ...deadlineIn(PARSE_BUDGET.key),
    });
    expect(syntaxTree(parsed as EditorState).length).toBe(state.doc.length);
  });
});

describe("a fence whose language is still loading", () => {
  // The parse covers every character but leaves no fragment over the fence, so
  // the parse context never calls itself done, and each ask would parse again.
  const text = page("```python\nprint(1)\n```");

  it("costs a second ask no clock and hands back the same state", () => {
    const first = parsedTo(
      markView(text, { mode: "hidden" }).state,
      text.length,
      deadlineIn(PARSE_BUDGET.key),
    );
    if (!first) throw new Error("the parse did not reach");
    clock.seen = 0;
    const again = parsedTo(first, text.length, deadlineIn(PARSE_BUDGET.key));
    expect(again).toBe(first);
    expect(clock.seen).toBe(0);
  });
});

describe("typing below a fence whose language is loading", () => {
  it("asks no further parse of a line the edit's own parse read", () => {
    const text = page("```python\nprint(1)\n```");
    const view = parsedView(markView(text, { mode: "hidden" }));
    const at = text.indexOf("The end.");
    view.dispatch({ changes: { from: at, insert: "Z" } });
    const { state } = view;
    clock.seen = 0;
    const parsed = parsedTo(
      state,
      state.doc.lineAt(at).to,
      deadlineIn(PARSE_BUDGET.typing),
    );
    expect(parsed).toBe(state);
    expect(clock.seen).toBe(0);
  });
});

describe("a trial whose reparse is cut short", () => {
  // A tree cut inside a quote is as long as the blocks it closed, with the
  // next line unread. One pause of the page at each look at the clock in turn:
  // no look may change what Enter writes.
  it.each([
    { text: "> - **a**", want: "> - **a**\n> - ", room: false },
    { text: "> - **a**", want: "> - **a**\n> - ", room: true },
    { text: "- **a**", want: "- **a**\n- ", room: false },
  ])("Enter writes the same, $text, room $room", ({ text, want, room }) => {
    for (let look = 1, looks = 30; look <= looks; look += 1) {
      const view = markView(text, { mode: "hidden", room });
      setCaret(view, text.length);
      clock.seen = 0;
      clock.pauseAt = look;
      keys(view, "Enter");
      clock.pauseAt = 0;
      looks = Math.min(looks, clock.seen + 1);
      expect({ look, bytes: bytes(view.state) }).toEqual({ look, bytes: want });
      cleanupMarkViews();
    }
  });
});

describe("a key whose own parse ran out acts on the page as drawn (ruling on F5)", () => {
  // On a short tree the line is drawn as text, so the person sees the fence
  // or the marker as letters, and the key does what they see. Refusing would
  // make a long page unusable during its first parse.
  const above = Array.from(
    { length: 4000 },
    (_, i) => `Paragraph ${String(i)} with **bold** text.`,
  ).join("\n\n");

  /** A long page in its first parse, the caret far below what it has read. */
  function far(target: string, word: string, off: number) {
    const text = `${above}\n\n${target}\n`;
    const view = markView(text, { mode: "hidden", parsed: false });
    const at = text.indexOf(word) + off;
    setCaret(view, at);
    expect(syntaxTree(view.state).length).toBeLessThan(at);
    return { text, view };
  }

  it("Enter is a plain line break", () => {
    const { text, view } = far("- one\n- [ ] two", "two", 3);
    clock.starved = true;
    keys(view, "Enter");
    clock.starved = false;
    expect(bytes(view.state)).toBe(`${text.slice(0, -1)}\n\n`);
  });

  it("Mod-Enter falls through to the default line break", () => {
    const { text, view } = far("- one\n- [ ] two", "two", 3);
    clock.starved = true;
    keys(view, "Mod-Enter");
    clock.starved = false;
    expect(bytes(view.state)).toBe(`${text.slice(0, -1)}\n\n`);
  });

  it("Backspace joins a code line to the fence the page draws as text", () => {
    const { text, view } = far("```js\nlet x = 1;\n```", "let x", 0);
    // The key's own parse runs out; the deletion check's reaches.
    clock.stall = 50;
    keys(view, "Backspace");
    clock.stall = 0;
    expect(bytes(view.state)).toBe(text.replace("```js\nlet", "```jslet"));
  });
});
