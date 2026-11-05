// Real mermaid wherever a test reads a db: where a new line lands decides which
// subgraph mermaid puts the new box in and which edge a linkStyle index means.

import { EditorState } from "@codemirror/state";
import { describe, expect, test } from "vitest";

import { flowDbFor, flowDelta } from "../../../test/mermaidOracle";
import { docText, lineSeparatorFor } from "../../setup";
import { applyEdits, toChanges } from "../edits";
import { agreesFlow } from "../verdict";
import { shiftLinkStyles } from "./linkStyle";
import { parseFlowchart } from "./parse";
import { type FlowOp, checked, compileFlowOp, newNodeId } from "./ops";

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

function created(body: string, op: FlowOp): unknown {
  const result = compileFlowOp(parseFlowchart(body), op);
  return "refused" in result ? result.refused : result.created;
}

/** `shiftLinkStyles` applied to `body`. */
function shifted(
  body: string,
  at: number,
  delta: number,
  removed?: ReadonlySet<number>,
): string {
  return applyEdits(
    body,
    shiftLinkStyles(parseFlowchart(body), at, delta, removed),
  );
}

const SKELETON = "flowchart TD\n    A[First step] --> B[Next step]";

/** The subgraph mermaid lists `id` in, null for none. */
async function holder(body: string, id: string): Promise<string | null> {
  const db = await flowDbFor(body);
  return db.subgraphs.find((s) => s.nodes.includes(id))?.id ?? null;
}

describe("addChild", () => {
  test("on the skeleton", () => {
    const op: FlowOp = { op: "addChild", id: "A" };
    expect(run(SKELETON, op)).toBe(
      "flowchart TD\n    A[First step] --> B[Next step]\n    A --> C[New box]",
    );
    expect(created(SKELETON, op)).toEqual({ kind: "node", id: "C" });
  });

  test("ids that are not single letters make n1", () => {
    const body = fixture("f17");
    const op: FlowOp = { op: "addChild", id: "step-1" };
    expect(created(body, op)).toEqual({ kind: "node", id: "n1" });
    expect(run(body, op)).toBe(
      body.replace(
        "a_b[Second]\n",
        "a_b[Second]\n    step-1 --> n1[New box]\n",
      ),
    );
  });

  test("a missing box is gone, and a subgraph id is no box (R4)", () => {
    expect(refusal(SKELETON, { op: "addChild", id: "Z" })).toBe("gone");
    const body = "flowchart TD\n  subgraph s\n    A\n  end\n  B --> s";
    expect(refusal(body, { op: "addChild", id: "s" })).toBe("gone");
    expect(refusal(body, { op: "addSibling", id: "s" })).toBe("gone");
  });

  test.each([
    [
      "X",
      "inner",
      "            X[Write] --> Y[Sync]\n            X --> n1[New box]\n        end",
    ],
    [
      "Y",
      "inner",
      "            X[Write] --> Y[Sync]\n            Y --> n1[New box]\n        end",
    ],
    [
      "Z",
      "outer",
      "        Z[Watcher] --> X\n        Z --> n1[New box]\n    end",
    ],
    ["Q", null, "    Q -.-> Z\n    Q --> n1[New box]"],
  ])("in f13, %s lands in %s", async (id, group, text) => {
    const body = fixture("f13");
    const after = run(body, { op: "addChild", id });
    expect(after).toContain(text);
    expect(await holder(after, "n1")).toBe(group);
    expect(agreesFlow(parseFlowchart(after), await flowDbFor(after))).toBe(
      null,
    );
  });

  test("a ref that sits on the subgraph's own line still means inside it", async () => {
    const body = "flowchart TD\n  subgraph s; A --> B\n  end\n  C --> D";
    const after = run(body, { op: "addChild", id: "A" });
    expect(after).toBe(
      "flowchart TD\n  subgraph s; A --> B\n  A --> E[New box]\n  end\n  C --> D",
    );
    expect(await holder(after, "E")).toBe("s");
  });

  test("with no line in its own subgraph it goes just before the end", async () => {
    const body = "flowchart TD\n  subgraph s\n    B\n    %% note\n    A; end";
    const after = run(body, { op: "addChild", id: "A" });
    expect(after).toBe(
      "flowchart TD\n  subgraph s\n    B\n    %% note\n    A --> C[New box]\n    A; end",
    );
    expect(await holder(after, "C")).toBe("s");
  });

  test("a box only a style line names goes to the end of the body", async () => {
    const body =
      "flowchart TD\n  A --> B\n  subgraph s\n    C\n    style Z fill:#fff\n  end";
    const after = run(body, { op: "addChild", id: "Z" });
    expect(after).toBe(`${body}\n  Z --> n1[New box]`);
    expect(await holder(after, "n1")).toBe(null);
  });

  test("a line that names the box in a style or class line counts as a reference", () => {
    const body = "flowchart TD\n  A --> B\n  style A fill:#fff\n  B --> C";
    expect(run(body, { op: "addChild", id: "A" })).toBe(
      "flowchart TD\n  A --> B\n  style A fill:#fff\n  A --> D[New box]\n  B --> C",
    );
  });

  test("never takes an accDescr block's } with it (R33)", async () => {
    const body = "flowchart TD\n  accDescr {\n    x\n  } A --> B; C --> D";
    const after = run(body, { op: "addChild", id: "A" });
    expect(after).toBe(`${body}\n    A --> E[New box]`);
    await flowDbFor(after);
  });
});

