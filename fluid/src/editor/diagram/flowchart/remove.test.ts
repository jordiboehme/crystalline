// Real mermaid wherever a test reads a db: a removal is the riskiest op, because
// a wrong one silently loses what a person wrote, and only the db shows it.

import { EditorState } from "@codemirror/state";
import { describe, expect, test } from "vitest";

import { flowDbFor } from "../../../test/mermaidOracle";
import { docText, lineSeparatorFor } from "../../setup";
import { applyEdits, toChanges } from "../edits";
import { agreesFlow } from "../verdict";
import { parseFlowchart } from "./parse";
import { type FlowOp, checked, compileFlowOp } from "./ops";

const FIXTURES = import.meta.glob<string>(
  "../../fixtures/diagrams/flowchart/*.mmd",
  { query: "?raw", import: "default", eager: true },
);

function fixture(prefix: string): string {
  const path = Object.keys(FIXTURES).find((p) => p.includes(`/${prefix}-`));
  if (path === undefined) {
    throw new Error(`no fixture ${prefix}`);
  }
  return (FIXTURES[path] ?? "").replace(/\n$/, "");
}

function run(body: string, op: FlowOp): string {
  const result = compileFlowOp(parseFlowchart(body), op);
  if ("refused" in result) {
    throw new Error(`refused: ${result.refused}`);
  }
  return applyEdits(body, result.edits);
}

function refusal(body: string, op: FlowOp): string | null {
  const result = compileFlowOp(parseFlowchart(body), op);
  return "refused" in result ? result.refused : null;
}

function edge(start: string, end: string, occurrence = 0): FlowOp {
  return { op: "deleteEdge", edge: { start, end, occurrence } };
}

function box(id: string): FlowOp {
  return { op: "deleteNode", id };
}

/** The body saved through a real state, with what `toChanges` makes of the edits. */
function saved(doc: string, op: FlowOp): string {
  const state = EditorState.create({ doc, extensions: lineSeparatorFor(doc) });
  const body = state.doc.sliceString(0, state.doc.length);
  const result = compileFlowOp(parseFlowchart(body), op);
  if ("refused" in result) {
    throw new Error(`refused: ${result.refused}`);
  }
  return docText(
    state.update({ changes: toChanges(0, result.edits, state.lineBreak) })
      .state,
  );
}

