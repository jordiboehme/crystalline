/**
 * The live preview of the dialect's elements, driven through a real view:
 * off the lines a selection touches, a callout head, a status label, a colour
 * code and math draw as the reading page draws them; on those lines the text
 * shows as written. Every assertion is on the rendered DOM, and the buffer
 * comes back out byte for byte.
 */

import { cursorCharLeft } from "@codemirror/commands";
import { EditorSelection, EditorState, StateEffect } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { render, waitFor } from "@testing-library/react";
import { createElement } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { cleanupMarkViews, markView } from "../test/markKit";
import { parsedState } from "../test/parse";
import CmEditor from "./CmEditor";
import type { ElementEdit } from "./elementEdit";
import { elementPreviews } from "./elementPreviews";
import { hiddenMarkLayer } from "./marks";
import { marksOnEditLine } from "./marks/mode";
import { livePreview } from "./preview";
import { baseExtensions, lineSeparatorFor } from "./setup";

let view: EditorView | null = null;

afterEach(() => {
  view?.destroy();
  view = null;
});

/**
 * A buffer with the preview layers, in the order the surface mounts them.
 * Show mode unless asked: most tests here pin what a touched line reveals.
 */
function editor(
  doc: string,
  at: number,
  options: { sections?: boolean; mode?: "hidden" | "show" } = {},
): EditorView {
  view = new EditorView({
    state: parsedState(
      EditorState.create({
        doc,
        selection: EditorSelection.cursor(at),
        extensions: [
          ...lineSeparatorFor(doc),
          ...baseExtensions(false),
          elementPreviews(options),
          livePreview(),
          hiddenMarkLayer(),
          marksOnEditLine.of((options.mode ?? "show") === "show"),
        ],
      }),
    ),
    parent: document.body,
  });
  return view;
}

/** The rendered text of one document line. */
function lineText(v: EditorView, number: number): string {
  const line = v.state.doc.line(number);
  const { node } = v.domAtPos(line.from + (line.length > 0 ? 1 : 0));
  const element = node instanceof Element ? node : node.parentElement;
  return element?.closest(".cm-line")?.textContent ?? "";
}

const CALLOUT = "> [!WARNING]\n> Mind the gap\n\nEnd\n";

describe("a callout", () => {
  it("draws its head as the kind word and icon off the active line", async () => {
    const v = editor(CALLOUT, CALLOUT.length);
    const head = v.dom.querySelector(".cm-callout-head");
    expect(head).not.toBeNull();
    await waitFor(() => {
      expect(head?.textContent).toBe("Warning");
      expect(head?.querySelector("svg")).not.toBeNull();
    });
    expect(lineText(v, 1)).not.toContain("[!WARNING]");
    // Every line of the quote wears the reading page's callout tint.
    const lines = v.dom.querySelectorAll(
      '.cm-line.callout[data-callout="warning"]',
    );
    expect(lines).toHaveLength(2);
    expect(v.state.doc.toString()).toBe(CALLOUT);
  });

  it("keeps a title as text beside the icon", async () => {
    const doc = "> [!TIP]- Short way\n> body\n\nEnd\n";
    const v = editor(doc, doc.length);
    const head = v.dom.querySelector(".cm-callout-head");
    await waitFor(() => {
      expect(head?.querySelector("svg")).not.toBeNull();
    });
    expect(head?.textContent).toBe("");
    expect(lineText(v, 1)).toContain("Short way");
    expect(lineText(v, 1)).not.toContain("[!TIP]-");
  });

  it("lets the caret move inside a title rather than over it", () => {
    const doc = "> [!TIP]- Short way\n> body\n\nEnd\n";
    // The end of the title, the head drawn: the caret is on the head line,
    // so a step left is checked against a line that is not drawn either.
    const v = editor(doc, doc.length);
    const end = doc.indexOf("\n");
    v.dispatch({ selection: EditorSelection.cursor(end) });
    cursorCharLeft(v);
    expect(v.state.selection.main.head).toBe(end - 1);
    // And with the caret elsewhere, nothing over the title is atomic.
    v.dispatch({ selection: EditorSelection.cursor(doc.length) });
    const atomic = v.state
      .facet(EditorView.atomicRanges)
      .map((source) => source(v));
    // Arrowing up from the body checks the head line as it is drawn now.
    const title = doc.indexOf("Short");
    const over: [number, number][] = [];
    for (const set of atomic) {
      set.between(title, title + 1, (from, to) => {
        if (from < title + 1 && to > title) {
          over.push([from, to]);
        }
      });
    }
    expect(over).toEqual([]);
  });

  it("shows the head as written while the caret is on it", () => {
    const v = editor(CALLOUT, 3);
    expect(v.dom.querySelector(".cm-callout-head")).toBeNull();
    expect(lineText(v, 1)).toContain("[!WARNING]");
  });
});