describe("where a new line goes and how it is indented", () => {
  test("the indent of the line it follows", () => {
    const body = "flowchart TD\n\tA --> B\n  B --> C";
    expect(run(body, { op: "addChild", id: "A" })).toBe(
      "flowchart TD\n\tA --> B\n\tA --> D[New box]\n  B --> C",
    );
    expect(run(body, { op: "addChild", id: "B" })).toBe(
      `${body}\n  B --> D[New box]`,
    );
  });

  test("a peer's comment line stays where it is", () => {
    const body = "flowchart TD\n  A --> B\n    %% peer note\n  B --> C";
    expect(run(body, { op: "addChild", id: "A" })).toBe(
      "flowchart TD\n  A --> B\n  A --> D[New box]\n    %% peer note\n  B --> C",
    );
    expect(run(`${body}\n      %% last`, { op: "addBox" })).toBe(
      `${body}\n      %% last\n  D[New box]`,
    );
  });

  test("after the subgraph's opening line it takes the subgraph's first statement", () => {
    const body =
      "flowchart TD\n  subgraph s\n      %% note\n      B\n      A; end";
    expect(run(body, { op: "addChild", id: "A" })).toBe(
      "flowchart TD\n  subgraph s\n      %% note\n      B\n      A --> C[New box]\n      A; end",
    );
    const held = "flowchart TD\n  subgraph s; A\n      B\n  end";
    expect(run(held, { op: "addChild", id: "A" })).toBe(
      "flowchart TD\n  subgraph s; A\n      A --> C[New box]\n      B\n  end",
    );
  });

  test("with no statement line it is four spaces", () => {
    expect(run("flowchart TD", { op: "addBox" })).toBe(
      "flowchart TD\n    A[New box]",
    );
    expect(run("graph TD; A --> B", { op: "addChild", id: "A" })).toBe(
      "graph TD; A --> B\n    A --> C[New box]",
    );
  });

  test("a trailing blank line stays last", () => {
    expect(run("flowchart TD\n  A\n", { op: "addBox" })).toBe(
      "flowchart TD\n  A\n  B[New box]\n",
    );
  });
});

describe("addSibling", () => {
  test("on the skeleton", () => {
    const op: FlowOp = { op: "addSibling", id: "B" };
    expect(run(SKELETON, op)).toBe(`${SKELETON}\n    A --> C[New box]`);
    expect(created(SKELETON, op)).toEqual({ kind: "node", id: "C" });
  });

  test("the parent is the first edge into the box in source order", () => {
    const body = "flowchart TD\n  A --> C\n  B --> C";
    expect(run(body, { op: "addSibling", id: "C" })).toBe(
      "flowchart TD\n  A --> C\n  A --> D[New box]\n  B --> C",
    );
  });

  test("a box with no parent gets a plain box next to it", () => {
    expect(run("flowchart TD\n  A[x]", { op: "addSibling", id: "A" })).toBe(
      "flowchart TD\n  A[x]\n  B[New box]",
    );
  });

  test("a box with no parent stays in its subgraph", async () => {
    const after = run(
      "flowchart TD\n  subgraph s\n    A[x]\n  end\n  C --> D",
      { op: "addSibling", id: "A" },
    );
    expect(after).toBe(
      "flowchart TD\n  subgraph s\n    A[x]\n    E[New box]\n  end\n  C --> D",
    );
    expect(await holder(after, "E")).toBe("s");
  });

  test("a subgraph is no parent (R4)", () => {
    const body = "flowchart TD\n  subgraph s\n    A\n  end\n  s --> B";
    expect(run(body, { op: "addSibling", id: "B" })).toBe(
      `${body}\n  C[New box]`,
    );
  });

  test("a box is not its own parent", () => {
    const body = "flowchart TD\n  B --> B\n  A --> B";
    expect(run(body, { op: "addSibling", id: "B" })).toBe(
      `${body}\n  A --> C[New box]`,
    );
    expect(run("flowchart TD\n  B --> B", { op: "addSibling", id: "B" })).toBe(
      "flowchart TD\n  B --> B\n  C[New box]",
    );
  });
});