describe("deleteEdge", () => {
  test.each<[string, string, FlowOp, string]>([
    [
      "both ends needed: a cut",
      "flowchart TD\n    A[Start] --> B[End]",
      edge("A", "B"),
      "flowchart TD\n    A[Start]\n    B[End]",
    ],
    [
      "only the left needed",
      "graph TD\n  A --> B\n  B --> C",
      edge("A", "B"),
      "graph TD\n  A\n  B --> C",
    ],
    [
      "only the right needed",
      "graph TD\n  B --> C\n  A --> C\n  A --> D",
      edge("A", "C"),
      "graph TD\n  B --> C\n  A --> D",
    ],
    [
      "only the right ref is the box's last",
      "graph TD\n  A --> B\n  A --> C",
      edge("A", "C"),
      "graph TD\n  A --> B\n  C",
    ],
    [
      "neither needed: the whole line with its break",
      "graph TD\n  A --> B\n  A --> C\n  B --> C",
      edge("A", "B"),
      "graph TD\n  A --> C\n  B --> C",
    ],
    [
      "the middle of a chain: both halves keep their bytes",
      "graph TD\n  A --> B -->  C --> D",
      edge("B", "C"),
      "graph TD\n  A --> B\n  C --> D",
    ],
    [
      "the first link of a chain, its left ref not needed",
      "graph TD\n  E --> A\n  A --> B --> C",
      edge("A", "B"),
      "graph TD\n  E --> A\n  B --> C",
    ],
    [
      "the first link of a chain, its left ref needed",
      "graph TD\n  A --> B --> C",
      edge("A", "B"),
      "graph TD\n  A\n  B --> C",
    ],
    [
      "the last link of a chain, its right ref not needed",
      "graph TD\n  A --> B --> C\n  C --> D",
      edge("B", "C"),
      "graph TD\n  A --> B\n  C --> D",
    ],
    [
      "the last link of a chain, its right ref needed",
      "graph TD\n  A --> B --> C",
      edge("B", "C"),
      "graph TD\n  A --> B\n  C",
    ],
    [
      "a group elsewhere in the chain stays",
      "graph TD\n  A & B --> C --> D\n  D --> E",
      edge("C", "D"),
      "graph TD\n  A & B --> C\n  D --> E",
    ],
    [
      "a second statement on the line stays",
      "graph TD\n  A --> B; A --> C; B --> C",
      edge("A", "B"),
      "graph TD\n  A --> C; B --> C",
    ],
    [
      "the last statement of a line goes with the ; before it",
      "graph TD\n  A --> B; A --> C; B --> C",
      edge("B", "C"),
      "graph TD\n  A --> B; A --> C",
    ],
    [
      "a statement on the keyword line, the keyword stays",
      "graph TD; A --> B; A --> C; B --> C",
      edge("A", "B"),
      "graph TD; A --> C; B --> C",
    ],
    [
      "a cut on a line that holds more keeps the rest",
      "graph TD\n  A --> B; C --> D",
      edge("A", "B"),
      "graph TD\n  A\n  B; C --> D",
    ],
    [
      "a last line goes with the break before it",
      "graph TD\n  A --> B\n  A --> C\n  B --> C\n  C --> A",
      edge("C", "A"),
      "graph TD\n  A --> B\n  A --> C\n  B --> C",
    ],
    [
      "a comment beside it stays",
      "graph TD\n  A --> B\n  %% about the next one\n  A --> C\n  %% and this\n  B --> C",
      edge("A", "C"),
      "graph TD\n  A --> B\n  %% about the next one\n  %% and this\n  B --> C",
    ],
    [
      "the indent of a tab is kept in a cut",
      "graph TD\n\tA[x] --> B[y]",
      edge("A", "B"),
      "graph TD\n\tA[x]\n\tB[y]",
    ],
    [
      "an explicit arrow id goes with its arrow",
      "graph TD\n  A e1@--> B\n  A --> C\n  C --> B",
      edge("A", "B"),
      "graph TD\n  A --> C\n  C --> B",
    ],
    [
      "a label goes with its arrow",
      "graph TD\n  A -->|Yes| B\n  A -- No --> C\n  B --> C",
      edge("A", "C"),
      "graph TD\n  A -->|Yes| B\n  B --> C",
    ],
    [
      "a box's only loop leaves the box",
      "graph TD\n  A --> A",
      edge("A", "A"),
      "graph TD\n  A",
    ],
    [
      "an arrow into a subgraph: the subgraph holds itself",
      "graph TD\n  subgraph s\n    A\n  end\n  B --> s\n  B --> A",
      edge("B", "s"),
      "graph TD\n  subgraph s\n    A\n  end\n  B --> A",
    ],
  ])("%s", (_, body, op, expected) => {
    expect(run(body, op)).toBe(expected);
  });

  test("a box's accDescr } is never deleted (R33)", () => {
    const body =
      "graph TD\n  accDescr {\n    words\n  } A --> B\n  A --> C\n  B --> C";
    expect(run(body, edge("A", "B"))).toBe(
      "graph TD\n  accDescr {\n    words\n  } \n  A --> C\n  B --> C",
    );
    expect(
      run(
        "graph TD\n  accDescr {\n    words\n  } A --> B; A --> C\n  B --> C",
        edge("A", "B"),
      ),
    ).toBe("graph TD\n  accDescr {\n    words\n  } A --> C\n  B --> C");
  });

  test("a ref that holds a class or an overridden label is kept, so nothing written is lost", () => {
    expect(
      run("graph TD\n  A --> B:::hot\n  A --> C\n  B --> C", edge("A", "B")),
    ).toBe("graph TD\n  B:::hot\n  A --> C\n  B --> C");
    expect(
      run(
        "graph TD\n  A[old] --> B\n  A[new] --> C\n  B --> C",
        edge("A", "B"),
      ),
    ).toBe("graph TD\n  A[old]\n  A[new] --> C\n  B --> C");
  });

  test("a group refuses, on either side", () => {
    expect(refusal("flowchart TD\n  A & B --> C", edge("A", "C"))).toBe(
      "group",
    );
    expect(refusal("flowchart TD\n  A --> B & C", edge("A", "C"))).toBe(
      "group",
    );
  });

  test("an arrow that is not there is gone", () => {
    expect(refusal("flowchart TD\n  A --> B", edge("B", "A"))).toBe("gone");
    expect(refusal("flowchart TD\n  A --> B", edge("A", "B", 1))).toBe("gone");
  });

  test("the later arrow of a pair keeps its linkStyle", async () => {
    const body =
      "flowchart TD\n  A --> B\n  A --> B\n  linkStyle 1 stroke:#ff0000";
    const after = run(body, edge("A", "B"));
    expect(after).toBe("flowchart TD\n  A --> B\n  linkStyle 0 stroke:#ff0000");
    const db = await flowDbFor(after);
    expect(db.edges[0]?.style).toContain("stroke:#ff0000");
    expect(db.edges.map((e) => e.id)).toEqual(["L_A_B_0"]);
  });

  test("on f10, index 1 goes from `linkStyle 1,3` and 3 turns into 2", () => {
    const after = run(fixture("f10"), edge("B", "C"));
    expect(after).toBe(
      [
        "flowchart LR",
        "    %% five edges, styled by index",
        "    A --> B",
        "    C --> D",
        "    D --> E",
        "    E --> A",
        "    linkStyle 0 stroke:#d33,stroke-width:2px",
        "    linkStyle 2 stroke:#36c",
        "    linkStyle default stroke:#888",
      ].join("\n"),
    );
  });

  test("on f10, a removed arrow's own linkStyle line goes with its break", () => {
    const after = run(fixture("f10"), edge("A", "B"));
    expect(after).toBe(
      [
        "flowchart LR",
        "    %% five edges, styled by index",
        "    B --> C",
        "    C --> D",
        "    D --> E",
        "    E --> A",
        "    linkStyle 0,2 stroke:#36c",
        "    linkStyle default stroke:#888",
      ].join("\n"),
    );
  });

  test("a line that would lose a box's class order falls back to a cut", async () => {
    const body = "flowchart TD\n  A --> B\n  class B hot\n  B --> C";
    const after = run(body, edge("A", "B"));
    expect(after).toBe("flowchart TD\n  A\n  B\n  class B hot\n  B --> C");
    const db = await flowDbFor(after);
    expect(db.vertices.find((v) => v.id === "B")?.classes).toEqual(["hot"]);
  });

  test("removing a ref never moves a box to another subgraph", async () => {
    const body =
      "flowchart TD\n  subgraph s\n    A --> B\n  end\n  subgraph t\n    B --> C\n  end";
    const after = run(body, edge("A", "B"));
    expect(after).toBe(
      "flowchart TD\n  subgraph s\n    A\n    B\n  end\n  subgraph t\n    B --> C\n  end",
    );
    const db = await flowDbFor(after);
    expect(db.subgraphs.find((s) => s.id === "s")?.nodes).toContain("B");
    expect(db.subgraphs.find((s) => s.id === "t")?.nodes).not.toContain("B");
  });

  test("an arrow that moved a subgraph into another keeps it there", async () => {
    const body =
      "flowchart TD\n  subgraph s\n    A\n  end\n  subgraph t\n    B --> s\n    B --> C\n  end";
    const before = await flowDbFor(body);
    const after = run(body, edge("B", "s"));
    const db = await flowDbFor(after);
    expect(db.subgraphs.find((s) => s.id === "t")?.nodes.toSorted()).toEqual(
      before.subgraphs.find((s) => s.id === "t")?.nodes.toSorted(),
    );
  });

  test("a line that would read as a direction with the next is refused (R7)", () => {
    expect(
      refusal(
        "flowchart TD\n  A --> direction --> LR",
        edge("direction", "LR"),
      ),
    ).toBe("invalid");
  });

  test("a line break in a document with one kind stays that kind", () => {
    expect(
      saved("flowchart TD\r\n  A[x] --> B[y]\r\n  B --> C", edge("A", "B")),
    ).toBe("flowchart TD\r\n  A[x]\r\n  B[y]\r\n  B --> C");
  });

  describe("a document with both line breaks keeps each line's own break (R51)", () => {
    test.each<[string, string, FlowOp, string]>([
      [
        "a whole line, in the middle",
        "flowchart TD\r\n  A --> B\n  A --> C\r\n  B --> C\n  C --> D",
        edge("A", "C"),
        "flowchart TD\r\n  A --> B\n  B --> C\n  C --> D",
      ],
      [
        "a whole line after a \\r\\n line",
        "flowchart TD\r\n  A --> C\n  A --> B\r\n  B --> C\r\n  C --> D",
        edge("A", "B"),
        "flowchart TD\r\n  A --> C\n  B --> C\r\n  C --> D",
      ],
      [
        "a last line",
        "flowchart TD\r\n  A --> B\r\n  A --> C\n  B --> C\n  C --> A",
        edge("C", "A"),
        "flowchart TD\r\n  A --> B\r\n  A --> C\n  B --> C",
      ],
      [
        "a cut, the new break is the state's",
        "flowchart TD\r\n  A[x] --> B[y]\n  B --> C",
        edge("A", "B"),
        "flowchart TD\r\n  A[x]\r\n  B[y]\n  B --> C",
      ],
      [
        "with a linkStyle line going too, the two not side by side",
        "flowchart TD\r\n  A --> B\n  A --> C\r\n  B --> C\n  linkStyle 0 stroke:#f00\r\n  C --> D",
        edge("A", "B"),
        "flowchart TD\r\n  A --> C\r\n  B --> C\n  C --> D",
      ],
      [
        "with a linkStyle line going too, the two side by side at the end",
        "flowchart TD\r\n  A --> C\n  B --> C\r\n  A --> B\n  linkStyle 2 stroke:#f00",
        edge("A", "B"),
        "flowchart TD\r\n  A --> C\n  B --> C",
      ],
    ])("%s", (_, doc, op, expected) => {
      expect(saved(doc, op)).toBe(expected);
    });
  });
});