describe("a status label", () => {
  const DOC = "Build :status[Done]{green} today\n\nEnd\n";

  it("draws as the pill off the active line", () => {
    const v = editor(DOC, DOC.length);
    const pill = v.dom.querySelector('.status-label[data-status="green"]');
    expect(pill?.textContent).toBe("Done");
    expect(lineText(v, 1)).toBe("Build Done today");
  });

  it("draws a label's inline Markdown as its words, and a bracket inside it", () => {
    const doc = "Build :status[**Done** [now]]{blue} today\n\nEnd\n";
    const v = editor(doc, doc.length);
    const pill = v.dom.querySelector('.status-label[data-status="blue"]');
    expect(pill?.textContent).toBe("Done [now]");
    expect(lineText(v, 1)).toBe("Build Done [now] today");
  });

  it("leaves a label of nothing but spaces as text, as the reader does", () => {
    const doc = "Build :status[ ]{blue} today\n\nEnd\n";
    const v = editor(doc, doc.length);
    expect(v.dom.querySelector(".status-label")).toBeNull();
    expect(lineText(v, 1)).toBe("Build :status[ ]{blue} today");
  });

  it("shows as written on the active line", () => {
    const v = editor(DOC, 2);
    expect(v.dom.querySelector(".status-label")).toBeNull();
    expect(lineText(v, 1)).toBe("Build :status[Done]{green} today");
  });
});

describe("with marks hidden on the caret line", () => {
  const DOC = "Build :status[Done]{green} at $`x^2`$ now\n\nEnd\n";
  const afterPill = DOC.indexOf(" at");
  const afterMath = DOC.indexOf(" now");

  it("keeps a status pill and inline math drawn with the caret right after them", async () => {
    for (const at of [afterPill, afterMath]) {
      const v = editor(DOC, at, { mode: "hidden" });
      expect(v.dom.querySelector(".status-label")?.textContent).toBe("Done");
      await waitFor(() => {
        expect(v.dom.querySelector(".cm-math-inline .katex")).not.toBeNull();
      });
      expect(lineText(v, 1)).not.toContain(":status[");
      expect(lineText(v, 1)).not.toContain("$`");
      v.destroy();
    }
    view = null;
  });

  it("hands them back as text in show mode", () => {
    for (const at of [afterPill, afterMath]) {
      const v = editor(DOC, at, { mode: "show" });
      expect(v.dom.querySelector(".status-label")).toBeNull();
      expect(v.dom.querySelector(".cm-math-inline")).toBeNull();
      expect(lineText(v, 1)).toBe("Build :status[Done]{green} at $`x^2`$ now");
      v.destroy();
    }
    view = null;
  });

  it("keeps a callout head and a display formula drawn with the caret on them", () => {
    const doc = "> [!NOTE]\n> body\n\n$$\na^2\n$$\n\nEnd\n";
    const v = editor(doc, doc.indexOf("a^2"), { mode: "hidden" });
    expect(v.dom.querySelector(".cm-math-block")).not.toBeNull();
    v.dispatch({ selection: EditorSelection.cursor(3) });
    expect(v.dom.querySelector(".cm-callout-head")).not.toBeNull();
    expect(lineText(v, 1)).not.toContain("[!NOTE]");
  });
});

