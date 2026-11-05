// NO vi.mock("mermaid") here: every corpus file runs through mermaid
// 12.2.0's real parser, and our model of it must agree with its db.

import { describe, expect, test } from "vitest";

import { type FlowDbView, flowDbOf } from "../mermaidDb";
import { scanFence } from "../scan";
import { agreesFlow, normalizeLabel } from "../verdict";
import { MAX_EDGES, MAX_TEXT_SIZE, parseFlowchart } from "./parse";

const FLOWCHART = import.meta.glob<string>(
  "../../fixtures/diagrams/flowchart/*.mmd",
  { query: "?raw", import: "default", eager: true },
);

function nameOf(path: string): string {
  const file = path.slice(path.lastIndexOf("/") + 1);
  return file.slice(0, file.lastIndexOf("."));
}

interface Fs {
  readdirSync: (path: string) => string[];
  readFileSync: (path: string, encoding: "utf8") => string;
}

/** Node's process, reached without Node types (as in `mermaidVersion.test.ts`). */
const node = globalThis as unknown as {
  process: {
    env: Record<string, string | undefined>;
    getBuiltinModule(id: string): unknown;
  };
};

const corpus = Object.entries(FLOWCHART)
  .map(([path, text]) => [nameOf(path), text] as const)
  .sort(([a], [b]) => a.localeCompare(b));

async function db(body: string): Promise<FlowDbView> {
  const view = await flowDbOf(body, false);
  if ("error" in view) {
    throw new Error(`mermaid refused: ${String(view.error)}`);
  }
  return view;
}

/**
 * What a row of the table below must do:
 * - `agrees`: understood, and mermaid's db reads it the same way
 * - `refused`: not understood, so it falls back to text before mermaid is asked
 * - `mermaidRefuses`: understood, but mermaid refuses it, so the verdict says no
 * - `gate`: understood, but mermaid reads it differently; `agreesFlow` names
 *   the difference, so the verdict says no and it falls back to text
 */
type EdgeState = "agrees" | "refused" | "mermaidRefuses" | "gate";

/**
 * Inputs at the edges of the grammar, each run through the parser and
 * mermaid. Understood and read differently by mermaid does happen (the `gate`
 * rows); the agreement check is the net that catches it, and the body stays
 * text.
 */
