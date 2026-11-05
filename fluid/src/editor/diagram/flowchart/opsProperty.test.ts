// NO vi.mock("mermaid") here: every result runs through mermaid 12.2.0's real
// parser, and the db it fills must differ from the one before by exactly the
// change the op means.

import { EditorState } from "@codemirror/state";
import { describe, expect, test } from "vitest";

import { STATUS_COLOURS } from "../../../markdown/directives";
import { onlyEditedBytes, untouchedLinesEqual } from "../../../test/fidelity";
import {
  HAND_WRITTEN_LABELS,
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
import { COLOUR_PRESETS, SHAPES, decodeLabel, tidyLabel } from "./labels";
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
  deltaOf: (before: FlowDbView, after: FlowDbView) => string[] = dbDelta,
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
  const got = deltaOf(beforeDb, db);
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
    const handWritten = HAND_WRITTEN_LABELS.map((label) => ({
      label,
      shown: decodeLabel(`"${label}"`, true),
    }));
    return `${JSON.stringify({ drawn: texts.map(tidyLabel), markup, bodies, handWritten }, null, 1)}\n`;
  }

  test("is what the op writes today", { timeout: TIMEOUT }, () => {
    // Regenerate with FLUID_WRITE_LABEL_TABLE=1.
    const fs = node.process.getBuiltinModule("node:fs") as Fs & {
      writeFileSync: (path: string, text: string) => void;
    };
    if (node.process.env.FLUID_WRITE_LABEL_TABLE !== undefined) {
      fs.writeFileSync(FILE, table());
    }
    expect(fs.readFileSync(FILE, "utf8")).toBe(table());
  });

  test(
    "every body is one mermaid and the parser read the same way",
    { timeout: TIMEOUT },
    async () => {
      const { bodies } = JSON.parse(table()) as {
        bodies: Record<string, string>;
      };
      for (const body of Object.values(bodies)) {
        expect(agreesFlow(parseFlowchart(body), await flowDbFor(body))).toBe(
          null,
        );
      }
    },
  );
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

type InsertName = "addChild" | "addSibling" | "connect" | "addBox";

interface InsertTally {
  applied: Record<InsertName, number>;
  shifted: number;
  refused: string[];
  failures: string[];
}

/** Which arrow each `linkStyle` index means in `body`, as `start end occurrence`. */
function linkStyleTargets(body: string): string[][] {
  const model = parseFlowchart(body);
  return model.directives
    .filter((d) => d.kind === "linkStyle")
    .map((d) =>
      d.indices.map(({ n }) => {
        const edge = model.edges[n];
        return `${String(edge?.start)} ${String(edge?.end)} ${String(edge?.occurrence)}`;
      }),
    );
}

/** The one arrow in `after` that `before` does not have: the first place the ids part. */
function newEdgeIndex(before: FlowDbView, after: FlowDbView): number {
  const at = before.edges.findIndex((e, i) => after.edges[i]?.id !== e.id);
  return at === -1 ? before.edges.length : at;
}

