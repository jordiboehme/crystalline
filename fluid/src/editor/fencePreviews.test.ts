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

import { fencePreviews } from "./fencePreviews";
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

  it("does not preview a table-shaped line that is fence content", () => {
    // The syntax tree, not a line-shaped regex, decides what a table is: a
    // pipe row inside a fence is code the author wrote about a table, not
    // one the parser mounts a `Table` node for.
    const view = editor("```text\n| a | b |\n|---|---|\n| 1 | 2 |\n```\n");
    expect(view.dom.querySelector(".cm-table-preview")).toBeNull();
    view.destroy();
  });
});