describe("connect", () => {
  test("appends the arrow and creates no box", () => {
    const op: FlowOp = { op: "connect", from: "B", to: "A" };
    expect(run(SKELETON, op)).toBe(`${SKELETON}\n    B --> A`);
    expect(created(SKELETON, op)).toBe(undefined);
  });

  test("goes after the later of the two", () => {
    const body = "flowchart TD\n  A --> B\n  C --> D\n  E --> F";
    const later = "flowchart TD\n  A --> B\n  C --> D\n  C --> A\n  E --> F";
    expect(run(body, { op: "connect", from: "C", to: "A" })).toBe(later);
    expect(run(body, { op: "connect", from: "A", to: "C" })).toBe(
      later.replace("C --> A", "A --> C"),
    );
  });

  test("either end missing is gone, and a subgraph id is no box (R4)", () => {
    expect(refusal(SKELETON, { op: "connect", from: "A", to: "Z" })).toBe(
      "gone",
    );
    expect(refusal(SKELETON, { op: "connect", from: "Z", to: "A" })).toBe(
      "gone",
    );
    const body = "flowchart TD\n  subgraph s\n    A\n  end\n  B --> s";
    expect(refusal(body, { op: "connect", from: "B", to: "s" })).toBe("gone");
  });

  test("the same pair again gets mermaid's next id", async () => {
    const after = run(SKELETON, { op: "connect", from: "A", to: "B" });
    expect(after).toBe(`${SKELETON}\n    A --> B`);
    const db = await flowDbFor(after);
    expect(db.edges.map((e) => e.id)).toEqual(["L_A_B_0", "L_A_B_2"]);
  });

  test("a box may connect to itself", async () => {
    const after = run(SKELETON, { op: "connect", from: "A", to: "A" });
    expect(after).toBe(`${SKELETON}\n    A --> A`);
    expect(await flowDelta(SKELETON, after)).toEqual(["edge 1 added"]);
  });

  test("an end that would move into the other's subgraph keeps the arrow outside", async () => {
    // Inside s, `B --> A` would make B a member of s.
    const body = "flowchart TD\n  B\n  subgraph s\n    A --> C\n  end";
    const after = run(body, { op: "connect", from: "B", to: "A" });
    expect(after).toBe(`${body}\n  B --> A`);
    expect(await flowDelta(body, after)).toEqual(["edge 1 added"]);
  });

  test("across two levels it lands in the level that holds both", async () => {
    const body = fixture("f13");
    const after = run(body, { op: "connect", from: "Z", to: "Y" });
    expect(after).toContain(
      "        Z[Watcher] --> X\n        Z --> Y\n    end",
    );
    expect(await holder(after, "Y")).toBe("inner");
    expect(await holder(after, "Z")).toBe("outer");
  });

  test("nested, an arrow that would pull its start inward stays one level out", async () => {
    const body =
      "flowchart TD\n  subgraph o\n    W\n    subgraph i\n      X --> Y\n    end\n  end";
    const after = run(body, { op: "connect", from: "W", to: "X" });
    expect(after).toBe(
      "flowchart TD\n  subgraph o\n    W\n    subgraph i\n      X --> Y\n    end\n    W --> X\n  end",
    );
    expect(await holder(after, "W")).toBe("o");
    expect(await holder(after, "X")).toBe("i");
  });

  test("two subgraphs side by side keep their members", async () => {
    const body =
      "flowchart TD\n  subgraph a\n    A\n  end\n  subgraph b\n    B\n  end";
    for (const op of [
      { op: "connect", from: "A", to: "B" },
      { op: "connect", from: "B", to: "A" },
    ] as const) {
      const after = run(body, op);
      expect(await holder(after, "A")).toBe("a");
      expect(await holder(after, "B")).toBe("b");
    }
  });
});

