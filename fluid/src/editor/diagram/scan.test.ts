import { describe, expect, test } from "vitest";

import { defined } from "../../test/assert";
import {
  type Scan,
  type ScannedLine,
  hasStrayComment,
  isDirectionLine,
  scanFence,
} from "./scan";
import { flowDbOf } from "./mermaidDb";

const FIXTURES = import.meta.glob<string>(
  [
    "../fixtures/diagrams/flowchart/*.mmd",
    "../fixtures/diagrams/fallback/*.mmd",
  ],
  { query: "?raw", import: "default", eager: true },
);

/**
 * A fixture as a fence body: the files end in a line break, and a body read
 * from a fence (`fenceBody`) does not.
 */
function fixture(prefix: string): string {
  const path = defined(
    Object.keys(FIXTURES).find((key) =>
      key.slice(key.lastIndexOf("/") + 1).startsWith(`${prefix}-`),
    ),
    `fixture ${prefix}`,
  );
  return defined(FIXTURES[path]).replace(/\n$/, "");
}

function kinds(scan: Scan): string[] {
  return scan.lines.map((line) => line.kind);
}

function texts(body: string, line: ScannedLine): string[] {
  return line.statements.map((span) => body.slice(span.from, span.to));
}

function line(scan: Scan, index: number): ScannedLine {
  return defined(scan.lines[index], `line ${index}`);
}

describe("scanFence lines", () => {
  test("f03 classifies its frontmatter, directive and comment lines", () => {
    const body = fixture("f03");
    const scan = scanFence(body);
    expect(kinds(scan)).toEqual([
      "frontmatter",
      "frontmatter",
      "frontmatter",
      "directive",
      "header",
      "comment",
      "statement",
    ]);
    expect(scan.header).toBe(scan.lines[4]);
    expect(scan.keyword).toBe("flowchart");
    expect(scan.understood).toBe(true);
    expect(scan.reasons).toEqual([]);
  });

  test("line offsets cover the body and exclude the line break", () => {
    const body = fixture("f03");
    const scan = scanFence(body);
    expect(scan.lines.map((l) => body.slice(l.from, l.to))).toEqual(
      body.split("\n"),
    );
    scan.lines.forEach((l, i) => expect(l.index).toBe(i));
  });

  test("a trailing line break gives a last blank line", () => {
    const scan = scanFence("flowchart TD\n    A --> B\n");
    expect(kinds(scan)).toEqual(["header", "statement", "blank"]);
    expect(line(scan, 2)).toMatchObject({ from: 25, to: 25, statements: [] });
  });

  test("the header's keyword is its first word", () => {
    expect(scanFence("graph TD;\n  A-->B").keyword).toBe("graph");
    expect(scanFence("  flowchart-elk LR\n  A-->B").keyword).toBe(
      "flowchart-elk",
    );
    expect(scanFence("sequenceDiagram\n  A->>B: hi").keyword).toBe(
      "sequenceDiagram",
    );
  });

  test("f02's header keeps its semicolon out of its statement", () => {
    const body = fixture("f02");
    const scan = scanFence(body);
    const header = defined(scan.header ?? undefined);
    expect(texts(body, header)).toEqual(["graph TD"]);
    expect(texts(body, line(scan, 2))).toEqual(["A[Open file]-->B{Changed?}"]);
  });

  test("a body with no header is not understood", () => {
    const scan = scanFence("%% only a comment\n");
    expect(scan.header).toBeNull();
    expect(scan.keyword).toBeNull();
    expect(scan.understood).toBe(false);
    expect(scan.reasons).toHaveLength(1);
  });

  test("f14's indent of a tab-indented line is a tab", () => {
    const scan = scanFence(fixture("f14"));
    expect(line(scan, 0).indent).toBe("  ");
    expect(line(scan, 1).indent).toBe("  \t");
    expect(line(scan, 2).indent).toBe("\t");
    expect(line(scan, 3).indent).toBe("\t");
    expect(kinds(scan)).toEqual([
      "header",
      "comment",
      "statement",
      "statement",
      "statement",
    ]);
  });

  test("f14's spans are trimmed of outer whitespace only", () => {
    const body = fixture("f14");
    const scan = scanFence(body);
    expect(texts(body, line(scan, 2))).toEqual(["A[Start]  -->  B[Middle]"]);
    expect(texts(body, line(scan, 3))).toEqual(["B\t-->\tC[End]"]);
    expect(texts(body, line(scan, 4))).toEqual(["C  -.->  A"]);
  });
});

