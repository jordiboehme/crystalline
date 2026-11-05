// NO vi.mock("mermaid") here: every corpus file runs through mermaid 12.2.0's
// real parser, and our model of it must agree with its db. The real lexer is
// also read back here: `parse.ts` keeps a copy of its rules, and the tables
// below fail when the copy and the original part.

import { describe, expect, test } from "vitest";

import { defined } from "../../../test/assert";
import { type SequenceDbView, sequenceDbOf } from "../mermaidDb";
import { scanFence } from "../scan";
import { agreesSequence } from "../verdict";
import type { SeqModel } from "./model";
import {
  LEXER_CONDITIONS,
  LEXER_RULES,
  lexSequence,
  parseSequence,
} from "./parse";

const SEQUENCE = import.meta.glob<string>(
  "../../fixtures/diagrams/sequence/*.mmd",
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

const corpus = Object.entries(SEQUENCE)
  .map(([path, text]) => [nameOf(path), text] as const)
  .sort(([a], [b]) => a.localeCompare(b));

async function db(body: string): Promise<SequenceDbView> {
  const view = await sequenceDbOf(body, false);
  if ("error" in view) {
    throw new Error(`mermaid refused: ${String(view.error)}`);
  }
  return view;
}

/** Mermaid's own lexer and its token names. */
interface RealLexer {
  rules: RegExp[];
  conditions: Record<string, { rules: number[]; inclusive: boolean }>;
  setInput(text: string, yy: object): void;
  lex(): number | string;
}
async function realLexer(): Promise<{
  lexer: RealLexer;
  terminals: Record<number, string>;
  text: (body: string) => Promise<string>;
}> {
  const { default: mermaid } = await import("mermaid");
  mermaid.initialize({ startOnLoad: false });
  const fromText = (body: string) =>
    mermaid.mermaidAPI.getDiagramFromText(body) as unknown as Promise<{
      text: string;
      parser: {
        parser: { lexer: RealLexer; terminals_: Record<number, string> };
      };
    }>;
  const diagram = await fromText("sequenceDiagram\n  A->>B: x");
  return {
    lexer: diagram.parser.parser.lexer,
    terminals: diagram.parser.parser.terminals_,
    text: async (body) => (await fromText(body)).text,
  };
}

describe("the lexer table is mermaid's", () => {
  test("rule for rule", async () => {
    const { lexer } = await realLexer();
    expect(LEXER_RULES.length).toBe(lexer.rules.length);
    for (const [i, rule] of lexer.rules.entries()) {
      expect(`^(?:${defined(LEXER_RULES[i])})`, `rule ${String(i)}`).toBe(
        rule.source,
      );
      expect(rule.ignoreCase).toBe(true);
    }
  });

  test("condition for condition", async () => {
    const { lexer } = await realLexer();
    const theirs = Object.fromEntries(
      Object.entries(lexer.conditions).map(([k, v]) => [k, v.rules]),
    );
    expect(LEXER_CONDITIONS).toEqual(theirs);
  });

  test.each(corpus)(
    "%s: the same tokens as mermaid's own lexer",
    async (_n, body) => {
      const { lexer, terminals, text } = await realLexer();
      lexer.setInput(await text(body), {});
      const theirs: string[] = [];
      for (let i = 0; i < 100_000; i++) {
        const token = lexer.lex();
        if (token === 1 || token === "EOF") {
          break;
        }
        theirs.push(
          typeof token === "number" ? defined(terminals[token]) : token,
        );
      }
      expect(lexSequence(body).map((t) => t.kind)).toEqual(theirs);
    },
  );
});

describe("agreement with mermaid's db", () => {
  test("the corpus has the named cases and fifteen realistic diagrams", () => {
    const names = corpus.map(([name]) => name);
    for (let i = 1; i <= 15; i++) {
      const prefix = `s${String(i).padStart(2, "0")}-`;
      expect(
        names.some((n) => n.startsWith(prefix)),
        prefix,
      ).toBe(true);
    }
    expect(
      names.filter((n) => n.startsWith("c")).length,
    ).toBeGreaterThanOrEqual(15);
  });

  test.each(corpus)("%s", async (_name, body) => {
    const model = parseSequence(body);
    expect(model.reasons).toEqual([]);
    expect(agreesSequence(model, await db(body))).toBe(null);
    // The same as a fence body, without the file's last line break.
    const fence = body.replace(/\n$/, "");
    const fenceModel = parseSequence(fence);
    expect(fenceModel.reasons).toEqual([]);
    expect(agreesSequence(fenceModel, await db(fence))).toBe(null);
  });

  test("a model that is not understood gives its reason", async () => {
    const model = parseSequence("sequenceDiagram\n  A-|\\B: x");
    expect(model.understood).toBe(false);
    expect(
      agreesSequence(model, await db("sequenceDiagram\n  A->>B: x")),
    ).toMatch(/half arrow/);
  });

  test("the empty diagram agrees", async () => {
    const model = parseSequence("sequenceDiagram");
    expect(agreesSequence(model, await db("sequenceDiagram"))).toBe(null);
  });
});

/** One row per comparison: our body against mermaid's db of another. */
const DIFFERENCES: [string, string, string, RegExp][] = [
  [
    "participant names",
    "sequenceDiagram\n  participant A\n  participant B",
    "sequenceDiagram\n  participant A\n  participant C",
    /participants differ/,
  ],
  [
    "participant order",
    "sequenceDiagram\n  participant A\n  participant B",
    "sequenceDiagram\n  participant B\n  participant A",
    /participants differ/,
  ],
  [
    "participant label",
    "sequenceDiagram\n  participant A as X",
    "sequenceDiagram\n  participant A as Y",
    /label of A/,
  ],
  [
    "participant type",
    "sequenceDiagram\n  participant A",
    "sequenceDiagram\n  actor A",
    /type of A/,
  ],
  [
    "record count",
    "sequenceDiagram\n  A->>A: x",
    "sequenceDiagram\n  A->>A: x\n  A->>A: y",
    /records here/,
  ],
  [
    "record from",
    "sequenceDiagram\n  participant A\n  participant B\n  activate A\n  deactivate A",
    "sequenceDiagram\n  participant A\n  participant B\n  activate B\n  deactivate B",
    /Record 0/,
  ],
  [
    "record to",
    "sequenceDiagram\n  participant A\n  participant B\n  Note over A,B: x",
    "sequenceDiagram\n  participant A\n  participant B\n  Note over A,A: x",
    /Record 0/,
  ],
  [
    "record type",
    "sequenceDiagram\n  participant A\n  loop x\n  end",
    "sequenceDiagram\n  participant A\n  opt x\n  end",
    /Record 0/,
  ],
  [
    "record text",
    "sequenceDiagram\n  participant A\n  loop x\n  end",
    "sequenceDiagram\n  participant A\n  loop y\n  end",
    /Record 0/,
  ],
  [
    "autonumber start",
    "sequenceDiagram\n  participant A\n  autonumber 1",
    "sequenceDiagram\n  participant A\n  autonumber 2",
    /Record 0/,
  ],
  [
    "autonumber step",
    "sequenceDiagram\n  participant A\n  autonumber 1 2",
    "sequenceDiagram\n  participant A\n  autonumber 1 3",
    /Record 0/,
  ],
  [
    "autonumber visible",
    "sequenceDiagram\n  participant A\n  autonumber",
    "sequenceDiagram\n  participant A\n  autonumber off",
    /Record 0/,
  ],
  [
    "note placement",
    "sequenceDiagram\n  participant A\n  Note left of A: x",
    "sequenceDiagram\n  participant A\n  Note right of A: x",
    /Record 0/,
  ],
  [
    "created",
    "sequenceDiagram\n  participant A\n  create participant B\n  A->>B: x",
    "sequenceDiagram\n  participant A\n  participant B\n  A->>B: x",
    /created/,
  ],
  [
    "destroyed",
    "sequenceDiagram\n  participant A\n  participant B\n  destroy B\n  A->>B: x",
    "sequenceDiagram\n  participant A\n  participant B\n  A->>B: x",
    /destroyed/,
  ],
  [
    "boxes",
    "sequenceDiagram\n  box x\n  participant A\n  end",
    "sequenceDiagram\n  participant A",
    /boxes differ/,
  ],
  [
    "box count",
    "sequenceDiagram\n  participant A",
    "sequenceDiagram\n  box x\n  participant A\n  end",
    /boxes differ/,
  ],
  [
    "box members",
    "sequenceDiagram\n  box x\n  participant A\n  end\n  participant B",
    "sequenceDiagram\n  participant A\n  box x\n  participant B\n  end",
    /participants differ|boxes differ/,
  ],
];

/** One row per comparison that only a wrong model can trip: the db stays right. */
const TAMPERED: [string, string, (model: SeqModel) => void, RegExp][] = [
  [
    "a message's record from",
    "sequenceDiagram\n  A->>B: x",
    (m) => {
      defined(m.messages[0]).from = "B";
    },
    /Message 0/,
  ],
  [
    "a message's record to",
    "sequenceDiagram\n  A->>B: x",
    (m) => {
      defined(m.messages[0]).to = "A";
    },
    /Message 0/,
  ],
  [
    "a message's text",
    "sequenceDiagram\n  A->>B: hello",
    (m) => {
      defined(m.messages[0]).text = { from: 16, to: 17 };
    },
    /Message 0/,
  ],
  [
    "a message's record index",
    "sequenceDiagram\n  A->>B: x\n  A->>B: y",
    (m) => {
      defined(m.messages[1]).record = 0;
    },
    /Message 1/,
  ],
  [
    "a record past the end",
    "sequenceDiagram\n  A->>B: x",
    (m) => {
      defined(m.messages[0]).record = 5;
    },
    /Message 0/,
  ],
  [
    "the arrow's line",
    "sequenceDiagram\n  A->>B: x",
    (m) => {
      defined(m.messages[0]).arrow.line = "dotted";
    },
    /drawn differently/,
  ],
  [
    "the arrow's head",
    "sequenceDiagram\n  A->>B: x",
    (m) => {
      defined(m.messages[0]).arrow.head = "cross";
    },
    /drawn differently/,
  ],
  [
    "the arrow's both",
    "sequenceDiagram\n  A->>B: x",
    (m) => {
      defined(m.messages[0]).arrow.both = true;
    },
    /drawn differently/,
  ],
  [
    "the statements' record list",
    "sequenceDiagram\n  A->>B: x",
    (m) => {
      defined(m.statements[0]).records = [];
    },
    /record list/,
  ],
  [
    "the statements' record order",
    "sequenceDiagram\n  A->>B: x\n  A->>B: y",
    (m) => {
      m.statements.reverse();
    },
    /record list/,
  ],
  [
    "one record too many",
    "sequenceDiagram\n  A->>B: x",
    (m) => {
      m.records.push(defined(m.records[0]));
      defined(m.statements[0]).records.push(1);
    },
    /records here/,
  ],
];

describe("each comparison fails on its own", () => {
  test.each(DIFFERENCES)("%s", async (_field, ours, theirs, words) => {
    const model = parseSequence(ours);
    expect(model.reasons).toEqual([]);
    expect(agreesSequence(model, await db(theirs))).toMatch(words);
  });

  test.each(TAMPERED)("%s", async (_field, body, tamper, words) => {
    const model = parseSequence(body);
    expect(agreesSequence(model, await db(body))).toBe(null);
    tamper(model);
    expect(agreesSequence(model, await db(body))).toMatch(words);
  });

  test("a record's id in mermaid's list", async () => {
    const body = "sequenceDiagram\n  A->>B: x";
    const view = await db(body);
    defined(view.records[0]).id = "7";
    expect(agreesSequence(parseSequence(body), view)).toMatch(/Record 0/);
  });
});

/**
 * What a row of the table below must do:
 * - `agrees`: understood, and mermaid's db reads it the same way
 * - `refused`: not understood, so it falls back to text before mermaid is asked
 * - `mermaidRefuses`: understood, but mermaid refuses it, so the verdict says no
 * - `gate`: understood, but mermaid reads it differently; `agreesSequence` names
 *   the difference, so the verdict says no and it falls back to text
 */
type EdgeState = "agrees" | "refused" | "mermaidRefuses" | "gate";

const S = "sequenceDiagram\n";
const EDGE_CASES: [string, EdgeState][] = [
  // Comments, as this lexer reads them (R8 does not carry over).
  [`${S}%%\n  A->>B: x`, "agrees"],
  [`${S}%\n  A->>B: x`, "agrees"],
  [`${S}% one\n  A->>B: x`, "agrees"],
  [`${S}# one\n  A->>B: x`, "agrees"],
  [`${S}  A->>B: x %% y`, "agrees"],
  [`${S}  A->>B: x # y`, "refused"],
  [`${S}  A->>B: Issue #12 open`, "refused"],
  [`${S}  participant A as L # y`, "refused"],
  [`${S}  loop l # y\n  end`, "refused"],
  [`${S}  loop l\n  end %% y`, "agrees"],
  [`${S}  loop l\n  end # y`, "agrees"],
  [`sequenceDiagram %% y\n  A->>B: x`, "agrees"],
  [`${S}  autonumber %% y\n  A->>B: x`, "agrees"],
  [`${S}  participant A %% y`, "refused"],
  [`${S}  activate A %% y\n  A->>A: x`, "refused"],
  [`${S}  A%% y\n  A->>B: x`, "refused"],
  [`${S}  A->>B: x %%{init: {}}%%`, "refused"],
  [`${S}  %%{ }%%\n  A->>B: x`, "refused"],
  // A `;` ends a statement whatever surrounds it.
  [`${S}  A->>B: "a;b"\n`, "refused"],
  [`${S}  A->>B: a; B->>A: b`, "agrees"],
  [`${S}  A->>B: [a;b]->>C: d`, "agrees"],
  [`${S}  A->>B: a#59;b`, "agrees"],
  [`${S}  participant A as x; participant B as y`, "agrees"],
  [`${S};;;\n  A->>B: x;;;`, "agrees"],
  // encodeEntities removes a `;`.
  [`${S}  A->>B: style a:#f00; B->>A: y`, "refused"],
  [`${S}  A->>B: style x:y#z;`, "refused"],
  [`${S}  Note over A: classDef a:#f00; A->>A: x`, "refused"],
  // A name is what the lexer says, and a keyword goes first.
  [`${S}  end-point->>B: x`, "refused"],
  [`${S}  End User->>B: x`, "refused"],
  [`${S}  loop l\n  End User->>B: x`, "agrees"],
  [`${S}  over->>B: x`, "refused"],
  [`${S}  off->>B: x`, "refused"],
  [`${S}  Note->>B: x`, "refused"],
  [`${S}  box->>B: x`, "refused"],
  [`${S}  title->>B: x`, "agrees"],
  [`${S}  link->>B: x`, "agrees"],
  [`${S}  links->>B: x`, "agrees"],
  [`${S}  endpoint->>loopback: x`, "agrees"],
  [`${S}  as->>B: x`, "agrees"],
  [`${S}  A B->>C D: x`, "agrees"],
  [`${S}  A-b->>C-d: x`, "agrees"],
  [`${S}  A--b->>C: x`, "refused"],
  [`${S}  A-xB->>C: x`, "refused"],
  [`${S}  1->>2: x`, "agrees"],
  [`${S}  1 ->>2: x`, "refused"],
  [`${S}  a.b->>c/d: x`, "agrees"],
  [`${S}  /a->>c: x`, "refused"],
  [`${S}  a"b->>c&d: x`, "agrees"],
  [`${S}  a#b->>c: x`, "refused"],
  [`${S}  a%b->>c: x`, "refused"],
  [`${S}  a#35;b->>c: x`, "refused"],
  [`${S}  alice@x->>c: x`, "agrees"],
  [`${S}  Köln->>🚀: x`, "agrees"],
  [`${S}  __proto__->>constructor: x`, "agrees"],
  // The arrows outside the model.
  [`${S}  A->>()B: x`, "refused"],
  [`${S}  A()->>B: x`, "refused"],
  [`${S}  A()->>()B: x`, "refused"],
  [`${S}  A-|\\B: x`, "refused"],
  [`${S}  A--//B: x`, "refused"],
  [`${S}  A/|-B: x`, "refused"],
  [`${S}  A->>B`, "refused"],
  [`${S}  A->>: x`, "refused"],
  [`${S}  ->>B: x`, "refused"],
  [`${S}  A->>+ B: x`, "agrees"],
  [`${S}  A->>-B: x`, "mermaidRefuses"],
  [`${S}  A-XB: x`, "agrees"],
  [`${S}  A<->B: x`, "refused"],
  // Lines that run together.
  [`${S}  A->>\n  B: x`, "refused"],
  [`${S}  A->> \n  B: x`, "refused"],
  [`${S}  participant A\n  as L`, "refused"],
  [`${S}  title\n  A->>B: x`, "refused"],
  [`${S}  accTitle:\n  A->>B: x`, "refused"],
  [`${S}  autonumber \n  A->>B: x`, "refused"],
  [`${S}  autonumber\n  A->>B: x`, "agrees"],
  [`${S}  loop l\n  end A->>B: x`, "agrees"],
  [`sequenceDiagram A->>B: x`, "agrees"],
  [`sequenceDiagram;A->>B: x`, "agrees"],
  // Line breaks.
  [`${S}  A->>B: x\ry`, "refused"],
  [`${S}  A->>B: x\u2028y`, "refused"],
  [`sequenceDiagram\r\n  autonumber\r\n  A->>B: x\r\n`, "agrees"],
  // A body that opens oddly.
  [`\n\n  ${S}  A->>B: x`, "agrees"],
  [`---\ntitle: x\n---\n${S}  A->>B: x`, "agrees"],
  [`%%{init: {"theme": "dark"}}%%\n${S}  A->>B: x`, "agrees"],
  [`%% first\n${S}  A->>B: x`, "agrees"],
  [`SEQUENCEDIAGRAM\n  A->>B: x`, "refused"],
  [`sequenceDiagramX\n  A->>B: x`, "refused"],
  [`${S}  A->>B: x\n${S}`, "refused"],
  // Blocks.
  [`${S}  loop\n  end`, "agrees"],
  [`${S}  loop\n`, "refused"],
  [`${S}  end`, "refused"],
  [`${S}  else x`, "refused"],
  [`${S}  loop\n  else x\n  end`, "refused"],
  [`${S}  alt\n  and x\n  end`, "refused"],
  [`${S}  par\n  else x\n  end`, "refused"],
  [`${S}  critical\n  and x\n  end`, "refused"],
  [`${S}  alt a\n  loop b\n  else c\n  end\n  end`, "refused"],
  [`${S}  par_over x\n  A->>B: y\n  and z\n  end`, "agrees"],
  [`${S}  rect rgba(0,0,0,0.1)\n  end`, "agrees"],
  [`${S}  loop a; A->>B: x; end`, "agrees"],
  // Boxes.
  [`${S}  box\n  end`, "agrees"],
  [`${S}  box x\n  A->>B: y\n  end`, "refused"],
  [`${S}  box x\n  box y\n  end\n  end`, "refused"],
  [
    `${S}  box x\n  participant A\n  end\n  box y\n  participant A\n  end`,
    "mermaidRefuses",
  ],
  [`${S}  box x\n  participant A\n  destroy A\n  end\n  B->>A: y`, "refused"],
  [`${S}  loop\n  box x\n  participant A\n  end\n  A->>A: y\n  end`, "agrees"],
  // What the db accepts and the render cannot draw.
  [`${S}  activate Z\n  A->>A: x`, "refused"],
  [`${S}  A->>A: x\n  destroy Z`, "refused"],
  [`${S}  destroy Z\n  A->>Z: x`, "agrees"],
  [`${S}  A->>B: x\n  create participant Q`, "refused"],
  [`${S}  create participant Q\n  Note over Q: n\n  A->>Q: x`, "refused"],
  [`${S}  create participant Q\n  A->>B: x`, "refused"],
  [`${S}  create participant Q\n  Q->>Q: x`, "agrees"],
  [`${S}  participant Q\n  create participant Q\n  A->>Q: x`, "mermaidRefuses"],
  [`${S}  create destroy Q\n  A->>Q: x`, "refused"],
  [`${S}  destroy A\n  B->>C: x`, "refused"],
  [`${S}  actor A\n  link A: L @ https://example.com`, "refused"],
  [`${S}  actor A\n  links A: {"L": "https://example.com"}`, "refused"],
  [`${S}  actor A\n  details A: nowhere`, "refused"],
  [`${S}  actor A\n  properties A: {"a": "b"}`, "agrees"],
  [
    `${S}  participant A@{ "type": "control" }\n  link A: L @ https://example.com`,
    "refused",
  ],
  [
    `${S}  participant A@{ "type": "boundary" }\n  link A: L @ https://example.com`,
    "refused",
  ],
  [
    `${S}  participant A@{ "type": "entity" }\n  link A: L @ https://example.com`,
    "refused",
  ],
  [
    `${S}  participant A@{ "type": "queue" }\n  link A: L @ https://example.com`,
    "agrees",
  ],
  [
    `${S}  participant A@{ "type": "database" }\n  link A: L @ https://example.com`,
    "agrees",
  ],
  [
    `${S}  participant A@{ "type": "collections" }\n  link A: L @ https://example.com`,
    "agrees",
  ],
  [`${S}  A->>B: $$x^2$$`, "refused"],
  [`${S}  Note over A: $$x$$`, "refused"],
  [`${S}  participant A as $$x$$`, "refused"],
  [`${S}  A->>B: one $ two`, "agrees"],
  // Declarations.
  [`${S}  participant A as`, "refused"],
  [`${S}  participant A as  `, "refused"],
  [`${S}  participant A B as C`, "agrees"],
  [`${S}  participant A<b>`, "refused"],
  [`${S}  participant`, "refused"],
  [`${S}  actor`, "refused"],
  [`${S}  participant a:b`, "refused"],
  [`${S}  participant a,b`, "refused"],
  [`${S}  participant a@b`, "refused"],
  // @{ }.
  [`${S}  participant A@{ "type": "database" }`, "agrees"],
  [`${S}  participant A@{ type: queue, alias: "Bee" }`, "agrees"],
  [`${S}  participant A @{ "type": "entity" }`, "agrees"],
  [`${S}  actor A@{ "type": "control" } as Cee`, "agrees"],
  [`${S}  participant A@{ "type": "foo" }`, "refused"],
  [`${S}  participant A@{ "alias": "" }`, "refused"],
  [`${S}  participant A@{ "alias": "  x " }`, "refused"],
  [`${S}  participant A@{ "type": "database  " }`, "refused"],
  [`${S}  participant A@{ type: database   }`, "agrees"],
  [`${S}  accDescr {\n  A->>B: %%{init}%%\n  B->>A: x`, "refused"],
  [`${S}  participant A@{ "icon": "x" }`, "refused"],
  [`${S}  participant A@{ "type": "queue" } as X@{ "alias": "Y" }`, "agrees"],
  [`${S}  participant A@{ "alias": "Y" } as X`, "refused"],
  [`${S}  participant A@{ "type": "queue"\n}`, "refused"],
  [`${S}  participant A@{ }`, "refused"],
  [`${S}  participant A@{ "type": "queue" } `, "agrees"],
  [`${S}  participant A@{ "type": "queue" } \n  A->>A: x`, "refused"],
  [`${S}  destroy A@{ "type": "queue" }\n  B->>A: x`, "refused"],
  // Texts mermaid changes before it reads them.
  [`${S}  A->>B: <a href="x">t</a>`, "gate"],
  [`${S}  A->>B: wrap: x`, "agrees"],
  [`${S}  A->>B: Wrap: x`, "agrees"],
  [`${S}  A->>B: wrap:`, "agrees"],
  [`${S}  A->>B: x<br>y<br/>z`, "agrees"],
  [`${S}  A->>B: #quot;#35;#59;`, "agrees"],
  // A config option that leaves participants without an element.
  [
    `---\nconfig:\n  sequence:\n    hideUnusedParticipants: true\n---\n${S}  participant Z\n  A->>B: x`,
    "refused",
  ],
  [
    `---\nconfig:\n  sequence:\n    hideUnusedParticipants: false\n---\n${S}  A->>B: x`,
    "refused",
  ],
  [
    `%%{init: {"sequence":{"hideUnusedParticipants":true}}}%%\n${S}  participant Z\n  A->>B: x`,
    "refused",
  ],
  [
    `%%{\n  init: {"sequence":\n  {"hideUnusedParticipants":true}}\n}%%\n${S}  participant Z\n  A->>B: x`,
    "refused",
  ],
  [
    `${S}  %%{init: {"sequence":{"hideUnusedParticipants":true}}}%%\n  A->>B: x`,
    "refused",
  ],
  [`${S}  %% hideUnusedParticipants is not set here\n  A->>B: x`, "agrees"],
  [`${S}  A->>B: hideUnusedParticipants`, "agrees"],
  [`---\nconfig:\n  sequence:\n    wrap: true\n---\n${S}  A->>B: x`, "agrees"],
  // A # right after a keyword takes the whole text.
  [`${S}  loop #1 retry\n  end`, "refused"],
  [`${S}  alt #1\n  end`, "refused"],
  [`${S}  alt a\n  else #2 no\n  end`, "refused"],
  [`${S}  opt #a\n  end`, "refused"],
  [`${S}  par #a\n  end`, "refused"],
  [`${S}  par_over #a\n  end`, "refused"],
  [`${S}  par a\n  and #b\n  end`, "refused"],
  [`${S}  critical #a\n  end`, "refused"],
  [`${S}  critical a\n  option #b\n  end`, "refused"],
  [`${S}  break #a\n  end`, "refused"],
  [`${S}  rect #fff\n  end`, "refused"],
  [`${S}  box #fff Group\n  participant A\n  end`, "refused"],
  [`${S}  rect rgb(1,2,3)\n  A->>B: x\n  end`, "agrees"],
  [`${S}  box rgb(1,2,3) Group\n  participant A\n  end`, "agrees"],
  [`${S}  loop\n# whole line\n  A->>B: x\n  end`, "agrees"],
  [`${S}  loop #35;1 retry\n  A->>B: x\n  end`, "agrees"],
  // A formula in a name, as in an alias.
  [`${S}  participant $$x$$`, "refused"],
  [`${S}  $$x$$->>B: y`, "refused"],
  [`${S}  A->>$$x$$: y`, "refused"],
  [`${S}  Note over $$x$$: y`, "refused"],
  [`${S}  a$$b->>C: y`, "agrees"],
  // Statements that fit nothing.
  [`${S}  A`, "refused"],
  [`${S}  (`, "refused"],
  [`${S}  /`, "refused"],
  [`${S}  autonumber 1 2 3`, "refused"],
  [`${S}  autonumber x`, "refused"],
  [`${S}  autonumber 1.5 0.25\n  A->>B: x`, "agrees"],
  [`${S}  Note A: x`, "refused"],
  [`${S}  Note over A`, "refused"],
  [`${S}  Note over A,B,C: x`, "refused"],
  [`${S}  Note left of A,B: x`, "refused"],
  [`${S}  links A`, "refused"],
  [`${S}  link A: x\n  properties A: not json\n  details A: nowhere`, "agrees"],
  [`${S}  accDescr {\n x\n }\n  A->>B: y`, "agrees"],
  [`${S}  accDescr {\n %% c\n x\n }`, "agrees"],
  [`${S}  accDescr { x } A->>B: y`, "refused"],
  [`${S}  accDescr {\n x`, "refused"],
  [`${S}  title a; title b`, "agrees"],
  [`${S}  title: a`, "agrees"],
];

describe("inputs at the edges of the grammar", () => {
  test.each(EDGE_CASES)("%j is %s", async (body, state) => {
    const model = parseSequence(body);
    const view = await sequenceDbOf(body, false);
    const seen: EdgeState = !model.understood
      ? "refused"
      : "error" in view
        ? "mermaidRefuses"
        : agreesSequence(model, view) === null
          ? "agrees"
          : "gate";
    expect(seen, model.reasons[0] ?? "").toBe(state);
  });

  test("the keywords ending a name in the body of a block", async () => {
    // Understood and read like mermaid: `End` closes the loop.
    const body = `${S}  loop l\n  End\n  User->>B: x`;
    const model = parseSequence(body);
    expect(agreesSequence(model, await db(body))).toBe(null);
    expect(model.order).toEqual(["User", "B"]);
  });
});

describe("the opt-in corpus", () => {
  const folder = node.process.env.FLUID_DIAGRAM_CORPUS;
  test.skipIf(folder === undefined)(
    "what is understood agrees; refused files are counted and listed (R57)",
    async () => {
      const dir = folder ?? "";
      const fs = node.process.getBuiltinModule("node:fs") as Fs;
      const files = fs.readdirSync(dir).filter((f) => f.endsWith(".mmd"));
      let sequences = 0;
      let understood = 0;
      let agreeing = 0;
      const refused: string[] = [];
      const failures: string[] = [];
      for (const file of files) {
        const body = fs.readFileSync(`${dir}/${file}`, "utf8");
        if (scanFence(body).keyword !== "sequenceDiagram") {
          continue;
        }
        sequences++;
        const model = parseSequence(body);
        if (!model.understood) {
          refused.push(`${file}: ${model.reasons[0] ?? ""}`);
          continue;
        }
        understood++;
        const view = await sequenceDbOf(body, false);
        if ("error" in view) {
          refused.push(`${file}: mermaid refuses it: ${String(view.error)}`);
          continue;
        }
        const difference = agreesSequence(model, view);
        if (difference === null) {
          agreeing++;
        } else {
          failures.push(`${file}: ${difference}`);
        }
      }
      console.log(
        [
          `sequence corpus: ${String(understood)} of ${String(sequences)} understood, ${String(agreeing)} agree with mermaid, ${String(refused.length)} refused`,
          ...refused,
        ].join("\n"),
      );
      expect(failures).toEqual([]);
    },
  );
});
