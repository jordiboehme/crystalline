/**
 * One parse bound per press (D35), on a jumping clock. The parse here is
 * timed by @codemirror/language 6.13.1 reading `Date.now` once per parse
 * step, so a stubbed clock that moves a fixed amount per look makes a parse
 * "take" a known time, at any load:
 *
 * 1. N is the number of looks a full parse of the fixture makes (about
 *    190 kB, the target line at its end, its first parse cut short).
 * 2. At 100/N ms a look the parse takes about 100 ms: within the bounds of a
 *    command (500 ms) and a key (150 ms), and past the old 50 ms one.
 * 3. At 1000/N ms a look it takes about a second: past every bound, so the
 *    press writes nothing and says the page is still loading.
 */

import { ensureSyntaxTree, syntaxTree } from "@codemirror/language";
import { EditorSelection, EditorState, StateEffect } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { bytes, cleanupMarkViews, keys, markView } from "../../test/markKit";
import { documentLanguage } from "../setup";
import { PAGE_LOADING } from "./parse";

const TARGET = "end words here";
const TEXT = `${Array.from(
  { length: 5000 },
  (_, i) => `Paragraph ${String(i)} with **bold** text.`,
).join("\n\n")}\n\n${TARGET}`;

/** How far the clock moves at each look, in ms; huge stops any parse. */
const clock = { step: 1e6, looks: 0 };
const STOPPED = 1e6;

beforeEach(() => {
  const base = Date.now();
  let ahead = 0;
  clock.step = STOPPED;
  vi.spyOn(Date, "now").mockImplementation(() => {
    clock.looks += 1;
    ahead += clock.step;
    return base + ahead;
  });
});

afterEach(() => {
  cleanupMarkViews();
  vi.restoreAllMocks();
});

/** The looks at the clock a full parse of `TEXT` makes, from a cut-short start. */
function looksOfFullParse(): number {
  clock.step = STOPPED;
  const state = EditorState.create({ doc: TEXT, extensions: documentLanguage });
  expect(syntaxTree(state).length).toBeLessThan(TEXT.length);
  clock.step = 0;
  clock.looks = 0;
  ensureSyntaxTree(state, TEXT.length, 1e9);
  const looks = clock.looks;
  clock.step = STOPPED;
  return looks;
}

/** A view on `TEXT` whose first parse stopped short, hearing what it says. */
function shortView(): { view: EditorView; said: string[] } {
  clock.step = STOPPED;
  const view = markView(TEXT, { parsed: false });
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
  expect(syntaxTree(view.state).length).toBeLessThan(TEXT.length / 2);
  return { view, said };
}

const end = TEXT.length;
const start = end - TARGET.length;

/** Presses `key` with the clock moving `perLook` ms at each look. */
function press(view: EditorView, key: string, perLook: number): void {
  clock.step = perLook;
  keys(view, key);
  clock.step = STOPPED;
}

describe("one parse bound per press (D35)", () => {
  it("parses a long page for Bold, and says it is loading past the bound", () => {
    const n = looksOfFullParse();
    const quick = shortView();
    quick.view.dispatch({ selection: EditorSelection.range(start, end) });
    press(quick.view, "Mod-b", 100 / n);
    expect(bytes(quick.view.state).endsWith(`**${TARGET}**`)).toBe(true);
    expect(quick.said).not.toContain(PAGE_LOADING);

    const slow = shortView();
    slow.view.dispatch({ selection: EditorSelection.range(start, end) });
    press(slow.view, "Mod-b", 1000 / n);
    expect(bytes(slow.view.state)).toBe(TEXT);
    expect(slow.said).toContain(PAGE_LOADING);
  });

  it("parses a long page for Backspace, and says it is loading past the bound", () => {
    const n = looksOfFullParse();
    const quick = shortView();
    quick.view.dispatch({ selection: EditorSelection.cursor(end) });
    press(quick.view, "Backspace", 100 / n);
    expect(bytes(quick.view.state)).toBe(TEXT.slice(0, -1));
    expect(quick.said).not.toContain(PAGE_LOADING);

    const slow = shortView();
    slow.view.dispatch({ selection: EditorSelection.cursor(end) });
    press(slow.view, "Backspace", 1000 / n);
    expect(bytes(slow.view.state)).toBe(TEXT);
    expect(slow.said).toContain(PAGE_LOADING);
  });

  it("parses a long page for a selection delete, and says it is loading past the bound", () => {
    const n = looksOfFullParse();
    const quick = shortView();
    quick.view.dispatch({ selection: EditorSelection.range(start + 4, end) });
    press(quick.view, "Backspace", 100 / n);
    expect(bytes(quick.view.state)).toBe(TEXT.slice(0, start + 4));
    expect(quick.said).not.toContain(PAGE_LOADING);

    const slow = shortView();
    slow.view.dispatch({ selection: EditorSelection.range(start + 4, end) });
    press(slow.view, "Backspace", 1000 / n);
    expect(bytes(slow.view.state)).toBe(TEXT);
    expect(slow.said).toContain(PAGE_LOADING);
  });
});

describe("a slow press on a page that is parsed already (D52)", () => {
  it("is never refused for time spent outside parsing", () => {
    clock.step = 0;
    const view = markView(TEXT);
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
    expect(syntaxTree(view.state).length).toBe(TEXT.length);
    // After "bold" on a line in the middle of the page.
    const line = view.state.doc.line(5001);
    const at = line.from + line.text.indexOf("bold") + 4;
    view.dispatch({ selection: EditorSelection.cursor(at) });
    // Every look at the clock takes 30 ms: the press's scans and reads alone
    // would use up a 150 ms bound many times over.
    press(view, "Backspace", 30);
    expect(said).not.toContain(PAGE_LOADING);
    expect(view.state.doc.line(5001).text).toBe(
      line.text.replace("**bold**", "**bol**"),
    );
  });
});

describe("a key on a line past a short parse (D35)", () => {
  it("reads the line's marks before it deletes, so a hidden mark is never taken as a letter", () => {
    const text = `${TEXT}\n\na [lk](u)`;
    clock.step = STOPPED;
    const view = markView(text, { parsed: false });
    expect(syntaxTree(view.state).length).toBeLessThan(text.length / 2);
    view.dispatch({ selection: EditorSelection.cursor(text.length) });
    // A frozen clock: every parse may take as long as it needs.
    clock.step = 0;
    keys(view, "Backspace");
    clock.step = STOPPED;
    expect(bytes(view.state).endsWith("a [l](u)")).toBe(true);
  });
});