describe("scanFence comments", () => {
  test("a whole line of %% and text is a comment, indented or not", () => {
    const scan = scanFence(
      "flowchart TD\n%% top\n    %% peer note\n\t%%x\n    A --> B",
    );
    expect(kinds(scan)).toEqual([
      "header",
      "comment",
      "comment",
      "comment",
      "statement",
    ]);
    expect(line(scan, 1).statements).toEqual([]);
  });

  test("a %% with a single space after it is still a comment", () => {
    expect(line(scanFence("flowchart TD\n%% "), 1).kind).toBe("comment");
  });

  test("a bare %% line is a statement with a stray comment mark", () => {
    const body = "flowchart TD\n    %%\n    A --> B";
    const scan = scanFence(body);
    expect(line(scan, 1).kind).toBe("statement");
    expect(hasStrayComment(body, line(scan, 1))).toBe(true);
    expect(hasStrayComment(body, line(scan, 2))).toBe(false);
  });

  test("a trailing %% after a statement is a stray comment mark", () => {
    const body = "flowchart TD\n    A --> B %% note";
    const scan = scanFence(body);
    expect(line(scan, 1).kind).toBe("statement");
    expect(hasStrayComment(body, line(scan, 1))).toBe(true);
  });

  test("a %% inside a label is not a stray comment mark", () => {
    const body = 'flowchart TD\n    A["50%% off"] --> B';
    expect(hasStrayComment(body, line(scanFence(body), 1))).toBe(false);
  });

  test("a comment line is never a stray comment mark", () => {
    const body = "flowchart TD\n    %% fine";
    expect(hasStrayComment(body, line(scanFence(body), 1))).toBe(false);
  });

  test("a %% line inside an accDescr block is a comment", () => {
    const scan = scanFence(
      "flowchart TD\n  accDescr {\n  %% hidden\n  text\n  }\n  A-->B",
    );
    expect(kinds(scan)).toEqual([
      "header",
      "acc",
      "comment",
      "acc",
      "acc",
      "statement",
    ]);
  });
});

describe("scanFence frontmatter, directives and acc", () => {
  test("frontmatter counts only on the first line", () => {
    const scan = scanFence("flowchart TD\n---\n  A-->B");
    expect(line(scan, 1).kind).toBe("statement");
  });

  test("frontmatter may carry trailing spaces and an indent", () => {
    const scan = scanFence("  ---  \n  title: x\n  ---\nflowchart TD");
    expect(kinds(scan)).toEqual([
      "frontmatter",
      "frontmatter",
      "frontmatter",
      "header",
    ]);
    expect(scan.understood).toBe(true);
  });

  test("an unclosed frontmatter is not understood", () => {
    const scan = scanFence("---\ntitle: x\nflowchart TD\n  A-->B");
    expect(scan.understood).toBe(false);
    expect(scan.reasons.join(" ")).toMatch(/frontmatter/);
  });

  test("a directive may run over several lines", () => {
    const scan = scanFence(
      '%%{init: {\n  "theme": "dark"\n}}%%\nflowchart TD\n  A-->B',
    );
    expect(kinds(scan)).toEqual([
      "directive",
      "directive",
      "directive",
      "header",
      "statement",
    ]);
    expect(scan.understood).toBe(true);
  });

  test("an unclosed directive is not understood", () => {
    const scan = scanFence('%%{init: {"theme": "dark"}\nflowchart TD');
    expect(scan.understood).toBe(false);
    expect(scan.reasons.join(" ")).toMatch(/directive/);
  });

  test("text after a directive's close is not understood", () => {
    const scan = scanFence("%%{init: {}}%% flowchart TD\n  A-->B");
    expect(scan.understood).toBe(false);
  });

  test("f20's accTitle and accDescr block are acc lines", () => {
    const scan = scanFence(fixture("f20"));
    expect(kinds(scan)).toEqual([
      "header",
      "comment",
      "acc",
      "acc",
      "acc",
      "acc",
      "acc",
      "statement",
    ]);
    expect(scan.understood).toBe(true);
    expect(line(scan, 2).statements).toEqual([]);
  });

  test("accDescr with a colon and a one-line block are single acc lines", () => {
    const scan = scanFence(
      "flowchart TD\n  accDescr: one line\n  accDescr { short }\n  A-->B",
    );
    expect(kinds(scan)).toEqual(["header", "acc", "acc", "statement"]);
    expect(scan.understood).toBe(true);
  });

  test("an unclosed accDescr block is not understood", () => {
    const scan = scanFence("flowchart TD\n  accDescr {\n  open\n  A-->B");
    expect(scan.understood).toBe(false);
    expect(scan.reasons.join(" ")).toMatch(/accDescr/);
  });
});

