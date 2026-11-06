/**
 * A press on the plain page right after a reparse that stopped short.
 *
 * A document change parses for 20 ms of `Date.now()` and keeps what it
 * finished: with the rest of the tree reused, that stops just below the edit.
 * A reference link's definition usually stands further down, so a press that
 * parsed only to its own line read the link as plain text, and the marks a
 * whole tree hides as letters to delete. The clock is stubbed, so this holds
 * at any load: while `starved`, each look finds a second gone and a parse
 * takes one step and stops (@codemirror/language 6.13.1, D26).
 */

import { syntaxTree } from "@codemirror/language";
import { StateEffect } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import {
  bytes,
  cleanupMarkViews,
  keys,
  markView,
  setCaret,
  typeText,
  undoOnce,
} from "../../test/markKit";
import { parsedView } from "../../test/parse";
import {
  PAGE_LOADING,
  PARSE_BUDGET,
  deadlineIn,
  parsedTo,
  viewParsedTo,
} from "./parse";

const clock = { starved: false };

beforeEach(() => {
  const now = Date.now.bind(Date);
  let ahead = 0;
  vi.spyOn(Date, "now").mockImplementation(() => {
    if (clock.starved) ahead += 1_000;
    return now() + ahead;
  });
});

afterEach(() => {
  clock.starved = false;
  vi.restoreAllMocks();
  cleanupMarkViews();
});

const PARAGRAPH = [
  "An autolink <https://example.com/path> stands here.",
  "A reference [link][ref], a collapsed [ref][] and a shortcut [ref].",
].join("\n");
const TEXT = `# Title\n\n${PARAGRAPH}\n\n[ref]: https://example.com/ref "Title"\n\nThe end.\n`;

function heard(view: EditorView): string[] {
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

describe.each([
  { mode: "hidden" as const, room: false },
  { mode: "show" as const, room: false },
  { mode: "hidden" as const, room: true },
  { mode: "show" as const, room: true },
])("a key after a short reparse, $mode, room $room", (options) => {
  /** One key at `at`, on a tree a cut undo left short of the definition, or not. */
  function press(key: string, at: number, cut: boolean) {
    const view = parsedView(markView(TEXT, options));
    const said = heard(view);
    setCaret(view, at);
    typeText(view, "Z");
    clock.starved = cut;
    undoOnce(view);
    clock.starved = false;
    const reach = syntaxTree(view.state).length;
    setCaret(view, at);
    keys(view, key);
    const result = { text: bytes(view.state), said, reach };
    cleanupMarkViews();
    return result;
  }

  it(
    "writes what it writes on a whole tree, around the two links",
    { timeout: 120_000 },
    () => {
      const definition = TEXT.indexOf("[ref]:");
      const around = (word: string, length: number) =>
        Array.from({ length }, (_, i) => TEXT.indexOf(word) + i);
      const places = [...around("<https", 26), ...around("[link][ref]", 12)];
      for (const key of ["Delete", "Backspace"]) {
        for (const at of places) {
          const whole = press(key, at, false);
          const short = press(key, at, true);
          expect(short.reach).toBeLessThan(definition);
          expect({ at, key, ...short, reach: 0 }).toEqual({
            at,
            key,
            ...whole,
            reach: 0,
          });
        }
      }
    },
  );
});

describe.each([
  { mode: "hidden" as const, room: false },
  { mode: "show" as const, room: false },
  { mode: "hidden" as const, room: true },
  { mode: "show" as const, room: true },
])("a line command after a short reparse, $mode, room $room", (options) => {
  /** A list below the edit, the caret at the end of `- [ ] two`. */
  function list() {
    const text = "# Title\n\nA first paragraph.\n\n- one\n- [ ] two\n";
    const view = parsedView(markView(text, options));
    const at = text.indexOf("two") + 3;
    setCaret(view, at);
    clock.starved = true;
    view.dispatch({ changes: { from: 9, insert: "Z" } });
    view.dispatch({ changes: { from: 9, to: 10 } });
    clock.starved = false;
    expect(syntaxTree(view.state).length).toBeLessThan(at);
    return { view, text, at };
  }

  it("Backspace does not join a code line to its hidden fence", () => {
    const text = "# Title\n\nA first paragraph.\n\n```sh\necho one\n```\n";
    const whole = parsedView(markView(text, options));
    setCaret(whole, text.indexOf("echo"));
    keys(whole, "Backspace");
    const wants = bytes(whole.state);
    const view = parsedView(markView(text, options));
    setCaret(view, text.indexOf("echo"));
    clock.starved = true;
    view.dispatch({ changes: { from: 9, insert: "Z" } });
    view.dispatch({ changes: { from: 9, to: 10 } });
    clock.starved = false;
    expect(syntaxTree(view.state).length).toBeLessThan(text.indexOf("echo"));
    keys(view, "Backspace");
    expect(bytes(view.state)).toBe(wants);
  });

  it("Enter continues the list", () => {
    const { view, text } = list();
    keys(view, "Enter");
    expect(bytes(view.state)).toBe(`${text.slice(0, -1)}\n- [ ] \n`);
  });

  it("Mod-Enter flips the task", () => {
    const { view, text } = list();
    keys(view, "Mod-Enter");
    expect(bytes(view.state)).toBe(text.replace("- [ ] two", "- [x] two"));
  });
});

describe("a press on a page that is parsed already", () => {
  it("looks at no clock, dispatches nothing and hands back the same state", () => {
    const view = parsedView(markView(TEXT, { mode: "hidden" }));
    const end = view.state.doc.length;
    const { state } = view;
    const looks = vi.spyOn(Date, "now");
    looks.mockClear();
    const dispatch = vi.spyOn(view, "dispatch");
    expect(viewParsedTo(view, end, deadlineIn(PARSE_BUDGET.key))).toBe(true);
    expect(parsedTo(state, end, deadlineIn(PARSE_BUDGET.command))).toBe(state);
    expect(looks).not.toHaveBeenCalled();
    expect(dispatch).not.toHaveBeenCalled();
  });
});

describe("typing on a short tree", () => {
  it("takes no parse time past the lines it reads, as a letter must not wait", () => {
    expect(deadlineIn(PARSE_BUDGET.typing).rest).toBe(0);
    expect(deadlineIn(PARSE_BUDGET.key).rest).toBeGreaterThan(0);
  });
});

describe("a press whose own parse runs out", () => {
  it("writes nothing on a long page and says the page is loading", () => {
    const above = Array.from(
      { length: 4000 },
      (_, i) => `Paragraph ${String(i)} with **bold** text.`,
    ).join("\n\n");
    const text = `${above}\n\nThe last line.\n`;
    // In its first parse: a cut reparse is too near the line it edited for a
    // starved press to fall short, and the free steps would reach it.
    const view = markView(text, { mode: "hidden", parsed: false });
    const said = heard(view);
    const at = text.indexOf("The last line.");
    setCaret(view, at + 2);
    clock.starved = true;
    keys(view, "Delete");
    clock.starved = false;
    expect(syntaxTree(view.state).length).toBeLessThan(at);
    expect(bytes(view.state)).toBe(text);
    expect(said).toEqual([PAGE_LOADING]);
  });
});
