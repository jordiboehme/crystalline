// NO vi.mock("mermaid") here: every result runs through mermaid 12.2.0's real
// parser, and the db it fills must differ from the one before by exactly the
// change the op means.

import { EditorState } from "@codemirror/state";
import { describe, expect, test } from "vitest";

import { STATUS_COLOURS } from "../../../markdown/directives";
import { onlyEditedBytes, untouchedLinesEqual } from "../../../test/fidelity";
import {
  LABEL_HOLDERS,
  LABEL_TEXTS,
  MARKDOWN_MARKUP,
  MARKDOWN_TEXTS,
  PUNCTUATION_TEXTS,
  REFUSED_LABELS,
  REFUSED_ON_PLAIN_LABELS,
} from "../../../test/labelTable";
import { dbDelta, flowDbFor, flowDelta } from "../../../test/mermaidOracle";
import { docText, lineSeparatorFor } from "../../setup";
import { type TextEdit, applyEdits, toChanges } from "../edits";
import { type FlowDbView, flowDbOf } from "../mermaidDb";
import { agreesFlow } from "../verdict";
import { COLOUR_PRESETS, SHAPES, tidyLabel } from "./labels";
import { parseFlowchart } from "./parse";
import { type FlowOp, compileFlowOp } from "./ops";

const TIMEOUT = 120_000;

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

const FLOWCHART = import.meta.glob<string>(
  "../../fixtures/diagrams/flowchart/*.mmd",
  { query: "?raw", import: "default", eager: true },
);

function nameOf(path: string): string {
  const file = path.slice(path.lastIndexOf("/") + 1);
  return file.slice(0, file.lastIndexOf("."));
}

/** A file as a fence body, which is what an op sees: no trailing break. */
function fenceBody(text: string): string {
  return text.replace(/\n$/, "");
}

const corpus = Object.entries(FLOWCHART)
  .map(([path, text]) => [nameOf(path), fenceBody(text)] as const)
  .sort(([a], [b]) => a.localeCompare(b));

/** Cases the corpus does not hold, each next to a place a new line could go wrong. */
const EXTRA: readonly (readonly [string, string])[] = [
  [
    "classDef above the boxes",
    "flowchart TD\n  classDef hot fill:#f00\n  A --> B:::hot\n  B --> C",
  ],
  [
    "click above a ref",
    'flowchart TD\n  A --> B\n  class A hot\n  click C "https://example.com"\n  C --> D',
  ],
  [
    "style inside a subgraph",
    "flowchart TD\n  subgraph s\n    A --> B\n    style A fill:#fff\n  end\n  C --> D\n  D --> s",
  ],
  [
    "two style lines",
    "flowchart TD\n  A --> B\n  style A fill:#fff\n  style A stroke:#000,opacity:1\n  B --> C",
  ],
  ["class in the same line", "graph TD; A --> B; class A hot; B --> C"],
];

type OpName = "rename" | "shape" | "colour" | "noColour" | "direction";

interface Tally {
  applied: Record<OpName, number>;
  /** `<node> <op> <code>`, one per refused op. */
  refused: string[];
  failures: string[];
}

function emptyTally(): Tally {
  return {
    applied: { rename: 0, shape: 0, colour: 0, noColour: 0, direction: 0 },
    refused: [],
    failures: [],
  };
}

function commentLines(body: string): string[] {
  return body.split("\n").filter((line) => /^\s*%%/.test(line));
}

async function view(body: string): Promise<FlowDbView | string> {
  const db = await flowDbOf(body, false);
  return "error" in db ? `mermaid refused: ${String(db.error)}` : db;
}

/**
 * Whether `edits` on `before` keep every promise, `null` when they do: only
 * the edited bytes change, no other line, no comment line, the result is
 * understood, mermaid reads it the same way and its db differs from the one
 * before by exactly `delta`.
 */