describe("scanFence statements", () => {
  test("f23's line has three statement spans", () => {
    const body = fixture("f23");
    const scan = scanFence(body);
    expect(texts(body, line(scan, 2))).toEqual([
      "A --> B",
      "B --> C",
      "C --> D",
    ]);
  });

  test("a ; inside brackets, quotes or pipes does not split", () => {
    const body =
      'flowchart TD\n  A["a;b"] --> B\n  B -->|x;y| C\n  C[p;q] --> D\n  D --> E{"r;s"}';
    const scan = scanFence(body);
    expect(texts(body, line(scan, 1))).toEqual(['A["a;b"] --> B']);
    expect(texts(body, line(scan, 2))).toEqual(["B -->|x;y| C"]);
    expect(texts(body, line(scan, 3))).toEqual(["C[p;q] --> D"]);
    expect(texts(body, line(scan, 4))).toEqual(['D --> E{"r;s"}']);
  });

  test("a ; inside backticks or an entity does not split", () => {
    const body = "flowchart TD\n  A -- a #59; b --> B\n  C --> D[`m;d`]";
    const scan = scanFence(body);
    expect(texts(body, line(scan, 1))).toEqual(["A -- a #59; b --> B"]);
    expect(texts(body, line(scan, 2))).toEqual(["C --> D[`m;d`]"]);
  });

  test("empty statements leave no span", () => {
    const body = "flowchart TD\n  A-->B;; ;C-->D;";
    expect(texts(body, line(scanFence(body), 1))).toEqual(["A-->B", "C-->D"]);
  });

  test("a style line's last ; is not a split, as mermaid drops it", () => {
    const body =
      "flowchart TD\n  style A fill:#f9f;\n  style A fill:#f9f; B-->C";
    const scan = scanFence(body);
    expect(texts(body, line(scan, 1))).toEqual(["style A fill:#f9f"]);
    expect(texts(body, line(scan, 2))).toEqual(["style A fill:#f9f; B-->C"]);
  });

  test("a direction line is one statement and is flagged", () => {
    const body =
      'flowchart TD\n  A["go direction LR now; ok"] --> B; C --> D\n  A --> B';
    const scan = scanFence(body);
    expect(texts(body, line(scan, 1))).toEqual([
      'A["go direction LR now; ok"] --> B; C --> D',
    ]);
    expect(isDirectionLine(body, line(scan, 1))).toBe(true);
    expect(isDirectionLine(body, line(scan, 2))).toBe(false);
  });

  test("a plain direction statement is a direction line", () => {
    const body = "flowchart TD\n  subgraph s\n    direction LR\n  end";
    expect(isDirectionLine(body, line(scanFence(body), 2))).toBe(true);
  });

  test("the direction rule applies to flowcharts only", () => {
    const body = "sequenceDiagram\n  A->>B: direction LR; ok";
    expect(texts(body, line(scanFence(body), 1))).toEqual([
      "A->>B: direction LR",
      "ok",
    ]);
  });

  test("every corpus file scans as understood", () => {
    for (const [path, text] of Object.entries(FIXTURES)) {
      const scan = scanFence(text.replace(/\n$/, ""));
      expect(scan.understood, path).toBe(true);
      expect(scan.keyword, path).toMatch(/^(flowchart|graph)$/);
    }
  });
});

