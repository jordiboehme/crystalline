// NO vi.mock("mermaid") here: these are the pins on mermaid 12.2.0's own
// flowchart db and on the ids its renderer writes, so the real parser runs
// (as in `mermaidStarters.test.ts`). jsdom cannot lay a diagram out, so the
// rendered side is read from the SVGs `scripts/record-diagram-svgs.ts`
// recorded in Chromium under the svg id `rec-<fixture name>`.
//
// What is pinned here is what the editor's hit mapping and its agreement
// check build on (Part 4 plan, Task 1): edge ids by start and end pair with
// `_1` skipped, explicit `e1@-->` ids, `&` groups in start-outer end-inner
// order with an explicit id on the last start and first end, chains left to
// right, node DOM ids taken from the db and never computed, and the comment
// and direction rules of mermaid's preprocessing and lexer.

import { describe, expect, test } from "vitest";

import { MERMAID_SKELETON } from "../toolbar";
import { type FlowDbView, flowDbOf } from "./mermaidDb";

const FLOWCHART = import.meta.glob<string>(
  "../fixtures/diagrams/flowchart/*.mmd",
  { query: "?raw", import: "default", eager: true },
);
const FALLBACK = import.meta.glob<string>(
  "../fixtures/diagrams/fallback/*.mmd",
  { query: "?raw", import: "default", eager: true },
);
const SVG = import.meta.glob<string>("../fixtures/diagrams/svg/*.svg", {
  query: "?raw",
  import: "default",
  eager: true,
});

/** `../fixtures/diagrams/flowchart/f16-duplicate-edges.mmd` -> `f16-duplicate-edges`. */
function nameOf(path: string): string {
  const file = path.slice(path.lastIndexOf("/") + 1);
  return file.slice(0, file.lastIndexOf("."));
}

function byName(files: Record<string, string>): Map<string, string> {
  return new Map(
    Object.entries(files).map(([path, text]) => [nameOf(path), text]),
  );
}

const flowcharts = byName(FLOWCHART);
const fallbacks = byName(FALLBACK);
const svgs = byName(SVG);
const corpus = new Map([...flowcharts, ...fallbacks]);

function fixture(name: string): string {
  const text = corpus.get(name);
  if (text === undefined) {
    throw new Error(`no fixture ${name}`);
  }
  return text;
}

async function db(body: string): Promise<FlowDbView> {
  const view = await flowDbOf(body, false);
  if ("error" in view) {
    throw new Error(`mermaid refused: ${String(view.error)}`);
  }
  return view;
}

function pairs(view: FlowDbView): string[] {
  return view.edges.map((e) => `${e.start}-${e.end}`);
}