/** Every op that adds something on `body`, each against mermaid's db. */
async function checkInserts(body: string): Promise<InsertTally> {
  const tally: InsertTally = {
    applied: { addChild: 0, addSibling: 0, connect: 0, addBox: 0 },
    shifted: 0,
    refused: [],
    failures: [],
  };
  const beforeDb = await flowDbFor(body);
  const model = parseFlowchart(body);
  const targets = JSON.stringify(linkStyleTargets(body));

  const attempt = async (
    what: string,
    name: InsertName,
    op: FlowOp,
    anchor: string | null,
    text: (id: string) => string,
    edge: boolean,
  ): Promise<void> => {
    const fail = (problem: string) => {
      tally.failures.push(`${what}: ${problem}\n--- body\n${body}`);
    };
    const result = compileFlowOp(model, op);
    if ("refused" in result) {
      tally.refused.push(`${what} ${result.refused}`);
      return;
    }
    const made = result.created as { kind: string; id: string } | undefined;
    if ((op.op === "connect") !== (made === undefined)) {
      fail(`created ${JSON.stringify(made)}`);
      return;
    }
    if (made !== undefined && made.kind !== "node") {
      fail(`created ${JSON.stringify(made)}`);
      return;
    }
    const id = made?.id ?? "";
    const group =
      anchor === null ? null : (model.nodes.get(anchor)?.subgraph ?? null);
    let added: FlowDbView["edges"][number] | undefined;
    const checked = await broken(
      body,
      beforeDb,
      result.edits,
      [
        ...(made ? [`vertex ${id} added`] : []),
        ...(made && group !== null ? [`subgraph ${group} nodes`] : []),
      ],
      (before, after) => {
        // Without the one new arrow, every arrow must read as it did, position
        // by position, its id and its linkStyle included.
        if (!edge) {
          return dbDelta(before, after);
        }
        const at = newEdgeIndex(before, after);
        added = after.edges[at];
        return dbDelta(before, {
          ...after,
          edges: after.edges.filter((_, i) => i !== at),
        });
      },
    );
    if ("problem" in checked) {
      fail(checked.problem);
      return;
    }
    const inserted = result.edits.filter((e) => e.insert.includes("\n"));
    if (inserted.length !== 1 || inserted[0]?.insert.trim() !== text(id)) {
      fail(`the new line is not ${text(id)}: ${JSON.stringify(inserted)}`);
    }
    if (made) {
      const holder = checked.db.subgraphs.find((s) => s.nodes.includes(id));
      if ((holder?.id ?? null) !== group) {
        fail(`${id} is in ${String(holder?.id)}, wanted ${String(group)}`);
      }
      const vertex = checked.db.vertices.find((v) => v.id === id);
      if (vertex?.text !== "New box" || vertex.type !== "square") {
        fail(`the box reads ${JSON.stringify(vertex)}`);
      }
    }
    if (
      edge &&
      (added?.type !== "arrow_point" ||
        added.stroke !== "normal" ||
        added.text !== "" ||
        added.style !== undefined)
    ) {
      fail(`the arrow reads ${JSON.stringify(added)}`);
    }
    if (JSON.stringify(linkStyleTargets(checked.after)) !== targets) {
      fail(`a linkStyle index moved: ${checked.after}`);
    }
    if (result.edits.length > 1) {
      tally.shifted++;
    }
    tally.applied[name]++;
  };

  for (const id of model.nodeOrder) {
    await attempt(
      `${id} addChild`,
      "addChild",
      { op: "addChild", id },
      id,
      (made) => `${id} --> ${made}[New box]`,
      true,
    );
    const parent = model.edges.find(
      (e) => e.end === id && e.start !== id && model.nodes.has(e.start),
    )?.start;
    await attempt(
      `${id} addSibling`,
      "addSibling",
      { op: "addSibling", id },
      parent ?? id,
      (made) =>
        parent === undefined
          ? `${made}[New box]`
          : `${parent} --> ${made}[New box]`,
      parent !== undefined,
    );
  }
  const first = model.nodeOrder.slice(0, 5);
  for (const from of first) {
    for (const to of first) {
      if (from !== to) {
        await attempt(
          `${from} connect ${to}`,
          "connect",
          { op: "connect", from, to },
          null,
          () => `${from} --> ${to}`,
          true,
        );
      }
    }
  }
  await attempt(
    "addBox",
    "addBox",
    { op: "addBox" },
    null,
    (made) => `${made}[New box]`,
    false,
  );
  return tally;
}

const insertTallies = new Map<string, Promise<InsertTally>>();

function insertTallyOf(name: string, body: string): Promise<InsertTally> {
  let tally = insertTallies.get(name);
  if (tally === undefined) {
    tally = checkInserts(body);
    insertTallies.set(name, tally);
  }
  return tally;
}

/** Cases the corpus does not hold, each next to a place a new line could go wrong. */
const INSERT_EXTRA: readonly (readonly [string, string])[] = [
  ...EXTRA,
  ["a peer's comment", "flowchart TD\n  A --> B\n    %% peer note\n  B --> C"],
  [
    "arrows after a linkStyle",
    "flowchart TD\n  A --> B\n  linkStyle 0 stroke:#000\n  B --> C\n  linkStyle 1 stroke:#111\n  C --> A",
  ],
  [
    "a linkStyle in a subgraph",
    "flowchart TD\n  X --> Y\n  subgraph s\n    A --> B\n    linkStyle 1 stroke:#000\n    B --> C\n  end\n  C --> X",
  ],
  [
    "an arrow into a subgraph",
    "flowchart TD\n  subgraph s\n    A --> B\n  end\n  C --> s\n  s --> D",
  ],
  [
    "style above the first ref",
    "flowchart TD\n  style C fill:#fff\n  A --> B\n  B --> C",
  ],
  ["a tabbed body", "flowchart TD\n\tA --> B\n\tsubgraph s\n\t\tC\n\tend"],
  ["no direction, no statement line", "graph TD; A --> B; B --> C"],
];