describe("addBox", () => {
  test("goes to the end of the body", () => {
    const op: FlowOp = { op: "addBox" };
    expect(run(SKELETON, op)).toBe(`${SKELETON}\n    C[New box]`);
    expect(created(SKELETON, op)).toEqual({ kind: "node", id: "C" });
  });

  test("after the last subgraph, outside it", async () => {
    const body = "flowchart TD\n  subgraph s\n    A\n  end";
    const after = run(body, { op: "addBox" });
    expect(after).toBe(`${body}\n  B[New box]`);
    expect(await holder(after, "B")).toBe(null);
  });
});

describe("newNodeId", () => {
  test.each([
    ["no box yet", "flowchart TD", "A"],
    ["the next letter", SKELETON, "C"],
    ["after the highest, not the first gap", "flowchart TD\n  A --> D", "E"],
    [
      "past a subgraph id",
      "flowchart TD\n  A\n  subgraph B\n    A\n  end",
      "C",
    ],
    ["past an arrow id", "flowchart TD\n  A B@--> A", "C"],
    ["past a generated arrow id", "flowchart TD\n  A --> A", "B"],
    ["past a class target", "flowchart TD\n  A\n  class B hot", "C"],
    ["past a click target", 'flowchart TD\n  A\n  click B "https://x.y/"', "C"],
    ["n1 when Z is taken", "flowchart TD\n  Z", "n1"],
    ["n1 when ids are not letters", "flowchart TD\n  ab --> cd", "n1"],
    ["n1 when a letter is lowercase", "flowchart TD\n  a", "n1"],
    ["the smallest free k", "flowchart TD\n  n1 --> n3", "n2"],
    [
      "n<k> past a subgraph id",
      "flowchart TD\n  ab\n  subgraph n1\n    ab\n  end",
      "n2",
    ],
    ["n<k> past an arrow id", "flowchart TD\n  ab n1@--> cd", "n2"],
    ["n<k> past a class target", "flowchart TD\n  ab\n  class n1 hot", "n2"],
    [
      "n1 when the next letter is past Z",
      "flowchart TD\n  Y\n  subgraph Z\n    Y\n  end",
      "n1",
    ],
  ])("%s", (_, body, id) => {
    const model = parseFlowchart(body);
    expect(model.understood).toBe(true);
    expect(newNodeId(model)).toBe(id);
  });

  test("every letter draws as a box in real mermaid", async () => {
    for (let code = 66; code <= 90; code++) {
      const before = `flowchart TD\n  ${String.fromCharCode(code - 1)}`;
      const letter = String.fromCharCode(code);
      const after = run(before, { op: "addBox" });
      expect(after).toBe(`${before}\n  ${letter}[New box]`);
      expect(await flowDelta(before, after)).toEqual([
        `vertex ${letter} added`,
      ]);
    }
  });
});

describe("the arrow indices", () => {
  test("in f10 a middle arrow moves only the digits after it", async () => {
    const body = fixture("f10");
    const op: FlowOp = { op: "addChild", id: "C" };
    const after = run(body, op);
    expect(after).toBe(
      body
        .replace("C --> D\n", "C --> D\n    C --> F[New box]\n")
        .replace("linkStyle 1,3 ", "linkStyle 1,4 "),
    );
    const before = await flowDbFor(body);
    const now = await flowDbFor(after);
    expect(now.edges[4]).toEqual(before.edges[3]);
    expect(now.edges[3]?.style).toBeUndefined();
  });

  test("past the last arrow nothing moves", () => {
    const body = fixture("f10");
    const after = run(body, { op: "addChild", id: "A" });
    expect(after).toContain("linkStyle 1,3 stroke:#36c");
    expect(after).toContain("    E --> A\n    A --> F[New box]\n    linkStyle");
  });

  test("the brief's linkStyle 2: only that digit moves", async () => {
    const body =
      "flowchart LR\n    A --> B\n    B --> C\n    C --> D\n    linkStyle 2 stroke:#36c,stroke-width:2px";
    const after = run(body, { op: "addChild", id: "B" });
    expect(after).toBe(
      "flowchart LR\n    A --> B\n    B --> C\n    B --> E[New box]\n    C --> D\n    linkStyle 3 stroke:#36c,stroke-width:2px",
    );
    const db = await flowDbFor(after);
    expect(db.edges[3]?.style).toEqual([
      "stroke:#36c",
      "stroke-width:2px",
      "fill:none",
    ]);
    expect(db.edges[2]?.style).toBeUndefined();
  });

  test("a digit count may grow", () => {
    const lines = Array.from(
      { length: 11 },
      (_, i) => `    A${String(i)} --> B${String(i)}`,
    );
    const body = `flowchart LR\n${lines.join("\n")}\n    linkStyle 9 stroke:#36c`;
    const after = run(body, { op: "addChild", id: "A0" });
    expect(after).toContain("linkStyle 10 stroke:#36c");
  });

  test("a linkStyle line above the new line stays", () => {
    const body =
      "flowchart LR\n    A --> B\n    linkStyle 0 stroke:#36c\n    B --> C";
    expect(run(body, { op: "addChild", id: "B" })).toBe(
      `${body}\n    B --> D[New box]`,
    );
  });
});