describe("a colour code", () => {
  const DOC = "Accent `#0969DA` here and `#define` there\n\nEnd\n";

  it("gets a swatch before it, and only a colour does", () => {
    const v = editor(DOC, DOC.length);
    const swatches = v.dom.querySelectorAll<HTMLElement>("[data-swatch]");
    expect(swatches).toHaveLength(1);
    // jsdom normalises the colour it is handed.
    expect(swatches[0]?.style.background).not.toBe("");
  });

  it("has none on the active line", () => {
    const v = editor(DOC, 2);
    expect(v.dom.querySelector("[data-swatch]")).toBeNull();
  });
});

describe("math", () => {
  it("sets inline math off the active line and shows it as written on it", async () => {
    const doc = "Area $`\\pi r^2`$ of a circle\n\nEnd\n";
    const v = editor(doc, doc.length);
    await waitFor(() => {
      expect(v.dom.querySelector(".cm-math-inline .katex")).not.toBeNull();
    });
    expect(lineText(v, 1)).not.toContain("$`");
    v.dispatch({ selection: EditorSelection.cursor(2) });
    expect(v.dom.querySelector(".cm-math-inline")).toBeNull();
    expect(lineText(v, 1)).toBe("Area $`\\pi r^2`$ of a circle");
  });

  it("sets a math fence and a dollar block as display math", async () => {
    const doc = "Intro\n\n```math\nE = mc^2\n```\n\n$$\na^2 + b^2\n$$\n\nEnd\n";
    const v = editor(doc, doc.length);
    await waitFor(() => {
      expect(
        v.dom.querySelectorAll(".cm-math-block .katex-display"),
      ).toHaveLength(2);
    });
    // The caret in a block brings the source back.
    v.dispatch({ selection: EditorSelection.cursor(doc.indexOf("a^2")) });
    expect(v.dom.querySelectorAll(".cm-math-block")).toHaveLength(1);
    expect(v.dom.textContent).toContain("a^2 + b^2");
    expect(v.state.doc.toString()).toBe(doc);
  });

  it("keeps the drawn formulas when a caret move touches none of them", () => {
    const doc = "Intro words\n\n$$\na^2\n$$\n\nEnd words\n";
    const v = editor(doc, doc.length);
    const drawn = () =>
      v.state
        .facet(EditorView.decorations)
        .map((d) => (typeof d === "function" ? d(v) : d));
    const before = drawn();
    v.dispatch({ selection: EditorSelection.cursor(2) });
    expect(drawn()).toEqual(before);
    expect(drawn().every((set, i) => set === before[i])).toBe(true);
    v.dispatch({ selection: EditorSelection.cursor(doc.indexOf("a^2")) });
    expect(v.dom.querySelector(".cm-math-block")).toBeNull();
  });

  it("leaves a single dollar pair alone", () => {
    const doc = "It costs $5 and $6 today\n\nEnd\n";
    const v = editor(doc, doc.length);
    expect(v.dom.querySelector(".cm-math-inline, .cm-math-block")).toBeNull();
  });
});

describe("inside the React screen that mounts the buffer", () => {
  it("draws a callout head without a word from React", async () => {
    const errors = vi.spyOn(console, "error").mockImplementation(() => {});
    try {
      const { container, unmount } = render(
        createElement(CmEditor, {
          initialDoc: "Top\n\n> [!NOTE]\n> body\n",
          extensions: [
            ...baseExtensions(false),
            elementPreviews(),
            hiddenMarkLayer(),
          ],
          ariaLabel: "Source",
          onReady: () => {},
          onDocChanged: () => {},
        }),
      );
      await waitFor(() => {
        expect(container.querySelector(".cm-callout-head svg")).not.toBeNull();
      });
      unmount();
      await Promise.resolve();
      expect(errors).not.toHaveBeenCalled();
    } finally {
      errors.mockRestore();
    }
  });
});

/** The TeX every formula on screen was set from, as KaTeX keeps it. */
function texOf(v: EditorView): string[] {
  return [...v.dom.querySelectorAll("annotation")].map(
    (note) => note.textContent ?? "",
  );
}