describe("the corpus", () => {
  test("holds the named cases and at least forty realistic flowcharts", () => {
    for (const name of [
      "f01-skeleton",
      "f05-amp-groups",
      "f16-duplicate-edges",
      "f24-real-shape",
    ]) {
      expect(flowcharts.has(name)).toBe(true);
    }
    expect([...fallbacks.keys()].sort()).toEqual([
      "x01-multiline-at-shape",
      "x02-multiline-markdown-label",
      "x03-unknown-statement",
      "x04-mermaid-rejects",
    ]);
    const realistic = [...flowcharts.keys()].filter((n) => n.startsWith("r"));
    expect(realistic.length).toBeGreaterThanOrEqual(40);
  });

  test("f01 is the toolbar skeleton's body, byte for byte", () => {
    expect(fixture("f01-skeleton")).toBe(
      `${MERMAID_SKELETON.slice(1, -1).join("\n")}\n`,
    );
  });

  test("every other fixture carries a whole-line %% comment", () => {
    for (const [name, text] of corpus) {
      if (name === "f01-skeleton") {
        continue;
      }
      expect(/^[ \t]*%%(?!\{)[^\n]+/m.test(text), name).toBe(true);
    }
  });

  test("every fixture but the rejected one has a recording, and only those", () => {
    const drawn = [...corpus.keys()].filter((n) => !n.startsWith("x04"));
    // Flowchart (f, r) and fallback (x) recordings only: the sequence set
    // (s, y) shares the folder from Task 14 on.
    const flowSvgs = [...svgs.keys()].filter((n) => /^[frx]\d/.test(n));
    expect(flowSvgs.sort()).toEqual(drawn.sort());
  });
});

describe("mermaid 12.2.0 flowchart ids", () => {
  test("a repeated start and end pair counts _0, _2, _3", async () => {
    const view = await db(fixture("f16-duplicate-edges"));
    expect(view.edges.map((e) => e.id)).toEqual([
      "L_A_B_0",
      "L_A_B_2",
      "L_A_B_3",
    ]);
  });

  test("an explicit edge id is the id", async () => {
    const view = await db(fixture("f09-edge-labels"));
    expect(view.edges.map((e) => [e.id, e.start, e.end, e.text])).toEqual([
      ["L_A_B_0", "A", "B", "pipe label"],
      ["L_B_C_0", "B", "C", "text label"],
      ["L_C_D_0", "C", "D", "quoted label"],
      ["e1", "D", "E", ""],
    ]);
  });

  test("an explicit id already taken falls back to the pair counter", async () => {
    const view = await db("flowchart LR\n  A e1@--> B\n  C e1@--> D\n");
    expect(view.edges.map((e) => e.id)).toEqual(["e1", "L_C_D_0"]);
  });

  test("an & group runs start outer, end inner", async () => {
    const view = await db(fixture("f05-amp-groups"));
    expect(pairs(view)).toEqual(["A-C", "A-D", "B-C", "B-D"]);
  });

  test("an explicit id on an & group lands on the last start and first end", async () => {
    const view = await db("flowchart TD\n  X & Y e2@--> P & Q\n");
    expect(view.edges.map((e) => [e.id, e.start, e.end])).toEqual([
      ["L_X_P_0", "X", "P"],
      ["L_X_Q_0", "X", "Q"],
      ["e2", "Y", "P"],
      ["L_Y_Q_0", "Y", "Q"],
    ]);
  });

  test("chains reduce left to right", async () => {
    const view = await db(fixture("f04-chains"));
    expect(pairs(view)).toEqual(["A-B", "B-C", "C-D", "B-E", "E-F"]);
  });

  test("the domId suffix counts every addVertex call, so it is read, never computed", async () => {
    const view = await db(fixture("f16-duplicate-edges"));
    expect(view.vertices.map((v) => v.domId)).toEqual([
      "flowchart-A-0",
      "flowchart-B-1",
    ]);
    const late = await db(fixture("f22-late-label"));
    expect(late.vertices.map((v) => [v.id, v.domId, v.text])).toEqual([
      ["A", "flowchart-A-0", "Start"],
      ["B", "flowchart-B-1", "B"],
      ["C", "flowchart-C-3", "C"],
    ]);
  });

  test("ids keep their dashes and underscores", async () => {
    const view = await db(fixture("f17-odd-ids"));
    expect(view.vertices.map((v) => v.domId)).toEqual([
      "flowchart-step-1-0",
      "flowchart-a_b-1",
      "flowchart-X1-3",
    ]);
    expect(view.edges.map((e) => e.id)).toEqual([
      "L_step-1_a_b_0",
      "L_a_b_X1_0",
    ]);
  });

  test("parses do not share state", async () => {
    const first = await db(fixture("f16-duplicate-edges"));
    const second = await db(fixture("f16-duplicate-edges"));
    expect(second).toEqual(first);
  });
});

describe("mermaid 12.2.0 flowchart grammar facts", () => {
  test("headers and directions", async () => {
    expect((await db(fixture("f18-no-direction"))).direction).toBe("TB");
    expect((await db(fixture("f19-graph-lr"))).direction).toBe("LR");
    expect((await db(fixture("f02-semicolons"))).direction).toBe("TB");
    expect((await db(fixture("f14-odd-spacing"))).direction).toBe("TB");
    expect((await db("flowchart-elk LR\n  A --> B\n")).direction).toBe("LR");
  });

  test("the classic shapes and their db types", async () => {
    const view = await db(fixture("f06-all-shapes"));
    expect(view.vertices.map((v) => [v.id, v.type])).toEqual([
      ["sq", "square"],
      ["ro", "round"],
      ["st", "stadium"],
      ["sub", "subroutine"],
      ["cyl", "cylinder"],
      ["ci", "circle"],
      ["od", "odd"],
      ["di", "diamond"],
      ["hx", "hexagon"],
      ["lr", "lean_right"],
      ["ll", "lean_left"],
      ["tr", "trapezoid"],
      ["it", "inv_trapezoid"],
      ["dc", "doublecircle"],
    ]);
    // A bare id has no type at all.
    const bare = await db(fixture("f16-duplicate-edges"));
    expect(bare.vertices.map((v) => v.type)).toEqual([undefined, undefined]);
  });

  test("(-x-) parses as an ellipse that mermaid 12.2.0 cannot draw", async () => {
    const view = await db("flowchart LR\n  A(-x-)\n");
    expect(view.vertices.map((v) => v.type)).toEqual(["ellipse"]);
  });

  test("an @{ shape } keeps the shape name as the db type", async () => {
    const view = await db(fixture("f07-at-shapes"));
    expect(view.vertices.map((v) => [v.id, v.type, v.text])).toEqual([
      ["A", "diam", "x"],
      ["B", "rect", "Square"],
      ["C", "cyl", "Store"],
    ]);
  });

  test("link tokens: type, stroke and length", async () => {
    const view = await db(fixture("f15-arrow-kinds"));
    expect(view.edges.map((e) => [e.end, e.type, e.stroke, e.length])).toEqual([
      ["B", "arrow_point", "normal", 2],
      ["C", "arrow_point", "dotted", 1],
      ["D", "arrow_point", "dotted", 2],
      ["E", "arrow_point", "thick", 1],
      ["F", "arrow_open", "thick", 1],
      ["G", "arrow_open", "invisible", 1],
      ["H", "arrow_circle", "normal", 1],
      ["I", "arrow_cross", "normal", 1],
      ["J", "double_arrow_point", "normal", 1],
      ["K", "double_arrow_point", "dotted", 1],
      ["L", "double_arrow_point", "thick", 1],
    ]);
  });

  test("labels reach the db with entities still encoded and <br> kept", async () => {
    const view = await db(fixture("f08-labels"));
    expect(view.vertices.map((v) => v.text)).toEqual([
      "Say ﬂ\xB0quot\xB6\xDFhelloﬂ\xB0quot\xB6\xDF",
      "first line<br>second line",
      "Grüße aus Köln",
      "Ship it 🚀",
      "has ( and ] inside",
    ]);
  });

  test("markdown labels lose their backticks and quotes in the db", async () => {
    const view = await db(fixture("f21-markdown-labels"));
    expect(view.vertices.map((v) => v.text)).toEqual([
      "**Engram** file",
      "_derived_ index",
    ]);
  });

  test("style, classDef, class, ::: and click", async () => {
    const view = await db(fixture("f11-style-class"));
    expect(view.vertices.map((v) => [v.id, v.classes, v.styles])).toEqual([
      ["A", ["hot"], []],
      ["B", ["hot"], []],
      ["C", ["done"], ["stroke-width:3px"]],
    ]);
    const click = await db(fixture("f12-click"));
    expect(click.vertices.map((v) => v.classes)).toEqual([
      ["clickable"],
      ["clickable"],
    ]);
  });

  test("subgraphs: ids, titles and the nodes the db gives them", async () => {
    const view = await db(fixture("f13-subgraphs"));
    expect(view.subgraphs).toMatchObject([
      { id: "inner", title: "Engine", nodes: ["Y", "X"] },
      { id: "outer", title: "Daemon", nodes: ["inner", "Z"] },
    ]);
    const plain = await db(
      'flowchart TD\n  subgraph "Spaced title"\n    A\n  end\n  subgraph s2\n    B\n  end\n  subgraph two words\n    C\n  end\n',
    );
    expect(plain.subgraphs).toMatchObject([
      { id: "subGraph0", title: "Spaced title", nodes: ["A"] },
      { id: "s2", title: "s2", nodes: ["B"] },
      { id: "subGraph2", title: "two words", nodes: ["C"] },
    ]);
  });

  test("the one-line accDescr parses beside the block form", async () => {
    const view = await db(
      "flowchart TD\n  accTitle: Save path\n  accDescr: One line\n  A --> B\n",
    );
    expect(view.vertices.map((v) => v.id)).toEqual(["A", "B"]);
  });

  test("an edge into a subgraph makes its id a vertex too", async () => {
    const view = await db(fixture("r23-env-overlay"));
    expect(view.subgraphs.map((s) => s.id)).toEqual(["loaded"]);
    expect(view.vertices.map((v) => v.id)).toContain("loaded");
  });

  test("comments: whole lines only, and a bare %% is a node", async () => {
    const bare = await db("flowchart TD\n%%\n  A --> B\n");
    expect(bare.vertices.map((v) => v.id)).toEqual(["%%", "A", "B"]);
    const trailing = await flowDbOf(
      "flowchart TD\n  A --> B %% trailing\n",
      false,
    );
    expect("error" in trailing).toBe(true);
  });

  test("a line that says direction LR anywhere is a direction statement", async () => {
    const view = await db(
      'flowchart TD\n  A["label direction LR here"] --> B\n',
    );
    expect(view.vertices).toEqual([]);
    expect(view.edges).toEqual([]);
  });

  test("reserved words cannot be node ids, default and v can", async () => {
    for (const word of [
      "end",
      "end-point",
      "graph",
      "style",
      "class",
      "click",
      "flowchart",
      "subgraph",
      "classDef",
      "linkStyle",
      "call",
      "href",
      "interpolate",
    ]) {
      const result = await flowDbOf(`flowchart TD\n  A --> ${word}\n`, false);
      expect("error" in result, word).toBe(true);
    }
    for (const word of ["default", "v", "end1", "endpoint", "subgraph6"]) {
      const view = await db(`flowchart TD\n  A --> ${word}\n`);
      expect(
        view.vertices.map((v) => v.id),
        word,
      ).toEqual(["A", word]);
    }
  });

  test("an arrow without spaces eats a leading o or x of the next id", async () => {
    const view = await db("flowchart TD\n  A---oauth\n");
    expect(view.edges.map((e) => [e.end, e.type])).toEqual([
      ["auth", "arrow_circle"],
    ]);
  });

  test("a linkStyle index past the last edge is refused", async () => {
    const result = await flowDbOf(
      "flowchart TD\n  A --> B\n  linkStyle 1 stroke:#f00\n",
      false,
    );
    expect("error" in result).toBe(true);
  });

  test("a diagram that is not a flowchart is an error, not a view", async () => {
    const result = await flowDbOf("sequenceDiagram\n  A->>B: hi\n", false);
    expect("error" in result).toBe(true);
  });

  test("the rejected fallback is an error, never a throw", async () => {
    const result = await flowDbOf(fixture("x04-mermaid-rejects"), false);
    expect("error" in result).toBe(true);
  });
});

describe("the recorded SVGs agree with the db", () => {
  const drawn = [...corpus.keys()].filter((n) => svgs.has(n)).sort();

  test.each(drawn)("%s", async (name) => {
    const view = await db(fixture(name));
    const div = document.createElement("div");
    div.innerHTML = svgs.get(name) ?? "";
    const prefix = `rec-${name}-`;
    const svg = div.querySelector("svg");
    expect(svg?.id).toBe(`rec-${name}`);

    // A vertex whose id is also a subgraph id (an edge that points at the
    // subgraph) is drawn as the cluster, never as a g.node.
    const groups = new Set(view.subgraphs.map((s) => s.id));
    const nodeIds = view.vertices
      .filter((v) => !groups.has(v.id))
      .map((v) => prefix + v.domId)
      .sort();
    const drawnNodes = [...div.querySelectorAll("g.node")]
      .map((g) => g.id)
      .sort();
    expect(drawnNodes).toEqual(nodeIds);

    const clusters = [...div.querySelectorAll("g.cluster")]
      .map((g) => g.id)
      .sort();
    expect(clusters).toEqual(view.subgraphs.map((s) => prefix + s.id).sort());

    const paths = [...div.querySelectorAll('path[data-et="edge"]')];
    expect(paths.map((p) => p.getAttribute("data-id")).sort()).toEqual(
      view.edges.map((e) => e.id).sort(),
    );
    for (const path of paths) {
      expect(path.id).toBe(prefix + (path.getAttribute("data-id") ?? ""));
    }
    const labels = [...div.querySelectorAll("g.label[data-id]")];
    expect(labels.map((l) => l.getAttribute("data-id")).sort()).toEqual(
      view.edges.map((e) => e.id).sort(),
    );
  });
});