describe("what the self-check refuses", () => {
  test("a new arrow whose id another arrow has makes mermaid's render throw", () => {
    const body = "flowchart TD\n    A L_A_C_0@--> B";
    expect(refusal(body, { op: "addChild", id: "A" })).toBe("invalid");
  });

  test("a new arrow whose id is a subgraph's", () => {
    const body =
      "flowchart TD\n    A --> B\n    subgraph L_A_C_0\n        D\n    end";
    expect(refusal(body, { op: "connect", from: "A", to: "D" })).toBe(null);
    const held =
      "flowchart TD\n    A --> B\n    C\n    subgraph L_A_C_0\n        Q\n    end";
    expect(refusal(held, { op: "connect", from: "A", to: "C" })).toBe(
      "invalid",
    );
  });

  test("an arrow between two the same pair would change the later one's id", () => {
    const body = "flowchart TD\n  subgraph s\n    A --> B\n  end\n  A --> B";
    expect(refusal(body, { op: "connect", from: "A", to: "B" })).toBe(
      "invalid",
    );
  });

  test("a new line holding a direction (R7)", () => {
    const model = parseFlowchart(SKELETON);
    const at = SKELETON.length;
    expect(
      checked(
        model,
        [{ from: at, to: at, insert: "\n    A --> C[go direction LR]" }],
        {
          boxes: null,
          arrows: null,
          "C label": null,
          "C markdown": null,
          "C shape": null,
          "C styles": null,
          "C colours": null,
          "C classes": null,
        },
      ),
    ).toEqual({ refused: "invalid" });
  });

  test("a new field nobody named", () => {
    const model = parseFlowchart(SKELETON);
    const at = SKELETON.length;
    const edit = { from: at, to: at, insert: "\n    C" };
    expect(checked(model, [edit], { boxes: null })).toEqual({
      refused: "invalid",
    });
  });

  test("a field that goes away", () => {
    const body = "flowchart TD\n    A\n    B";
    const model = parseFlowchart(body);
    const at = body.indexOf("\n    B");
    expect(
      checked(model, [{ from: at, to: body.length, insert: "" }], {
        boxes: null,
      }),
    ).toEqual({ refused: "invalid" });
  });

  test("a linkStyle index that points at another arrow", () => {
    const body =
      "flowchart TD\n    A --> B\n    B --> C\n    linkStyle 0 stroke:#000";
    const at = body.indexOf("0 stroke");
    expect(
      checked(
        parseFlowchart(body),
        [{ from: at, to: at + 1, insert: "1" }],
        {},
      ),
    ).toEqual({ refused: "invalid" });
  });
});

describe("a CRLF document", () => {
  test.each<[string, string, FlowOp, string]>([
    [
      "addChild",
      SKELETON,
      { op: "addChild", id: "A" },
      "flowchart TD\r\n    A[First step] --> B[Next step]\r\n    A --> C[New box]",
    ],
    [
      "addSibling",
      SKELETON,
      { op: "addSibling", id: "B" },
      "flowchart TD\r\n    A[First step] --> B[Next step]\r\n    A --> C[New box]",
    ],
    [
      "connect",
      SKELETON,
      { op: "connect", from: "B", to: "A" },
      "flowchart TD\r\n    A[First step] --> B[Next step]\r\n    B --> A",
    ],
    [
      "addBox",
      SKELETON,
      { op: "addBox" },
      "flowchart TD\r\n    A[First step] --> B[Next step]\r\n    C[New box]",
    ],
    [
      "a middle arrow",
      "flowchart LR\n    A --> B\n    B --> C\n    linkStyle 1 stroke:#36c",
      { op: "addChild", id: "A" },
      "flowchart LR\r\n    A --> B\r\n    A --> D[New box]\r\n    B --> C\r\n    linkStyle 2 stroke:#36c",
    ],
  ])("%s saves with its own line breaks", (_, lf, op, saved) => {
    const crlf = lf.replaceAll("\n", "\r\n");
    const state = EditorState.create({
      doc: crlf,
      extensions: lineSeparatorFor(crlf),
    });
    const body = state.doc.sliceString(0, state.doc.length);
    const result = compileFlowOp(parseFlowchart(body), op);
    if ("refused" in result) {
      throw new Error(result.refused);
    }
    expect(
      docText(
        state.update({ changes: toChanges(0, result.edits, state.lineBreak) })
          .state,
      ),
    ).toBe(saved);
  });
});