async function broken(
  before: string,
  beforeDb: FlowDbView,
  edits: readonly TextEdit[],
  delta: readonly string[],
): Promise<{ problem: string } | { after: string; db: FlowDbView }> {
  const after = applyEdits(before, edits);
  if (!onlyEditedBytes(before, after, edits)) {
    return { problem: "bytes outside the edits changed" };
  }
  const lines = untouchedLinesEqual(before, after, edits);
  if (lines.length > 0) {
    return { problem: `lines changed: ${JSON.stringify(lines)}` };
  }
  if (commentLines(before).join("\n") !== commentLines(after).join("\n")) {
    return { problem: "a comment line changed" };
  }
  const model = parseFlowchart(after);
  if (!model.understood) {
    return { problem: `not understood: ${model.reasons.join("; ")}` };
  }
  const db = await view(after);
  if (typeof db === "string") {
    return { problem: db };
  }
  const difference = agreesFlow(model, db);
  if (difference !== null) {
    return { problem: `disagrees: ${difference}` };
  }
  const got = dbDelta(beforeDb, db);
  if (JSON.stringify(got) !== JSON.stringify(delta)) {
    return {
      problem: `delta ${JSON.stringify(got)}, wanted ${JSON.stringify(delta)}`,
    };
  }
  return { after, db };
}

/** The last value of `key` in a vertex's styles, the one CSS uses. */
function lastStyle(styles: readonly string[], key: string): string | undefined {
  return styles
    .map((entry) => /^\s*([\w-]+)\s*:\s*(.*?)\s*$/.exec(entry))
    .filter((match) => match?.[1]?.toLowerCase() === key)
    .at(-1)?.[2];
}

/** Every op on every box of `body`, and both directions. */
async function checkBody(body: string): Promise<Tally> {
  const tally = emptyTally();
  const beforeDb = await flowDbFor(body);
  const model = parseFlowchart(body);
  const fail = (what: string, problem: string) => {
    tally.failures.push(`${what}: ${problem}\n--- body\n${body}`);
  };

  /** Runs `op`, returns the checked result when it applied and changed text. */
  const attempt = async (
    what: string,
    name: OpName,
    op: FlowOp,
    delta: readonly string[],
  ): Promise<{ after: string; db: FlowDbView; edits: TextEdit[] } | null> => {
    const result = compileFlowOp(model, op);
    if ("refused" in result) {
      tally.refused.push(`${what} ${result.refused}`);
      return null;
    }
    if (result.edits.length === 0) {
      return null;
    }
    const checked = await broken(body, beforeDb, result.edits, delta);
    if ("problem" in checked) {
      fail(what, checked.problem);
      return null;
    }
    tally.applied[name]++;
    return { ...checked, edits: result.edits };
  };

  for (const id of model.nodeOrder) {
    const node = model.nodes.get(id);
    if (!node) {
      continue;
    }
    const bare = node.defining === null;
    const renamed = await attempt(
      `${id} rename`,
      "rename",
      { op: "rename", id, text: "Renamed" },
      bare ? [`vertex ${id} text`, `vertex ${id} type`] : [`vertex ${id} text`],
    );
    if (renamed) {
      const text = renamed.db.vertices.find((v) => v.id === id)?.text;
      if (text !== "Renamed") {
        fail(`${id} rename`, `the db says ${String(text)}`);
      }
    }

    for (const shape of SHAPES) {
      await attempt(
        `${id} shape ${shape.name}`,
        "shape",
        { op: "shape", id, shape: shape.name },
        [`vertex ${id} type`],
      );
    }

    const styled = model.directives.some(
      (d) => d.kind === "style" && d.targets[0]?.id === id,
    );
    for (const colour of STATUS_COLOURS) {
      const what = `${id} colour ${colour}`;
      const coloured = await attempt(
        what,
        "colour",
        { op: "colour", id, colour },
        [`vertex ${id} styles`],
      );
      if (!coloured) {
        continue;
      }
      const vertex = coloured.db.vertices.find((v) => v.id === id);
      const preset = COLOUR_PRESETS[colour];
      for (const key of ["fill", "stroke", "color"] as const) {
        if (lastStyle(vertex?.styles ?? [], key) !== preset[key]) {
          fail(what, `the db's ${key} is not the preset's`);
        }
      }

      // Back again: a box that had no style line ends up byte-identical.
      if (!styled) {
        const clear = compileFlowOp(parseFlowchart(coloured.after), {
          op: "colour",
          id,
          colour: null,
        });
        if ("refused" in clear) {
          fail(`${what} back`, clear.refused);
        } else if (applyEdits(coloured.after, clear.edits) !== body) {
          fail(`${what} back`, "not byte-identical");
        } else {
          tally.applied.noColour++;
        }
      }
    }

    const cleared = await attempt(
      `${id} noColour`,
      "noColour",
      { op: "colour", id, colour: null },
      [`vertex ${id} styles`],
    );
    if (cleared) {
      const vertex = cleared.db.vertices.find((v) => v.id === id);
      for (const key of ["fill", "stroke", "color"]) {
        if (lastStyle(vertex?.styles ?? [], key) !== undefined) {
          fail(`${id} noColour`, `the db still has ${key}`);
        }
      }
    }
  }

  for (const value of ["TD", "LR"] as const) {
    const moved = await attempt(
      `direction ${value}`,
      "direction",
      { op: "direction", value },
      ["direction"],
    );
    if (moved && moved.db.direction !== (value === "TD" ? "TB" : "LR")) {
      fail(`direction ${value}`, `the db says ${String(moved.db.direction)}`);
    }
  }
  return tally;
}

