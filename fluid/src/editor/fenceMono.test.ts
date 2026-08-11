/**
 * The face of a code block in preview mode.
 *
 * `tags.monospace` cannot carry this on its own: `@lezer/markdown` maps
 * `CodeText` to it, but the moment a fence names a language the nested parser
 * mounts over the body and `@lezer/highlight` recurses with `inheritedClass`
 * reset to "", so the mono class never reaches the tokens inside. With the
 * scroller set proportional that hole is visible - a ```json fence would read
 * in the reading font - which is what these tests pin.
 */

import { EditorState } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { afterEach, describe, expect, it } from "vitest";

import { fenceMono } from "./fenceMono";
import { baseExtensions } from "./setup";

const DOC = [
  "Prose line, deliberately proportional.",
  "",
  "```js",
  "const answer = 42;",
  "```",
  "",
  "```",
  "plain fence text",
  "```",
  "",
  "    indented code line",
  "",
  "Tail prose.",
  "",
].join("\n");

const views: EditorView[] = [];

afterEach(() => {
  while (views.length > 0) {
    views.pop()?.destroy();
  }
});

function open(): EditorView {
  const view = new EditorView({
    state: EditorState.create({
      doc: DOC,
      extensions: [...baseExtensions(false), fenceMono()],
    }),
    parent: document.body,
  });
  views.push(view);
  return view;
}

/** The rendered line whose text is exactly `text`. */
function line(view: EditorView, text: string): Element {
  const found = Array.from(view.dom.querySelectorAll(".cm-line")).find(
    (element) => element.textContent === text,
  );
  expect(found, `no rendered line reading ${text}`).toBeDefined();
  return found as Element;
}

function fontOf(element: Element): string {
  return getComputedStyle(element).fontFamily;
}

describe("code blocks keep their mono face", () => {
  it("draws a language-tagged fence in mono, delimiters included", () => {
    const view = open();
    // The exact case the proportional scroller broke: the body of a fence
    // with an info string, whose tokens the nested language parser owns.
    expect(fontOf(line(view, "const answer = 42;"))).toMatch(/mono/i);
    expect(fontOf(line(view, "```js"))).toMatch(/mono/i);
  });

  it("covers a bare fence and an indented block too", () => {
    const view = open();
    expect(fontOf(line(view, "plain fence text"))).toMatch(/mono/i);
    expect(fontOf(line(view, "    indented code line"))).toMatch(/mono/i);
  });

  it("leaves prose alone", () => {
    const view = open();
    expect(
      fontOf(line(view, "Prose line, deliberately proportional.")),
    ).not.toMatch(/mono/i);
    expect(line(view, "Tail prose.").className).not.toContain("cm-fence-mono");
  });
});