/**
 * Every refusal of an op that adds something, as `<fixture> <what> <code>`.
 * Asserted whole (R28).
 */
const EXPECTED_INSERT_REFUSALS: readonly string[] = [
  // `call` draws only with its shape right after the id: a bare `call --> x`
  // is a parse error in mermaid, pinned below. `each`'s parent is `call`.
  "r14-reflection-write call addChild invalid",
  "r14-reflection-write each addSibling invalid",
  "r14-reflection-write call connect each invalid",
  "r14-reflection-write call connect readable invalid",
  "r14-reflection-write call connect hidden invalid",
  "r14-reflection-write call connect wide invalid",
  "r14-reflection-write each connect call invalid",
  "r14-reflection-write readable connect call invalid",
  "r14-reflection-write hidden connect call invalid",
  "r14-reflection-write wide connect call invalid",
];

describe("every op that adds something, on the corpus", () => {
  test.each(corpus)(
    "%s",
    async (name, body) => {
      const tally = await insertTallyOf(name, body);
      expect(tally.failures).toEqual([]);
    },
    TIMEOUT,
  );

  test.each(INSERT_EXTRA)(
    "%s",
    async (name, body) => {
      const tally = await insertTallyOf(`extra ${name}`, body);
      expect(tally.failures).toEqual([]);
      expect(tally.applied.addChild).toBeGreaterThan(0);
    },
    TIMEOUT,
  );

  test(
    "the refusals over the corpus are these (R28)",
    async () => {
      const refused: string[] = [];
      for (const [name, body] of corpus) {
        const tally = await insertTallyOf(name, body);
        refused.push(...tally.refused.map((r) => `${name} ${r}`));
      }
      expect(refused).toEqual(EXPECTED_INSERT_REFUSALS);
    },
    TIMEOUT,
  );

  test(
    "each op applies on the named fixtures, so no suite passes by refusing (R28)",
    async () => {
      let shifted = 0;
      const applied = { addChild: 0, addSibling: 0, connect: 0, addBox: 0 };
      for (const [name, body] of corpus) {
        if (name.startsWith("f")) {
          const tally = await insertTallyOf(name, body);
          shifted += tally.shifted;
          for (const op of Object.keys(applied) as InsertName[]) {
            applied[op] += tally.applied[op];
          }
        }
      }
      expect(applied.addBox).toBe(24);
      expect(applied.addChild).toBeGreaterThanOrEqual(24);
      expect(applied.addSibling).toBeGreaterThanOrEqual(24);
      expect(applied.connect).toBeGreaterThanOrEqual(24);
      expect(shifted).toBeGreaterThan(0);
    },
    TIMEOUT,
  );

  test.each([
    "o",
    "x",
    "v",
    "default",
    "end1",
    "style1",
    "graph_a",
    "1",
    "A-1",
    "subgraphs",
    "TD",
    "LR",
    "direction",
    "directions",
    "n",
    "L_A_B_0",
    "href1",
    "_self1",
    "click1",
  ])(
    "the id %s next to a new line, each result read by mermaid or refused",
    async (id) => {
      const body = `flowchart TD\n  ${id} --> Q1\n  Q1 --> R1\n  R1 --> ${id}`;
      expect(parseFlowchart(body).understood).toBe(true);
      await flowDbFor(body);
      const tally = await checkInserts(body);
      expect(tally.failures).toEqual([]);
      expect(tally.refused).toEqual([]);
    },
    TIMEOUT,
  );

  test(
    "a bare call really is one mermaid refuses",
    async () => {
      expect(
        await flowDbOf("flowchart TD\n  call[x]", false),
      ).not.toHaveProperty("error");
      expect(
        await flowDbOf("flowchart TD\n  call[x]\n  call --> B", false),
      ).toHaveProperty("error");
    },
    TIMEOUT,
  );

  test.each(["f10-linkstyle", "f13-subgraphs", "r05-slab-apply"])(
    "%s saves with its own line breaks",
    (name) => {
      const lf = corpus.find(([n]) => n === name)?.[1] ?? "";
      const crlf = lf.replaceAll("\n", "\r\n");
      const state = EditorState.create({
        doc: crlf,
        extensions: lineSeparatorFor(crlf),
      });
      const body = state.doc.sliceString(0, state.doc.length);
      const model = parseFlowchart(body);
      const ops: FlowOp[] = [
        { op: "addBox" },
        ...model.nodeOrder.flatMap((id): FlowOp[] => [
          { op: "addChild", id },
          { op: "addSibling", id },
          { op: "connect", from: id, to: model.nodeOrder[0] ?? id },
        ]),
      ];
      for (const op of ops) {
        const result = compileFlowOp(model, op);
        if ("refused" in result) {
          throw new Error(`refused: ${JSON.stringify(op)} ${result.refused}`);
        }
        const saved = docText(
          state.update({
            changes: toChanges(0, result.edits, state.lineBreak),
          }).state,
        );
        expect(saved).toBe(
          applyEdits(body, result.edits).replaceAll("\n", "\r\n"),
        );
      }
    },
    TIMEOUT,
  );
});