const tallies = new Map<string, Promise<Tally>>();

function tallyOf(name: string, body: string): Promise<Tally> {
  let tally = tallies.get(name);
  if (tally === undefined) {
    tally = checkBody(body);
    tallies.set(name, tally);
  }
  return tally;
}

/**
 * Every refusal over the corpus, as `<fixture> <box> <op> <code>`: what a box's
 * label or place cannot take. Asserted whole, so a suite that starts refusing
 * more goes red instead of passing by refusing.
 */
const EXPECTED_REFUSALS: readonly string[] = STATUS_COLOURS.map(
  // `call` is a box id here, and the parser reads no reserved word as the
  // target of a style line, so the new line would not be understood.
  (colour) => `r14-reflection-write call colour ${colour} invalid`,
);

describe("flowDelta", () => {
  test.each([
    [
      "the same text",
      "flowchart TD\n  A[x] --> B",
      "flowchart TD\n  A[x] --> B",
      [],
    ],
    ["a direction", "flowchart TD\n  A", "flowchart LR\n  A", ["direction"]],
    [
      "a label",
      "flowchart TD\n  A[x]",
      "flowchart TD\n  A[y]",
      ["vertex A text"],
    ],
    [
      "a shape",
      "flowchart TD\n  A[x]",
      "flowchart TD\n  A{x}",
      ["vertex A type"],
    ],
    [
      "a style",
      "flowchart TD\n  A",
      "flowchart TD\n  A\n  style A fill:#fff",
      ["vertex A styles"],
    ],
    [
      "a class",
      "flowchart TD\n  A",
      "flowchart TD\n  A\n  class A hot",
      ["vertex A classes"],
    ],
    [
      "a new box",
      "flowchart TD\n  A",
      "flowchart TD\n  A\n  B",
      ["vertex B added"],
    ],
    [
      "a lost box",
      "flowchart TD\n  A\n  B",
      "flowchart TD\n  A",
      ["vertex B removed"],
    ],
    [
      "the box order",
      "flowchart TD\n  A\n  B",
      "flowchart TD\n  B\n  A",
      ["vertex order"],
    ],
    [
      "a new arrow",
      "flowchart TD\n  A --> B",
      "flowchart TD\n  A --> B\n  A --> B",
      ["edge 1 added"],
    ],
    [
      "a lost arrow",
      "flowchart TD\n  A --> B\n  A --> B",
      "flowchart TD\n  A --> B",
      ["edge 1 removed"],
    ],
    [
      "an arrow's label",
      "flowchart TD\n  A --> B",
      "flowchart TD\n  A -->|x| B",
      ["edge 0 text"],
    ],
    [
      "an arrow's kind",
      "flowchart TD\n  A --> B",
      "flowchart TD\n  A -.- B",
      ["edge 0 type", "edge 0 stroke"],
    ],
    [
      "a link",
      "flowchart TD\n  A",
      'flowchart TD\n  A\n  click A "https://x.y/"',
      ["vertex A classes", "vertex A link"],
    ],
    [
      "a tooltip",
      'flowchart TD\n  A\n  click A "https://x.y/"',
      'flowchart TD\n  A\n  click A "https://x.y/" "tip"',
      ["vertex A tooltip"],
    ],
    [
      "a label type",
      'flowchart TD\n  A["x"]',
      'flowchart TD\n  A["`x`"]',
      ["vertex A labelType"],
    ],
    [
      "a classDef",
      "flowchart TD\n  A\n  classDef hot fill:#f00",
      "flowchart TD\n  A\n  classDef hot fill:#0f0",
      ["classDef hot styles"],
    ],
    [
      "a new classDef",
      "flowchart TD\n  A",
      "flowchart TD\n  A\n  classDef hot fill:#f00",
      ["classDef hot added"],
    ],
    [
      "a subgraph's direction",
      "flowchart TD\n  subgraph s\n    A\n  end",
      "flowchart TD\n  subgraph s\n    direction LR\n    A\n  end",
      ["subgraph s dir"],
    ],
    [
      "a subgraph's title",
      "flowchart TD\n  subgraph s [a]\n    A\n  end",
      "flowchart TD\n  subgraph s [b]\n    A\n  end",
      ["subgraph s title"],
    ],
    [
      "a subgraph's members",
      "flowchart TD\n  A\n  subgraph s\n    B\n  end",
      "flowchart TD\n  subgraph s\n    A\n    B\n  end",
      ["subgraph s nodes"],
    ],
  ] as const)("sees %s", async (_, before, after, delta) => {
    expect(await flowDelta(before, after)).toEqual(delta);
  });

  test("a new style line moving a later domId is no difference", async () => {
    const before = "flowchart TD\n  A\n  B";
    const after = "flowchart TD\n  A\n  style A fill:#fff\n  B";
    expect(await flowDelta(before, after)).toEqual(["vertex A styles"]);
  });
});

