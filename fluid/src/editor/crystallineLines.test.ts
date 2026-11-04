import { CompletionContext } from "@codemirror/autocomplete";
import { EditorSelection, EditorState } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { afterEach, describe, expect, it } from "vitest";

import type { Vocabulary } from "../api/vocabulary";
import {
  bytes,
  caret,
  cleanupMarkViews,
  keys,
  markView,
  setCaret,
  typeText,
} from "../test/markKit";
import { parsedState } from "../test/parse";
import {
  crystallineCompletions,
  crystallineLines,
  observationCategory,
} from "./crystallineLines";
import { marksOnLine } from "./marks/hiddenMarks";
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

  it("draws no category for a bullet the server's literal '- ' prefix would reject, but reads a spaced relation", () => {
    const doc =
      "---\nt: x\n---\n\n-  [decision] two spaces\n-\t[decision] a tab\n-  relates_to [[Beta]]\n";
    const view = mount(doc);
    expect(view.dom.querySelector(".cm-obs-category")).toBeNull();
    // `parse_relation` trims the words before the target, so the second
    // space is no obstacle to a relation, as it is to a category.
    expect(
      [...view.dom.querySelectorAll(".cm-rel-type")].map(
        (el) => el.textContent,
      ),
    ).toEqual(["relates_to"]);
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

describe("task lines", () => {
  it("never reads a task box as an observation category", () => {
    expect(observationCategory("- [x] done ")).toBeNull();
    expect(observationCategory("- [X] done ")).toBeNull();
    expect(observationCategory("- [~] skipped ")).toBeNull();
    expect(observationCategory("- [ ] open ")).toBeNull();
    expect(observationCategory("- [fact] x ")).not.toBeNull();
    expect(observationCategory("- [xy] x ")).not.toBeNull();
  });

  it("never reads a footnote reference as an observation category", () => {
    expect(observationCategory("- [^1] the source")).toBeNull();
    expect(observationCategory("- [^note] more")).toBeNull();
    expect(observationCategory("- [a^b] x")).toEqual({ from: 2, to: 7 });
  });
});

describe("chips in hidden mode", () => {
  const LINE = "- [decision] text #tag";

  afterEach(cleanupMarkViews);

  it("draws the category word as a chip, with no bracket", () => {
    const view = markView(LINE);
    setCaret(view, LINE.length);
    expect(view.dom.querySelector(".cm-category-chip")?.textContent).toBe(
      "decision",
    );
    expect(view.dom.querySelector(".cm-obs-category")).toBeNull();
    expect(view.dom.textContent).not.toContain("[");
    expect(view.dom.querySelector(".cm-tag-chip")?.textContent).toBe("#tag");
    expect(view.dom.querySelector(".cm-line-tag")).toBeNull();
    expect(bytes(view.state)).toBe(LINE);
  });

  it("types after the drawn category word into the word", () => {
    const view = markView(LINE);
    setCaret(view, 11);
    expect(caret(view)).toBe(11);
    typeText(view, "s");
    expect(bytes(view.state)).toBe("- [decisions] text #tag");
    expect(view.dom.textContent).not.toContain("[");
  });

  it("puts Home inside the category and types there", () => {
    const view = markView(LINE);
    setCaret(view, 14);
    keys(view, "Home");
    expect(caret(view)).toBe(3);
    typeText(view, "x");
    expect(bytes(view.state)).toBe("- [xdecision] text #tag");
    expect(view.dom.textContent).not.toContain("[");
    expect(view.dom.querySelector(".cm-category-chip")?.textContent).toBe(
      "xdecision",
    );
  });

  it("draws the relation type as a chip", () => {
    const view = markView("- relates_to [[T]]");
    setCaret(view, 0);
    expect(view.dom.querySelector(".cm-relation-chip")?.textContent).toBe(
      "relates_to",
    );
    expect(view.dom.querySelector(".cm-rel-type")).toBeNull();
  });

  it("keeps the line's bytes", () => {
    const doc = "- [decision] text #a #b\n- relates_to [[T]]\n";
    const view = markView(doc);
    expect(bytes(view.state)).toBe(doc);
    expect(
      [...view.dom.querySelectorAll(".cm-tag-chip")].map((e) => e.textContent),
    ).toEqual(["#a", "#b"]);
  });

  it("keeps today's marking in show mode on the line the caret touches", () => {
    const view = markView(LINE, { mode: "show" });
    setCaret(view, 5);
    expect(view.dom.querySelector(".cm-obs-category")?.textContent).toBe(
      "[decision]",
    );
    expect(view.dom.querySelector(".cm-category-chip")).toBeNull();
    expect(view.dom.querySelector(".cm-line-tag")?.textContent).toBe("#tag");
  });

  it("draws the chips in show mode on a line the caret is not on", () => {
    const doc = `${LINE}\nother`;
    const view = markView(doc, { mode: "show" });
    setCaret(view, doc.length);
    expect(view.dom.querySelector(".cm-category-chip")?.textContent).toBe(
      "decision",
    );
    setCaret(view, 3);
    expect(view.dom.querySelector(".cm-category-chip")).toBeNull();
    expect(view.dom.querySelector(".cm-obs-category")).not.toBeNull();
  });

  it("shows a comment in an engram in show mode with the caret on the line", () => {
    const doc = "x <!-- c --> y";
    const view = markView(doc, { mode: "show" });
    setCaret(view, 1);
    expect(view.dom.textContent).toContain("<!-- c -->");
  });
});

describe("tag chips follow the tags the server reads", () => {
  afterEach(cleanupMarkViews);

  const tagsOf = (doc: string): string[] => {
    const view = markView(doc);
    setCaret(view, doc.length);
    return [...view.dom.querySelectorAll(".cm-tag-chip")].map(
      (el) => el.textContent ?? "",
    );
  };

  it.each([
    ["- [fact] use #include here", []],
    ["- [fact] see issue#12", []],
    ["- [fact] a #x then text", []],
    ["- [fact] text #area/sub", ["#area/sub"]],
    ["- [fact] text #a #b", ["#a", "#b"]],
    ["- [fact] text #tag (the context)", ["#tag"]],
    ["- [fact] text (the context) #tag", ["#tag"]],
    ["- [fact] text #tag.", []],
    ["- [fact] text #tag-1_x ", ["#tag-1_x"]],
    ["- [fact] #only", ["#only"]],
  ])("%s", (doc, expected) => {
    expect(tagsOf(doc)).toEqual(expected);
  });
});

describe("relation chips follow the relations the server reads", () => {
  afterEach(cleanupMarkViews);

  const chipOf = (doc: string): string | null => {
    const view = markView(doc);
    setCaret(view, doc.length);
    return view.dom.querySelector(".cm-relation-chip")?.textContent ?? null;
  };

  it.each([
    ["- relates_to [[T]]", "relates_to"],
    ["- rel[[T]]", "rel"],
    ['- "part of" [[T]]', '"part of"'],
    ["- See [[T]]", "See"],
    ["- part_of [Doc](doc.md)", "part_of"],
    ["- part_of [Doc](sub/Doc.MD#top)", "part_of"],
    ['- "part of" [Doc](doc.md)', '"part of"'],
    ["- rel [Doc](a[[x]]b.md)", "rel"],
    ["- relates_to\n  [[T]]", "relates_to"],
    ["- part_of\n  [Doc](doc.md)", "part_of"],
    // Rust's char::is_lowercase is the Unicode Lowercase property.
    ["- ªrel [Doc](doc.md)", "ªrel"],
    // Offsets are UTF-16 throughout: an astral character before a mask.
    ["- rel [\u{1F600} Doc](a[[x]]b.md)", "rel"],
    // Not relations to the server.
    ["- See [Three](chapter-3.md)", null],
    ["- uses [x](page.html)", null],
    ["- uses ![x](a.md)", null],
    ["- uses [x](http://a.org/b.md)", null],
    ["- uses [x](index.md)", null],
    ["- uses [x](sub/log.md)", null],
    ["- uses [[index.md]]", null],
    ["- two words [[T]]", null],
    ["- :x [[T]]", null],
    ["- [x] done [[T]]", null],
    ["- uses [[|alias]]", null],
    ["- [fact] see [[T]]", null],
    ["- uses `[x](a.md)`", null],
    ["- relates_to [[T]\n", null],
    ["- rel[\u{1F600}]([[x]]b.html)", null],
    ["  - relates_to [[T]]", null],
  ])("%j", (doc, expected) => {
    expect(chipOf(doc)).toBe(expected);
  });
});

describe("tag chips on a wrapped observation", () => {
  afterEach(cleanupMarkViews);

  const chips = (doc: string): string[] => {
    const view = markView(doc);
    setCaret(view, doc.length);
    return [...view.dom.querySelectorAll(".cm-tag-chip")].map(
      (el) => el.textContent ?? "",
    );
  };

  it.each([
    // The server reads the tags from the joined text: only the last run.
    ["- [fact] a #x\n  b #y", ["#y"]],
    ["- [fact] a\n  #x #y", ["#x", "#y"]],
    ["- [fact] a #x\n  b #y\n  c", []],
    ["- [fact] a\n  b #y (ctx)", ["#y"]],
    ["- [fact] a #x\n\n  #y", ["#x"]],
  ])("%j", (doc, expected) => {
    expect(chips(doc)).toEqual(expected);
  });
});

/**
 * The category as the server's `split_category` (crates/core/src/parse.rs)
 * reads it, on a zero-indent `- ` bullet: the nine rows of the final review's
 * table (Must-cover 4). The bracket span is in line offsets, or null.
 */
describe("the category, as the server reads it (M1)", () => {
  afterEach(cleanupMarkViews);

  const ROWS: [string, string, { from: number; to: number } | null][] = [
    ["a category and text", "- [decision] text", { from: 2, to: 12 }],
    ["a category with a space", "- [a b] x", { from: 2, to: 7 }],
    ["a category before a tab", "- [a]\tx", { from: 2, to: 5 }],
    ["a done box before text", "- [x]text", null],
    ["an open box", "- [ ] x", null],
    ["a footnote reference", "- [^1] x", null],
    ["a category right before text", "- [decision]text", { from: 2, to: 12 }],
    ["a category that starts with a space", "- [ a] x", { from: 2, to: 6 }],
    ["a category at the line end", "- [a]", { from: 2, to: 5 }],
    [
      "a category before a wrapped line",
      "- [decision]\n  wraps",
      { from: 2, to: 12 },
    ],
    ["a category before a colon", "- [a]: x", { from: 2, to: 5 }],
    ["a bracket inside the category", "- [a[b] x", null],
  ];

  it.each(ROWS)("%s", (_name, doc, span) => {
    const line = doc.split("\n")[0] ?? "";
    expect(observationCategory(line)).toEqual(span);
    const view = markView(`x\n${doc}`);
    const categories = marksOnLine(parsedState(view.state), 2).filter(
      (mark) => mark.kind === "category",
    );
    expect(categories.map((mark) => [mark.from - 2, mark.to - 2])).toEqual(
      span
        ? [
            [span.from, span.from + 1],
            [span.to - 1, span.to],
          ]
        : [],
    );
    const chip = view.contentDOM.querySelector(".cm-category-chip");
    expect(chip?.textContent ?? null).toBe(
      span ? line.slice(span.from + 1, span.to - 1) : null,
    );
  });
});