type RemovalName =
  | "deleteNode"
  | "deleteEdge"
  | "label"
  | "noLabel"
  | "lineStyle"
  | "arrowhead"
  | "reverse";

interface RemovalTally {
  applied: Record<RemovalName, number>;
  /** `<what> <code>`, one per refused op. */
  refused: string[];
  /** Applied results where mermaid meets the kept boxes in another order. */
  reordered: number;
  failures: string[];
}

/** `db` without the box and the arrows an op took away, member lists as sets. */
function withoutGone(
  db: FlowDbView,
  box: string | null,
  arrows: ReadonlySet<number>,
  clusters: ReadonlySet<string>,
): FlowDbView {
  return {
    ...db,
    vertices: db.vertices.filter((v) => v.id !== box && !clusters.has(v.id)),
    edges: db.edges.filter((_, i) => !arrows.has(i)),
    subgraphs: db.subgraphs.map((s) => ({
      ...s,
      nodes: s.nodes.filter((n) => n !== box).toSorted(),
    })),
  };
}

const LINE_STROKES = {
  solid: "normal",
  dotted: "dotted",
  thick: "thick",
} as const;

const HEAD_TYPES = {
  arrow: "arrow_point",
  none: "arrow_open",
  both: "double_arrow_point",
} as const;

function generatedId(id: string, start: string, end: string): boolean {
  const prefix = `L_${start}_${end}_`;
  return id.startsWith(prefix) && /^\d+$/.test(id.slice(prefix.length));
}

/**
 * The removals and arrow ops on `body`, each against mermaid's db. A refusal must
 * have the reason its op can have: `group` only on a link that makes more than one
 * arrow, `chain` only in a chain, `unsupported` only on an invisible link. The db
 * after a result must be the db before with exactly the intended change: the box
 * and its arrows gone, or the one field of the one arrow. A kept box keeps its
 * text, type, styles, classes, link and subgraph; a kept arrow its ends, label,
 * look and style. What can move: the order mermaid meets the boxes in (the plan's
 * own `A --> B` removal moves it), and the number in a generated id of a later
 * arrow of the same pair.
 */