describe("every op on the corpus", () => {
  test.each(corpus)(
    "%s",
    async (name, body) => {
      const tally = await tallyOf(name, body);
      expect(tally.failures).toEqual([]);
    },
    TIMEOUT,
  );

  test.each(EXTRA)(
    "%s",
    async (name, body) => {
      const tally = await tallyOf(name, body);
      expect(tally.failures).toEqual([]);
      expect(tally.applied.colour).toBeGreaterThan(0);
    },
    TIMEOUT,
  );

  test(
    "the refusals over the corpus are these (R28)",
    async () => {
      const refused: string[] = [];
      for (const [name, body] of corpus) {
        const tally = await tallyOf(name, body);
        refused.push(...tally.refused.map((r) => `${name} ${r}`));
      }
      expect(refused).toEqual(EXPECTED_REFUSALS);
    },
    TIMEOUT,
  );

  test(
    "each op applies on the named fixtures, so no suite passes by refusing (R28)",
    async () => {
      const applied = emptyTally().applied;
      for (const [name, body] of corpus) {
        if (name.startsWith("f")) {
          const tally = await tallyOf(name, body);
          for (const op of Object.keys(applied) as OpName[]) {
            applied[op] += tally.applied[op];
          }
        }
      }
      expect(applied.rename).toBeGreaterThanOrEqual(24);
      expect(applied.shape).toBeGreaterThanOrEqual(24 * 13);
      expect(applied.colour).toBeGreaterThanOrEqual(24 * 6);
      expect(applied.noColour).toBeGreaterThanOrEqual(24);
      expect(applied.direction).toBeGreaterThanOrEqual(24);
    },
    TIMEOUT,
  );

  test.each([
    ["r04-sync-classify", "moved"],
    ["r13-collab-door", "deny"],
    ["r21-merge-file", "conflict"],
    ["r41-session-end", "hook"],
  ])(
    "no colour takes the line out of %s",
    async (name, id) => {
      const body = corpus.find(([n]) => n === name)?.[1] ?? "";
      const result = compileFlowOp(parseFlowchart(body), {
        op: "colour",
        id,
        colour: null,
      });
      expect("edits" in result && result.edits.length).toBe(1);
      const tally = await tallyOf(name, body);
      expect(tally.applied.noColour).toBeGreaterThan(0);
    },
    TIMEOUT,
  );
});

