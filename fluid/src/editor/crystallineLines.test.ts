import { CompletionContext } from "@codemirror/autocomplete";
import { EditorSelection, EditorState } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { describe, expect, it } from "vitest";

import type { Vocabulary } from "../api/vocabulary";
import { parsedState } from "../test/parse";
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
  types: [],
  statuses: [],
};

/**
 * A mounted editor over `doc`.
 *
 * The state goes through `parsedState` first: these marks are read off the
 * syntax tree, and a new state's first parse is cut off after 20ms of wall
 * clock, which on a loaded machine leaves a fence unclosed and the lines
 * inside it decorated as if they were prose.
 */
function mount(doc: string): EditorView {
  return new EditorView({
    state: parsedState(
      EditorState.create({
        doc,
        selection: EditorSelection.cursor(0),
        extensions: [...baseExtensions(false), crystallineLines()],
      }),
    ),
    parent: document.body,
  });
}

describe("line affordances", () => {
  it("marks the category, the rel type and the tags", () => {
    const view = mount(DOC);
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

describe("grammar fidelity", () => {
  it("draws nothing for an observation or relation line inside a fence", () => {
    const doc =
      "---\nt: x\n---\n\n- [decision] outside the fence #tag\n\n```\n- [decision] inside the fence #tag\n- relates_to [[Beta]]\n```\n";
    const view = mount(doc);
    const categories = [...view.dom.querySelectorAll(".cm-obs-category")].map(
      (el) => el.textContent,
    );
    expect(categories).toEqual(["[decision]"]);
    expect(view.dom.querySelector(".cm-rel-type")).toBeNull();
    // The fenced line's own tag is left alone too - it belongs to the whole
    // skipped line, not to a reference somebody wrote about one.
    expect(
      [...view.dom.querySelectorAll(".cm-line-tag")].map(
        (el) => el.textContent,
      ),
    ).toEqual(["#tag"]);
    view.destroy();
  });

  it("does not draw for a bullet the server's literal '- ' prefix would reject", () => {
    const doc =
      "---\nt: x\n---\n\n-  [decision] two spaces\n-\t[decision] a tab\n-  relates_to [[Beta]]\n";
    const view = mount(doc);
    expect(view.dom.querySelector(".cm-obs-category")).toBeNull();
    expect(view.dom.querySelector(".cm-rel-type")).toBeNull();
    view.destroy();
  });

  it("recognizes a quoted relation type and a mixed-case bare one", () => {
    const doc =
      '---\nt: x\n---\n\n- "relates to" [[Beta]]\n- SupersedesV2 [[Gamma]]\n';
    const view = mount(doc);
    const relTypes = [...view.dom.querySelectorAll(".cm-rel-type")].map(
      (el) => el.textContent,
    );
    expect(relTypes).toEqual(['"relates to"', "SupersedesV2"]);
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

/** Ask with the markdown language mounted, so the syntax tree is real. */
async function completionsInParsed(
  doc: string,
  at: number,
): Promise<string[] | null> {
  const state = parsedState(
    EditorState.create({
      doc,
      selection: EditorSelection.cursor(at),
      extensions: baseExtensions(false),
    }),
  );
  const result = await crystallineCompletions(() => VOCAB)(
    new CompletionContext(state, at, false),
  );
  return result?.options.map((option) => option.label) ?? null;
}

describe("completion stays out of code and frontmatter", () => {
  it("offers nothing for a YAML bullet inside a fenced code block", async () => {
    const doc = "```yaml\n- rel\n```\n";
    expect(await completionsInParsed(doc, doc.indexOf("rel") + 3)).toBeNull();
  });

  it("offers nothing for a comment tag inside a fenced code block", async () => {
    const doc = "```sh\n# ar\n```\n";
    expect(await completionsInParsed(doc, doc.indexOf("ar") + 2)).toBeNull();
  });

  it("offers nothing for a bullet inside the frontmatter block", async () => {
    const doc = "---\ntags:\n- e\n---\n\nprose\n";
    expect(await completionsInParsed(doc, doc.indexOf("- e") + 3)).toBeNull();
  });

  it("still completes a relation line in prose below the frontmatter", async () => {
    const doc = "---\nt: x\n---\n\n- rel";
    expect(await completionsInParsed(doc, doc.length)).toEqual([
      "relates_to",
      "supersedes",
    ]);
  });
});
