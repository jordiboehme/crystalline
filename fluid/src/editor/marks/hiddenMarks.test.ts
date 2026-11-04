/**
 * The hidden-mark model: which source ranges of a line are Markdown marks,
 * read off the syntax tree, in hidden mode and in show mode with the caret on
 * the line. The drawing is checked in `preview.test.ts`; here it is the data.
 */

import { afterEach, describe, expect, it } from "vitest";

import { cleanupMarkViews, markView, setCaret } from "../../test/markKit";
import type { Mark } from "./hiddenMarks";
import { marksOnLine, runAt, runsOnLine } from "./hiddenMarks";
import { parsedState } from "../../test/parse";

afterEach(cleanupMarkViews);

type Mode = "hidden" | "show";

/** A mark as `[from, to) kind side`, the way the cases below spell them. */
type Spelled = [number, number, Mark["kind"], Mark["side"]];

interface Case {
  name: string;
  doc: string;
  /** The line asked about and the caret's line (1-based). */
  line: number;
  /** The marks in hidden mode, and in show mode with the caret on the line. */
  hidden: Spelled[];
  show?: Spelled[];
}

const CASES: Case[] = [
  {
    name: "bold",
    doc: "a **bold** c",
    line: 1,
    hidden: [
      [2, 4, "strong", "open"],
      [8, 10, "strong", "close"],
    ],
  },
  {
    name: "a heading",
    doc: "## Title",
    line: 1,
    hidden: [[0, 3, "heading", "prefix"]],
  },
  { name: "a # alone", doc: "#", line: 1, hidden: [] },
  { name: "a bullet alone", doc: "-", line: 1, hidden: [] },
  { name: "a setext title", doc: "Title\n---", line: 1, hidden: [] },
  { name: "a setext underline", doc: "Title\n---", line: 2, hidden: [] },
  {
    name: "a link with a title",
    doc: '[t](https://x.y "T")',
    line: 1,
    hidden: [
      [0, 1, "link", "open"],
      [2, 20, "link", "close"],
    ],
  },
  { name: "footnotes", doc: "[^1] and [^1]: x", line: 1, hidden: [] },
  {
    name: "a reference link",
    doc: "[a][r]\n\n[r]: https://x",
    line: 1,
    hidden: [
      [0, 1, "link", "open"],
      [2, 6, "link", "close"],
    ],
  },
  {
    name: "a link definition",
    doc: "[a][r]\n\n[r]: https://x",
    line: 3,
    hidden: [],
  },
  {
    name: "a task",
    doc: "- [ ] task",
    line: 1,
    hidden: [
      [0, 2, "list", "prefix"],
      [2, 6, "task", "prefix"],
    ],
  },
  {
    name: "a nested quote",
    doc: "> > q",
    line: 1,
    hidden: [
      [0, 2, "quote", "prefix"],
      [2, 4, "quote", "prefix"],
    ],
  },
  {
    name: "inline code",
    doc: "`code`",
    line: 1,
    hidden: [
      [0, 1, "code", "open"],
      [5, 6, "code", "close"],
    ],
  },
  {
    name: "a fenced block",
    doc: "```md\n**b** [t](u) # x\n```",
    line: 2,
    hidden: [],
  },
  {
    name: "an observation category",
    doc: "- [decision] text #tag",
    line: 1,
    hidden: [
      [0, 2, "list", "prefix"],
      [2, 3, "category", "open"],
      [11, 12, "category", "close"],
    ],
  },
  {
    name: "an HTML comment",
    doc: "x <!-- c --> y",
    line: 1,
    hidden: [[2, 12, "comment", "whole"]],
    show: [],
  },
  {
    name: "an empty heading being given its words",
    doc: "## ",
    line: 1,
    hidden: [[0, 3, "heading", "prefix"]],
  },
  {
    name: "a code line of a fence in a callout",
    doc: "> [!NOTE]\n> ```\n> code\n> ```",
    line: 3,
    hidden: [[16, 18, "quote", "prefix"]],
  },
  {
    name: "the closing fence of a fence in a callout",
    doc: "> [!NOTE]\n> ```\n> code\n> ```",
    line: 4,
    hidden: [[23, 25, "quote", "prefix"]],
  },
];

function spelled(marks: Mark[]): Spelled[] {
  return marks.map((mark) => [mark.from, mark.to, mark.kind, mark.side]);
}

/** The case's document with CRLF line breaks, and the shift of each line. */
function crlf(doc: string): string {
  return doc.replaceAll("\n", "\r\n");
}

function check(c: Case, mode: Mode, shift: number, doc: string) {
  const view = markView(doc, { mode });
  const line = view.state.doc.line(c.line);
  setCaret(view, line.from);
  const expected =
    mode === "hidden" ? c.hidden : (c.show ?? ([] satisfies Spelled[]));
  expect(spelled(marksOnLine(parsedState(view.state), c.line))).toEqual(
    expected.map(([from, to, kind, side]) => [
      from + shift,
      to + shift,
      kind,
      side,
    ]),
  );
}

