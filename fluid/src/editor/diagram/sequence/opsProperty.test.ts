// NO vi.mock("mermaid") here: every result runs through mermaid 12.2.0's real
// parser, and the db it fills must be exactly the one the op means. The expected
// db is built here from the db before, not from what `compileSeqOp` did.

import { describe, expect, test } from "vitest";

import { onlyEditedBytes, untouchedLinesEqual } from "../../../test/fidelity";
import { LABEL_TEXTS } from "../../../test/labelTable";
import { seqDbDelta } from "../../../test/mermaidOracle";
import { type TextEdit, applyEdits } from "../edits";
import { type SequenceDbView, sequenceDbOf } from "../mermaidDb";
import { agreesSequence, normalizeLabel } from "../verdict";
import { drawsOtherwise, tidySeqText } from "./labels";
import type { SeqHead, SeqModel, SeqTarget } from "./model";
import { type SeqOp, compileSeqOp } from "./ops";
import { decodeSeqText, parseSequence } from "./parse";

const TIMEOUT = 300_000;

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

const SEQUENCE = import.meta.glob<string>(
  "../../fixtures/diagrams/sequence/*.mmd",
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

const corpus = Object.entries(SEQUENCE)
  .map(([path, text]) => [nameOf(path), fenceBody(text)] as const)
  .sort(([a], [b]) => a.localeCompare(b));

/** What the corpus does not hold, each next to a place an op could go wrong. */
const EXTRA: readonly (readonly [string, string])[] = [
  [
    "no text",
    "sequenceDiagram\n  A->>B:\n  B->>A: wrap:\n  A->>B: \n  B->>A: y",
  ],
  ["no text on a shared line", "sequenceDiagram\n  A->>B:;B->>A:\n  A->>A: x"],
  [
    "spaced names",
    "sequenceDiagram\n  participant Web App\n  User One->>Web App: x\n  Web App-->>User One: y",
  ],
  [
    "spaced with an alias around it",
    "sequenceDiagram\n  participant a as A\n  a->>Web App: x\n  Web App->>b: y",
  ],
  [
    "config aliases",
    'sequenceDiagram\n  participant a@{ "type": "queue", "alias": "Old" }\n  participant b@{ alias: Bare }\n  participant c@{ "type": "actor" }\n  a->>b: x\n  b->>c: y',
  ],
  [
    "config alias with forms",
    'sequenceDiagram\n  participant a@{ "alias": "x#35;y#59;z" }\n  a->>a: x',
  ],
  [
    "undeclared in blocks",
    "sequenceDiagram\n  participant a\n  loop one\n    alt two\n      a->>b: x\n    else three\n      c->>d: y\n      Note over e,f: n\n    end\n  end\n  rect rgb(1,2,3)\n    f->>g: z\n  end",
  ],
  [
    "second mention on a line",
    "sequenceDiagram\n  a->>b: x;c->>d: y\n  d->>a: z",
  ],
  [
    "shared lines",
    "sequenceDiagram\n  participant a; participant b as B; a->>b:x;b-->>a:y\n  loop l; a->>b: z; b->>a: w; end\n  a->>b:v;",
  ],
  [
    "create first",
    "sequenceDiagram\n  participant h\n  create participant w\n  h->>w: 1\n  w->>h: 2\n  create actor x as X\n  w->>x: 3\n  destroy x\n  x-->>w: 4\n  w->>w: 5",
  ],
  [
    "create with an undeclared sender",
    "sequenceDiagram\n  create participant w\n  s->>w: 1\n  destroy w\n  t-xw: 2\n  t->>s: 3",
  ],
  [
    "activations",
    "sequenceDiagram\n  participant a\n  participant b\n  a->>+b: 1\n  a->>+b: 2\n  b-->>-a: 3\n  activate a\n  a->>a: 4\n  deactivate a\n  b-->>-a: 5",
  ],
  ["open activation", "sequenceDiagram\n  a->>+b: 1\n  a->>b: 2"],
  [
    "hand-empty sections",
    "sequenceDiagram\n  participant a\n  loop empty\n  end\n  alt x\n  else y\n    a->>a: 1\n    a->>a: 2\n  end\n  a->>a: 3",
  ],
  [
    "only messages in blocks",
    "sequenceDiagram\n  participant a\n  participant b\n  loop l\n    a->>b: 1\n  end\n  par x\n    Note over a: 2\n  and y\n    b->>a: 3\n  end\n  opt o\n    Note over a,b: 4\n  end",
  ],
  [
    "boxes",
    "sequenceDiagram\n  box Aqua One\n    participant a\n  end\n  box Two\n    participant b\n    actor c\n  end\n  a->>b: 1\n  b->>c: 2",
  ],
  [
    "box then a loose one",
    "sequenceDiagram\n  participant z\n  box One\n    participant a\n  end\n  participant y\n  a->>z: 1",
  ],
  [
    "declared after",
    "sequenceDiagram\n  a->>b: 1\n  participant b as B\n  participant c\n  b->>c: 2",
  ],
  [
    "two aliases",
    "sequenceDiagram\n  participant a as One\n  a->>a: 1\n  participant a as Two",
  ],
  [
    "duplicates",
    "sequenceDiagram\n  participant a\n  participant b\n  a->>b: same\n  b->>a: other\n  a->>b: same\n  a->>b: same\n  a->>b:\n  a->>b:",
  ],
  [
    "notes first",
    "sequenceDiagram\n  Note over a,b: n\n  participant c\n  a->>c: x",
  ],
  [
    "comments everywhere",
    "---\ntitle: T\n---\n%%{init: {}}%%\nsequenceDiagram\n%% one\n  participant a\n%% two\n  a->>b: x\n%% three\n  b->>a: y\n%% four",
  ],
  [
    "comment forms beside lines",
    "sequenceDiagram\n  participant a\n  loop l\n    a->>a: 1\n    a->>a: 2\n  end %% tail\n  % other\n  # hash\n  a->>a: 3",
  ],
  [
    "acc blocks",
    "sequenceDiagram\n  participant a\n  a->>a: 1\n  accDescr {\n      words\n  }\n  a->>a: 2\n  accTitle: T\n  title Z",
  ],
  [
    "keyword-like names",
    "sequenceDiagram\n  participant loop\n  participant as as N\n  link->>as: x\n  endpoint->>link: y",
  ],
  [
    "odd names",
    "sequenceDiagram\n  a-b->>c.d: 1\n  c.d->>e_f: 2\n  x@y->>a-b: 3\n  1->>2: 4",
  ],
  [
    "trailing break and blanks",
    "sequenceDiagram\n  participant a\n  a->>a: 1\n\n\n",
  ],
  ["no statements", "sequenceDiagram"],
  ["only a comment", "sequenceDiagram\n  %% c"],
  ["header takes a statement", "sequenceDiagram; a->>b: x"],
  [
    "menus",
    'sequenceDiagram\n  participant a\n  participant b\n  link a: L @ https://e.com\n  links b: {"x": "https://e.com"}\n  a->>b: 1\n  properties a: {"c": "d"}',
  ],
  [
    "wrap in an alias",
    "sequenceDiagram\n  participant a as wrap: One\n  a->>a: nowrap: Two",
  ],
];

type OpName =
  | "renameParticipant"
  | "renameMessage"
  | "arrowStyle"
  | "deleteMessage"
  | "deleteParticipant"
  | "addParticipant"
  | "addMessage";

interface Tally {
  applied: Record<OpName, number>;
  /** `<op> <code>`, one per refused op. */
  refused: string[];
  /** Compiled, and mermaid's db refused it: what the apply layer's verify refuses. */
  verifyRefused: string[];
  /** Ops where the source already says this. */
  noEdit: number;
  failures: string[];
}

function emptyTally(): Tally {
  return {
    applied: {
      renameParticipant: 0,
      renameMessage: 0,
      arrowStyle: 0,
      deleteMessage: 0,
      deleteParticipant: 0,
      addParticipant: 0,
      addMessage: 0,
    },
    refused: [],
    verifyRefused: [],
    noEdit: 0,
    failures: [],
  };
}

function commentLines(body: string): string[] {
  return body.split("\n").filter((line) => /^\s*%%/.test(line));
}

type MessageTarget = SeqTarget & { kind: "message" };

function targetsOf(model: SeqModel): MessageTarget[] {
  return model.messages.map((m) => ({
    kind: "message",
    from: m.from,
    to: m.to,
    text: m.text ? decodeSeqText(model.body.slice(m.text.from, m.text.to)) : "",
    occurrence: m.occurrence,
  }));
}

/** mermaid's LINETYPE, written out here on its own (research note, "Arrows"). */
const LINETYPE: Readonly<Record<string, number>> = {
  "solid arrow": 0,
  "dotted arrow": 1,
  "solid cross": 3,
  "dotted cross": 4,
  "solid open": 5,
  "dotted open": 6,
  "solid async": 24,
  "dotted async": 25,
  "solid both": 33,
  "dotted both": 34,
};

const HEADS: readonly (SeqHead | "both")[] = [
  "arrow",
  "open",
  "cross",
  "async",
  "both",
];
const LINES = ["solid", "dotted"] as const;

/** The db with these records out, ids renumbered and the indices moved as mermaid would. */
function withoutRecords(
  db: SequenceDbView,
  gone: ReadonlySet<number>,
): SequenceDbView {
  const moved = ([name, index]: [string, number]): [string, number] => [
    name,
    index - [...gone].filter((g) => g < index).length,
  ];
  return {
    ...db,
    records: db.records
      .filter((_, i) => !gone.has(i))
      .map((record, i) => ({ ...record, id: String(i) })),
    created: db.created.map(moved),
    destroyed: db.destroyed.map(moved),
  };
}

/** The records of a statement: mine by `agreesSequence`'s proof that they are mermaid's. */
function recordsOfLine(model: SeqModel, record: number): number[] {
  return (
    model.statements.find((s) => s.records.includes(record))?.records ?? []
  );
}

/** What deleting `name` means, from the db before and the statements' record lists. */
function dbWithout(
  model: SeqModel,
  db: SequenceDbView,
  name: string,
): SequenceDbView {
  const gone = new Set<number>();
  const records = db.records.map((record) => ({ ...record }));
  for (const statement of model.statements) {
    const names = statement.actors.map((a) => a.name);
    if (!names.includes(name)) {
      continue;
    }
    const other = names.find((n) => n !== name);
    const row = records[statement.records[0] ?? -1];
    if (statement.kind === "note" && other !== undefined && row) {
      row.from = other;
      row.to = other;
    } else {
      for (const record of statement.records) {
        gone.add(record);
      }
    }
  }
  const out = withoutRecords({ ...db, records }, gone);
  return {
    ...out,
    actors: out.actors.filter((a) => a.name !== name),
    boxes: out.boxes.map((b) => ({
      actors: b.actors.filter((a) => a !== name),
    })),
    created: out.created.filter(([n]) => n !== name),
    destroyed: out.destroyed.filter(([n]) => n !== name),
  };
}

/** The occurrence each message must have, counted from the db on its own. */
function occurrencesIn(model: SeqModel, db: SequenceDbView): number[] {
  const counts = new Map<string, number>();
  return model.messages.map((message) => {
    const record = db.records[message.record];
    const key = JSON.stringify([
      record?.from,
      record?.to,
      decodeSeqText(normalizeLabel(record?.message ?? "")),
    ]);
    const seen = counts.get(key) ?? 0;
    counts.set(key, seen + 1);
    return seen;
  });
}

interface Wanted {
  /** The exact differences from the db before. */
  delta?: readonly string[];
  /** Or: the db the result must be. */
  db?: SequenceDbView;
  /** A last look at the db after. */
  holds?: (after: SequenceDbView) => string | null;
}

/** Whether `edits` keep every promise, `null` when they do. */
async function broken(
  before: string,
  beforeDb: SequenceDbView,
  edits: readonly TextEdit[],
  wanted: Wanted,
): Promise<string | null> {
  const after = applyEdits(before, edits);
  if (!onlyEditedBytes(before, after, edits)) {
    return "edited bytes outside the edits";
  }
  const moved = untouchedLinesEqual(before, after, edits);
  if (moved.length > 0) {
    return `untouched lines changed: ${JSON.stringify(moved)}`;
  }
  if (
    JSON.stringify(commentLines(before)) !== JSON.stringify(commentLines(after))
  ) {
    return "a %% line changed";
  }
  const model = parseSequence(after);
  if (!model.understood) {
    return `not understood: ${model.reasons.join("; ")}`;
  }
  const db = await sequenceDbOf(after, false);
  if ("error" in db) {
    // Only an activation left unbalanced gets this far: the parser does not count them.
    return /inactivate/i.test(String(db.error))
      ? "verify"
      : `mermaid refused: ${String(db.error)}`;
  }
  const difference = agreesSequence(model, db);
  if (difference !== null) {
    return `disagrees: ${difference}`;
  }
  const delta = wanted.db
    ? seqDbDelta(wanted.db, db)
    : seqDbDelta(beforeDb, db);
  const expected = wanted.db ? [] : wanted.delta;
  if (expected && JSON.stringify(delta) !== JSON.stringify(expected)) {
    return `delta ${JSON.stringify(delta)}, wanted ${JSON.stringify(expected)}`;
  }
  const occurrences = model.messages.map((m) => m.occurrence);
  const counted = occurrencesIn(model, db);
  if (JSON.stringify(occurrences) !== JSON.stringify(counted)) {
    return `occurrences ${JSON.stringify(occurrences)}, counted ${JSON.stringify(counted)}`;
  }
  return wanted.holds?.(db) ?? null;
}

/** Runs one op on `body` and files the result. */
async function runOp(
  tally: Tally,
  body: string,
  db: SequenceDbView,
  label: string,
  name: OpName,
  op: SeqOp,
  wanted: Wanted,
  mayRefuse: readonly string[],
): Promise<void> {
  const result = compileSeqOp(parseSequence(body), op);
  if ("refused" in result) {
    tally.refused.push(`${name} ${result.refused}`);
    if (!mayRefuse.includes(result.refused)) {
      tally.failures.push(`${label}: refused ${result.refused}`);
    }
    return;
  }
  if (result.edits.length === 0) {
    tally.noEdit++;
    return;
  }
  const failure = await broken(body, db, result.edits, wanted);
  if (failure === "verify") {
    tally.verifyRefused.push(name);
  } else if (failure !== null) {
    tally.failures.push(`${label}: ${failure}`);
  } else {
    tally.applied[name]++;
  }
}

/** Every op of the brief on `body`. */
async function checkBody(body: string): Promise<Tally> {
  const tally = emptyTally();
  const model = parseSequence(body);
  const db = await sequenceDbOf(body, false);
  if ("error" in db || !model.understood) {
    tally.failures.push(`not a starting point: ${model.reasons.join("; ")}`);
    return tally;
  }

  for (const name of model.order) {
    const mayRefuse = /\s/.test(name) ? ["unsupported"] : ["invalid"];
    await runOp(
      tally,
      body,
      db,
      `rename ${name}`,
      "renameParticipant",
      { op: "renameParticipant", name, text: "Renamed" },
      {
        delta: [`actor ${name} description`],
        holds: (after) =>
          after.actors.find((a) => a.name === name)?.description === "Renamed"
            ? null
            : "the label did not land",
      },
      mayRefuse,
    );
    await runOp(
      tally,
      body,
      db,
      `delete ${name}`,
      "deleteParticipant",
      { op: "deleteParticipant", name },
      { db: dbWithout(model, db, name) },
      ["invalid", "emptiesBlock"],
    );
  }

  for (const [i, target] of targetsOf(model).entries()) {
    const message = model.messages[i];
    if (message === undefined) {
      continue;
    }
    const record = message.record;
    await runOp(
      tally,
      body,
      db,
      `rename message ${String(i)}`,
      "renameMessage",
      { op: "renameMessage", target, text: "Renamed" },
      {
        delta: [`record ${String(record)} message`],
        holds: (after) =>
          after.records[record]?.message === "Renamed"
            ? null
            : "the text did not land",
      },
      [],
    );
    for (const line of LINES) {
      for (const head of HEADS) {
        const type = LINETYPE[`${line} ${head}`];
        await runOp(
          tally,
          body,
          db,
          `style message ${String(i)} ${line} ${head}`,
          "arrowStyle",
          { op: "arrowStyle", target, line, head },
          {
            delta: [`record ${String(record)} type`],
            holds: (after) =>
              after.records[record]?.type === type ? null : "the wrong arrow",
          },
          ["invalid"],
        );
      }
    }
    await runOp(
      tally,
      body,
      db,
      `delete message ${String(i)}`,
      "deleteMessage",
      { op: "deleteMessage", target },
      { db: withoutRecords(db, new Set(recordsOfLine(model, record))) },
      ["invalid", "emptiesBlock"],
    );
  }

  const free = (() => {
    let k = 1;
    while (db.actors.some((a) => a.name === `p${String(k)}`)) {
      k++;
    }
    return `p${String(k)}`;
  })();
  await runOp(
    tally,
    body,
    db,
    "add participant",
    "addParticipant",
    { op: "addParticipant" },
    {
      delta: [`actor ${free} added`],
      holds: (after) => {
        const last = after.actors.at(-1);
        return last?.name === free &&
          last.description === "New participant" &&
          last.type === "participant" &&
          !after.boxes.some((b) => b.actors.includes(free))
          ? null
          : "the new participant is not the rightmost";
      },
    },
    [],
  );

  const first = model.order.slice(0, 3);
  for (const from of first) {
    for (const to of first) {
      const n = db.records.length;
      await runOp(
        tally,
        body,
        db,
        `add message ${from} ${to}`,
        "addMessage",
        { op: "addMessage", from, to },
        {
          delta: [`record ${String(n)} added`],
          holds: (after) => {
            const last = after.records[n];
            return last?.from === from &&
              last.to === to &&
              last.message === "New message" &&
              last.type === 0
              ? null
              : "the new message is not the last record";
          },
        },
        ["invalid"],
      );
    }
  }
  return tally;
}

function counted(list: readonly string[]): Record<string, number> {
  const out: Record<string, number> = {};
  for (const key of [...list].sort()) {
    out[key] = (out[key] ?? 0) + 1;
  }
  return out;
}

function addUp(total: Tally, tally: Tally): void {
  for (const key of Object.keys(total.applied) as OpName[]) {
    total.applied[key] += tally.applied[key];
  }
  total.refused.push(...tally.refused);
  total.verifyRefused.push(...tally.verifyRefused);
  total.noEdit += tally.noEdit;
  total.failures.push(...tally.failures);
}

describe("every op on the corpus", () => {
  const total = emptyTally();

  test.each(corpus)(
    "%s",
    async (name, body) => {
      const tally = await checkBody(body);
      expect(tally.failures, name).toEqual([]);
      addUp(total, tally);
    },
    TIMEOUT,
  );

  test("each op applied at least this often", () => {
    // R28: a suite where everything refuses must go red.
    expect(total.applied.renameParticipant).toBeGreaterThanOrEqual(126);
    expect(total.applied.renameMessage).toBeGreaterThanOrEqual(243);
    expect(total.applied.arrowStyle).toBeGreaterThanOrEqual(2187);
    expect(total.applied.deleteMessage).toBeGreaterThanOrEqual(211);
    expect(total.applied.deleteParticipant).toBeGreaterThanOrEqual(92);
    expect(total.applied.addParticipant).toBe(corpus.length);
    expect(total.applied.addMessage).toBeGreaterThanOrEqual(249);
    // Each message has one arrow out of ten that it already is.
    expect(total.noEdit).toBe(243);
  });

  test("the refusals are these, asserted whole", () => {
    expect(counted(total.refused)).toEqual(CORPUS_REFUSALS);
    expect(counted(total.verifyRefused)).toEqual(CORPUS_VERIFY_REFUSALS);
  });
});

describe("every op on the extra bodies", () => {
  const total = emptyTally();

  test.each(EXTRA)(
    "%s",
    async (name, body) => {
      const tally = await checkBody(body);
      expect(tally.failures, name).toEqual([]);
      addUp(total, tally);
    },
    TIMEOUT,
  );

  test("each op applied, and the refusals are these, asserted whole", () => {
    for (const count of Object.values(total.applied)) {
      expect(count).toBeGreaterThan(0);
    }
    expect(counted(total.refused)).toEqual(EXTRA_REFUSALS);
    expect(counted(total.verifyRefused)).toEqual(EXTRA_VERIFY_REFUSALS);
  });
});

/** One way a text can land. `typed` has to be read back from the result as typed. */
const TEXT_HOLDERS: readonly (readonly [
  string,
  string,
  (text: string) => SeqOp,
  (model: SeqModel) => string,
])[] = [
  [
    "a message's text",
    "sequenceDiagram\n  A->>B: old",
    (text) => ({
      op: "renameMessage",
      target: {
        kind: "message",
        from: "A",
        to: "B",
        text: "old",
        occurrence: 0,
      },
      text,
    }),
    (model) => {
      const span = model.records[0]?.text;
      return span ? decodeSeqText(model.body.slice(span.from, span.to)) : "";
    },
  ],
  [
    "a message without a text",
    "sequenceDiagram\n  A->>B:",
    (text) => ({
      op: "renameMessage",
      target: { kind: "message", from: "A", to: "B", text: "", occurrence: 0 },
      text,
    }),
    (model) => {
      const span = model.records[0]?.text;
      return span ? decodeSeqText(model.body.slice(span.from, span.to)) : "";
    },
  ],
  [
    "an alias",
    "sequenceDiagram\n  participant A as old",
    (text) => ({ op: "renameParticipant", name: "A", text }),
    (model) => model.participants.get("A")?.label ?? "",
  ],
  [
    "a declaration with no alias",
    "sequenceDiagram\n  participant A",
    (text) => ({ op: "renameParticipant", name: "A", text }),
    (model) => model.participants.get("A")?.label ?? "",
  ],
  [
    "an undeclared participant",
    "sequenceDiagram\n  A->>A: x",
    (text) => ({ op: "renameParticipant", name: "A", text }),
    (model) => model.participants.get("A")?.label ?? "",
  ],
  [
    "a quoted alias in a config",
    'sequenceDiagram\n  participant A@{ "alias": "old" }',
    (text) => ({ op: "renameParticipant", name: "A", text }),
    (model) => model.participants.get("A")?.label ?? "",
  ],
  [
    "a bare alias in a config",
    "sequenceDiagram\n  participant A@{ alias: old }",
    (text) => ({ op: "renameParticipant", name: "A", text }),
    (model) => model.participants.get("A")?.label ?? "",
  ],
];

/** What the table adds to `LABEL_TEXTS`: this grammar's own edges. */
const SEQ_TEXTS: readonly string[] = [
  "wrap: x",
  "nowrap: x",
  ":wrap: x",
  "Wrap: x",
  ":x",
  "a: b",
  "#br#",
  "$$x$$",
  "$$x$$ y",
  "a $ b",
  "%%{init}%%",
  "a %%{b: c",
  "style a:#b; c",
  'style:"x"; y',
  "classDef:#x; y",
  "a;b;c",
  "#",
  "##",
  ";",
  ";;",
  "#59;",
  "<br>",
  "<br/>",
  "a<br>b",
  "}",
  "{",
  "a}b",
  '"',
  "\\",
  "x as y",
  "participant",
  "note over A",
  "A->>B: x",
  "->>",
  "one\n\ntwo",
  "\u00B6\u00DF",
  "\uFB02\u00B0",
  "\u00E9\u{1F600}",
  "%",
  "% x",
  "# x",
];

describe("a text lands as typed, on every holder", () => {
  const texts = [...new Set([...LABEL_TEXTS, ...SEQ_TEXTS])];

  test.each(TEXT_HOLDERS)(
    "%s",
    async (holder, body, op, read) => {
      const failures: string[] = [];
      let applied = 0;
      for (const typed of texts) {
        const result = compileSeqOp(parseSequence(body), op(typed));
        const text = tidySeqText(typed);
        if ("refused" in result) {
          // Only what `drawnLabels` would change, and nothing else.
          if (!drawsOtherwise(text)) {
            failures.push(`${JSON.stringify(typed)}: ${result.refused}`);
          }
          continue;
        }
        if (drawsOtherwise(text)) {
          failures.push(`${JSON.stringify(typed)}: applied`);
        }
        if (result.edits.length === 0) {
          continue;
        }
        const after = applyEdits(body, result.edits);
        const model = parseSequence(after);
        const db = await sequenceDbOf(after, false);
        if ("error" in db) {
          failures.push(`${JSON.stringify(typed)}: ${String(db.error)}`);
          continue;
        }
        const difference = agreesSequence(model, db);
        if (difference !== null) {
          failures.push(`${JSON.stringify(typed)}: ${difference}`);
          continue;
        }
        if (read(model) !== text) {
          failures.push(
            `${JSON.stringify(typed)}: reads ${JSON.stringify(read(model))}`,
          );
          continue;
        }
        applied++;
      }
      expect(failures, holder).toEqual([]);
      expect(applied).toBeGreaterThanOrEqual(texts.length - 8);
    },
    TIMEOUT,
  );
});

const folder = node.process.env.FLUID_DIAGRAM_CORPUS;

test.skipIf(folder === undefined)(
  "the opt-in corpus: every op on what is understood keeps its promises",
  async () => {
    const dir = folder ?? "";
    const fs = node.process.getBuiltinModule("node:fs") as Fs;
    const total = emptyTally();
    let used = 0;
    const listed: string[] = [];
    for (const file of fs.readdirSync(dir).filter((f) => f.endsWith(".mmd"))) {
      const body = fenceBody(fs.readFileSync(`${dir}/${file}`, "utf8"));
      if (!body.includes("sequenceDiagram")) {
        continue;
      }
      const model = parseSequence(body);
      const before = await sequenceDbOf(body, false);
      if (
        !model.understood ||
        "error" in before ||
        agreesSequence(model, before) !== null
      ) {
        listed.push(`${file}: ${model.reasons[0] ?? "mermaid refuses it"}`);
        continue;
      }
      used++;
      const tally = await checkBody(body);
      addUp(total, {
        ...tally,
        failures: tally.failures.map((f) => `${file}: ${f}`),
      });
    }
    console.log(
      `sequence ops: ${String(used)} files, refused ${JSON.stringify(counted(total.refused))}, verify ${JSON.stringify(counted(total.verifyRefused))}, not used ${String(listed.length)}${listed.map((l) => `\n  ${l}`).join("")}`,
    );
    expect(total.failures).toEqual([]);
  },
  1_800_000,
);

/**
 * addMessage: s10's two destroyed participants, in eight of its nine pairs.
 * emptiesBlock: R56. deleteMessage invalid: a message a `create` or `destroy` waits
 * for, and s02's first message, whose loss would move `browser`. deleteParticipant
 * invalid: a participant others are mentioned only with.
 */
const CORPUS_REFUSALS: Record<string, number> = {
  "addMessage invalid": 8,
  "deleteMessage emptiesBlock": 13,
  "deleteMessage invalid": 8,
  "deleteParticipant emptiesBlock": 29,
  "deleteParticipant invalid": 4,
};
/** An activation left unbalanced, which only mermaid's db sees. */
const CORPUS_VERIFY_REFUSALS: Record<string, number> = {
  deleteMessage: 11,
  deleteParticipant: 1,
};
const EXTRA_REFUSALS: Record<string, number> = {
  "addMessage invalid": 15,
  "deleteMessage emptiesBlock": 4,
  "deleteMessage invalid": 19,
  "deleteParticipant emptiesBlock": 10,
  "deleteParticipant invalid": 27,
  // `x@y`, which the declaration form reads otherwise.
  "renameParticipant invalid": 1,
  // R61: `Web App`, `User One` and the `Web App` of the body with an alias beside it.
  "renameParticipant unsupported": 3,
};
const EXTRA_VERIFY_REFUSALS: Record<string, number> = { deleteMessage: 2 };