async function checkRemovals(body: string): Promise<RemovalTally> {
  const tally: RemovalTally = {
    applied: {
      deleteNode: 0,
      deleteEdge: 0,
      label: 0,
      noLabel: 0,
      lineStyle: 0,
      arrowhead: 0,
      reverse: 0,
    },
    refused: [],
    reordered: 0,
    failures: [],
  };
  const beforeDb = await flowDbFor(body);
  const model = parseFlowchart(body);
  const clusterIds = model.subgraphs.map((s) => s.id);

  const attempt = async (
    what: string,
    name: RemovalName,
    op: FlowOp,
    codes: readonly string[],
    wanted: readonly string[],
    box: string | null,
    arrows: ReadonlySet<number>,
    alsoNoOp: boolean,
    after: ((db: FlowDbView) => string | null) | null = null,
    /** How an arrow of the after db reads as its old self, for `reverse`. */
    restore: (db: FlowDbView) => FlowDbView = (db) => db,
  ): Promise<void> => {
    const fail = (problem: string) => {
      tally.failures.push(`${what}: ${problem}\n--- body\n${body}`);
    };
    const result = compileFlowOp(model, op);
    if ("refused" in result) {
      if (!codes.includes(result.refused)) {
        fail(`refused ${result.refused}`);
      }
      tally.refused.push(`${what} ${result.refused}`);
      return;
    }
    if (result.edits.length === 0) {
      if (!alsoNoOp) {
        fail("no edits");
      }
      return;
    }
    let reordered = false;
    const checked = await broken(
      body,
      beforeDb,
      result.edits,
      wanted,
      (dbBefore, dbAfter) => {
        const clusters = new Set(
          clusterIds.filter((id) => !dbAfter.vertices.some((v) => v.id === id)),
        );
        const was = withoutGone(dbBefore, box, arrows, clusters);
        const now = withoutGone(restore(dbAfter), box, new Set(), clusters);
        return dbDelta(was, now).filter((difference) => {
          if (difference === "vertex order") {
            reordered = true;
            return false;
          }
          const id = /^edge (\d+) id$/.exec(difference);
          if (id === null) {
            return true;
          }
          const a = was.edges[Number(id[1])];
          const b = now.edges[Number(id[1])];
          return !(
            a !== undefined &&
            b !== undefined &&
            generatedId(a.id, a.start, a.end) &&
            generatedId(b.id, b.start, b.end)
          );
        });
      },
    );
    if ("problem" in checked) {
      fail(checked.problem);
      return;
    }
    const problem = after?.(checked.db) ?? null;
    if (problem !== null) {
      fail(problem);
      return;
    }
    if (reordered) {
      tally.reordered++;
    }
    tally.applied[name]++;
  };

  for (const id of model.nodeOrder) {
    const gone = new Set(
      model.edges
        .filter((e) => e.start === id || e.end === id)
        .map((e) => e.index),
    );
    await attempt(
      `${id} deleteNode`,
      "deleteNode",
      { op: "deleteNode", id },
      ["invalid"],
      [],
      id,
      gone,
      false,
      (db) =>
        db.vertices.some((v) => v.id === id)
          ? "the box is still there"
          : db.edges.some((e) => e.start === id || e.end === id)
            ? "an arrow of the box is still there"
            : null,
    );
  }

  for (const edge of model.edges) {
    const name = `${edge.start}>${edge.end}#${edge.occurrence}`;
    const key = {
      start: edge.start,
      end: edge.end,
      occurrence: edge.occurrence,
    };
    const chain = model.chains[edge.chain];
    const link = chain?.links[edge.link];
    if (chain === undefined || link === undefined) {
      continue;
    }
    const grouped = link.edges.length > 1;
    const inGroup = grouped ? ["group", "invalid"] : ["invalid"];
    const n = edge.index;
    const old = beforeDb.edges[n];
    if (old === undefined) {
      continue;
    }
    const keep = new Set<number>();

    await attempt(
      `${name} deleteEdge`,
      "deleteEdge",
      { op: "deleteEdge", edge: key },
      inGroup,
      [],
      null,
      new Set([n]),
      false,
      (db) =>
        db.edges.length === beforeDb.edges.length - 1
          ? null
          : "not one arrow fewer",
    );

    await attempt(
      `${name} label`,
      "label",
      { op: "edgeLabel", edge: key, text: "Label" },
      inGroup,
      [`edge ${n} text`],
      null,
      keep,
      old.text === "Label",
      (db) =>
        db.edges[n]?.text === "Label"
          ? null
          : `the label reads ${String(db.edges[n]?.text)}`,
    );

    await attempt(
      `${name} noLabel`,
      "noLabel",
      { op: "edgeLabel", edge: key, text: "" },
      inGroup,
      old.labelType === "markdown"
        ? [`edge ${n} text`, `edge ${n} labelType`]
        : [`edge ${n} text`],
      null,
      keep,
      old.text === "",
      (db) =>
        db.edges[n]?.text === ""
          ? null
          : `the label reads ${String(db.edges[n]?.text)}`,
    );

    for (const line of ["solid", "dotted", "thick"] as const) {
      const invisible = old.stroke === "invisible";
      await attempt(
        `${name} lineStyle ${line}`,
        "lineStyle",
        { op: "lineStyle", edge: key, line },
        inGroup,
        [
          ...(invisible && old.type !== "arrow_point"
            ? [`edge ${n} type`]
            : []),
          ...(old.stroke === LINE_STROKES[line] ? [] : [`edge ${n} stroke`]),
        ],
        null,
        keep,
        old.stroke === LINE_STROKES[line],
        (db) =>
          db.edges[n]?.stroke === LINE_STROKES[line]
            ? null
            : `the stroke reads ${String(db.edges[n]?.stroke)}`,
      );
    }

    for (const head of ["arrow", "none", "both"] as const) {
      const invisible = old.stroke === "invisible";
      await attempt(
        `${name} arrowhead ${head}`,
        "arrowhead",
        { op: "arrowhead", edge: key, head },
        invisible ? [...inGroup, "unsupported"] : inGroup,
        [`edge ${n} type`],
        null,
        keep,
        old.type === HEAD_TYPES[head],
        (db) =>
          db.edges[n]?.type === HEAD_TYPES[head]
            ? null
            : `the type reads ${String(db.edges[n]?.type)}`,
      );
    }

    const loop = edge.start === edge.end;
    await attempt(
      `${name} reverse`,
      "reverse",
      { op: "reverse", edge: key },
      chain.links.length > 1 ? ["chain"] : grouped ? ["group"] : ["invalid"],
      [],
      null,
      keep,
      loop,
      (db) =>
        db.edges[n]?.start === edge.end && db.edges[n]?.end === edge.start
          ? null
          : "the arrow does not run the other way",
      (db) => ({
        ...db,
        edges: db.edges.map((e, i) =>
          i === n
            ? {
                ...e,
                start: old.start,
                end: old.end,
                id:
                  generatedId(e.id, e.start, e.end) || e.id === old.id
                    ? old.id
                    : e.id,
              }
            : e,
        ),
      }),
    );
  }
  return tally;
}