const EDGE_CASES: [string, EdgeState][] = [
  ["flowchart LR\n  A(-x-)", "refused"],
  ['flowchart TD\n  A["go direction LR now"] --> B', "refused"],
  ["flowchart TD\n  direction LR\n  A --> B", "refused"],
  ["flowchart TD\n%%\n  A --> B", "refused"],
  ["flowchart TD\n  A --> B %% note", "refused"],
  ["flowchart TD\n  A --> end", "refused"],
  ["flowchart TD\n  A --> end-point", "refused"],
  ["flowchart TD\n  subgraph s\n    A", "refused"],
  ["flowchart TD\n  A\n  end", "refused"],
  ["flowchart-elk LR\n  A --> B", "refused"],
  ["flowchart TD\n  A --> B\n  A B", "refused"],
  ["flowchart TD\n  A -->", "refused"],
  ['flowchart TD\n  A["one\n  two"] --> B', "refused"],
  ["flowchart TD\n  A --> B\n  linkStyle 1 stroke:#f00", "refused"],
  ["flowchart TD\n  A <--- B", "refused"],
  ['flowchart TD\n  A@{ shape: rect, label: "x", icon: "y" }', "refused"],
  ["flowchart TD\n  subgraph g\n    A\n  end\n  g[Label] --> A", "refused"],
  ["flowchart TD\n  A&B --> C", "refused"],
  ["flowchart TD\n  A --x B", "agrees"],
  ["flowchart TD\n  A x--x B", "agrees"],
  ["flowchart TD\n  A o--o B", "agrees"],
  ["flowchart TD\n  A -- box--> B", "refused"],
  ["flowchart TD\n  A---oauth", "agrees"],
  ["flowchart TD\n  A -->|  spaced  | B", "agrees"],
  ["flowchart TD\n  A --> |x| B", "agrees"],
  ["flowchart TD\n  A ==text==> B", "agrees"],
  ["flowchart TD\n  A == text ==> B", "agrees"],
  ["flowchart TD\n  A -. text .-> B", "agrees"],
  ["flowchart TD\n  A -. text ..-> B", "agrees"],
  ["flowchart TD\n  A <-- text --> B", "agrees"],
  ["flowchart TD\n  A ----------------> B", "agrees"],
  ["flowchart TD\n  A[ padded ] --> B", "agrees"],
  ["flowchart TD\n  A[a/b] --> B[\\x\\]", "agrees"],
  ["flowchart TD\n  A[/a\\b/] --> B", "agrees"],
  ["flowchart TD\n  1A --> B", "agrees"],
  ["flowchart TD\n  v --> default", "agrees"],
  ["flowchart TD\n  A.B --> C", "refused"],
  ["flowchart TD\n  A:::c --> B[x]:::d", "agrees"],
  ["flowchart TD\n  A --> B\n  style A fill:#f9f;", "agrees"],
  ["flowchart TD\n  A --> B\n  classDef c fill:#f9f;", "agrees"],
  ["flowchart >\n  A --> B", "agrees"],
  ["graph\n  A --> B", "agrees"],
  ["flowchart TD\n  A@{ shape: rect, label: Plain words }", "agrees"],
  ['flowchart TD\n  A@{ label: "x", shape: diam } --> B', "agrees"],
  ['flowchart TD\n  A@{ shape: foo, label: "x" }', "mermaidRefuses"],
  [
    "flowchart TD\n  subgraph a\n    A\n  end\n  subgraph b\n    A --> B\n  end",
    "agrees",
  ],
  [
    "flowchart TD\n  subgraph a\n    subgraph b\n      A\n    end\n    A --> C\n  end",
    "agrees",
  ],
  [
    "flowchart TD\n  A --> s\n  subgraph s\n    B\n  end\n  style s fill:#fff",
    "agrees",
  ],
  ["flowchart TD\n  subgraph x[Title]\n    A\n  end", "agrees"],
  ['flowchart TD\n  subgraph x ["Quoted title"]\n    A\n  end', "agrees"],
  ["flowchart TD\n  A e1@--> B\n  B --> e1", "refused"],
  ["flowchart TD\n  A[Issue #12] --> B[a #35; b]", "agrees"],
  ['flowchart TD\n  A["a &amp; b"] --> B["<b>bold</b>"]', "agrees"],
  ["flowchart TD\n  A[x<br/>y] --> B[x<br />y]", "agrees"],
  ["flowchart TD\n  A --> B & C\n  linkStyle 1 stroke:#f00", "agrees"],
  ["flowchart TD\n  A --> B\n  click A call cb()", "agrees"],
  ["flowchart TD\n  A --> B\n  click A cb", "agrees"],
  [
    "flowchart TD\n  A --> B\n  linkStyle 0 interpolate basis stroke:#f00",
    "agrees",
  ],
  ["flowchart TD\n  A --> B\n  linkStyle 0 interpolate basis", "agrees"],
  ["flowchart TD\n  A-->B-->C", "agrees"],
  ["flowchart TD\n  A -.- B", "agrees"],
  ["flowchart TD\n  A ~~~~ B", "agrees"],
  ["flowchart TD\n  A ==o B", "agrees"],
  ["flowchart TD\n  A -.-x B", "agrees"],
  ["flowchart TD\n  A(((x))) --> B((y))", "agrees"],
  ['flowchart TD\n  A["`md`"] -- "`md edge`" --> B', "agrees"],
  ['flowchart TD\n  A -->|"q"| B', "agrees"],
  ["flowchart TD\n  A --> B;C --> D;", "agrees"],
  ["flowchart TD\n  Köln --> B", "refused"],
  ["flowchart TD\n  A[Köln 🚀] --> B", "agrees"],
  ['flowchart TD\n  A["x"] --> A[y]', "agrees"],
  ['flowchart TD\n  A --> B\n  A@{ shape: rect, label: "later" }', "agrees"],
  ["flowchart TD\n  A[x] --> B\n  A", "agrees"],
  ["flowchart TD\n  A[x]:::c", "agrees"],
  ["flowchart TD\n  A[\"it's\"] --> B['single']", "agrees"],
  [
    "flowchart TD\n  A --> B\n  class A,B c\n  classDef c fill:#fff,stroke:#000",
    "agrees",
  ],
  ["flowchart TD\n  A --> B\n  style A fill:#fff, stroke:#000", "agrees"],
  ["flowchart TD\n  A --> B\n  style A stroke-dasharray: 5 5", "agrees"],
  ["flowchart TD\n  A -- x -- y --> B", "refused"],
  ["flowchart TD\n  A --> o\n  o --> x", "agrees"],
  ["flowchart TD\n  A --> xB", "agrees"],
  ["flowchart TD\n  A-->xB", "agrees"],
  ["flowchart TD\n  A-->oB", "agrees"],
  ["flowchart TD\n  A -->x B", "refused"],
  ["flowchart TD\n  subgraph end1\n    A\n  end", "agrees"],
  ["flowchart TD\n  subgraph\n    A\n  end", "refused"],
  [
    "flowchart TD\n  subgraph one two\n    A\n  end\n  subgraph three\n    B\n  end",
    "agrees",
  ],
  [
    "flowchart TD\n  A --> B\n  style A fill:#f00; style B fill:#0f0",
    "refused",
  ],
  [
    "flowchart TD\n  A --> B\n  classDef a fill:#f00; classDef b fill:#0f0",
    "refused",
  ],
  ["flowchart TD\n  A --> B\n  style A fill:#abc;x:1", "refused"],
  ["flowchart TD\n  A_B --> C\n  A --> B_C", "refused"],
  ["flowchart TD\n  A_B --> C_D\n  A --> B_C_D", "refused"],
  ["flowchart TD\n  C L_A_B_0@--> D\n  A --> B", "refused"],
  ["flowchart TD\n  subgraph s\n    C\n  end\n  A s@--> B", "refused"],
  ["flowchart TD\n  A:::1x-.-> B", "refused"],
  ["flowchart TD\n  A:::1o-.- B", "refused"],
  ["flowchart TD\n  A --> B\n  class A 1x", "refused"],
  ["flowchart TD\n  A:::c1 --> B", "agrees"],
  [
    'flowchart TD\n  class K hot\n  K --> B\n  class K warm\n  click K href "u"',
    "agrees",
  ],
  [
    "flowchart TD\n  subgraph s\n  accTitle: direction LR\n  A\n  end",
    "refused",
  ],
  ["flowchart TD\n  accTitle: direction LR\n  A", "refused"],
  ["flowchart TD\n  accDescr: direction LR\n  A", "refused"],
  ["flowchart TD\n  accDescr { direction BT }\n  A", "refused"],
  ["flowchart TD\n  accDescr {\n    direction BT\n  }\n  A", "agrees"],
  ["flowchart TD\naccTitle: direction LR\n  A", "agrees"],
  ["flowchart TD\n  A@{ shape: rect, label: True }", "refused"],
  ["flowchart TD\n  A@{ shape: rect, label: TRUE }", "refused"],
  ["flowchart TD\n  A@{ shape: rect, label: False }", "refused"],
  ["flowchart TD\n  A@{ shape: rect, label: Null }", "refused"],
  ["flowchart TD\n  A@{ shape: rect, label: NULL }", "refused"],
  ["flowchart TD\n  A@{ shape: rect, label: yes }", "agrees"],
  ['flowchart TD\n  A@{ shape: rect, label: "True" }', "agrees"],
  ['flowchart TD\n  A@{ shape: icon, label: "x" }', "refused"],
  ['flowchart TD\n  A@{ shape: image, label: "x" }', "mermaidRefuses"],
  ["flowchart TD\n  1x-->2", "gate"],
  ["flowchart TD\n  1o---2", "gate"],
  ["flowchart TD\n  subgraph s\n  accTitle: hello\n  A\n  end", "gate"],
  ["flowchart TD\n  A -- a<b --> B", "gate"],
  ["flowchart TD\n  A@{shape:rect,label:x}", "gate"],
];