describe("the CRLF copy", () => {
  /** Every op on every box, each with the delta it must show. */
  const OPS = (ids: readonly string[]): [FlowOp, (string | null)[]][] => [
    [{ op: "direction", value: "LR" }, ["direction"]],
    [{ op: "direction", value: "TD" }, ["direction"]],
    ...ids.flatMap((id): [FlowOp, (string | null)[]][] => [
      [{ op: "rename", id, text: "Renamed" }, [`vertex ${id} text`, null]],
      ...SHAPES.map((s): [FlowOp, string[]] => [
        { op: "shape", id, shape: s.name },
        [`vertex ${id} type`],
      ]),
      ...STATUS_COLOURS.map((colour): [FlowOp, string[]] => [
        { op: "colour", id, colour },
        [`vertex ${id} styles`],
      ]),
      [{ op: "colour", id, colour: null }, [`vertex ${id} styles`]],
    ]),
  ];

  const STYLED =
    "flowchart TD\n  %% keep\n  A[a] --> B\n  style A fill:#fff\n  style B fill:#000,opacity:1\n  B --> C\n  style C stroke:#000\n  style C color:#000";

  test.each([
    ["f04-chains", corpus.find(([n]) => n === "f04-chains")?.[1] ?? ""],
    [
      "f14-odd-spacing",
      corpus.find(([n]) => n === "f14-odd-spacing")?.[1] ?? "",
    ],
    ["a styled body, so whole lines go", STYLED],
  ])(
    "%s saves with its own line breaks",
    async (_, lf) => {
      const crlf = lf.replaceAll("\n", "\r\n");
      const state = EditorState.create({
        doc: crlf,
        extensions: lineSeparatorFor(crlf),
      });
      const body = state.doc.sliceString(0, state.doc.length);
      expect(body).toBe(lf);
      const model = parseFlowchart(body);
      const beforeDb = await flowDbFor(body);
      let applied = 0;
      let linesGone = 0;
      for (const [op, wanted] of OPS(model.nodeOrder)) {
        const result = compileFlowOp(model, op);
        if ("refused" in result) {
          throw new Error(`refused: ${JSON.stringify(op)} ${result.refused}`);
        }
        const after = applyEdits(body, result.edits);
        const saved = docText(
          state.update({
            changes: toChanges(0, result.edits, state.lineBreak),
          }).state,
        );
        expect(saved).toBe(after.replaceAll("\n", "\r\n"));
        expect(saved.replaceAll("\r\n", "")).not.toContain("\n");
        if (result.edits.length === 0) {
          continue;
        }
        applied++;
        if (after.split("\n").length < lf.split("\n").length) {
          linesGone++;
        }
        // A bare box's rename also moves its type, as in the corpus suite.
        const bare =
          op.op === "rename" && model.nodes.get(op.id)?.defining === null;
        const delta = wanted.flatMap((d) =>
          d !== null
            ? [d]
            : bare && op.op === "rename"
              ? [`vertex ${op.id} type`]
              : [],
        );
        const checked = await broken(body, beforeDb, result.edits, delta);
        expect(checked).not.toHaveProperty("problem");
      }
      expect(applied).toBeGreaterThan(0);
      if (lf === STYLED) {
        expect(linesGone).toBeGreaterThan(0);
      }
    },
    TIMEOUT,
  );
});

