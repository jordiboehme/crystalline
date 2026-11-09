import { EditorState } from "@codemirror/state";
import { describe, expect, test } from "vitest";

import { defined } from "../../test/assert";
import { parsedState } from "../../test/parse";
import { baseExtensions, docText, lineSeparatorFor } from "../setup";
import { fenceAt, mermaidFences } from "./fence";

function stateOf(text: string): EditorState {
  return parsedState(
    EditorState.create({
      doc: text,
      extensions: [...lineSeparatorFor(text), ...baseExtensions(false)],
    }),
  );
}

const LF = [
  "# Title",
  "",
  "Some text.",
  "",
  "```mermaid",
  "flowchart TD",
  "    A --> B",
  "```",
  "",
  "After.",
  "",
].join("\n");

describe("mermaidFences", () => {
  test("refuses a fence inside the frontmatter", () => {
    const text =
      "---\nnote: |\n  ```mermaid\n  flowchart TD\n    A --> B\n  ```\n---\n\nbody";
    expect(mermaidFences(stateOf(text))).toEqual([]);
  });

  test("reads a closed fence's positions and body", () => {
    const state = stateOf(LF);
    const fences = mermaidFences(state);
    expect(fences).toHaveLength(1);
    const fence = defined(fences[0]);
    expect(fence.body).toBe("flowchart TD\n    A --> B");
    expect(state.doc.sliceString(fence.bodyFrom, fence.bodyTo)).toBe(
      fence.body,
    );
    expect(state.doc.sliceString(fence.from, fence.to)).toBe(
      "```mermaid\nflowchart TD\n    A --> B\n```",
    );
    expect(fence.topLevel).toBe(true);
  });

  test("a CRLF document gives the same positions and body", () => {
    // CodeMirror counts a line break as one position whatever the separator,
    // so the positions are equal; only the saved bytes differ.
    const crlf = LF.replaceAll("\n", "\r\n");
    const lfState = stateOf(LF);
    const crlfState = stateOf(crlf);
    expect(crlfState.lineBreak).toBe("\r\n");
    expect(docText(crlfState)).toBe(crlf);
    const a = defined(mermaidFences(lfState)[0]);
    const b = defined(mermaidFences(crlfState)[0]);
    expect(b).toEqual(a);
    // In the saved bytes the fence starts one byte later per earlier line.
    const earlierLines = lfState.doc.lineAt(a.from).number - 1;
    expect(crlf.indexOf("```mermaid")).toBe(a.from + earlierLines);
  });

  test("a fence in a blockquote or a list item is not top level", () => {
    const text = [
      "> ```mermaid",
      "> flowchart TD",
      ">     A --> B",
      "> ```",
      "",
      "- item",
      "",
      "  ```mermaid",
      "  flowchart LR",
      "      C --> D",
      "  ```",
      "",
      "```mermaid",
      "graph TD",
      "  E --> F",
      "```",
      "",
    ].join("\n");
    const fences = mermaidFences(stateOf(text));
    expect(fences.map((f) => f.topLevel)).toEqual([false, false, true]);
    expect(defined(fences[2]).body).toBe("graph TD\n  E --> F");
  });

  test("leaves out unclosed, empty and other fences, and keeps tildes", () => {
    const text = [
      "```mermaid",
      "```",
      "",
      "```mermaid",
      "   ",
      "```",
      "",
      "```js",
      "flowchart TD",
      "```",
      "",
      "~~~mermaid title",
      "flowchart TD",
      "  A --> B",
      "~~~",
      "",
      "```mermaid",
      "flowchart TD",
      "  open",
      "",
    ].join("\n");
    const fences = mermaidFences(stateOf(text));
    expect(fences.map((f) => f.body)).toEqual(["flowchart TD\n  A --> B"]);
  });
});

describe("fenceAt", () => {
  test("finds the fence whose range holds the position, ends included", () => {
    const state = stateOf(LF);
    const fence = defined(mermaidFences(state)[0]);
    expect(fenceAt(state, fence.from)).toEqual(fence);
    expect(fenceAt(state, fence.to)).toEqual(fence);
    expect(fenceAt(state, fence.bodyFrom + 3)).toEqual(fence);
    expect(fenceAt(state, fence.from - 1)).toBeNull();
    expect(fenceAt(state, fence.to + 1)).toBeNull();
    expect(fenceAt(state, 0)).toBeNull();
  });
});
