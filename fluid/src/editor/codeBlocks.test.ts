/**
 * The code box: a closed fence draws as a box with a header (its language
 * and a button for more actions) where its fence lines would be, in both
 * modes of the setting, and Raw shows the fences. Nothing here writes a
 * byte: the box is a view over the source, and the caret never rests on a
 * hidden fence line.
 */

import { LanguageDescription, syntaxTree } from "@codemirror/language";
import { languages } from "@codemirror/language-data";
import { EditorSelection, EditorState, StateEffect } from "@codemirror/state";
import { render } from "@testing-library/react";
import { createElement } from "react";
import ReactMarkdown from "react-markdown";
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
  undoOnce,
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
import { codeLanguageFor, languageOf } from "./codeLanguage";
import { toggleLinePrefix } from "./toolbar";
import { parsedState, parsedView } from "../test/parse";

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
    ["text", "a\n\n```text\nx\n```"],
    ["plaintext", "a\n\n```plaintext\nx\n```"],
    ["context, which holds tex", "a\n\n```context\nx\n```"],
  ])("names Plain text for %s", (_name, text) => {
    const view = markView(text, { mode });
    setCaret(view, 0);
    expect(languageButton(view).getAttribute("aria-label")).toBe(
      "Code language: Plain text",
    );
  });

  it("names Math for an empty math fence, never Plain text (I6)", () => {
    const view = markView("a\n\n```math\n\n```", { mode });
    setCaret(view, 0);
    expect(languageButton(view).getAttribute("aria-label")).toBe(
      "Code language: Math",
    );
  });

  it.each([
    ["after a blank line", "- item\n\n  ```math\n  x^2\n  ```"],
    ["right under the item", "- item\n  ```math\n  x^2\n  ```"],
  ])(
    "draws no code box for a math fence in a list item, %s (I6)",
    (_n, text) => {
      const view = markView(text, { mode });
      setCaret(view, 0);
      expect(header(view)).toBeNull();
    },
  );

  it("reads the word after spaces, as CommonMark does", () => {
    const view = markView("a\n\n```  ts\nx\n```", { mode });
    setCaret(view, 0);
    expect(languageButton(view).getAttribute("aria-label")).toBe(
      "Code language: TypeScript",
    );
  });

  it.each([
    ["an empty math fence", "a\n\n```math\n\n```"],
    ["an empty mermaid fence", "a\n\n```mermaid\n\n```"],
  ])("draws a box for %s, which no other layer draws", (_name, text) => {
    const view = markView(text, { mode });
    setCaret(view, 0);
    expect(header(view)).not.toBeNull();
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

describe.each([
  ["in a list item", "- a\n\n  ```ts\n  x\n  ```\n\nb", 5],
  ["with three spaces", "a\n\n   ```ts\n   x\n   ```\n\nb", 3],
])("an indented fence (%s)", (_name, text, lineStart) => {
  const marks = text.indexOf("```");
  const wordFrom = marks + 3;

  it("asks from both header buttons and from Shift+F10", () => {
    const { view, heard } = listened(text);
    setCaret(view, 0);
    press(languageButton(view));
    expect(heard.at(-1)).toMatchObject({ kind: "language", at: lineStart });
    press(moreButton(view));
    expect(heard.at(-1)).toMatchObject({ kind: "code-actions", at: lineStart });
    setCaret(view, text.indexOf("x"));
    keys(view, "Shift-F10");
    expect(heard.at(-1)).toMatchObject({ kind: "code-actions", at: lineStart });
  });

  it("writes the word after the indent, and a paragraph below", () => {
    const view = markView(text, { onElementEdit: () => {} });
    expect(languageChanges(view.state, lineStart, "python")).toEqual([
      { from: wordFrom, to: wordFrom + 2, insert: "python" },
    ]);
    requestElementEdit(view, {
      kind: "language",
      at: lineStart,
      rect: new DOMRect(),
    });
    expect(
      commitElementEdit(view, (state, edit) =>
        edit.kind === "language"
          ? languageChanges(state, edit.at, "python")
          : null,
      ),
    ).toBe(true);
    expect(bytes(view.state)).toBe(text.replace("```ts", "```python"));
    const below = paragraphBelowChanges(parsedState(view.state), lineStart);
    expect(below).not.toBeNull();
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

  it("reaches the last code line with Mod-End and a press below the text", () => {
    // a=0, ```=2..5, x=6, ```=8..11
    const end = "a\n```\nx\n```";
    const view = markView(end);
    setCaret(view, 0);
    keys(view, "Mod-End");
    expect(caret(view)).toBe(7);
    setCaret(view, 0);
    setCaret(view, end.length);
    expect(caret(view)).toBe(7);
  });

  it("reaches the first code line with Mod-Home", () => {
    const start = "```\nx\n```\nb";
    const view = markView(start);
    setCaret(view, start.length);
    keys(view, "Mod-Home");
    expect(caret(view)).toBe(4);
  });

  it("keeps the goal column across a box", () => {
    const view = markView(TEXT);
    view.dispatch({
      selection: EditorSelection.create([
        EditorSelection.cursor(0, 0, undefined, 123),
      ]),
    });
    keys(view, "ArrowDown");
    expect(caret(view)).toBe(8);
    expect(view.state.selection.main.goalColumn).toBe(123);
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

  it("ask on a plain click (a screen reader, Enter or Space), once per press", () => {
    const { view, heard } = listened(DOC);
    setCaret(view, 0);
    languageButton(view).dispatchEvent(
      new MouseEvent("click", { bubbles: true, cancelable: true }),
    );
    expect(heard.at(-1)).toMatchObject({ kind: "language", at: 3 });
    moreButton(view).dispatchEvent(
      new MouseEvent("click", { bubbles: true, cancelable: true }),
    );
    expect(heard.at(-1)).toMatchObject({ kind: "code-actions", at: 3 });
    const count = heard.filter((edit) => edit !== null).length;
    // A mouse press, then the click the browser sends after it: one ask.
    press(languageButton(view));
    languageButton(view).dispatchEvent(
      new MouseEvent("click", { bubbles: true, cancelable: true }),
    );
    expect(heard.filter((edit) => edit !== null).length).toBe(count + 1);
    expect(caret(view)).toBe(0);
    expect(bytes(view.state)).toBe(DOC);
  });

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

  it("puts the word right after the marks when spaces come first", () => {
    expect(
      apply("```  ts title=x\ny\n```", (s) => languageChanges(s, 0, "rust")),
    ).toBe("```rust title=x\ny\n```");
    expect(apply("``` ts\ny\n```", (s) => languageChanges(s, 0, null))).toBe(
      "```\ny\n```",
    );
  });

  it("refuses Plain text when more info text follows the word", () => {
    const view = markView("```ts title=x\ny\n```");
    expect(languageChanges(view.state, 0, null)).toBeNull();
  });

  it("takes the word away for Plain text", () => {
    expect(apply("```ts\nx\n```", (s) => languageChanges(s, 0, null))).toBe(
      "```\nx\n```",
    );
  });

  it.each([
    ["```ts\ny\n```", "python", "python"],
    ["``` ts title=x\ny\n```", "rust", "rust"],
    ["```ts title=x\ny\n```", "go", "go"],
    ["``` ts\ny\n```", null, null],
  ])(
    "writes %s so the editor and the reading page read the same word",
    (text, word, read) => {
      const after = apply(text, (s) => languageChanges(s, 0, word));
      if (after === null) throw new Error("no change");
      const view = markView(after);
      // The editor: the parser's info and the header's word.
      const info = codeFenceAt(parsedState(view.state), 0);
      const editorWord = info
        ? view.state.doc.sliceString(info.word.from, info.word.to)
        : "";
      expect(editorWord === "" ? null : editorWord).toBe(read);
      let parsed: string | null = null;
      syntaxTree(parsedState(view.state)).iterate({
        enter: (node) => {
          // The parser's info is the whole info string; its first word is
          // what the highlighting looks up.
          if (node.name === "CodeInfo") {
            parsed =
              view.state.doc.sliceString(node.from, node.to).split(/\s/)[0] ??
              null;
          }
        },
      });
      expect(parsed).toBe(read);
      // CommonMark, as the reading page renders it.
      const page = render(createElement(ReactMarkdown, null, after));
      const code = page.container.querySelector("code");
      expect(code?.className || null).toBe(
        read === null ? null : `language-${read}`,
      );
      page.unmount();
    },
  );

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
    expect(codeFenceAt(parsedState(view.state), 20)).toBeNull();
    expect(codeFenceAt(parsedState(view.state), 9)).toMatchObject({
      from: 3,
      to: 14,
    });
  });
});

describe("Add a paragraph below", () => {
  it("puts one line break after the closing fence", () => {
    expect(
      apply("```\nx\n```", (s) => paragraphBelowChanges(parsedState(s), 0)),
    ).toBe("```\nx\n```\n");
    expect(apply(DOC, (s) => paragraphBelowChanges(parsedState(s), 3))).toBe(
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

  it("goes into the box right after the peer's edit, so no verb writes into a fence", async () => {
    const { local, peer, sync } = markPeers(TEXT);
    setCaret(local, 16);
    peer.dispatch({ changes: { from: 15, to: 17 }, userEvent: "delete" });
    sync();
    await Promise.resolve();
    expect(caret(local)).toBe(11);
    toggleLinePrefix(local, "> ");
    expect(bytes(local.state)).toBe("a\n```ts\nx\n> y\n```");
    // Moving the caret is no step of its own to undo.
    undoOnce(local);
    expect(bytes(local.state)).toBe("a\n```ts\nx\ny\n```");
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

describe("the language lookup the editor and its highlighting share", () => {
  it.each(["text", "plaintext", "txt", "plain", "context", "TEXT"])(
    "names no language for %s",
    (word) => {
      expect(languageOf(word)).toBeNull();
    },
  );

  it.each([
    ["ts", "TypeScript"],
    ["typescript", "TypeScript"],
    ["Python", "Python"],
    ["tex", "LaTeX"],
    ["c++", "C++"],
  ])("finds %s exactly", (word, name) => {
    expect(languageOf(word)?.name).toBe(name);
  });

  it("hands the highlighting the same answer for a fence's info", () => {
    expect(codeLanguageFor("context")).toBeNull();
    expect(codeLanguageFor("text")).toBeNull();
    expect(codeLanguageFor("ts")?.name).toBe("TypeScript");
    expect(codeLanguageFor("")).toBeNull();
  });
});

describe("words the old fuzzy lookup found and the exact one keeps", () => {
  // Every name and alias of @codemirror/language-data plus common fence
  // words, compared between the old fuzzy match and `languageOf`: these are
  // the words the fuzzy match mapped to the right language and the exact
  // one missed (text, plaintext and context, which it got wrong, aside).
  it.each([
    ["jsonc", "JSON"],
    ["jsonl", "JSON"],
    ["ndjson", "JSON"],
  ])("reads %s as %s", (word, name) => {
    expect(languageOf(word)?.name).toBe(name);
    expect(codeLanguageFor(word)?.name).toBe(name);
  });

  it("names JSON in the header for a jsonc fence", () => {
    const view = markView("a\n\n```jsonc\n{}\n```");
    setCaret(view, 0);
    expect(
      view.contentDOM
        .querySelector('button[aria-label^="Code language"]')
        ?.getAttribute("aria-label"),
    ).toBe("Code language: JSON");
  });

  it("agrees with the old lookup on every other word", () => {
    const common =
      "jsonc jsonl ndjson json5 yml sh zsh shell console ps1 dockerfile makefile tsx jsx mjs cjs md html xml svg toml ini sql kt swift rb pl lua r cs c++ h hpp objc scala hs elixir ex erl vue svelte".split(
        " ",
      );
    const words = new Set<string>(common);
    for (const language of languages) {
      words.add(language.name);
      for (const alias of language.alias) words.add(alias);
    }
    const differ: string[] = [];
    for (const word of words) {
      const old = LanguageDescription.matchLanguageName(languages, word, true);
      if (old?.name !== languageOf(word)?.name) differ.push(word);
    }
    expect(differ).toEqual([]);
  });
});

/**
 * A commit whose own parse fell short (D64). On a busy machine the commit's
 * parse slice stopped before the box, so the filter that keeps a caret off
 * a hidden fence line saw no hidden line and let the caret rest on the
 * fence. When the parse moves on and hides that line, the caret goes into
 * the box, as after a peer's change. A clock that jumps a second at every
 * look gives the commit's parse one step at any load.
 */
describe("the caret after a commit whose parse fell short (D64)", () => {
  it("goes into the box once the parse hides its fence line", async () => {
    const above = Array.from(
      { length: 200 },
      (_, i) => `Paragraph ${String(i)} of the page.`,
    ).join("\n\n");
    const text = `${above}\n\n${DOC}`;
    const fence = text.indexOf("```ts");
    const view = markView(text, { onElementEdit: () => {} });
    setCaret(view, fence + 6);
    requestElementEdit(view, {
      kind: "language",
      at: fence,
      rect: new DOMRect(),
    });
    const now = Date.now.bind(Date);
    let ahead = 0;
    const clock = vi.spyOn(Date, "now").mockImplementation(() => {
      ahead += 1_000;
      return now() + ahead;
    });
    try {
      expect(
        commitElementEdit(view, (state, edit) =>
          edit.kind === "language"
            ? languageChanges(state, edit.at, "py")
            : null,
        ),
      ).toBe(true);
    } finally {
      clock.mockRestore();
    }
    // The commit's tree stops short of the box: the caret is on its fence.
    expect(syntaxTree(view.state).length).toBeLessThan(fence);
    expect(view.state.doc.lineAt(caret(view)).text).toBe("```py");
    // The parse moves on, as the worker publishes it.
    parsedView(view);
    await Promise.resolve();
    expect(view.state.doc.lineAt(caret(view)).text).toBe("x");
  });
});