describe("the table the browser draws", () => {
  const FILE = "e2e/fixtures/label-table.json";
  const texts = [
    ...new Set([...LABEL_TEXTS, ...PUNCTUATION_TEXTS, ...MARKDOWN_TEXTS]),
  ];

  /** Each text through `rename` on one box per text, per holder. */
  function table(): string {
    const bodies: Record<string, string> = {};
    for (const [holder, ref] of Object.entries(LABEL_HOLDERS)) {
      let body = "flowchart TD";
      texts.forEach((_, i) => {
        body += `\n  ${ref.replace(/^A/, `N${String(i)}`)}`;
      });
      texts.forEach((text, i) => {
        const result = compileFlowOp(parseFlowchart(body), {
          op: "rename",
          id: `N${String(i)}`,
          text,
        });
        if ("refused" in result) {
          throw new Error(`${holder} refused ${JSON.stringify(text)}`);
        }
        body = applyEdits(body, result.edits);
      });
      bodies[holder] = body;
    }
    const markup = texts.flatMap((text, i) =>
      MARKDOWN_MARKUP.includes(text) ? [i] : [],
    );
    return `${JSON.stringify({ drawn: texts.map(tidyLabel), markup, bodies }, null, 1)}\n`;
  }

  test("is what the op writes today", () => {
    // Regenerate with FLUID_WRITE_LABEL_TABLE=1.
    const fs = node.process.getBuiltinModule("node:fs") as Fs & {
      writeFileSync: (path: string, text: string) => void;
    };
    if (node.process.env.FLUID_WRITE_LABEL_TABLE !== undefined) {
      fs.writeFileSync(FILE, table());
    }
    expect(fs.readFileSync(FILE, "utf8")).toBe(table());
  });

  test("every body is one mermaid and the parser read the same way", async () => {
    const { bodies } = JSON.parse(table()) as {
      bodies: Record<string, string>;
    };
    for (const body of Object.values(bodies)) {
      expect(agreesFlow(parseFlowchart(body), await flowDbFor(body))).toBe(
        null,
      );
    }
  });
});

