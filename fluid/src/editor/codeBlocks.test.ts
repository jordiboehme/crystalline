/**
 * The code box: a closed fence draws as a box with a header (its language
 * and a button for more actions) where its fence lines would be, in both
 * modes of the setting, and Raw shows the fences. Nothing here writes a
 * byte: the box is a view over the source, and the caret never rests on a
 * hidden fence line.
 */

import { EditorSelection, EditorState, StateEffect } from "@codemirror/state";
import type { EditorView } from "@codemirror/view";
import { afterEach, describe, expect, it, vi } from "vitest";

import { cleanupCells, openAt, soloMain } from "../test/cellKit";
import {
  bytes,
  caret,
  cleanupMarkViews,
  keys,
  markView,
  setCaret,
  markPeers,
  setRaw,
  setShowMarks,
  typeText,
} from "../test/markKit";
import type { ElementEdit } from "./elementEdit";
import {
  commitElementEdit,
  elementEditCell,
  elementEditListener,
  requestElementEdit,
} from "./elementEdit";
import {
  codeFenceAt,
  languageChanges,
  paragraphBelowChanges,
} from "./elementVerbs";
import { cellForwarding } from "./grid/cellEditor";

afterEach(() => {
  cleanupMarkViews();
  cleanupCells();
  vi.restoreAllMocks();
});

type Mode = "hidden" | "show";
const MODES: Mode[] = ["hidden", "show"];

const DOC = "a\n\n```ts\nx\n```\n\nb";

function header(view: EditorView): HTMLElement | null {
  return view.contentDOM.querySelector(".cm-code-header");
}

function languageButton(view: EditorView): HTMLElement {
  const el = view.contentDOM.querySelector<HTMLElement>(
    'button[aria-label^="Code language"]',
  );
  if (!el) throw new Error("no language button is drawn");
  return el;
}