function stateOf(
  body: string,
  view: FlowDbView | { error: unknown },
): EdgeState {
  const model = parseFlowchart(body);
  if (!model.understood) {
    expect(model.reasons.length).toBeGreaterThan(0);
    return "refused";
  }
  if ("error" in view) {
    return "mermaidRefuses";
  }
  return agreesFlow(model, view) === null ? "agrees" : "gate";
}

describe("edge cases", () => {
  test.each(EDGE_CASES)("%j is %s", async (body, state) => {
    expect(stateOf(body, await flowDbOf(body, false))).toBe(state);
  });

  test("the indent decides whether mermaid lexes an acc line as a direction", async () => {
    const indented = await db(
      "flowchart TD\n  subgraph s\n  accTitle: direction LR\n  A\n  end",
    );
    expect(indented.subgraphs[0]?.nodes).toEqual(["A"]);
    const flush = await db(
      "flowchart TD\n  subgraph s\naccTitle: direction LR\n  A\n  end",
    );
    expect(flush.subgraphs[0]?.nodes).toContain("direction LR");
  });

  test("two edges can share an id in mermaid's db, which then cannot draw", async () => {
    const view = await db("flowchart TD\n  A_B --> C\n  A --> B_C");
    expect(view.edges.map((e) => e.id)).toEqual(["L_A_B_C_0", "L_A_B_C_0"]);
  });

  test("the db holds a joined style line as one value", async () => {
    const view = await db(
      "flowchart TD\n  A --> B\n  style A fill:#f00; style B fill:#0f0",
    );
    expect(view.vertices.find((v) => v.id === "B")?.styles).toEqual([]);
  });
});

