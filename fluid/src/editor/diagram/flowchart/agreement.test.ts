// NO vi.mock("mermaid") here: every corpus file runs through mermaid
// 12.2.0's real parser, and our model of it must agree with its db.

import { describe, expect, test } from "vitest";

import { type FlowDbView, flowDbOf } from "../mermaidDb";
import { scanFence } from "../scan";
import { agreesFlow, normalizeLabel } from "../verdict";
import { parseFlowchart } from "./parse";

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
 * Inputs at the edges of the grammar, each run through the parser and
 * mermaid: whatever the parser understands, mermaid either refuses (the
 * verdict then says no) or reads the same way. Never understood and
 * different.
 */
const EDGE_CASES: string[] = [
  "flowchart LR\n  A(-x-)",
  'flowchart TD\n  A["go direction LR now"] --> B',
  "flowchart TD\n  direction LR\n  A --> B",
  "flowchart TD\n%%\n  A --> B",
  "flowchart TD\n  A --> B %% note",
  "flowchart TD\n  A --> end",
  "flowchart TD\n  A --> end-point",
  "flowchart TD\n  subgraph s\n    A",
  "flowchart TD\n  A\n  end",
  "flowchart-elk LR\n  A --> B",
  "flowchart TD\n  A --> B\n  A B",
  "flowchart TD\n  A -->",
  'flowchart TD\n  A["one\n  two"] --> B',
  "flowchart TD\n  A --> B\n  linkStyle 1 stroke:#f00",
  "flowchart TD\n  A <--- B",
  'flowchart TD\n  A@{ shape: rect, label: "x", icon: "y" }',
  "flowchart TD\n  subgraph g\n    A\n  end\n  g[Label] --> A",
  "flowchart TD\n  A&B --> C",
  "flowchart TD\n  A --x B",
  "flowchart TD\n  A x--x B",
  "flowchart TD\n  A o--o B",
  "flowchart TD\n  A -- box--> B",
  "flowchart TD\n  A---oauth",
  "flowchart TD\n  A -->|  spaced  | B",
  "flowchart TD\n  A --> |x| B",
  "flowchart TD\n  A ==text==> B",
  "flowchart TD\n  A == text ==> B",
  "flowchart TD\n  A -. text .-> B",
  "flowchart TD\n  A -. text ..-> B",
  "flowchart TD\n  A <-- text --> B",
  "flowchart TD\n  A ----------------> B",
  "flowchart TD\n  A[ padded ] --> B",
  "flowchart TD\n  A[a/b] --> B[\\x\\]",
  "flowchart TD\n  A[/a\\b/] --> B",
  "flowchart TD\n  1A --> B",
  "flowchart TD\n  v --> default",
  "flowchart TD\n  A.B --> C",
  "flowchart TD\n  A:::c --> B[x]:::d",
  "flowchart TD\n  A --> B\n  style A fill:#f9f;",
  "flowchart TD\n  A --> B\n  classDef c fill:#f9f;",
  "flowchart >\n  A --> B",
  "graph\n  A --> B",
  "flowchart TD\n  A@{ shape: rect, label: Plain words }",
  'flowchart TD\n  A@{ label: "x", shape: diam } --> B',
  'flowchart TD\n  A@{ shape: foo, label: "x" }',
  "flowchart TD\n  subgraph a\n    A\n  end\n  subgraph b\n    A --> B\n  end",
  "flowchart TD\n  subgraph a\n    subgraph b\n      A\n    end\n    A --> C\n  end",
  "flowchart TD\n  A --> s\n  subgraph s\n    B\n  end\n  style s fill:#fff",
  "flowchart TD\n  subgraph x[Title]\n    A\n  end",
  'flowchart TD\n  subgraph x ["Quoted title"]\n    A\n  end',
  "flowchart TD\n  A e1@--> B\n  B --> e1",
  "flowchart TD\n  A[Issue #12] --> B[a #35; b]",
  'flowchart TD\n  A["a &amp; b"] --> B["<b>bold</b>"]',
  "flowchart TD\n  A[x<br/>y] --> B[x<br />y]",
  "flowchart TD\n  A --> B & C\n  linkStyle 1 stroke:#f00",
  "flowchart TD\n  A --> B\n  click A call cb()",
  "flowchart TD\n  A --> B\n  click A cb",
  "flowchart TD\n  A --> B\n  linkStyle 0 interpolate basis stroke:#f00",
  "flowchart TD\n  A --> B\n  linkStyle 0 interpolate basis",
  "flowchart TD\n  A-->B-->C",
  "flowchart TD\n  A -.- B",
  "flowchart TD\n  A ~~~~ B",
  "flowchart TD\n  A ==o B",
  "flowchart TD\n  A -.-x B",
  "flowchart TD\n  A(((x))) --> B((y))",
  'flowchart TD\n  A["`md`"] -- "`md edge`" --> B',
  'flowchart TD\n  A -->|"q"| B',
  "flowchart TD\n  A --> B;C --> D;",
  "flowchart TD\n  Köln --> B",
  "flowchart TD\n  A[Köln 🚀] --> B",
  'flowchart TD\n  A["x"] --> A[y]',
  'flowchart TD\n  A --> B\n  A@{ shape: rect, label: "later" }',
  "flowchart TD\n  A[x] --> B\n  A",
  "flowchart TD\n  A[x]:::c",
  "flowchart TD\n  A[\"it's\"] --> B['single']",
  "flowchart TD\n  A --> B\n  class A,B c\n  classDef c fill:#fff,stroke:#000",
  "flowchart TD\n  A --> B\n  style A fill:#fff, stroke:#000",
  "flowchart TD\n  A --> B\n  style A stroke-dasharray: 5 5",
  "flowchart TD\n  A -- x -- y --> B",
  "flowchart TD\n  A --> o\n  o --> x",
  "flowchart TD\n  A --> xB",
  "flowchart TD\n  A-->xB",
  "flowchart TD\n  A-->oB",
  "flowchart TD\n  A -->x B",
  "flowchart TD\n  subgraph end1\n    A\n  end",
  "flowchart TD\n  subgraph\n    A\n  end",
  "flowchart TD\n  subgraph one two\n    A\n  end\n  subgraph three\n    B\n  end",
];

describe("edge cases: what is understood agrees", () => {
  test.each(EDGE_CASES)("%j", async (body) => {
    const model = parseFlowchart(body);
    if (!model.understood) {
      expect(model.reasons.length).toBeGreaterThan(0);
      return;
    }
    const view = await flowDbOf(body, false);
    if (!("error" in view)) {
      expect(agreesFlow(model, view)).toBe(null);
    }
  });

  test("a bare subgraph line, which mermaid throws on, is not understood", () => {
    expect(
      parseFlowchart("flowchart TD\n  subgraph\n    A\n  end").understood,
    ).toBe(false);
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
