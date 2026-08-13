/**
 * The parse budget, made deterministic in both directions.
 *
 * `@codemirror/language` gives the first parse of a new state 20 milliseconds
 * of WALL CLOCK and truncates the tree when they run out, so on a loaded
 * machine a test that reads the tree on the same tick reads whatever fitted.
 * These tests do not wait for anything: they move the clock, which starves the
 * budget on its first check without touching the CPU, and then assert on both
 * sides of `parsedState` - the truncated tree that produced the intermittent
 * failures in this directory, and the finished one every other editor test now
 * asserts against.
 *
 * If `parsedState` is ever quietly weakened into a longer timeout, the starved
 * half of each pair below is what fails.
 */

import { syntaxTree } from "@codemirror/language";
import { EditorSelection, EditorState } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { afterEach, describe, expect, it, vi } from "vitest";

import { parsedState } from "../test/parse";
import { fenceMono } from "./fenceMono";
import { livePreview } from "./preview";
import { baseExtensions } from "./setup";
import { tableContextAt } from "./tableVerbs";

const DOC =
  "---\ntitle: Alpha\n---\n\n# Heading\n\nSome *emphasis* here.\n\n- [ ] a task\n";
const TABLE = "Before\n\n| a | b |\n| --- | --- |\n| 1 | 2 |\n\nAfter\n";

afterEach(() => {
  vi.restoreAllMocks();
});

/**
 * Run `body` against a clock that jumps a whole budget's worth on every read,
 * so every wall-clock deadline inside the parser has already expired the first
 * time it is checked. Nothing is slowed down; the parse simply gets one chunk
 * of work and is then cut off, exactly as it is when the scheduler takes the
 * process away for 20 milliseconds.
 */
function starved<T>(body: () => T): T {
  let now = Date.now();
  vi.spyOn(Date, "now").mockImplementation(() => {
    now += 50;
    return now;
  });
  return body();
}

function state(doc: string, extensions: unknown[]): EditorState {
  return EditorState.create({
    doc,
    selection: EditorSelection.cursor(doc.length),
    extensions: extensions as never[],
  });
}

describe("a starved parse", () => {
  it("truncates the tree, and parsedState finishes it", () => {
    const short = starved(() => state(DOC, [baseExtensions(false)]));
    expect(syntaxTree(short).length).toBeLessThan(short.doc.length);
    // Same starved clock, so this is the budget being removed rather than the
    // machine being quicker.
    const whole = starved(() => parsedState(short));
    expect(syntaxTree(whole).length).toBe(whole.doc.length);
  });

  it("leaves the live-preview marks unfolded, and parsedState folds them", () => {
    const short = starved(() =>
      state(DOC, [baseExtensions(false), livePreview()]),
    );
    const raw = new EditorView({ state: short, parent: document.body });
    // The failure this file exists for, on demand: the heading mark and the
    // emphasis stars are still on screen because the tree never reached them.
    expect(raw.contentDOM.textContent).toContain("# Heading");
    expect(raw.contentDOM.textContent).toContain("*emphasis*");
    raw.destroy();

    const folded = new EditorView({
      state: starved(() => parsedState(short)),
      parent: document.body,
    });
    expect(folded.contentDOM.textContent).not.toContain("# Heading");
    expect(folded.contentDOM.textContent).not.toContain("*emphasis*");
    expect(folded.contentDOM.textContent).toContain("Heading");
    folded.destroy();
  });

  it("leaves a fence in the prose face until the parse reaches it", () => {
    // The state-field half of the same story, and the reason `parseAdvanced`
    // exists: `fenceMono` computes its lines when the state is CREATED, so a
    // tree that grows afterwards has to be a reason to recompute them. Without
    // that, this is what a reader of a long engram sees below the cut-off.
    const doc = "Prose line.\n\n```js\nconst answer = 42;\n```\n";
    const short = starved(() =>
      state(doc, [baseExtensions(false), fenceMono()]),
    );
    const raw = new EditorView({ state: short, parent: document.body });
    expect(raw.dom.querySelector(".cm-fence-mono")).toBeNull();
    raw.destroy();

    const whole = new EditorView({
      state: starved(() => parsedState(short)),
      parent: document.body,
    });
    expect(
      [...whole.dom.querySelectorAll(".cm-fence-mono")].map(
        (el) => el.textContent,
      ),
    ).toEqual(["```js", "const answer = 42;", "```"]);
    whole.destroy();
  });

  it("hides the table from tableContextAt, and parsedState reveals it", () => {
    const at = TABLE.indexOf("| 1") + 2;
    const short = starved(() =>
      EditorState.create({
        doc: TABLE,
        selection: EditorSelection.cursor(at),
        extensions: baseExtensions(false),
      }),
    );
    expect(tableContextAt(short)).toBeNull();
    expect(tableContextAt(starved(() => parsedState(short)))?.row).toBe(2);
  });
});
