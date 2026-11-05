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

import { MERMAID_STARTER_GROUPS } from "../mermaidStarters";
import { MERMAID_SKELETON } from "../toolbar";
import {
  type FlowDbView,
  type SequenceDbView,
  flowDbOf,
  sequenceDbOf,
} from "./mermaidDb";
import { decodeSeqText, parseSequence } from "./sequence/parse";

const FLOWCHART = import.meta.glob<string>(
  "../fixtures/diagrams/flowchart/*.mmd",
  { query: "?raw", import: "default", eager: true },
);
const SEQUENCE = import.meta.glob<string>(
  "../fixtures/diagrams/sequence/*.mmd",
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
const sequences = byName(SEQUENCE);
const corpus = new Map(
  [...flowcharts, ...fallbacks].filter(([name]) => !name.startsWith("y")),
);

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
    expect(
      [...fallbacks.keys()].filter((n) => n.startsWith("x")).sort(),
    ).toEqual([
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

async function seqDb(body: string): Promise<SequenceDbView> {
  const view = await sequenceDbOf(body, false);
  if ("error" in view) {
    throw new Error(`mermaid refused: ${String(view.error)}`);
  }
  return view;
}

function sequenceSvg(name: string): HTMLElement {
  const div = document.createElement("div");
  div.innerHTML = svgs.get(name) ?? "";
  return div;
}

describe("the sequence corpus", () => {
  test("s01 is the toolbar's Sequence starter, byte for byte", () => {
    const starter = MERMAID_STARTER_GROUPS.flatMap((g) => g.starters).find(
      (s) => s.label === "Sequence",
    );
    expect(sequences.get("s01-starter")).toBe(
      `${(starter?.lines ?? []).join("\n")}\n`,
    );
  });

  test("every other file carries a whole-line %% comment (R3 keeps s01 as it is)", () => {
    const sequenceFallbacks = [...fallbacks].filter(([n]) => n.startsWith("y"));
    expect(sequenceFallbacks.length).toBe(2);
    for (const [name, text] of [...sequences, ...sequenceFallbacks]) {
      if (name !== "s01-starter") {
        expect(/^[ \t]*%%(?!\{)[^\n]+/m.test(text), name).toBe(true);
      }
    }
  });

  test("every file and y01 have a recording, y02 has none", () => {
    expect([...svgs.keys()].filter((n) => /^[scy]\d/.test(n)).sort()).toEqual(
      [...sequences.keys(), "y01-unknown-statement"].sort(),
    );
  });

  test("mermaid draws y01 and refuses y02", async () => {
    const drawn = fallbacks.get("y01-unknown-statement") ?? "";
    expect("error" in (await sequenceDbOf(drawn, false))).toBe(false);
    const refused = fallbacks.get("y02-mermaid-rejects") ?? "";
    expect("error" in (await sequenceDbOf(refused, false))).toBe(true);
  });
});

describe("mermaid 12.2.0 sequence records", () => {
  const S = "sequenceDiagram\n";
  const shape = (view: SequenceDbView) =>
    view.records.map((r) => [r.id, r.from, r.to, r.type]);

  test("a new db per diagram", async () => {
    await seqDb(`${S}  A->>B: x`);
    const view = await seqDb(`${S}  C->>D: y`);
    expect(view.actors.map((a) => a.name)).toEqual(["C", "D"]);
    expect(view.records.length).toBe(1);
  });

  test("the type of each arrow token", async () => {
    const view = await seqDb(
      `${S}  A->B: x\n  A-->B: x\n  A->>B: x\n  A-->>B: x\n  A<<->>B: x\n  A<<-->>B: x\n  A-xB: x\n  A--xB: x\n  A-)B: x\n  A--)B: x`,
    );
    expect(view.records.map((r) => r.type)).toEqual([
      5, 6, 0, 1, 33, 34, 3, 4, 24, 25,
    ]);
  });

  test("+ adds ACTIVE_START on the receiver, - ACTIVE_END on the sender", async () => {
    const view = await seqDb(`${S}  A->>+B: x\n  B-->>-A: y`);
    expect(shape(view)).toEqual([
      ["0", "A", "B", 0],
      ["1", "B", undefined, 17],
      ["2", "B", "A", 1],
      ["3", "B", undefined, 18],
    ]);
  });

  test("activate and deactivate add one record each and declare nobody", async () => {
    const view = await seqDb(
      `${S}  participant A\n  activate A\n  deactivate A\n  activate Z`,
    );
    expect(view.actors.map((a) => a.name)).toEqual(["A"]);
    expect(shape(view)).toEqual([
      ["0", "A", undefined, 17],
      ["1", "A", undefined, 18],
      ["2", "Z", undefined, 17],
    ]);
  });

  test("a note is a record, with both ends and its placement", async () => {
    const view = await seqDb(
      `${S}  Note left of A: l\n  Note right of A: r\n  Note over A,B: o\n  Note over B: p`,
    );
    expect(
      view.records.map((r) => [r.from, r.to, r.type, r.placement]),
    ).toEqual([
      ["A", "A", 2, 0],
      ["A", "A", 2, 1],
      ["A", "B", 2, 2],
      ["B", "B", 2, 2],
    ]);
  });

  test("autonumber's record has no text and its numbers apart", async () => {
    const view = await seqDb(
      `${S}  autonumber 10 5\n  autonumber off\n  autonumber 3\n  autonumber`,
    );
    expect(view.records.map((r) => [r.type, r.message, r.autonumber])).toEqual([
      [26, "", { start: 10, step: 5, visible: true }],
      [26, "", { visible: false }],
      [26, "", { start: 3, step: 1, visible: true }],
      [26, "", { visible: true }],
    ]);
  });

  test("every block keyword's records, with the text on the opening ones", async () => {
    const view = await seqDb(
      `${S}  rect rgb(1,2,3)\n  end\n  alt a\n  else b\n  end\n  opt o\n  end\n  par p\n  and q\n  end\n  par_over r\n  end\n  critical c\n  option d\n  end\n  break e\n  end\n  loop l\n  end`,
    );
    expect(view.records.map((r) => [r.type, r.message])).toEqual([
      [22, "rgb(1,2,3)"],
      [23, ""],
      [12, "a"],
      [13, "b"],
      [14, ""],
      [15, "o"],
      [16, ""],
      [19, "p"],
      [20, "q"],
      [21, ""],
      [32, "r"],
      [21, ""],
      [27, "c"],
      [28, "d"],
      [29, ""],
      [30, "e"],
      [31, ""],
      [10, "l"],
      [11, ""],
    ]);
  });

  test("central connections add one or two records, half arrows have their own types", async () => {
    const view = await seqDb(
      `${S}  A->>()B: c\n  A()->>B: d\n  A()->>()B: e\n  A-|\\B: f\n  A--//B: g`,
    );
    expect(shape(view)).toEqual([
      ["0", "A", "B", 0],
      ["1", "B", undefined, 59],
      ["2", "A", "B", 0],
      ["3", "A", undefined, 60],
      ["4", "A", "B", 0],
      ["5", "B", undefined, 59],
      ["6", "A", undefined, 60],
      ["7", "A", "B", 41],
      ["8", "A", "B", 54],
    ]);
  });

  test("a later declaration without an alias changes neither type nor label", async () => {
    const view = await seqDb(
      `${S}  participant A\n  participant A as X\n  participant A\n  actor A\n  B->>A: y\n  actor B as Bee\n  actor C\n  participant C as Sea\n  actor D as E\n  participant D`,
    );
    expect(view.actors).toEqual([
      { name: "A", description: "X", type: "participant" },
      { name: "B", description: "Bee", type: "actor" },
      { name: "C", description: "Sea", type: "participant" },
      { name: "D", description: "E", type: "actor" },
    ]);
  });

  test("create and destroy keep the record index of the next message", async () => {
    const view = await seqDb(
      `${S}  create actor X as Y\n  A->>X: hi\n  destroy X\n  X-->>A: bye`,
    );
    expect(view.created).toEqual([["X", 0]]);
    expect(view.destroyed).toEqual([["X", 1]]);
  });

  test("boxes list their members in order", async () => {
    const view = await seqDb(
      `${S}  box Aqua x\n  participant A\n  actor B\n  end\n  participant C`,
    );
    expect(view.boxes).toEqual([{ actors: ["A", "B"] }]);
  });

  test("a # cuts a text and a %% stays in it", async () => {
    const view = await seqDb(`${S}  A->>B: Issue #12 open\n  A->>B: x %% y`);
    expect(view.records.map((r) => r.message)).toEqual(["Issue", "x %% y"]);
  });

  test("a bare %%, a single % and a # line are comments, and a %{ is not", async () => {
    const view = await seqDb(`${S}%%\n% one\n# two\n  A->>B: x`);
    expect(view.records.length).toBe(1);
    expect(view.actors.map((a) => a.name)).toEqual(["A", "B"]);
  });

  test("a name after an ID keyword takes a trailing comment with it", async () => {
    const view = await seqDb(`${S}  participant A %% y\n  participant B # z`);
    expect(view.actors.map((a) => a.name)).toEqual(["A %% y", "B # z"]);
  });

  test("a x%% at a statement's start hides the whole line", async () => {
    const view = await seqDb(`${S}  A%% y\n  B->>C: x`);
    expect(view.actors.map((a) => a.name)).toEqual(["B", "C"]);
  });

  test("a ; ends a statement inside quotes too, unless it is an entity's", async () => {
    expect("error" in (await sequenceDbOf(`${S}  A->>B: "a;b"`, false))).toBe(
      true,
    );
    const view = await seqDb(`${S}  A->>B: a#59;b; B->>A: c`);
    expect(view.records.map((r) => r.message)).toEqual([
      "a\uFB02\xB0\xB059\xB6\xDFb",
      "c",
    ]);
  });

  test("encodeEntities drops a ; here too, so the next statement joins the text", async () => {
    const view = await seqDb(`${S}  A->>B: style a:x#f00; B->>A: y`);
    expect(view.records.length).toBe(1);
  });

  test("keywords go first in any case, ended by a word boundary", async () => {
    expect(
      "error" in (await sequenceDbOf(`${S}  end-point->>B: x`, false)),
    ).toBe(true);
    const view = await seqDb(`${S}  LOOP l\n  End\n  endpoint->>loopback: x`);
    expect(view.records.map((r) => r.type)).toEqual([10, 11, 0]);
    expect(view.actors.map((a) => a.name)).toEqual(["endpoint", "loopback"]);
  });

  test("a diagram that is no sequence diagram is an error", async () => {
    const view = await sequenceDbOf("flowchart TD\n  A --> B", false);
    expect("error" in view).toBe(true);
    expect(view).not.toHaveProperty("unavailable");
  });

  test("a wrap prefix is stripped from each text, in lower case only", async () => {
    const view = await seqDb(
      `${S}  A->>B:wrap:x\n  A->>B: nowrap: y\n  A->>B: Wrap: z`,
    );
    expect(view.records.map((r) => r.message)).toEqual(["x", "y", "Wrap: z"]);
  });
});

describe("the recorded sequence SVGs agree with the db", () => {
  test("s05: the id of every message line is the record of that message", () => {
    const body = sequences.get("s05-activation") ?? "";
    const model = parseSequence(body);
    const drawn = [
      ...sequenceSvg("s05-activation").querySelectorAll('[data-et="message"]'),
    ];
    expect(drawn.map((e) => e.getAttribute("data-id"))).toEqual(
      model.messages.map((m) => `i${String(m.record)}`),
    );
    expect(
      drawn.map((e) => [
        e.getAttribute("data-from"),
        e.getAttribute("data-to"),
      ]),
    ).toEqual(model.messages.map((m) => [m.from, m.to]));
  });

  const recorded = [...sequences.keys()];

  test.each(recorded)(
    "%s: messages, participants, life lines, notes and blocks",
    async (name) => {
      const body = sequences.get(name) ?? "";
      const model = parseSequence(body);
      const view = await seqDb(body);
      const svg = sequenceSvg(name);
      expect(svg.querySelector("svg")?.id).toBe(`rec-${name}`);
      const attr = (selector: string, key = "data-id") =>
        [...svg.querySelectorAll(selector)].map(
          (e) => e.getAttribute(key) ?? "",
        );

      // Only the top box has the attributes, one per participant; a self message is a path.
      expect(attr('[data-et="participant"]').sort()).toEqual(
        [...model.order].sort(),
      );
      expect(attr('[data-et="life-line"]').sort()).toEqual(
        [...model.order].sort(),
      );
      expect(attr('[data-et="message"]')).toEqual(
        model.messages.map((m) => `i${String(m.record)}`),
      );
      for (const element of svg.querySelectorAll('[data-et="message"]')) {
        const message = model.messages.find(
          (m) => `i${String(m.record)}` === element.getAttribute("data-id"),
        );
        expect(element.tagName.toLowerCase()).toBe(
          message?.from === message?.to ? "path" : "line",
        );
      }

      // Notes carry their own record; a block carries the record of its `end`,
      // and a rect (LINETYPE 23 at its end) is drawn with no attributes at all.
      const noted = view.records
        .filter((r) => r.type === 2)
        .map((r) => `i${r.id}`);
      expect(attr('[data-et="note"]').sort()).toEqual(noted.sort());
      const ends = model.statements
        .filter((s) => s.kind === "blockEnd")
        .map((s) => s.records[0])
        .filter((r) => view.records[r ?? -1]?.type !== 23)
        .map((r) => `i${String(r)}`);
      expect(attr('[data-et="control-structure"]').sort()).toEqual(ends.sort());
    },
  );

  test.each(recorded)(
    "%s: each .messageText precedes its own message's line",
    (name) => {
      const body = sequences.get(name) ?? "";
      const model = parseSequence(body);
      const svg = sequenceSvg(name);
      const texts = [...svg.querySelectorAll("text.messageText")];
      const seen = new Map<string, string[]>();
      for (const text of texts) {
        let next = text.nextElementSibling;
        while (next && next.getAttribute("data-et") !== "message") {
          next = next.nextElementSibling;
        }
        const id = next?.getAttribute("data-id") ?? "none";
        seen.set(id, [...(seen.get(id) ?? []), text.textContent]);
      }
      for (const message of model.messages) {
        const raw = message.text
          ? body.slice(message.text.from, message.text.to)
          : "";
        const lines = decodeSeqText(raw).split("\n");
        const drawn = seen.get(`i${String(message.record)}`) ?? [];
        // An empty text draws one zero width space.
        expect(drawn.map((t) => t.replace(/\u200B/g, ""))).toEqual(lines);
      }
      expect([...seen.keys()]).not.toContain("none");
    },
  );
});