const removalTallies = new Map<string, Promise<RemovalTally>>();

function removalTallyOf(name: string, body: string): Promise<RemovalTally> {
  let tally = removalTallies.get(name);
  if (tally === undefined) {
    tally = checkRemovals(body);
    removalTallies.set(name, tally);
  }
  return tally;
}

/** Cases the corpus does not hold, each next to a place a removal could go wrong. */
const REMOVAL_EXTRA: readonly (readonly [string, string])[] = [
  ...INSERT_EXTRA,
  ["a loop", "flowchart TD\n  A --> A\n  A --> B\n  B --> B"],
  ["only a loop", "flowchart TD\n  A --> A"],
  [
    "a pair twice, styled",
    "flowchart TD\n  A --> B\n  A --> B\n  A --> B\n  linkStyle 1 stroke:#f00\n  linkStyle 2 stroke:#00f",
  ],
  [
    "a shared class line",
    "flowchart TD\n  A --> B --> C\n  class A,B x\n  class B,C y\n  classDef x fill:#fff",
  ],
  [
    "a shared click line",
    'flowchart TD\n  A --> B --> C\n  click A,B "https://x.y"\n  click C "https://x.y" "tip"',
  ],
  [
    "a click URL with a ;",
    'flowchart TD\n  A --> B; click A "https://x.y/?a=1;b=2"\n  B --> C',
  ],
  [
    "statements side by side",
    "flowchart TD\n  A --> B; B --> C; style B stroke-width:1px; linkStyle 0 stroke-width:2px; linkStyle 1 stroke-width:3px",
  ],
  [
    "an accDescr block",
    "flowchart TD\n  accDescr {\n    words\n  } A --> B; B --> C\n  C --> D",
  ],
  [
    "an accDescr block with one statement",
    "flowchart TD\n  accDescr {\n    words\n  } A --> B\n  A --> C",
  ],
  [
    "one-line subgraphs",
    "flowchart TD\n  subgraph s; A --> B; end\n  subgraph t; B --> C; C; end",
  ],
  [
    "a box two subgraphs name",
    "flowchart TD\n  subgraph s\n    A --> B\n  end\n  subgraph t\n    B --> C\n  end",
  ],
  [
    "nested, the last box",
    "flowchart TD\n  subgraph outer\n    subgraph inner\n      A\n    end\n    B\n  end\n  B --> inner",
  ],
  [
    "a subgraph moved by an arrow",
    "flowchart TD\n  subgraph s\n    A\n  end\n  subgraph t\n    B --> s\n    C\n  end",
  ],
  [
    "a direction in a subgraph",
    "flowchart TD\n  subgraph s\n    direction LR\n    A --> B\n  end\n  B --> C",
  ],
  [
    "a class before the box",
    "flowchart TD\n  A --> B\n  class B hot\n  B --> C",
  ],
  [
    "a click before the box",
    'flowchart TD\n  A --> B\n  click B "https://x.y"\n  B --> C',
  ],
  [
    "a box defined twice",
    "flowchart TD\n  A[x] --> B\n  A[y] --> C\n  B --> C",
  ],
  [
    "explicit arrow ids",
    "flowchart TD\n  A e1@--> B\n  B e2@-.->|x| C\n  C e3@-- y --> A",
  ],
  ["an id on a group", "flowchart TD\n  A & B e1@--> C & D"],
  [
    "every arrow kind",
    "flowchart TD\n  A --- B\n  A --> B\n  A -.- B\n  A -.-> B\n  A === B\n  A ==> B\n  A ~~~ B\n  A --o B\n  A --x B\n  A o--o B\n  A x--x B\n  A <--> B\n  A <-.-> B\n  A <==> B\n  A ---> B\n  A -..-> B\n  A ===> B",
  ],
  [
    "every label form",
    'flowchart TD\n  A -->|p| B\n  A -->|"q"| B\n  A -->|"`m`"| B\n  A -- t --> B\n  A -- "u" --> B\n  A -. d .-> B\n  A == h ==> B\n  A <-- b --> B\n  A o-- c --o B\n  A ~~~|i| B',
  ],
  [
    "tight ids with o and x",
    "flowchart TD\n  A-->xray\n  B==>oauth\n  C-.->xo\n  xray---ox\n  ox-->A",
  ],
  [
    "a trailing ; and a style",
    "flowchart TD\n  A --> B;\n  B --> C;\n  style B fill:#fff;\n  linkStyle 0 stroke:#f00",
  ],
  [
    "a comment on every side",
    "flowchart TD\n  %% before\n  A --> B\n  %% between\n  B --> C\n  %% after",
  ],
  ["a trailing break and blanks", "flowchart TD\n\n  A --> B\n\n  B --> C\n\n"],
  [
    "frontmatter and a directive",
    "---\ntitle: x\n---\n%%{init: {}}%%\nflowchart TD\n  A --> B\n  B --> C",
  ],
  [
    "ids a removal can leave side by side",
    "flowchart TD\n  A --> direction\n  B --> C\n  LR --> D",
  ],
];