// Review round 1. Each case is checked against mermaid 12.2.0's own parse
// (the real parser, as in `mermaidDb.test.ts`), so the scan says what
// mermaid does, not what the rule's prose says.
describe("scanFence against mermaid 12.2", () => {
  /** The edges mermaid's db holds for `body`, or "error". */
  async function edgesOf(body: string): Promise<string[] | "error"> {
    const db = await flowDbOf(body, false);
    return "error" in db ? "error" : db.edges.map((e) => `${e.start}>${e.end}`);
  }

  test("a frontmatter block cannot close on its second line", async () => {
    const tooSoon = "---\n---\nflowchart TD\n  A-->B";
    expect(await edgesOf(tooSoon)).toBe("error");
    const scan = scanFence(tooSoon);
    expect(line(scan, 0).kind).toBe("frontmatter");
    expect(line(scan, 1).kind).not.toBe("frontmatter");
    expect(scan.understood).toBe(false);

    const empty = "---\n\n---\nflowchart TD\n  A-->B";
    expect(await edgesOf(empty)).toEqual(["A>B"]);
    const ok = scanFence(empty);
    expect(kinds(ok)).toEqual([
      "frontmatter",
      "frontmatter",
      "frontmatter",
      "header",
      "statement",
    ]);
    expect(ok.understood).toBe(true);
  });

  test("text after an accDescr block's close is scanned as statements", async () => {
    for (const body of [
      "flowchart TD\n  accDescr {\n  x\n  } A-->B; C-->D",
      "flowchart TD\n  accDescr { x } A-->B; C-->D",
    ]) {
      expect(await edgesOf(body)).toEqual(["A>B", "C>D"]);
      const scan = scanFence(body);
      const closing = defined(scan.lines.at(-1));
      expect(closing.kind).toBe("acc");
      expect(texts(body, closing)).toEqual(["A-->B", "C-->D"]);
      expect(scan.understood).toBe(true);
    }
  });

  test("the predicates read the statements after an accDescr block", () => {
    const body =
      "flowchart TD\n  accDescr { direction LR }\n  accDescr { x } A --> B %% n\n  accDescr { y } C --> D direction LR";
    const scan = scanFence(body);
    expect(isDirectionLine(body, line(scan, 1))).toBe(false);
    expect(hasStrayComment(body, line(scan, 1))).toBe(false);
    expect(hasStrayComment(body, line(scan, 2))).toBe(true);
    expect(isDirectionLine(body, line(scan, 3))).toBe(true);
    expect(texts(body, line(scan, 3))).toEqual(["C --> D direction LR"]);
  });

  test("an accDescr block's close with nothing after it has no statements", async () => {
    const body = "flowchart TD\n  accDescr { x }   \n  C-->D";
    expect(await edgesOf(body)).toEqual(["C>D"]);
    expect(line(scanFence(body), 1).statements).toEqual([]);
  });

  test("a direction runs over blank, comment and directive lines", async () => {
    // The run-on direction token swallows the lines up to its direction,
    // so their nodes and edges vanish from mermaid's db.
    for (const [body, edges] of [
      ["flowchart TD\n  A[direction\n\n  LR] --> B\n  C --> D", ["C>D"]],
      [
        "flowchart TD\n  A[direction\n  %% c\n\n  LR] --> B\n  C --> D",
        ["C>D"],
      ],
      [
        "flowchart TD\n  A --> B\n  C[direction\n  %%{init: {}}%%\n  LR] --> D",
        ["A>B"],
      ],
    ] as const) {
      expect(await edgesOf(body), body).toEqual(edges);
      const scan = scanFence(body);
      expect(scan.understood, body).toBe(false);
      expect(scan.reasons.join(" "), body).toMatch(/direction/);
    }
  });

  test("a direction word with no direction after it is left to the parser", () => {
    const scan = scanFence("flowchart TD\n  A[direction\n\n  E --> F");
    expect(scan.reasons.join(" ")).not.toMatch(/direction/);
  });

  test("a %%{ line is a directive only with a word after the brace", async () => {
    for (const body of [
      "%%{ x }%%\nflowchart TD\n  A-->B",
      "%%{ 1 }%%\nflowchart TD\n  A-->B",
      "%%{\n  init: {}\n}%%\nflowchart TD\n  A-->B",
    ]) {
      expect(await edgesOf(body), body).toEqual(["A>B"]);
      const scan = scanFence(body);
      expect(scan.understood, body).toBe(true);
      expect(defined(scan.header ?? undefined).index, body).toBe(
        body.split("\n").indexOf("flowchart TD"),
      );
    }
    for (const body of [
      "%%{ }%%\nflowchart TD\n  A-->B",
      "%%{}%%\nflowchart TD\n  A-->B",
      "flowchart TD\n  %%{ }%%\n  A-->B",
    ]) {
      expect(await edgesOf(body), body).toBe("error");
      const scan = scanFence(body);
      const braces = defined(
        scan.lines.find((l) => body.slice(l.from, l.to).includes("%%{")),
      );
      expect(["header", "statement"], body).toContain(braces.kind);
      expect(hasStrayComment(body, braces), body).toBe(true);
    }
  });
});