describe("math, the remaining forms", () => {
  it("sets double-dollar math on one line as inline math", async () => {
    const doc = "Area $$x^2$$ here\n\n$$y_1$$\n\nEnd\n";
    const v = editor(doc, doc.length);
    await waitFor(() => {
      expect(v.dom.querySelectorAll(".cm-math-inline .katex")).toHaveLength(2);
    });
    expect(v.dom.querySelector(".cm-math-block")).toBeNull();
    expect(texOf(v)).toEqual(["x^2", "y_1"]);
  });

  it("sets display math inside a callout without its quote marks", async () => {
    const doc =
      "> [!NOTE]\n> Two forms:\n>\n> ```math\n> E = mc^2\n> ```\n>\n> $$\n> a^2 + b^2\n> $$\n\nEnd\n";
    const v = editor(doc, doc.length);
    await waitFor(() => {
      expect(
        v.dom.querySelectorAll(".cm-math-block .katex-display"),
      ).toHaveLength(2);
    });
    const tex = texOf(v);
    expect(tex).toEqual(["E = mc^2", "a^2 + b^2"]);
    for (const each of tex) {
      expect(each).not.toContain(">");
    }
    // Each formula keeps the callout's tint, and starts at a line's start.
    for (const block of v.dom.querySelectorAll(".cm-math-block")) {
      expect(block.classList.contains("callout")).toBe(true);
      expect(block.getAttribute("data-callout")).toBe("note");
    }
    expect(v.state.doc.toString()).toBe(doc);
  });

  it("never pairs dollar fences across a page's sections", () => {
    const doc = "## A\n\n$$\nx\n\n## B\n\nWords\n\n$$\n\nEnd\n";
    const page = editor(doc, doc.length, { sections: true });
    expect(page.dom.querySelector(".cm-math-block")).toBeNull();
    expect(page.dom.textContent).toContain("B");
    page.destroy();
    // In one document the reader pairs them, and so does the preview.
    const engram = editor(doc, doc.length);
    expect(engram.dom.querySelector(".cm-math-block")).not.toBeNull();
  });

  it("measures a display formula again whenever its size changes", () => {
    const observed: Element[] = [];
    let disconnected = 0;
    const original = globalThis.ResizeObserver;
    globalThis.ResizeObserver = class {
      observe(target: Element) {
        observed.push(target);
      }
      unobserve() {}
      disconnect() {
        disconnected += 1;
      }
    };
    try {
      const doc = "$$\nx\n$$\n\nEnd\n";
      const v = editor(doc, doc.length);
      expect(observed).toContain(v.dom.querySelector(".cm-math-block"));
      v.dispatch({ selection: EditorSelection.cursor(2) });
      expect(disconnected).toBe(1);
    } finally {
      globalThis.ResizeObserver = original;
    }
  });
});

describe("a callout's inset", () => {
  it("marks its first and last lines for the head and foot padding", () => {
    const v = editor(CALLOUT, CALLOUT.length);
    const lines = [...v.dom.querySelectorAll(".cm-line.callout")];
    expect(lines).toHaveLength(2);
    expect(lines[0]?.classList.contains("cm-callout-first")).toBe(true);
    expect(lines[1]?.classList.contains("cm-callout-last")).toBe(true);
  });
});

describe("math, the edges", () => {
  it("keeps a callout's foot when a formula ends it", async () => {
    const doc = "> [!TIP]\n> Words\n>\n> $$\n> x\n> $$\n\nEnd\n";
    const v = editor(doc, doc.length);
    await waitFor(() => {
      expect(
        v.dom.querySelector(".cm-math-block .katex-display"),
      ).not.toBeNull();
    });
    const block = v.dom.querySelector(".cm-math-block");
    expect(block?.classList.contains("cm-callout-last")).toBe(true);
    expect(block?.classList.contains("cm-callout-first")).toBe(false);
  });

  it("reads a deeper dollar line as content, as the reader does", async () => {
    const doc = "$$\n> $$\n$$\n\nEnd\n";
    const v = editor(doc, doc.length);
    expect(v.dom.querySelectorAll(".cm-math-block")).toHaveLength(1);
    // KaTeX cannot set a dollar sign and shows the TeX it was handed.
    await waitFor(() => {
      expect(v.dom.querySelector(".cm-math-block")?.textContent).toContain(
        "> $$",
      );
    });
  });

  it("draws no fence that runs across a page's section heading", () => {
    const doc = "## A\n\n```math\nx\n\n## B\n\nWords\n\n```\n\nEnd\n";
    const page = editor(doc, doc.length, { sections: true });
    expect(page.dom.querySelector(".cm-math-block")).toBeNull();
    page.destroy();
    const engram = editor(doc, doc.length);
    expect(engram.dom.querySelector(".cm-math-block")).not.toBeNull();
  });
});