function moreButton(view: EditorView): HTMLElement {
  const el = view.contentDOM.querySelector<HTMLElement>(
    'button[aria-label="More code block actions"]',
  );
  if (!el) throw new Error("no actions button is drawn");
  return el;
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

function apply(
  text: string,
  write: (state: EditorState) => ReturnType<typeof languageChanges>,
): string | null {
  const view = markView(text);
  const changes = write(view.state);
  if (changes === null) return null;
  return view.state.update({ changes }).state.doc.toString();
}

function listened(
  text: string,
  options: Parameters<typeof markView>[1] = {},
): { view: EditorView; heard: (ElementEdit | null)[] } {
  const heard: (ElementEdit | null)[] = [];
  const view = markView(text, {
    ...options,
    onElementEdit: (edit) => {
      heard.push(edit);
    },
  });
  return { view, heard };
}

describe.each(MODES)("the code box (%s mode)", (mode) => {
  it("draws a header with the language in place of the fences", () => {
    const view = markView(DOC, { mode });
    setCaret(view, 0);
    expect(view.contentDOM.textContent).not.toContain("`");
    expect(header(view)).not.toBeNull();
    expect(languageButton(view).getAttribute("aria-label")).toBe(
      "Code language: TypeScript",
    );
    expect(languageButton(view).textContent).toBe("TypeScript");
    expect(moreButton(view)).not.toBeNull();
    const code = [...view.contentDOM.querySelectorAll(".cm-line")].find(
      (line) => line.textContent === "x",
    );
    expect(code?.classList.contains("cm-code-box")).toBe(true);
  });

  it.each([
    ["no word", "a\n\n```\nx\n```"],
    ["a word no language has", "a\n\n```foo\nx\n```"],
    ["a word after a space", "a\n\n``` ts\nx\n```"],
  ])("names Plain text for %s", (_name, text) => {
    const view = markView(text, { mode });
    setCaret(view, 0);
    expect(languageButton(view).getAttribute("aria-label")).toBe(
      "Code language: Plain text",
    );
  });

  it("draws a tilde fence as a box too", () => {
    const view = markView("a\n\n~~~rust\nx\n~~~", { mode });
    setCaret(view, 0);
    expect(view.contentDOM.textContent).not.toContain("~");
    expect(languageButton(view).getAttribute("aria-label")).toBe(
      "Code language: Rust",
    );
  });

  it("writes nothing while it draws: the bytes stay through moves, flips and Raw", () => {
    const view = markView(DOC, { mode });
    for (const pos of [0, 9, 10, 16, 2]) {
      setCaret(view, pos);
    }
    keys(view, "ArrowDown", "ArrowDown", "ArrowUp", "ArrowRight");
    setShowMarks(view, mode === "hidden");
    setRaw(view, true);
    setRaw(view, false);
    expect(bytes(view.state)).toBe(DOC);
  });

  it.each([
    ["a mermaid fence", "a\n\n```mermaid\ngraph TD\n```"],
    ["a math fence", "a\n\n```math\nx^2\n```"],
    ["an unclosed fence", "a\n\n```ts\nx"],
    ["a fence with no code line", "a\n\n```ts\n```"],
    ["a fence in a quote", "a\n\n> ```ts\n> x\n> ```"],
    ["a fence in an HTML comment", "a\n\n<!--\n```ts\nx\n```\n-->"],
    ["a fence in the frontmatter", "---\nk: |\n  ```ts\n  x\n  ```\n---\nb"],
  ])("keeps today's look for %s", (_name, text) => {
    const view = markView(text, { mode });
    setCaret(view, text.length);
    expect(header(view)).toBeNull();
    expect(view.contentDOM.querySelector(".cm-code-box")).toBeNull();
  });

  it("shows the unclosed fence's line as written", () => {
    const view = markView("a\n\n```ts\nx", { mode });
    setCaret(view, 0);
    expect(view.contentDOM.textContent).toContain("```ts");
  });

  it("reads the box from a fence in a list item", () => {
    const view = markView("- a\n\n  ```ts\n  x\n  ```\n\nb", { mode });
    setCaret(view, view.state.doc.length);
    expect(languageButton(view).getAttribute("aria-label")).toBe(
      "Code language: TypeScript",
    );
  });
});

describe("show mode", () => {
  const TEXT = "a\n```ts\nx\ny\nz\n```\nb";

  it.each([
    ["the first code line", TEXT.indexOf("x")],
    ["the last code line", TEXT.indexOf("z")],
  ])("shows the fence lines while the caret is on %s", (_name, at) => {
    const view = markView(TEXT, { mode: "show" });
    setCaret(view, at);
    expect(header(view)).toBeNull();
    expect(view.contentDOM.textContent).toContain("```ts");
    // The box stays a box around its lines.
    expect(view.contentDOM.querySelector(".cm-code-box")).not.toBeNull();
  });

  it("keeps the header while the caret is on a middle code line", () => {
    const view = markView(TEXT, { mode: "show" });
    setCaret(view, TEXT.indexOf("y"));
    expect(header(view)).not.toBeNull();
    expect(view.contentDOM.textContent).not.toContain("`");
  });
});

describe("Raw", () => {
  it("shows the fences and draws no box", () => {
    const view = markView(DOC, { raw: true });
    expect(view.contentDOM.textContent).toContain("```ts");
    expect(header(view)).toBeNull();
    expect(view.contentDOM.querySelector(".cm-code-box")).toBeNull();
  });
});

describe("the caret around the box", () => {
  // a=0, ```ts=2..7, x=8, y=10, ```=12..15, b=16
  const TEXT = "a\n```ts\nx\ny\n```\nb";

  it.each(MODES)(
    "goes in from the line above with ArrowDown (%s mode)",
    (mode) => {
      const view = markView(TEXT, { mode });
      setCaret(view, 0);
      expect(keys(view, "ArrowDown")).toBe(true);
      expect(caret(view)).toBe(8);
    },
  );

  it("leaves from the last code line to the line below with ArrowDown", () => {
    const view = markView(TEXT);
    setCaret(view, 10);
    keys(view, "ArrowDown");
    expect(caret(view)).toBe(16);
  });

  it("goes in from the line below with ArrowUp and leaves upward", () => {
    const view = markView(TEXT);
    setCaret(view, 16);
    keys(view, "ArrowUp");
    expect(caret(view)).toBe(10);
    setCaret(view, 8);
    keys(view, "ArrowUp");
    expect(caret(view)).toBe(0);
  });

  it("steps over each fence line with ArrowRight and ArrowLeft", () => {
    const view = markView(TEXT);
    setCaret(view, 1);
    keys(view, "ArrowRight");
    expect(caret(view)).toBe(8);
    keys(view, "ArrowLeft");
    expect(caret(view)).toBe(1);
    setCaret(view, 11);
    keys(view, "ArrowRight");
    expect(caret(view)).toBe(16);
    keys(view, "ArrowLeft");
    expect(caret(view)).toBe(11);
  });

  it("stays on the last code line when the box ends the document", () => {
    const view = markView("```\nx\n```");
    setCaret(view, 4);
    keys(view, "ArrowDown");
    expect(caret(view)).toBe(4);
    setCaret(view, 5);
    keys(view, "ArrowRight");
    expect(caret(view)).toBe(5);
  });

  it("stays on the first code line when the box starts the document", () => {
    const view = markView("```\nx\n```\nb");
    setCaret(view, 4);
    keys(view, "ArrowUp");
    expect(caret(view)).toBe(4);
    keys(view, "ArrowLeft");
    expect(caret(view)).toBe(4);
  });

  it("steps from one box into the next when they touch", () => {
    // a=4, the fences at 6 and 10, b=14
    const touching = "```\na\n```\n```\nb\n```";
    const view = markView(touching);
    setCaret(view, 4);
    keys(view, "ArrowDown");
    expect(caret(view)).toBe(14);
    setCaret(view, 5);
    keys(view, "ArrowRight");
    expect(caret(view)).toBe(14);
    keys(view, "ArrowLeft");
    expect(caret(view)).toBe(5);
    setCaret(view, 14);
    keys(view, "ArrowUp");
    expect(view.state.doc.lineAt(caret(view)).number).toBe(2);
  });

  it("puts a caret mapped onto a hidden fence line into the box", () => {
    const view = markView(TEXT);
    view.dispatch({ selection: EditorSelection.cursor(4) });
    expect(caret(view)).toBe(8);
    view.dispatch({ selection: EditorSelection.cursor(13) });
    expect(caret(view)).toBe(11);
  });

  it("leaves a selection over the box as it is, and copies the source", () => {
    const view = markView(TEXT);
    view.focus();
    view.dispatch({
      selection: EditorSelection.range(0, TEXT.length),
      userEvent: "select",
    });
    expect(view.state.selection.main.from).toBe(0);
    expect(view.state.selection.main.to).toBe(TEXT.length);
    const data = new Map<string, string>();
    const event = new Event("copy", { bubbles: true, cancelable: true });
    Object.defineProperty(event, "clipboardData", {
      value: {
        clearData: () => {
          data.clear();
        },
        setData: (type: string, value: string) => data.set(type, value),
        getData: (type: string) => data.get(type) ?? "",
      },
    });
    view.contentDOM.dispatchEvent(event);
    expect(data.get("text/plain")).toBe(TEXT);
    expect(bytes(view.state)).toBe(TEXT);
  });
});

describe("the header's buttons", () => {
  it.each(MODES)(
    "ask for the language menu and the actions menu (%s mode)",
    (mode) => {
      const { view, heard } = listened(DOC, { mode });
      setCaret(view, 0);
      const event = press(languageButton(view));
      expect(event.defaultPrevented).toBe(true);
      expect(heard.at(-1)).toMatchObject({ kind: "language", at: 3 });
      press(moreButton(view));
      expect(heard.at(-1)).toMatchObject({ kind: "code-actions", at: 3 });
      expect(caret(view)).toBe(0);
      expect(bytes(view.state)).toBe(DOC);
    },
  );

  it("ask for nothing on a Mac Ctrl+click", () => {
    vi.spyOn(navigator, "platform", "get").mockReturnValue("MacIntel");
    const { view, heard } = listened(DOC);
    for (const el of [languageButton(view), moreButton(view)]) {
      for (const type of ["mousedown", "contextmenu", "mouseup", "click"]) {
        el.dispatchEvent(
          new MouseEvent(type, {
            bubbles: true,
            cancelable: true,
            button: type === "contextmenu" ? 2 : 0,
            ctrlKey: true,
          }),
        );
      }
    }
    expect(heard).toEqual([]);
    press(languageButton(view));
    expect(heard).toHaveLength(1);
  });

  it("ask for nothing on a page that takes no writes (C12)", () => {
    const { view, heard } = listened(DOC, { adapter: "reflection" });
    view.dispatch({
      effects: StateEffect.appendConfig.of(EditorState.readOnly.of(true)),
    });
    press(languageButton(view));
    press(moreButton(view));
    setCaret(view, 9);
    keys(view, "Shift-F10");
    expect(heard).toEqual([]);
  });

  it("open the actions menu on Shift+F10 on a code line", () => {
    const { view, heard } = listened(DOC);
    setCaret(view, 9);
    keys(view, "Shift-F10");
    expect(heard.at(-1)).toMatchObject({ kind: "code-actions", at: 3 });
  });
});

describe("the language word writer", () => {
  it("replaces only the info word", () => {
    const view = markView(DOC);
    expect(languageChanges(view.state, 3, "python")).toEqual([
      { from: 6, to: 8, insert: "python" },
    ]);
  });

  it("keeps the rest of the info text", () => {
    expect(
      apply("```ts title=x\ny\n```", (s) => languageChanges(s, 0, "rust")),
    ).toBe("```rust title=x\ny\n```");
  });

  it("takes the word away for Plain text", () => {
    expect(apply("```ts\nx\n```", (s) => languageChanges(s, 0, null))).toBe(
      "```\nx\n```",
    );
  });

  it("puts a word back where Plain text took one away", () => {
    const view = markView("```ts title=x\ny\n```");
    const remove = languageChanges(view.state, 0, null);
    if (!remove) throw new Error("no change");
    view.dispatch({ changes: remove });
    expect(bytes(view.state)).toBe("``` title=x\ny\n```");
    const add = languageChanges(view.state, 0, "rust");
    if (!add) throw new Error("no change");
    view.dispatch({ changes: add });
    expect(bytes(view.state)).toBe("```rust title=x\ny\n```");
  });

  it("keeps a tilde fence's tildes and lets its word hold a backtick", () => {
    expect(apply("~~~ts\nx\n~~~", (s) => languageChanges(s, 0, "python"))).toBe(
      "~~~python\nx\n~~~",
    );
    expect(apply("~~~ts\nx\n~~~", (s) => languageChanges(s, 0, "a`b"))).toBe(
      "~~~a`b\nx\n~~~",
    );
  });

  it.each([
    ["a space", "a b"],
    ["a tab", "a\tb"],
    ["a backtick in a backtick fence", "a`b"],
    ["nothing", ""],
  ])("refuses a word with %s", (_name, word) => {
    const view = markView("```ts\nx\n```");
    expect(languageChanges(view.state, 0, word)).toBeNull();
  });

  it.each([
    ["the same word", "```ts\nx\n```", "ts"],
    ["Plain text on no word", "```\nx\n```", null],
  ])("writes nothing for %s", (_name, text, word) => {
    const view = markView(text);
    expect(languageChanges(view.state, 0, word)).toEqual([]);
  });

  it("writes the word as given, whatever language the old one named", () => {
    expect(
      apply("```typescript\nx\n```", (s) => languageChanges(s, 0, "ts")),
    ).toBe("```ts\nx\n```");
    expect(apply("```foo\nx\n```", (s) => languageChanges(s, 0, null))).toBe(
      "```\nx\n```",
    );
  });

  it("writes a word no language has over another one", () => {
    expect(apply("```foo\nx\n```", (s) => languageChanges(s, 0, "bar"))).toBe(
      "```bar\nx\n```",
    );
  });

  it("reads no fence away from a fence's start, or in a mermaid fence", () => {
    const view = markView("a\n\n```ts\nx\n```\n\n```mermaid\ng\n```");
    expect(languageChanges(view.state, 4, "python")).toBeNull();
    expect(languageChanges(view.state, 0, "python")).toBeNull();
    expect(codeFenceAt(view.state, 20)).toBeNull();
    expect(codeFenceAt(view.state, 9)).toMatchObject({ from: 3, to: 14 });
  });
});

describe("Add a paragraph below", () => {
  it("puts one line break after the closing fence", () => {
    expect(apply("```\nx\n```", (s) => paragraphBelowChanges(s, 0))).toBe(
      "```\nx\n```\n",
    );
    expect(apply(DOC, (s) => paragraphBelowChanges(s, 3))).toBe(
      "a\n\n```ts\nx\n```\n\n\nb",
    );
  });
});

describe("the caret after a commit", () => {
  it("never rests on a hidden fence line", () => {
    const view = markView(DOC, {
      onElementEdit: () => {},
    });
    setCaret(view, 9);
    requestElementEdit(view, {
      kind: "language",
      at: 3,
      rect: new DOMRect(),
    });
    expect(
      commitElementEdit(view, (state, edit) =>
        edit.kind === "language" ? languageChanges(state, edit.at, "py") : null,
      ),
    ).toBe(true);
    expect(bytes(view.state)).toBe("a\n\n```py\nx\n```\n\nb");
    const line = view.state.doc.lineAt(caret(view)).text;
    expect(line).toBe("x");
  });

  it("puts the caret on a tall box's first line after a language pick", () => {
    const text = "a\n```ts\n1\n2\n3\n4\n5\n```\nb";
    const view = markView(text, { onElementEdit: () => {} });
    setCaret(view, 0);
    requestElementEdit(view, { kind: "language", at: 2, rect: new DOMRect() });
    commitElementEdit(view, (state, edit) =>
      edit.kind === "language" ? languageChanges(state, edit.at, "py") : null,
    );
    expect(view.state.doc.lineAt(caret(view)).text).toBe("1");
  });
});

describe("a caret a peer's edit leaves on a hidden fence line", () => {
  // a=0, ```ts=2..7, x=8, y=10, ```=12..15, b=16
  const TEXT = "a\n```ts\nx\ny\n```\nb";

  it("is not moved by the peer's edit, and typing from it keeps the fence", () => {
    const { local, peer, sync } = markPeers(TEXT);
    setCaret(local, 16);
    peer.dispatch({ changes: { from: 15, to: 17 }, userEvent: "delete" });
    sync();
    expect(bytes(local.state)).toBe("a\n```ts\nx\ny\n```");
    expect(caret(local)).toBe(15);
    typeText(local, "z");
    expect(bytes(local.state)).toBe("a\n```ts\nx\ny\n```\nz");
    expect(caret(local)).toBe(17);
  });

  it("writes into the first code line from the opening fence", () => {
    const { local, peer, sync } = markPeers(TEXT);
    setCaret(local, 1);
    peer.dispatch({ changes: { from: 0, to: 2 }, userEvent: "delete" });
    sync();
    expect(caret(local)).toBe(0);
    typeText(local, "z");
    expect(bytes(local.state)).toBe("```ts\nzx\ny\n```\nb");
  });

  it("writes nothing on Backspace and goes into the box", () => {
    const { local, peer, sync } = markPeers(TEXT);
    setCaret(local, 16);
    peer.dispatch({ changes: { from: 15, to: 17 }, userEvent: "delete" });
    sync();
    keys(local, "Backspace");
    expect(bytes(local.state)).toBe("a\n```ts\nx\ny\n```");
    expect(caret(local)).toBe(11);
  });
});

describe("a language request from the open cell (C9)", () => {
  it("is heard with the cell's view and writes nothing there", () => {
    const TABLE = "| h |\n| - |\n| `a` |\n";
    const heard: { edit: ElementEdit | null; view: EditorView }[] = [];
    const listener = elementEditListener((edit, from) => {
      heard.push({ edit, view: from });
    });
    const main = soloMain(TABLE, [cellForwarding, listener]);
    const cell = openAt(main, TABLE.indexOf("a"), true, [
      elementEditCell(main),
    ]);
    requestElementEdit(cell.view, {
      kind: "language",
      at: 0,
      rect: new DOMRect(),
    });
    expect(heard.at(-1)?.view).toBe(cell.view);
    expect(() =>
      commitElementEdit(main, (state, edit) =>
        edit.kind === "language" ? languageChanges(state, edit.at, "py") : null,
      ),
    ).not.toThrow();
    expect(bytes(main.state)).toBe(TABLE);
  });
});

describe("a long document", () => {
  it("stays fast to type in and move through", () => {
    const parts: string[] = [];
    for (let n = 0; n < 100; n += 1) {
      parts.push(
        `Paragraph ${String(n)} with some words.`,
        "",
        "More text on a line of its own.",
        "",
        "```ts",
        "const a = 1;",
        "const b = 2;",
        "const c = 3;",
        "const d = 4;",
        "const e = 5;",
        "const f = 6;",
        "const g = 7;",
        "const h = 8;",
        "const i = 9;",
        "const j = 10;",
        "const k = 11;",
        "const l = 12;",
        "```",
        "",
        "",
      );
    }
    const text = parts.join("\n");
    expect(text.split("\n").length).toBe(2000);
    const view = markView(text, { mode: "show" });
    const inside = text.indexOf("const a") + 5;
    setCaret(view, inside);
    const start = performance.now();
    typeText(view, "abcdefghijklmnopqrstuvwxyz".repeat(4));
    for (let step = 0; step < 100; step += 1) {
      setCaret(view, text.indexOf("Paragraph") + step);
    }
    const spent = performance.now() - start;
    // Generous: it measures a rescan per keystroke, not a tight budget.
    expect(spent).toBeLessThan(5000);
    expect(
      view.contentDOM.querySelectorAll(".cm-code-header").length,
    ).toBeGreaterThan(0);
  });
});