describe("shiftLinkStyles", () => {
  const FIVE =
    "flowchart LR\n    A --> B --> C\n    C --> D\n    D --> E\n    E --> A";

  test("only digits at or past the index move", () => {
    const body = `${FIVE}\n    linkStyle 0,1,3 stroke:#000\n    linkStyle default stroke:#888`;
    expect(shifted(body, 1, 1)).toBe(
      `${FIVE}\n    linkStyle 0,2,4 stroke:#000\n    linkStyle default stroke:#888`,
    );
  });

  test("the edits touch nothing but the digits", () => {
    const body = `${FIVE}\n    linkStyle 3 stroke:#000`;
    const edits = shiftLinkStyles(parseFlowchart(body), 0, 1);
    expect(edits).toEqual([
      {
        from: body.indexOf("3 stroke"),
        to: body.indexOf("3 stroke") + 1,
        insert: "4",
      },
    ]);
  });

  test("f10 without edge 1 is the removal Task 6 needs", () => {
    const body = fixture("f10");
    expect(shifted(body, 1, -1, new Set([1]))).toBe(
      body.replace("linkStyle 1,3 ", "linkStyle 2 "),
    );
  });

  test.each([
    ["the first", "0,1,2", new Set([0]), 0, "0,1"],
    ["the middle", "0,1,2", new Set([1]), 1, "0,1"],
    ["the last", "0,1,2", new Set([2]), 2, "0,1"],
    ["two in a row", "0,1,2,3", new Set([1, 2]), 1, "0,1"],
    ["two apart", "0,1,2,3,4", new Set([1, 3]), 1, "0,1,2"],
  ])("removing %s", (_, list, removed, at, wanted) => {
    const body = `${FIVE}\n    linkStyle ${list} stroke:#000`;
    expect(shifted(body, at, -1, removed)).toBe(
      `${FIVE}\n    linkStyle ${wanted} stroke:#000`,
    );
  });

  test("a line with all its indices removed goes with its line break", () => {
    const line = "    linkStyle 1,2 stroke:#000";
    expect(
      shifted(
        `${FIVE}\n${line}\n    %% peer note\n    E --> B`,
        1,
        -1,
        new Set([1, 2]),
      ),
    ).toBe(`${FIVE}\n    %% peer note\n    E --> B`);
    expect(shifted(`${FIVE}\n${line}`, 1, -1, new Set([1, 2]))).toBe(FIVE);
  });

  test("a statement on a shared line goes without the rest of the line (R33)", () => {
    // No # in the value: mermaid would drop that `;` and swallow what follows.
    const shared = `${FIVE}\n    linkStyle 1 stroke-width:2px; E --> B`;
    expect(shifted(shared, 1, -1, new Set([1]))).toBe(`${FIVE}\n    E --> B`);
    const after = `${FIVE}\n    E --> B; linkStyle 1 stroke:#000`;
    expect(shifted(after, 1, -1, new Set([1]))).toBe(`${FIVE}\n    E --> B`);
    const acc = `flowchart LR\n    A --> B\n    accDescr {\n      x\n    } linkStyle 0 stroke:#000`;
    expect(shifted(acc, 0, -1, new Set([0]))).toBe(
      "flowchart LR\n    A --> B\n    accDescr {\n      x\n    } ",
    );
  });

  test("linkStyle default is never touched", () => {
    const body = `${FIVE}\n    linkStyle default stroke:#888`;
    expect(shifted(body, 0, 1)).toBe(body);
    expect(shifted(body, 0, -1, new Set([0, 1, 2]))).toBe(body);
  });
});