describe("the drawn elements ask for their editors", () => {
  it("draws the callout's kind icon and chevron as named buttons", async () => {
    const doc = "> [!TIP]- Short way\n> body\n\nEnd\n";
    const v = editor(doc, doc.length);
    const head = v.dom.querySelector(".cm-callout-head");
    await waitFor(() => {
      expect(head?.querySelector("button")).not.toBeNull();
    });
    const kind = head?.querySelector('button[aria-label="Callout kind: Tip"]');
    const fold = head?.querySelector('button[aria-label="Callout fold"]');
    expect(kind).not.toBeNull();
    expect(fold?.getAttribute("aria-expanded")).toBe("false");
  });

  it.each([
    ["a pill", "x\n:status[Done]{green}", ".status-label", "status", 2],
    ["inline math", "x\na $`y`$", ".cm-math-inline", "math", 4],
    ["a display formula", "x\n\n$$\na\n$$", ".cm-math-block", "math", 3],
  ])(
    "asks for %s's editor on a press (show mode)",
    (_name, doc, selector, kind, at) => {
      const heard: (ElementEdit | null)[] = [];
      const v = markView(doc, {
        mode: "show",
        onElementEdit: (edit) => {
          heard.push(edit);
        },
      });
      const el = v.contentDOM.querySelector(selector);
      expect(el).not.toBeNull();
      el?.dispatchEvent(
        new MouseEvent("mousedown", { bubbles: true, cancelable: true }),
      );
      expect(heard.at(-1)).toMatchObject({ kind, at });
      cleanupMarkViews();
    },
  );

  it("asks nothing for a pill in Raw or on a page that takes no writes", () => {
    const heard: (ElementEdit | null)[] = [];
    const v = markView("x\n:status[Done]", {
      onElementEdit: (edit) => {
        heard.push(edit);
      },
    });
    v.dispatch({
      effects: StateEffect.appendConfig.of(EditorState.readOnly.of(true)),
    });
    v.contentDOM
      .querySelector(".status-label")
      ?.dispatchEvent(
        new MouseEvent("mousedown", { bubbles: true, cancelable: true }),
      );
    expect(heard).toEqual([]);
    cleanupMarkViews();
    const raw = markView("x\n:status[Done]", { raw: true });
    expect(raw.contentDOM.querySelector(".status-label")).toBeNull();
    cleanupMarkViews();
  });
});

describe("inline math read as the page reads it", () => {
  it("draws a formula after an escaped backslash, and none after an escaped dollar", () => {
    const v = editor("a \\\\$`x`$ b \\$`y`$\n\nEnd", 0, { mode: "hidden" });
    const drawn = v.contentDOM.querySelectorAll(".cm-math-inline");
    expect(drawn).toHaveLength(1);
  });
});

describe("a math fence in a list item (I6)", () => {
  it.each([
    ["after a blank line", "- item\n\n  ```math\n  x^2\n  ```\n\nEnd\n"],
    ["right under the item", "- item\n  ```math\n  x^2\n  ```\n\nEnd\n"],
  ])("sets it as display math, %s", async (_name, doc) => {
    const v = editor(doc, doc.length);
    await waitFor(() => {
      expect(
        v.dom.querySelectorAll(".cm-math-block .katex-display"),
      ).toHaveLength(1);
    });
    expect(v.dom.querySelector(".cm-code-box")).toBeNull();
  });
});