/**
 * How many results refuse over the corpus, per op and code. Asserted whole (R28),
 * so a suite that refuses more goes red, and each is checked above to have the
 * reason its op can have. Printed by the opt-in path too.
 */
const EXPECTED_REMOVAL_REFUSALS: Readonly<Record<string, number>> = {
  "arrowhead group": 57,
  "arrowhead unsupported": 4,
  "deleteEdge group": 19,
  "label group": 19,
  "lineStyle group": 57,
  "noLabel group": 19,
  "reverse chain": 9,
  "reverse group": 19,
};

function refusalCounts(refusals: readonly string[]): Record<string, number> {
  const counts: Record<string, number> = {};
  for (const refusal of refusals) {
    const words = refusal.split(" ");
    const key = `${words[1] ?? ""} ${words.at(-1) ?? ""}`;
    counts[key] = (counts[key] ?? 0) + 1;
  }
  return counts;
}

/** Whether every line no edit overlaps keeps its own line break in `saved`. */
function ownBreaksKept(
  doc: string,
  edits: readonly TextEdit[],
  saved: string,
): string | null {
  const lineBreaks = (text: string) =>
    [...text.matchAll(/([^\r\n]*)(\r\n|\n|$)/g)]
      .slice(0, text.endsWith("\n") ? undefined : -1)
      .map((m) => ({ text: m[1] ?? "", end: m[2] ?? "" }));
  const before = lineBreaks(doc);
  const now = lineBreaks(saved);
  let from = 0;
  let at = 0;
  for (const line of before) {
    const to = from + line.text.length;
    const whole = !edits.some(
      (e) =>
        e.from < to + 1 && e.to > from && e.to - e.from + e.insert.length > 0,
    );
    from = to + 1;
    if (!whole) {
      continue;
    }
    while (at < now.length && now[at]?.text !== line.text) {
      at++;
    }
    const kept = now[at];
    if (kept === undefined) {
      return `the line ${JSON.stringify(line.text)} is gone`;
    }
    if (at < now.length - 1 && line.end !== "" && kept.end !== line.end) {
      return `the line ${JSON.stringify(line.text)} ends in ${JSON.stringify(kept.end)}`;
    }
    at++;
  }
  return null;
}