describe("marksOnLine", () => {
  for (const mode of ["hidden", "show"] as const) {
    for (const c of CASES) {
      it(`${c.name}, ${mode} mode`, () => {
        check(c, mode, 0, c.doc);
      });

      it(`${c.name}, ${mode} mode, CRLF`, () => {
        // One more position per line break before the line asked about.
        check(c, mode, c.line - 1, crlf(c.doc));
      });
    }
  }

  it("pairs the two marks of a bold word with each other", () => {
    const view = markView("a **bold** c");
    const marks = marksOnLine(parsedState(view.state), 1);
    expect(marks.map((mark) => mark.partner)).toEqual([1, 0]);
  });

  it("hides a comment in a Reflection in show mode too, with the caret on its line", () => {
    const view = markView("x <!-- c --> y", {
      mode: "show",
      adapter: "reflection",
    });
    setCaret(view, 1);
    expect(spelled(marksOnLine(parsedState(view.state), 1))).toEqual([
      [2, 12, "comment", "whole"],
    ]);
    expect(view.contentDOM.textContent).not.toContain("c -->");
    expect(view.contentDOM.textContent).toContain("x");
  });

  it("finds nothing in the source view", () => {
    const view = markView("a **bold** c", { raw: true });
    expect(marksOnLine(parsedState(view.state), 1)).toEqual([]);
  });

  it("keeps a mark of a pair on its own line", () => {
    const view = markView("a **b\nc** d");
    expect(spelled(marksOnLine(parsedState(view.state), 1))).toEqual([
      [2, 4, "strong", "open"],
    ]);
    expect(spelled(marksOnLine(parsedState(view.state), 2))).toEqual([
      [7, 9, "strong", "close"],
    ]);
    expect(marksOnLine(parsedState(view.state), 1)[0]?.partner).toBeNull();
  });

  it("takes an undefined [label] as text", () => {
    const view = markView("a [label] b");
    expect(marksOnLine(parsedState(view.state), 1)).toEqual([]);
  });

  it("folds an image's brackets and address, keeping its alt text", () => {
    const view = markView("![alt](p.png)");
    expect(spelled(marksOnLine(parsedState(view.state), 1))).toEqual([
      [0, 2, "link", "open"],
      [5, 13, "link", "close"],
    ]);
  });

  it("folds an autolink's angle brackets", () => {
    const view = markView("<https://x.y>");
    expect(spelled(marksOnLine(parsedState(view.state), 1))).toEqual([
      [0, 1, "link", "open"],
      [12, 13, "link", "close"],
    ]);
  });

  it("folds a heading's closing run", () => {
    const view = markView("## Title ##");
    expect(spelled(marksOnLine(parsedState(view.state), 1))).toEqual([
      [0, 3, "heading", "prefix"],
      [9, 11, "heading", "close"],
    ]);
  });

  it("folds strikethrough and the not-applicable box", () => {
    const view = markView("~~gone~~\n\n- [~] skipped");
    expect(spelled(marksOnLine(parsedState(view.state), 1))).toEqual([
      [0, 2, "strike", "open"],
      [6, 8, "strike", "close"],
    ]);
    expect(spelled(marksOnLine(parsedState(view.state), 3))).toEqual([
      [10, 12, "list", "prefix"],
      [12, 16, "task", "prefix"],
    ]);
  });

  it("leaves the frontmatter alone", () => {
    const view = markView("---\ntitle: **x**\n---\n\n**y**");
    expect(marksOnLine(parsedState(view.state), 2)).toEqual([]);
    expect(marksOnLine(parsedState(view.state), 5)).toHaveLength(2);
  });
});

describe("show mode, on a line the caret does not touch", () => {
  const UNTOUCHED: Case[] = [
    {
      name: "a quote",
      doc: "> q\nx",
      line: 1,
      hidden: [[0, 2, "quote", "prefix"]],
    },
    {
      name: "a bullet",
      doc: "- item\nx",
      line: 1,
      hidden: [[0, 2, "list", "prefix"]],
    },
    {
      name: "a code line of a fence in a callout",
      doc: "> [!NOTE]\n> ```\n> code\n> ```\nx",
      line: 3,
      hidden: [[16, 18, "quote", "prefix"]],
    },
  ];
  for (const c of UNTOUCHED) {
    it(`folds ${c.name}`, () => {
      const view = markView(c.doc, { mode: "show" });
      setCaret(view, view.state.doc.length);
      expect(spelled(marksOnLine(parsedState(view.state), c.line))).toEqual(
        c.hidden,
      );
    });
  }
});

describe("runsOnLine and runAt", () => {
  it("gives each hidden mark of a bold word its own run", () => {
    const view = markView("a **bold** c");
    expect(
      runsOnLine(parsedState(view.state), 1).map((run) => [run.from, run.to]),
    ).toEqual([
      [2, 4],
      [8, 10],
    ]);
  });

  it("joins touching marks into one run", () => {
    const view = markView("- [ ] task");
    const runs = runsOnLine(parsedState(view.state), 1);
    expect(runs.map((run) => [run.from, run.to, run.marks.length])).toEqual([
      [0, 6, 2],
    ]);
  });

  it("finds the run at either edge of it and none between runs", () => {
    const view = markView("a **bold** c");
    expect(runAt(parsedState(view.state), 2)?.from).toBe(2);
    expect(runAt(parsedState(view.state), 4)?.to).toBe(4);
    expect(runAt(parsedState(view.state), 6)).toBeNull();
    expect(runAt(parsedState(view.state), 10)?.from).toBe(8);
  });

  it("finds no run in show mode on the caret line", () => {
    const view = markView("a **bold** c", { mode: "show" });
    setCaret(view, 6);
    expect(runsOnLine(parsedState(view.state), 1)).toEqual([]);
    expect(runAt(parsedState(view.state), 2)).toBeNull();
  });
});