describe("deleteNode", () => {
  test.each<[string, string, string, string]>([
    [
      "the middle of a chain, both sides needed",
      "graph TD\n  A -->  B --> C",
      "B",
      "graph TD\n  A\n  C",
    ],
    [
      "in a group, first",
      "graph TD\n  B & A --> C",
      "B",
      "graph TD\n  A --> C",
    ],
    ["in a group, last", "graph TD\n  A & B --> C", "B", "graph TD\n  A --> C"],
    [
      "in a group, middle",
      "graph TD\n  A & B  &  C --> D",
      "B",
      "graph TD\n  A  &  C --> D",
    ],
    [
      "in a group on the right",
      "graph TD\n  D --> A & B",
      "B",
      "graph TD\n  D --> A",
    ],
    [
      "in a group, first on the right",
      "graph TD\n  D --> B & A",
      "B",
      "graph TD\n  D --> A",
    ],
    [
      "in a group, the box also alone on the other side",
      "graph TD\n  A & B --> B",
      "B",
      "graph TD\n  A",
    ],
    [
      "both groups of a chain",
      "graph TD\n  A & B --> C --> B & D",
      "B",
      "graph TD\n  A --> C --> D",
    ],
    [
      "at the start of a chain",
      "graph TD\n  B --> C --> D",
      "B",
      "graph TD\n  C --> D",
    ],
    [
      "at the end of a chain",
      "graph TD\n  C --> D --> B",
      "B",
      "graph TD\n  C --> D",
    ],
    [
      "at both ends of a chain",
      "graph TD\n  B --> C --> B",
      "B",
      "graph TD\n  C",
    ],
    [
      "a lone statement goes with its break",
      "graph TD\n  A --> C\n  B\n  C --> D",
      "B",
      "graph TD\n  A --> C\n  C --> D",
    ],
    [
      "a part that is not needed is dropped too",
      "graph TD\n  A --> B\n  A --> C",
      "B",
      "graph TD\n  A --> C",
    ],
    [
      "a part that is needed stays",
      "graph TD\n  A --> B",
      "B",
      "graph TD\n  A",
    ],
    [
      "one needed and one not",
      "graph TD\n  A --> B --> C\n  C --> D",
      "B",
      "graph TD\n  A\n  C --> D",
    ],
    [
      "two refs of another box, the first stays and the box with it",
      "graph TD\n  A --> B --> C\n  C --> B",
      "B",
      "graph TD\n  A\n  C",
    ],
    [
      "a loop on the box",
      "graph TD\n  A --> C\n  B --> B\n  B --> C",
      "B",
      "graph TD\n  A --> C",
    ],
    ["the last box", "graph TD\n  A", "A", "graph TD"],
    ["the last two boxes' arrow", "graph TD\n  A --> B", "A", "graph TD\n  B"],
    [
      "a box only a style line names",
      "graph TD\n  A --> C\n  style B fill:#fff",
      "B",
      "graph TD\n  A --> C",
    ],
    [
      "style, class and click lines naming only it go",
      'graph TD\n  A --> C\n  B\n  style B fill:#fff\n  class B hot\n  click B "https://x.y"\n  classDef hot fill:#f00',
      "B",
      "graph TD\n  A --> C\n  classDef hot fill:#f00",
    ],
    [
      "a shared class line keeps the rest",
      "graph TD\n  A --> B\n  class A,B x",
      "B",
      "graph TD\n  A\n  class A x",
    ],
    [
      "a shared class line, first",
      "graph TD\n  A --> B\n  class B,A x",
      "B",
      "graph TD\n  A\n  class A x",
    ],
    [
      "a shared class line, middle",
      "graph TD\n  A --> B --> C\n  class A,B,C x",
      "B",
      "graph TD\n  A\n  C\n  class A,C x",
    ],
    [
      "the box named twice on a shared class line",
      "graph TD\n  A --> B\n  class A,B,B x",
      "B",
      "graph TD\n  A\n  class A x",
    ],
    [
      "neighbouring indices of a shared linkStyle line",
      "graph TD\n  A --> B\n  B --> C\n  C --> D\n  linkStyle 0,1,2 stroke:#f00",
      "C",
      "graph TD\n  A --> B\n  D\n  linkStyle 0 stroke:#f00",
    ],
    [
      "a shared click line keeps the rest",
      'graph TD\n  A --> B\n  click A,B "https://x.y"',
      "B",
      'graph TD\n  A\n  click A "https://x.y"',
    ],
    [
      "someone else's style line stays",
      "graph TD\n  A --> B\n  style A fill:#fff\n  style B fill:#000",
      "B",
      "graph TD\n  A\n  style A fill:#fff",
    ],
    [
      "comments about the box stay",
      "graph TD\n  A --> C\n  %% B is the odd one\n  B --> C",
      "B",
      "graph TD\n  A --> C\n  %% B is the odd one",
    ],
    [
      "a style statement on a shared line",
      "graph TD\n  A --> B; style B stroke-width:1px; C --> A",
      "B",
      "graph TD\n  C --> A",
    ],
    [
      "a shared line keeps what is not the box's",
      "graph TD\n  A --> C; B --> C; class B x; C --> A",
      "B",
      "graph TD\n  A --> C; C --> A",
    ],
    [
      "a style line and a linkStyle line that end the body",
      "graph TD\n  A --> C\n  C --> B\n  style B fill:#fff\n  linkStyle 1 stroke:#000",
      "B",
      "graph TD\n  A --> C",
    ],
    [
      "a style and a linkStyle statement that share a line",
      "graph TD\n  A --> C\n  C --> B\n  style B stroke-width:1px; linkStyle 1 stroke-width:2px",
      "B",
      "graph TD\n  A --> C",
    ],
    [
      "a shared linkStyle line and a style statement",
      "graph TD\n  A --> C\n  C --> B\n  linkStyle 1 stroke-width:2px; style B stroke-width:1px; linkStyle 0 stroke-width:3px",
      "B",
      "graph TD\n  A --> C\n  linkStyle 0 stroke-width:3px",
    ],
    [
      "the last box of a subgraph: the subgraph stays, the others' too",
      "graph TD\n  subgraph s\n    A\n  end\n  subgraph t\n    B\n  end",
      "A",
      "graph TD\n  subgraph s\n  end\n  subgraph t\n    B\n  end",
    ],
    [
      "a direction line in a subgraph stays",
      "graph TD\n  subgraph s\n    direction LR\n    A\n  end\n  B",
      "A",
      "graph TD\n  subgraph s\n    direction LR\n  end\n  B",
    ],
    [
      "one-line subgraph",
      "graph TD\n  subgraph s; A; B; end",
      "A",
      "graph TD\n  subgraph s; B; end",
    ],
    [
      "an arrow into a subgraph",
      "graph TD\n  subgraph s\n    A\n  end\n  B --> s",
      "B",
      "graph TD\n  subgraph s\n    A\n  end",
    ],
    [
      "a shaped box keeps every byte of the others",
      'graph TD\n  A["a  b"]:::x -->|"l"| B\n  B --> C{"c"}',
      "B",
      'graph TD\n  A["a  b"]:::x\n  C{"c"}',
    ],
  ])("%s", (_, body, id, expected) => {
    expect(run(body, box(id))).toBe(expected);
  });

  test("a box's accDescr } is never deleted (R33)", () => {
    expect(
      run("graph TD\n  accDescr {\n    words\n  } B\n  A --> C", box("B")),
    ).toBe("graph TD\n  accDescr {\n    words\n  } \n  A --> C");
    expect(
      run(
        "graph TD\n  accDescr {\n    words\n  } A --> B; style B stroke-width:1px\n  A --> C",
        box("B"),
      ),
    ).toBe("graph TD\n  accDescr {\n    words\n  } \n  A --> C");
  });

  test("of two refs of another box, the first stays, which is what holds it in its subgraph", () => {
    expect(
      run("graph TD\n  subgraph s\n    B --> C\n  end\n  B --> C", box("B")),
    ).toBe("graph TD\n  subgraph s\n    C\n  end");
  });

  test("the self-check wants the box's fields gone, and only that box's", () => {
    const body = "graph TD\n  A --> B\n  style B fill:#fff";
    const model = parseFlowchart(body);
    const line = body.indexOf("  style");
    const style = [{ from: line - 1, to: body.length, insert: "" }];
    expect(
      checked(model, style, { "B styles": null, "B colours": null }),
    ).toHaveProperty("edits");
    expect(
      checked(model, style, { "B styles": null, "B colours": null }, "B"),
    ).toEqual({ refused: "invalid" });
    // Without naming it, no field may go.
    const plain = "graph TD\n  A --> B";
    const arrow = [
      { from: plain.indexOf(" -->"), to: plain.length, insert: "" },
    ];
    expect(checked(parseFlowchart(plain), arrow, {})).toEqual({
      refused: "invalid",
    });
    expect(
      checked(parseFlowchart(plain), arrow, { boxes: null, arrows: null }),
    ).toEqual({ refused: "invalid" });
    expect(
      checked(parseFlowchart(plain), arrow, { boxes: null, arrows: null }, "B"),
    ).toHaveProperty("edits");
  });

  test("a subgraph id is no box (R4)", () => {
    expect(
      refusal("graph TD\n  subgraph s\n    A\n  end\n  B --> s", box("s")),
    ).toBe("gone");
    expect(refusal("graph TD\n  A", box("B"))).toBe("gone");
  });

  test("an id only a class or click line names is no box", () => {
    expect(refusal("graph TD\n  A\n  class C x", box("C"))).toBe("gone");
  });

  test("one shift for every removed arrow", () => {
    // Edges: A-B 0, B-C 1, X-B 2, C-D 3, D-E 4. C-D must become 0 and D-E 1, by
    // the three gone below them, and one call per edge would overlap.
    const body =
      "graph TD\n  A --> B --> C\n  X --> B\n  C --> D\n  D --> E\n  linkStyle 0,1,3 stroke:#f00\n  linkStyle 4 stroke:#00f";
    expect(run(body, box("B"))).toBe(
      "graph TD\n  A\n  X\n  C --> D\n  D --> E\n  linkStyle 0 stroke:#f00\n  linkStyle 1 stroke:#00f",
    );
  });

  test("a whole linkStyle line of its arrows goes", () => {
    const body =
      "graph TD\n  A --> B\n  B --> C\n  A --> D\n  linkStyle 0,1 stroke:#f00\n  linkStyle 2 stroke:#00f";
    expect(run(body, box("B"))).toBe(
      "graph TD\n  C\n  A --> D\n  linkStyle 0 stroke:#00f",
    );
  });

  test("an arrow id that would move to another arrow refuses", () => {
    // `e1` sits on (last start, first end), so without B it would name A to C.
    expect(refusal("graph TD\n  A & B e1@--> C", box("B"))).toBe("invalid");
    // Without A it stays on B to C, where it was.
    expect(run("graph TD\n  A & B e1@--> C", box("A"))).toBe(
      "graph TD\n  B e1@--> C",
    );
  });

  test("an arrow that moved a subgraph into another keeps it there", async () => {
    const body =
      "graph TD\n  subgraph s\n    A\n  end\n  subgraph t\n    B --> s\n    C\n  end";
    const before = await flowDbFor(body);
    const db = await flowDbFor(run(body, box("B")));
    expect(db.subgraphs.find((g) => g.id === "t")?.nodes.toSorted()).toEqual(
      before.subgraphs
        .find((g) => g.id === "t")
        ?.nodes.filter((n) => n !== "B")
        .toSorted(),
    );
  });

  test("a box defined twice goes whole", async () => {
    const body = "graph TD\n  A[x] --> B\n  A[y] --> C";
    const after = run(body, box("A"));
    expect(after).toBe("graph TD\n  B\n  C");
    expect((await flowDbFor(after)).vertices.map((v) => v.id)).toEqual([
      "B",
      "C",
    ]);
  });

  test("a line break in a document with one kind stays that kind", () => {
    expect(saved("graph TD\r\n  A -->  B --> C\r\n  D", box("B"))).toBe(
      "graph TD\r\n  A\r\n  C\r\n  D",
    );
  });

  describe("a document with both line breaks keeps each line's own break (R51)", () => {
    test.each<[string, string, string, string]>([
      [
        "two deleted lines apart",
        "graph TD\r\n  A --> C\n  B\r\n  C --> D\n  style B fill:#fff\r\n  D --> A",
        "B",
        "graph TD\r\n  A --> C\n  C --> D\n  D --> A",
      ],
      [
        "two deleted lines side by side",
        "graph TD\r\n  A --> C\n  B\r\n  style B fill:#fff\n  C --> D\r\n  D --> A",
        "B",
        "graph TD\r\n  A --> C\n  C --> D\r\n  D --> A",
      ],
      [
        "the body's last lines",
        "graph TD\r\n  A --> C\n  C --> B\r\n  style B fill:#fff\n  class B x",
        "B",
        "graph TD\r\n  A --> C",
      ],
      [
        "a part stays, the cut takes the state's break",
        "graph TD\r\n  A -->  B --> C\n  D",
        "B",
        "graph TD\r\n  A\r\n  C\n  D",
      ],
      [
        "with its linkStyle line at the end",
        "graph TD\r\n  A --> B\n  B --> C\r\n  C --> A\n  linkStyle 2 stroke:#00f",
        "A",
        "graph TD\r\n  B --> C",
      ],
      [
        "with a linkStyle digit",
        "graph TD\r\n  A --> B\n  B --> C\r\n  C --> D\n  linkStyle 2 stroke:#00f\r\n  D",
        "A",
        "graph TD\r\n  B --> C\r\n  C --> D\n  linkStyle 1 stroke:#00f\r\n  D",
      ],
    ])("%s", (_, doc, id, expected) => {
      expect(saved(doc, box(id))).toBe(expected);
    });
  });
});

describe("the db after a removal", () => {
  test.each<[string, string, FlowOp]>([
    ["a chain", "graph TD\n  A --> B --> C\n  C --> B", box("B")],
    ["f10", fixture("f10"), box("C")],
    ["f13", fixture("f13"), box("X")],
    ["f12", fixture("f12"), box("A")],
    ["f11", fixture("f11"), box("A")],
    ["f10 edge", fixture("f10"), edge("C", "D")],
    ["f13 edge", fixture("f13"), edge("Z", "X")],
  ])("%s is read back as the model says", async (_, body, op) => {
    const after = run(body, op);
    const model = parseFlowchart(after);
    expect(model.understood).toBe(true);
    expect(agreesFlow(model, await flowDbFor(after))).toBeNull();
  });
});