describe("every removal and arrow op, on the corpus", () => {
  test.each(corpus)(
    "%s",
    async (name, body) => {
      const tally = await removalTallyOf(name, body);
      expect(tally.failures).toEqual([]);
    },
    TIMEOUT,
  );

  test.each(REMOVAL_EXTRA)(
    "%s",
    async (name, body) => {
      const tally = await removalTallyOf(`extra ${name}`, body);
      expect(tally.failures).toEqual([]);
      expect(tally.applied.deleteNode).toBeGreaterThan(0);
    },
    TIMEOUT,
  );

  test(
    "the refusals over the corpus are these (R28)",
    async () => {
      const refused: string[] = [];
      for (const [name, body] of corpus) {
        const tally = await removalTallyOf(name, body);
        refused.push(...tally.refused);
      }
      expect(refusalCounts(refused)).toEqual(EXPECTED_REMOVAL_REFUSALS);
    },
    TIMEOUT,
  );

  test(
    "each op applies on the named fixtures, so no suite passes by refusing (R28)",
    async () => {
      const applied: Record<RemovalName, number> = {
        deleteNode: 0,
        deleteEdge: 0,
        label: 0,
        noLabel: 0,
        lineStyle: 0,
        arrowhead: 0,
        reverse: 0,
      };
      let reordered = 0;
      for (const [name, body] of corpus) {
        if (name.startsWith("f")) {
          const tally = await removalTallyOf(name, body);
          reordered += tally.reordered;
          for (const op of Object.keys(applied) as RemovalName[]) {
            applied[op] += tally.applied[op];
          }
        }
      }
      expect(applied.deleteNode).toBeGreaterThanOrEqual(110);
      expect(applied.deleteEdge).toBeGreaterThanOrEqual(91);
      expect(applied.label).toBeGreaterThanOrEqual(91);
      expect(applied.noLabel).toBeGreaterThanOrEqual(11);
      expect(applied.lineStyle).toBeGreaterThanOrEqual(183);
      expect(applied.arrowhead).toBeGreaterThanOrEqual(182);
      expect(applied.reverse).toBeGreaterThanOrEqual(82);
      expect(reordered).toBeGreaterThan(0);
    },
    TIMEOUT,
  );

  test(
    "a refusal really is one: without the trap's check the arrow would change ends",
    async () => {
      const db = await flowDbFor("flowchart TD\n  A---xray");
      expect(db.edges[0]?.end).toBe("ray");
      expect(db.edges[0]?.type).toBe("arrow_cross");
    },
    TIMEOUT,
  );

  test.each([
    "f10-linkstyle",
    "f13-subgraphs",
    "f11-style-class",
    "r05-slab-apply",
  ])(
    "%s keeps every line's own break, in one kind and in both",
    (name) => {
      const lf = corpus.find(([n]) => n === name)?.[1] ?? "";
      const crlf = lf.replaceAll("\n", "\r\n");
      let k = 0;
      const mixed = lf.replaceAll("\n", () => (k++ % 2 === 0 ? "\r\n" : "\n"));
      const model = parseFlowchart(lf);
      const ops: FlowOp[] = [
        ...model.nodeOrder.map((id): FlowOp => ({ op: "deleteNode", id })),
        ...model.edges.flatMap((e): FlowOp[] => {
          const edge = {
            start: e.start,
            end: e.end,
            occurrence: e.occurrence,
          };
          return [
            { op: "deleteEdge", edge },
            { op: "edgeLabel", edge, text: "Label" },
            { op: "edgeLabel", edge, text: "" },
            { op: "lineStyle", edge, line: "thick" },
            { op: "arrowhead", edge, head: "both" },
            { op: "reverse", edge },
          ];
        }),
      ];
      let applied = 0;
      for (const op of ops) {
        const result = compileFlowOp(model, op);
        if ("refused" in result || result.edits.length === 0) {
          continue;
        }
        applied++;
        const of = (doc: string) => {
          const state = EditorState.create({
            doc,
            extensions: lineSeparatorFor(doc),
          });
          return docText(
            state.update({
              changes: toChanges(0, result.edits, state.lineBreak),
            }).state,
          );
        };
        expect(of(crlf)).toBe(
          applyEdits(lf, result.edits).replaceAll("\n", "\r\n"),
        );
        const saved = of(mixed);
        expect(saved.replaceAll("\r\n", "\n")).toBe(
          applyEdits(lf, result.edits),
        );
        expect(ownBreaksKept(mixed, result.edits, saved)).toBeNull();
      }
      expect(applied).toBeGreaterThan(10);
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
      const inserts = await checkInserts(body);
      const removals = await checkRemovals(body);
      failures.push(
        ...[...tally.failures, ...inserts.failures, ...removals.failures].map(
          (f) => `${file}: ${f}`,
        ),
      );
      for (const [key, count] of Object.entries(
        refusalCounts(removals.refused),
      )) {
        counts[key] = (counts[key] ?? 0) + count;
      }
      for (const refusal of inserts.refused) {
        const words = refusal.split(" ");
        const key = `${words.at(-2) === "connect" ? "connect" : (words.at(1) ?? "")} ${words.at(-1) ?? ""}`;
        counts[key] = (counts[key] ?? 0) + 1;
      }
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