/**
 * One row per check in `agreesFlow`: parse `ours`, read the db of `theirs`,
 * which differs in exactly that field. Removing the check turns the answer
 * into null, so the row fails.
 */
const DIFFERENCES: [string, string, string, RegExp][] = [
  ["direction", "flowchart TD\n  A", "flowchart LR\n  A", /direction/i],
  [
    "subgraph id",
    "flowchart TD\n  subgraph s\n    A\n  end",
    "flowchart TD\n  subgraph t\n    A\n  end",
    /subgraph ids/,
  ],
  [
    "subgraph members",
    "flowchart TD\n  subgraph s\n    A\n  end\n  B",
    "flowchart TD\n  subgraph s\n    A\n    B\n  end",
    /subgraph s holds/,
  ],
  ["box ids", "flowchart TD\n  A", "flowchart TD\n  B", /box ids/],
  ["label", "flowchart TD\n  A[One]", "flowchart TD\n  A[Two]", /label of A/],
  ["shape", "flowchart TD\n  A[x]", "flowchart TD\n  A(x)", /shape of A/],
  [
    "@{ } shape",
    'flowchart TD\n  A@{ shape: diam, label: "x" }',
    'flowchart TD\n  A@{ shape: rect, label: "x" }',
    /shape of A/,
  ],
  [
    "box styles",
    "flowchart TD\n  A\n  style A fill:#f00",
    "flowchart TD\n  A\n  style A fill:#0f0",
    /styles of A/,
  ],
  [
    "box classes",
    "flowchart TD\n  A:::x",
    "flowchart TD\n  A:::y",
    /classes of A/,
  ],
  [
    "arrow count",
    "flowchart TD\n  A --> B",
    "flowchart TD\n  A --> B\n  A --> B",
    /arrows here/,
  ],
  [
    "arrow start",
    "flowchart TD\n  A e1@--> B\n  C",
    "flowchart TD\n  C e1@--> B\n  A",
    /Arrow 0/,
  ],
  [
    "arrow end",
    "flowchart TD\n  A e1@--> B\n  C",
    "flowchart TD\n  A e1@--> C\n  B",
    /Arrow 0/,
  ],
  [
    "arrow id",
    "flowchart TD\n  A e1@--> B",
    "flowchart TD\n  A e2@--> B",
    /Arrow 0/,
  ],
  [
    "arrow type",
    "flowchart TD\n  A --> B",
    "flowchart TD\n  A --- B",
    /drawn differently/,
  ],
  [
    "arrow stroke",
    "flowchart TD\n  A --> B",
    "flowchart TD\n  A ==> B",
    /drawn differently/,
  ],
  [
    "arrow length",
    "flowchart TD\n  A --> B",
    "flowchart TD\n  A ---> B",
    /drawn differently/,
  ],
  [
    "arrow label",
    "flowchart TD\n  A -->|x| B",
    "flowchart TD\n  A -->|y| B",
    /label of arrow 0/,
  ],
];

describe("every check of agreesFlow can say no", () => {
  test.each(DIFFERENCES)("%s", async (_field, ours, theirs, words) => {
    const model = parseFlowchart(ours);
    expect(model.understood, model.reasons.join("; ")).toBe(true);
    expect(agreesFlow(model, await db(theirs))).toMatch(words);
    // The pair itself is sound: `ours` agrees with its own db.
    expect(agreesFlow(model, await db(ours))).toBe(null);
  });

  test("a body the parser does not understand never agrees", async () => {
    const model = parseFlowchart("flowchart TD\n  A(-x-)");
    expect(agreesFlow(model, await db("flowchart TD\n  A"))).toMatch(/ellipse/);
  });
});