describe("labels at the edges of the grammar, with real mermaid", () => {
  const HOLDERS = LABEL_HOLDERS;
  const TEXTS: readonly (readonly [string, readonly string[]])[] = [
    ...new Set([...LABEL_TEXTS, ...PUNCTUATION_TEXTS, ...MARKDOWN_TEXTS]),
  ].map((text) => [text, []] as const);

  test.each(REFUSED_LABELS)(
    "rename to %j is refused on every holder",
    (text) => {
      for (const ref of Object.values(HOLDERS)) {
        const body = `flowchart TD\n  ${ref} --> B`;
        expect(
          compileFlowOp(parseFlowchart(body), { op: "rename", id: "A", text }),
          ref,
        ).toEqual({ refused: "invalid" });
      }
    },
  );

  test.each(REFUSED_ON_PLAIN_LABELS)(
    "rename to %j is refused on a plain label and agrees on the others",
    async (text) => {
      for (const [holder, ref] of Object.entries(HOLDERS)) {
        const body = `flowchart TD\n  ${ref} --> B`;
        const result = compileFlowOp(parseFlowchart(body), {
          op: "rename",
          id: "A",
          text,
        });
        if (holder === "markdown" || holder.startsWith("at")) {
          if ("refused" in result) {
            throw new Error(`${holder} refused`);
          }
          const checked = await broken(
            body,
            await flowDbFor(body),
            result.edits,
            ["vertex A text"],
          );
          expect(checked, holder).not.toHaveProperty("problem");
        } else {
          expect(result, holder).toEqual({ refused: "invalid" });
        }
      }
    },
    TIMEOUT,
  );

  test.each(TEXTS)(
    "rename to %j",
    async (text, refusedOn) => {
      for (const [holder, ref] of Object.entries(HOLDERS)) {
        const body = `flowchart TD\n  ${ref} --> B`;
        const result = compileFlowOp(parseFlowchart(body), {
          op: "rename",
          id: "A",
          text,
        });
        if (refusedOn.includes(holder)) {
          expect(result, holder).toEqual({ refused: "invalid" });
          continue;
        }
        if ("refused" in result) {
          throw new Error(`${holder} refused`);
        }
        const beforeDb = await flowDbFor(body);
        const checked = await broken(
          body,
          beforeDb,
          result.edits,
          holder === "bare"
            ? ["vertex A text", "vertex A type"]
            : ["vertex A text"],
        );
        expect(checked, holder).not.toHaveProperty("problem");
      }
    },
    TIMEOUT,
  );

  test(
    "the refused label really is one mermaid would change (R9)",
    async () => {
      const body = 'flowchart TD\n  A["style a:#35;b; c"] --> B';
      expect(agreesFlow(parseFlowchart(body), await flowDbFor(body))).toMatch(
        /label of A/,
      );
    },
    TIMEOUT,
  );

  test(
    "the refused shape really is one mermaid refuses (R5)",
    async () => {
      const db = await flowDbOf("flowchart TD\n  A((-a)) --> B", false);
      expect(db).toHaveProperty("error");
    },
    TIMEOUT,
  );

  const LABELS = [
    "-a",
    "a-",
    "a/",
    "/a",
    "\\a",
    "a\\",
    "/a/",
    "\\a\\",
    "/a\\",
    "\\a/",
    "a b",
    '"q"',
    '"`m`"',
  ];

  test(
    "every shape to every shape keeps these labels, or refuses",
    async () => {
      let applied = 0;
      let refused = 0;
      const mermaidRefuses = new Set<string>();
      const problems: string[] = [];
      for (const label of LABELS) {
        for (const source of SHAPES) {
          const body = `flowchart TD\n  A${source.open}${label}${source.close} --> B`;
          const model = parseFlowchart(body);
          const beforeDb = await flowDbOf(body, false);
          if (
            !model.understood ||
            "error" in beforeDb ||
            agreesFlow(model, beforeDb) !== null ||
            model.nodes.get("A")?.defining?.shape?.name !== source.name
          ) {
            continue;
          }
          for (const target of SHAPES) {
            const result = compileFlowOp(model, {
              op: "shape",
              id: "A",
              shape: target.name,
            });
            if ("refused" in result) {
              refused++;
              continue;
            }
            if (result.edits.length === 0) {
              continue;
            }
            const after = applyEdits(body, result.edits);
            const checked = await broken(body, beforeDb, result.edits, [
              "vertex A type",
            ]);
            if ("after" in checked) {
              applied++;
            } else if (checked.problem.startsWith("mermaid refused")) {
              mermaidRefuses.add(after.split("\n")[1]?.trim() ?? "");
            } else {
              problems.push(`${after}: ${checked.problem}`);
            }
          }
        }
      }
      expect(problems).toEqual([]);
      expect([...mermaidRefuses]).toEqual([]);
      expect(applied).toBeGreaterThan(1000);
      expect(refused).toBeGreaterThan(0);
    },
    TIMEOUT,
  );
});

const folder = node.process.env.FLUID_DIAGRAM_CORPUS;

test.skipIf(folder === undefined)(
  "the opt-in corpus: every op on what is drawn keeps its promises",
  async () => {
    const dir = folder ?? "";
    const fs = node.process.getBuiltinModule("node:fs") as Fs;
    const counts: Record<string, number> = {};
    let drawn = 0;
    const failures: string[] = [];
    for (const file of fs.readdirSync(dir).filter((f) => f.endsWith(".mmd"))) {
      const body = fenceBody(fs.readFileSync(`${dir}/${file}`, "utf8"));
      const model = parseFlowchart(body);
      const before = await flowDbOf(body, false);
      if (
        !model.understood ||
        "error" in before ||
        agreesFlow(model, before) !== null
      ) {
        continue;
      }
      drawn++;
      const tally = await checkBody(body);
      failures.push(...tally.failures.map((f) => `${file}: ${f}`));
      for (const refusal of tally.refused) {
        const words = refusal.split(" ");
        const key = `${words[1] === "colour" || words[1] === "shape" ? words[1] : (words[0] ?? "")} ${words.at(-1) ?? ""}`;
        counts[key] = (counts[key] ?? 0) + 1;
      }
    }
    console.log(
      `flowchart ops: ${drawn} diagrams, refusals ${JSON.stringify(counts)}`,
    );
    expect(failures).toEqual([]);
  },
  600_000,
);
