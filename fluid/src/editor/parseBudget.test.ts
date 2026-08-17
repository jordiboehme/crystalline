/**
 * The parse budget, made deterministic in both directions.
 *
 * `@codemirror/language` gives the first parse of a new state 20 milliseconds
 * of WALL CLOCK and truncates the tree when they run out, so on a loaded
 * machine a test that reads the tree on the same tick reads whatever fitted.
 * These tests do not wait for anything: they move the clock, which starves the
 * budget on its first check without touching the CPU, and then assert on both
 * sides of the two helpers.
 *
 * Two halves, because the defect has two.
 *
 * The first describe is about what a TEST reads: a truncated tree is what
 * produced the intermittent failures in this directory, and `parsedState` is
 * what every other editor test now builds its state through.
 *
 * The second is about what a READER sees, and it is the app's own path rather
 * than a test's. A buffer is mounted while the tree is still short, the parse
 * catches up afterwards, and the advance arrives as a transaction carrying no
 * document change, no selection change and no viewport change - which is
 * exactly the transaction every layer here used to ignore. There is one test
 * per tree-driven layer on purpose: a single shared green run proves nothing
 * about the four that are not in it, and reverting any one `parseAdvanced`
 * call must turn exactly one of them red.
 *
 * If either helper is ever quietly weakened into a longer timeout, the starved
 * half of each pair below is what fails.
 */

import { syntaxTree } from "@codemirror/language";
import { EditorSelection, EditorState } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { afterEach, describe, expect, it, vi } from "vitest";

import { parsedState, parsedView } from "../test/parse";
import { crystallineLines } from "./crystallineLines";
import { fenceMono } from "./fenceMono";
import { fencePreviews } from "./fencePreviews";
import { livePreview } from "./preview";
import { baseExtensions } from "./setup";
import { tableContextAt } from "./tableVerbs";
import { wikilinkChips, wikilinkResolverFacet } from "./wikilinkChips";

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

/**
 * A buffer mounted against a short tree, with the parse catching up under it.
 *
 * `mounted` is the app at the moment an engram opens on a busy machine, and
 * `caughtUp` is the `parseWorker` finishing a beat later. Each test asserts
 * that the layer draws nothing (or draws the wrong thing) at the first moment
 * and is correct at the second - which is only true if that layer treats parse
 * progress as a reason to redraw.
 */
describe("a parse that catches up under a mounted view", () => {
  const views: EditorView[] = [];

  afterEach(() => {
    while (views.length > 0) {
      views.pop()?.destroy();
    }
  });

  /** Mounted over a deliberately truncated tree, exactly as the app mounts. */
  function mounted(doc: string, extensions: unknown[], at = doc.length) {
    const view = new EditorView({
      state: starved(() =>
        EditorState.create({
          doc,
          selection: EditorSelection.cursor(at),
          extensions: extensions as never[],
        }),
      ),
      parent: document.body,
    });
    views.push(view);
    // The premise of every test below: the tree really is short at mount.
    expect(syntaxTree(view.state).length).toBeLessThan(view.state.doc.length);
    return view;
  }

  /** The idle worker's advance, published the way the worker publishes it. */
  function caughtUp(view: EditorView): EditorView {
    return starved(() => parsedView(view));
  }

  it("folds the live-preview marks that mounted unfolded", () => {
    const view = mounted(DOC, [baseExtensions(false), livePreview()]);
    expect(view.contentDOM.textContent).toContain("# Heading");
    expect(view.contentDOM.textContent).toContain("*emphasis*");

    caughtUp(view);
    expect(view.contentDOM.textContent).not.toContain("# Heading");
    expect(view.contentDOM.textContent).not.toContain("*emphasis*");
    expect(view.contentDOM.textContent).toContain("Heading");
  });

  it("takes back the crystalline marks it drew inside an unclosed fence", () => {
    // The sharp end of a short tree: with no fence in the tree yet, the line
    // inside the block reads as an ordinary observation and gets marked. This
    // is the third flake this task measured, in its deterministic form.
    const doc =
      "---\nt: x\n---\n\n- [decision] outside the fence #tag\n\n```\n- [decision] inside the fence #tag\n```\n";
    const view = mounted(doc, [baseExtensions(false), crystallineLines()], 0);
    const drawn = () =>
      [...view.dom.querySelectorAll(".cm-obs-category")].map(
        (el) => el.textContent,
      );
    expect(drawn()).toEqual(["[decision]", "[decision]"]);

    caughtUp(view);
    expect(drawn()).toEqual(["[decision]"]);
  });

  it("takes back the wikilink chip it drew inside an unclosed fence", () => {
    // The brackets are found by a regex over the text, so a short tree does
    // not hide a chip - it hides the CODE BLOCK the chip should have been
    // suppressed inside, which is the wrong-thing-drawn half of the same bug.
    const doc = "See [[Beta Note]] here.\n\n```\n[[ghost]] in code\n```\n";
    const view = mounted(
      doc,
      [
        baseExtensions(false),
        wikilinkResolverFacet.of((inner) =>
          inner === "Beta Note"
            ? { kind: "resolved", href: "/d/eng/e/beta", label: "Beta Note" }
            : { kind: "unresolved" },
        ),
        wikilinkChips(),
      ],
      0,
    );
    const chips = () =>
      [...view.dom.querySelectorAll(".cm-wikilink")].map(
        (el) => el.textContent,
      );
    expect(chips()).toEqual(["Beta Note", "ghost"]);

    caughtUp(view);
    expect(chips()).toEqual(["Beta Note"]);
  });

  it("gives a code block its mono face once the fence is in the tree", () => {
    const doc = "Prose line.\n\n```js\nconst answer = 42;\n```\n";
    const view = mounted(doc, [baseExtensions(false), fenceMono()]);
    expect(view.dom.querySelector(".cm-fence-mono")).toBeNull();

    caughtUp(view);
    expect(
      [...view.dom.querySelectorAll(".cm-fence-mono")].map(
        (el) => el.textContent,
      ),
    ).toEqual(["```js", "const answer = 42;", "```"]);
  });

  it("draws the table preview under a table the short tree had not reached", () => {
    // A table rather than a mermaid fence: same state field, no async render.
    const doc = "Prose line.\n\n| a | b |\n| --- | --- |\n| 1 | 2 |\n";
    const view = mounted(doc, [baseExtensions(false), fencePreviews(false)], 0);
    expect(view.dom.querySelector(".cm-table-preview")).toBeNull();

    caughtUp(view);
    expect(
      view.dom.querySelector(".cm-table-preview")?.querySelectorAll("td"),
    ).toHaveLength(2);
  });
});