describe("mermaid's limits", () => {
  test("are the ones the parser refuses at", async () => {
    await flowDbOf("flowchart TD\n  A", false);
    const { default: mermaid } = await import("mermaid");
    const config = mermaid.mermaidAPI.getSiteConfig();
    expect(config.maxTextSize).toBe(MAX_TEXT_SIZE);
    expect(config.maxEdges).toBe(MAX_EDGES);
  });

  test("500 arrows are understood and agree, the 501st is refused by both", async () => {
    const line = (n: number) =>
      `flowchart TD\n${"  A --> B\n".repeat(n)}`.trimEnd();
    const fine = parseFlowchart(line(500));
    expect(fine.understood).toBe(true);
    expect(agreesFlow(fine, await db(line(500)))).toBe(null);
    expect(parseFlowchart(line(501)).understood).toBe(false);
    expect("error" in (await flowDbOf(line(501), false))).toBe(true);
  });
});

describe("the parser agrees with mermaid 12.2.0's db", () => {
  test.each(corpus)("%s", async (_name, text) => {
    for (const body of [text, text.replace(/\n$/, "")]) {
      const model = parseFlowchart(body);
      expect(model.understood, model.reasons.join("; ")).toBe(true);
      const view = await db(body);
      expect(agreesFlow(model, view)).toBe(null);
      const clusters = new Set(view.subgraphs.map((s) => s.id));
      expect(model.nodeOrder).toEqual(
        view.vertices.map((v) => v.id).filter((id) => !clusters.has(id)),
      );
    }
  });

  test("a disagreement is named in words", async () => {
    const model = parseFlowchart("flowchart TD\n  A[One] --> B");
    const view = await db("flowchart TD\n  A[Two] --> B");
    expect(agreesFlow(model, view)).toMatch(/A/);
    const edges = await db("flowchart TD\n  A[One] --> B --> C");
    expect(agreesFlow(model, edges)).not.toBe(null);
    const direction = await db("flowchart LR\n  A[One] --> B");
    expect(agreesFlow(model, direction)).toMatch(/direction/i);
  });

  test("a class line names no new vertex in the db, a style line does", async () => {
    const body = "flowchart TD\n  A --> B\n  style S fill:#fff\n  class K hot";
    const view = await db(body);
    expect(view.vertices.map((v) => v.id)).toEqual(["A", "B", "S"]);
    expect(agreesFlow(parseFlowchart(body), view)).toBe(null);
  });

  test("labels are compared through one normalizer (R6)", async () => {
    const view = await db('flowchart TD\n  A["Say #quot;hi#quot; #35;1"]');
    const text = view.vertices[0]?.text ?? "";
    expect(text).not.toContain("#quot;");
    expect(normalizeLabel(text)).toBe(
      normalizeLabel("Say #quot;hi#quot; #35;1"),
    );
  });

  test("a label of any type goes through the normalizer", () => {
    expect(normalizeLabel(true)).toBe("true");
  });

  const folder = node.process.env.FLUID_DIAGRAM_CORPUS;
  test.skipIf(folder === undefined)(
    "the opt-in corpus: what is understood agrees",
    async () => {
      const dir = folder ?? "";
      const fs = node.process.getBuiltinModule("node:fs") as Fs;
      const files = fs.readdirSync(dir).filter((f) => f.endsWith(".mmd"));
      let flowcharts = 0;
      let understood = 0;
      let agreeing = 0;
      let refused = 0;
      const failures: string[] = [];
      for (const file of files) {
        const body = fs.readFileSync(`${dir}/${file}`, "utf8");
        const keyword = scanFence(body).keyword;
        if (keyword !== "flowchart" && keyword !== "graph") {
          continue;
        }
        flowcharts++;
        const model = parseFlowchart(body);
        if (!model.understood) {
          continue;
        }
        understood++;
        const view = await flowDbOf(body, false);
        if ("error" in view) {
          refused++;
          continue;
        }
        const difference = agreesFlow(model, view);
        if (difference === null) {
          agreeing++;
        } else {
          failures.push(`${file}: ${difference}`);
        }
      }
      console.log(
        `flowchart corpus: ${understood} of ${flowcharts} understood, ${refused} of those refused by mermaid, ${agreeing} agree with mermaid`,
      );
      expect(failures).toEqual([]);
    },
  );
});
