import { CompletionContext } from "@codemirror/autocomplete";
import { EditorSelection, EditorState } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { describe, expect, it } from "vitest";

import type { Vocabulary } from "../api/vocabulary";
import { crystallineCompletions, crystallineLines } from "./crystallineLines";
import { baseExtensions } from "./setup";

const DOC =
  "---\nt: x\n---\n\n- [decision] we chose X #arch #speed\n- relates_to [[Beta]]\n";

const VOCAB: Vocabulary = {
  tags: [
    { name: "arch", engrams: 4 },
    { name: "speed", engrams: 1 },
  ],
  categories: [
    { name: "decision", count: 3 },
    { name: "insight", count: 1 },
  ],
  relationTypes: [
    { name: "relates_to", count: 5 },
    { name: "supersedes", count: 1 },
  ],
};

describe("line affordances", () => {
  it("marks the category, the rel type and the tags", () => {
    const view = new EditorView({
      doc: DOC,
      selection: EditorSelection.cursor(0),
      extensions: [...baseExtensions(false), crystallineLines()],
      parent: document.body,
    });
    expect(view.dom.querySelector(".cm-obs-category")?.textContent).toBe(
      "[decision]",
    );
    expect(view.dom.querySelector(".cm-rel-type")?.textContent).toBe(
      "relates_to",
    );
    const tags = [...view.dom.querySelectorAll(".cm-line-tag")].map(
      (el) => el.textContent,
    );
    expect(tags).toEqual(["#arch", "#speed"]);
    view.destroy();
  });
});

async function completionsAt(doc: string): Promise<string[] | null> {
  const state = EditorState.create({
    doc,
    selection: EditorSelection.cursor(doc.length),
  });
  const result = await crystallineCompletions(() => VOCAB)(
    new CompletionContext(state, doc.length, false),
  );
  return result?.options.map((option) => option.label) ?? null;
}

describe("vocabulary completion", () => {
  it("offers categories after '- [', rel types after '- ', tags after '#'", async () => {
    expect(await completionsAt("- [dec")).toEqual(["decision", "insight"]);
    expect(await completionsAt("- rel")).toEqual(["relates_to", "supersedes"]);
    expect(await completionsAt("text #ar")).toEqual(["arch", "speed"]);
    expect(await completionsAt("plain prose")).toBeNull();
  });
});
