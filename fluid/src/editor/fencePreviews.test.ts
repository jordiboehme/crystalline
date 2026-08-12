/**
 * Preview widgets below the two block kinds that earn them: a mermaid fence
 * renders its diagram, a pipe table renders a readable grid. Both draw AFTER
 * their source, never in place of it, so every assertion checks the widget's
 * own DOM while confirming the buffer itself is untouched.
 */

import { EditorSelection } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import mermaid from "mermaid";
import { describe, expect, it, vi } from "vitest";

import { describeMermaidError, fencePreviews } from "./fencePreviews";
import { baseExtensions } from "./setup";

vi.mock("mermaid", () => ({
  default: {
    initialize: vi.fn(),
    render: vi.fn(() => Promise.resolve({ svg: "<svg data-diagram></svg>" })),
  },
}));

function editor(doc: string): EditorView {
  return new EditorView({
    doc,
    selection: EditorSelection.cursor(0),
    extensions: [...baseExtensions(false), fencePreviews(false)],
    parent: document.body,
  });
}

describe("fence previews", () => {
  it("renders a mermaid fence's diagram below the fence", async () => {
    const view = editor("```mermaid\ngraph TD; A-->B;\n```\n");
    await vi.waitFor(() => {
      expect(view.dom.querySelector(".cm-mermaid-preview svg")).not.toBeNull();
    });
    // The source is still the buffer, untouched.
    expect(view.state.doc.toString()).toContain("graph TD; A-->B;");
    view.destroy();
  });

  it("suppresses mermaid's own error rendering", async () => {
    // This preview redraws on every keystroke, so it renders half-typed
    // diagrams constantly and most of those fail. Mermaid's default is to
    // append its error graphic to `document.body`, outside the editor and
    // outside anything that ever tears it down: without this flag the bombs
    // pile up under the page for the whole session.
    const view = editor("```mermaid\ngraph TD; A-->B;\n```\n");
    await vi.waitFor(() => {
      expect(mermaid.initialize).toHaveBeenCalled();
    });
    expect(vi.mocked(mermaid.initialize).mock.calls.at(-1)?.[0]).toMatchObject({
      suppressErrorRendering: true,
    });
    view.destroy();
  });

  it("previews in the same palette the reading view draws in", async () => {
    // One configuration serves both surfaces: a diagram that changed color
    // between the editor and the page would read as two different diagrams.
    const view = editor("```mermaid\ngraph TD; A-->B;\n```\n");
    await vi.waitFor(() => {
      expect(mermaid.initialize).toHaveBeenCalled();
    });
    const config = vi.mocked(mermaid.initialize).mock.calls.at(-1)?.[0];
    expect(config).toMatchObject({ theme: "base" });
    expect(config?.themeVariables).toMatchObject({
      primaryColor: "#ccfbf1",
      primaryBorderColor: "#0f766e",
      noteBkgColor: "#f1f5f9",
      noteTextColor: "#0f172a",
      titleColor: "#0f172a",
    });
    view.destroy();
  });

  it("renders a table preview below the pipe syntax", () => {
    const view = editor("| a | b |\n|---|---|\n| 1 | 2 |\n");
    const table = view.dom.querySelector(".cm-table-preview table");
    expect(table).not.toBeNull();
    expect(table?.querySelectorAll("th").length).toBe(2);
    expect(table?.querySelectorAll("td").length).toBe(2);
    expect(table?.textContent).toContain("1");
    view.destroy();
  });

  it("shows nothing for a fence of another language", () => {
    const view = editor("```rust\nfn main() {}\n```\n");
    expect(view.dom.querySelector(".cm-mermaid-preview")).toBeNull();
    view.destroy();
  });

  it("a diagram that will not parse says why, quietly", async () => {
    vi.mocked(mermaid.render).mockRejectedValueOnce(
      new Error("Parse error on line 2:\nExpecting 'ARROW', got 'NODE_STRING'"),
    );
    const view = editor("```mermaid\nflowchart TD\n  A[Step\n```\n");
    await vi.waitFor(() => {
      expect(view.dom.querySelector(".cm-mermaid-error")).not.toBeNull();
    });
    const caption = view.dom.querySelector(".cm-mermaid-error");
    expect(caption?.textContent).toContain("Line 2");
    expect(caption?.textContent).toContain("Expecting 'ARROW'");
    // The buffer is untouched and the caption is not a live region.
    expect(view.state.doc.toString()).toContain("A[Step");
    expect(caption?.getAttribute("role")).toBeNull();
    view.destroy();
  });

  it("a good render never shows the caption", async () => {
    const view = editor("```mermaid\ngraph TD; A-->B;\n```\n");
    await vi.waitFor(() => {
      expect(view.dom.querySelector(".cm-mermaid-preview svg")).not.toBeNull();
    });
    expect(view.dom.querySelector(".cm-mermaid-error")).toBeNull();
    view.destroy();
  });

  it("does not preview a table-shaped line that is fence content", () => {
    // The syntax tree, not a line-shaped regex, decides what a table is: a
    // pipe row inside a fence is code the author wrote about a table, not
    // one the parser mounts a `Table` node for.
    const view = editor("```text\n| a | b |\n|---|---|\n| 1 | 2 |\n```\n");
    expect(view.dom.querySelector(".cm-table-preview")).toBeNull();
    view.destroy();
  });
});

describe("describeMermaidError", () => {
  it("names the line and the first informative thing mermaid said", () => {
    expect(
      describeMermaidError(
        new Error(
          "Parse error on line 2:\nExpecting 'ARROW', got 'NODE_STRING'",
        ),
      ),
    ).toBe("Line 2: Expecting 'ARROW', got 'NODE_STRING'");
  });

  it("reads the line out of a lexical error the same way", () => {
    // Mermaid's other prefix for the same kind of failure; the caption must
    // not carry either prefix through, since the line number already says it.
    expect(
      describeMermaidError(
        new Error("Lexical error on line 3:\nUnrecognized text."),
      ),
    ).toBe("Line 3: Unrecognized text.");
  });

  it("falls back to one plain sentence when no line is named", () => {
    // Half-typed diagrams fail this way constantly (no type word yet), and a
    // caption that quoted mermaid's internals here would be noise.
    expect(
      describeMermaidError(
        new Error("No diagram type detected matching given configuration"),
      ),
    ).toBe("This diagram does not render yet.");
  });

  it("uses the same sentence when the line is all mermaid said", () => {
    expect(describeMermaidError(new Error("Parse error on line 7:"))).toBe(
      "Line 7: This diagram does not render yet.",
    );
  });

  it("caps a long complaint at 160 characters, tail included", () => {
    // Mermaid quotes the offending source back, carets and all; the caption
    // is one line under a fence, not a transcript.
    const caption = describeMermaidError(
      new Error(`Parse error on line 4:\n${"x".repeat(400)}`),
    );
    expect(caption).toHaveLength(160);
    expect(caption.endsWith("...")).toBe(true);
    expect(caption.startsWith("Line 4: xxx")).toBe(true);
  });

  it("survives a cause that is not an Error", () => {
    // A rejected import or a thrown string reaches the same `.catch`.
    expect(describeMermaidError(undefined)).toBe(
      "This diagram does not render yet.",
    );
    expect(describeMermaidError({ trouble: true })).toBe(
      "This diagram does not render yet.",
    );
    expect(describeMermaidError("Parse error on line 5:\nBad shape")).toBe(
      "Line 5: Bad shape",
    );
  });
});
