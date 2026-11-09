import { describe, expect, test } from "vitest";

import { defined } from "../../../test/assert";
import type { Span } from "../edits";
import { droppedSemicolons } from "../scan";
import {
  MAX_SEQ_LINE,
  decodeSeqText,
  losesSemicolon,
  parseSequence,
} from "./parse";
import type { SeqModel, SeqStatement } from "./model";

const SEQUENCE = import.meta.glob<string>(
  "../../fixtures/diagrams/sequence/*.mmd",
  { query: "?raw", import: "default", eager: true },
);
const FALLBACK = import.meta.glob<string>(
  "../../fixtures/diagrams/fallback/y*.mmd",
  { query: "?raw", import: "default", eager: true },
);

function nameOf(path: string): string {
  const file = path.slice(path.lastIndexOf("/") + 1);
  return file.slice(0, file.lastIndexOf("."));
}

const corpus = Object.entries(SEQUENCE)
  .map(([path, text]) => [nameOf(path), text] as const)
  .sort(([a], [b]) => a.localeCompare(b));

function fixture(prefix: string): string {
  const hit = corpus.find(([name]) => name.startsWith(`${prefix}-`));
  return defined(hit, `fixture ${prefix}`)[1];
}

const reads = (body: string, span: Span | null | undefined): string | null =>
  span ? body.slice(span.from, span.to) : null;

function understood(body: string): SeqModel {
  const model = parseSequence(body);
  expect(model.reasons).toEqual([]);
  expect(model.understood).toBe(true);
  return model;
}

function only<T>(list: readonly T[]): T {
  expect(list.length).toBe(1);
  return defined(list[0]);
}

const BODY = "sequenceDiagram\n";

describe("spans", () => {
  test("participant caller as Caller", () => {
    const body = `${BODY}    participant caller as Caller`;
    const model = understood(body);
    const participant = defined(model.participants.get("caller"));
    const declaration = only(participant.declarations);
    expect(reads(body, declaration.nameSpan)).toBe("caller");
    expect(reads(body, declaration.alias)).toBe("Caller");
    expect(reads(body, declaration.span)).toBe("participant caller as Caller");
    expect(declaration.keyword).toBe("participant");
    expect(declaration.create).toBe(false);
    expect(declaration.inBox).toBe(false);
    expect(declaration.line).toBe(1);
    expect(participant.label).toBe("Caller");
    expect(participant.firstLine).toBe(1);
    expect(model.order).toEqual(["caller"]);
  });

  test("caller->>service: Request", () => {
    const body = `${BODY}    caller->>service: Request`;
    const model = understood(body);
    const message = only(model.messages);
    expect(reads(body, message.text)).toBe("Request");
    expect(reads(body, message.arrow.span)).toBe("->>");
    expect(reads(body, message.fromSpan)).toBe("caller");
    expect(reads(body, message.toSpan)).toBe("service");
    expect(reads(body, message.span)).toBe("caller->>service: Request");
    expect(message.from).toBe("caller");
    expect(message.to).toBe("service");
    expect(message.line).toBe(1);
    expect(message.record).toBe(0);
    expect(message.occurrence).toBe(0);
    expect(message.arrow).toMatchObject({
      line: "solid",
      head: "arrow",
      both: false,
      activation: null,
    });
  });

  test("A->>+B: x has the activation span +", () => {
    const body = `${BODY}  A->>+B: x\n  B-->>-A: y`;
    const model = understood(body);
    const [first, second] = model.messages;
    expect(reads(body, first?.arrow.activation)).toBe("+");
    expect(reads(body, first?.arrow.span)).toBe("->>");
    expect(reads(body, first?.toSpan)).toBe("B");
    expect(reads(body, second?.arrow.activation)).toBe("-");
    expect(reads(body, second?.arrow.span)).toBe("-->>");
    // A + and a - each add a record after the message's own.
    expect(first?.record).toBe(0);
    expect(second?.record).toBe(2);
    expect(model.statements.map((s) => s.records)).toEqual([
      [0, 1],
      [2, 3],
    ]);
  });

  test.each([
    ["->", "solid", "open", false],
    ["-->", "dotted", "open", false],
    ["->>", "solid", "arrow", false],
    ["-->>", "dotted", "arrow", false],
    ["<<->>", "solid", "arrow", true],
    ["<<-->>", "dotted", "arrow", true],
    ["-x", "solid", "cross", false],
    ["--x", "dotted", "cross", false],
    ["-)", "solid", "async", false],
    ["--)", "dotted", "async", false],
  ])("the arrow %s", (token, line, head, both) => {
    const body = `${BODY}  A${token}B: x`;
    const message = only(understood(body).messages);
    expect(reads(body, message.arrow.span)).toBe(token);
    expect(message.arrow).toMatchObject({ line, head, both });
  });

  test("spaces around a name and a text stay out of the spans", () => {
    const body = `${BODY}\tA One  ->>  B Two  :   many words   `;
    const message = only(understood(body).messages);
    expect(message.from).toBe("A One");
    expect(message.to).toBe("B Two");
    expect(reads(body, message.fromSpan)).toBe("A One");
    expect(reads(body, message.toSpan)).toBe("B Two");
    expect(reads(body, message.text)).toBe("many words");
  });

  test("a message without text has no text span", () => {
    const message = only(understood(`${BODY}  A->>B:`).messages);
    expect(message.text).toBe(null);
  });

  test("a wrap prefix stays out of the text span", () => {
    const body = `${BODY}  A->>B:wrap:hello\n  A->>B: nowrap: bye\n  participant C as wrap: Sea`;
    const model = understood(body);
    expect(model.messages.map((m) => reads(body, m.text))).toEqual([
      "hello",
      "bye",
    ]);
    const c = defined(model.participants.get("C"));
    expect(reads(body, c.declarations[0]?.alias)).toBe("Sea");
    expect(c.label).toBe("Sea");
  });

  test("a CRLF body keeps its spans on the text", () => {
    const body =
      "sequenceDiagram\r\n  %% note\r\n  participant a as Al\r\n  autonumber\r\n  a->>b: hi\r\n";
    const model = understood(body);
    expect(
      reads(body, model.participants.get("a")?.declarations[0]?.alias),
    ).toBe("Al");
    const message = only(model.messages);
    expect(reads(body, message.text)).toBe("hi");
    expect(message.line).toBe(4);
  });

  test("two statements on one line have their own spans", () => {
    const body = `${BODY}  a->>b:x;b-->>a:y`;
    const model = understood(body);
    expect(model.messages.map((m) => reads(body, m.span))).toEqual([
      "a->>b:x",
      "b-->>a:y",
    ]);
    expect(model.statements.map((s) => s.line)).toEqual([1, 1]);
  });

  test("a @{ } declaration has its config span and its type", () => {
    const body = `${BODY}  participant s@{ "type": "database" } as Store\n  participant j@{ "type": "queue", "alias": "Jobs" }`;
    const model = understood(body);
    const s = defined(model.participants.get("s"));
    expect(reads(body, s.declarations[0]?.nameSpan)).toBe("s");
    expect(reads(body, s.declarations[0]?.alias)).toBe("Store");
    expect(reads(body, s.declarations[0]?.config)).toBe(
      '@{ "type": "database" }',
    );
    expect(s.type).toBe("database");
    const j = defined(model.participants.get("j"));
    expect(reads(body, j.declarations[0]?.alias)).toBe("Jobs");
    expect(j.label).toBe("Jobs");
    expect(j.type).toBe("queue");
  });

  test("a note over two names has both actor spans", () => {
    const body = `${BODY}  Note over A , B: both`;
    const statement = only(understood(body).statements);
    expect(statement.kind).toBe("note");
    expect(statement.actors.map((a) => [a.name, reads(body, a.span)])).toEqual([
      ["A", "A"],
      ["B", "B"],
    ]);
    expect(reads(body, statement.text)).toBe("both");
  });
});

describe("participants", () => {
  test("the order is first mention, implicit or declared", () => {
    expect(understood(fixture("s02")).order).toEqual([
      "browser",
      "proxy",
      "daemon",
    ]);
    const model = understood(
      `${BODY}  B->>A: x\n  participant A as Al\n  participant C`,
    );
    expect(model.order).toEqual(["B", "A", "C"]);
    expect(model.participants.get("A")?.firstLine).toBe(1);
    expect(model.participants.get("A")?.declarations.length).toBe(1);
    expect(model.participants.get("B")?.declarations).toEqual([]);
  });

  test("a later declaration without an alias changes nothing, one with an alias replaces", () => {
    const model = understood(
      `${BODY}  participant A\n  participant A as X\n  participant A\n  actor A\n  B->>A: y\n  actor B as Bee\n  actor C\n  participant C as Sea`,
    );
    expect(model.participants.get("A")).toMatchObject({
      label: "X",
      type: "participant",
    });
    expect(model.participants.get("B")).toMatchObject({
      label: "Bee",
      type: "actor",
    });
    expect(model.participants.get("C")).toMatchObject({
      label: "Sea",
      type: "participant",
    });
    expect(model.participants.get("A")?.declarations.length).toBe(4);
  });

  test("create and box are on the declaration", () => {
    const model = understood(fixture("s10"));
    expect(model.participants.get("worker")?.declarations[0]?.create).toBe(
      true,
    );
    expect(model.participants.get("reviewer")?.declarations[0]).toMatchObject({
      create: true,
      keyword: "actor",
    });
    const boxed = understood(fixture("s09"));
    expect(boxed.participants.get("bridge")?.declarations[0]?.inBox).toBe(true);
    expect(boxed.boxes.map((b) => b.actors)).toEqual([
      ["bridge", "person"],
      ["daemon"],
      ["github"],
    ]);
  });
});

describe("occurrence", () => {
  test("counts messages with the same from, to and decoded text", () => {
    const model = understood(
      `${BODY}  A->>B: x\n  A->>B: y\n  A->>B: x\n  B->>A: x\n  A-->>B: x\n  A->>B: #quot;\n  A->>B: #34;`,
    );
    expect(model.messages.map((m) => m.occurrence)).toEqual([
      0, 0, 1, 0, 2, 0, 1,
    ]);
  });
});

describe("occurrence of a text that shows a tag", () => {
  test("is its own series beside a real line break", () => {
    const model = understood(
      `${BODY}  A->>B: a#lt;br#gt;b\n  A->>B: a<br>b\n  A->>B: a<br/>b`,
    );
    expect(model.messages.map((m) => m.occurrence)).toEqual([0, 0, 1]);
  });
});

describe("decodeSeqText", () => {
  test.each([
    ["plain", "plain"],
    ["say #quot;hi#quot;", 'say "hi"'],
    ["#35;1", "#1"],
    ["a#59; b", "a; b"],
    ["one<br>two<br/>three<BR />four", "one\ntwo\nthree\nfour"],
    ["#9829;", "\u2665"],
    ["#unknownname;", "#unknownname;"],
    ["  spaced  ", "spaced"],
    ["a#lt;br#gt;b", "a<br>b"],
    ["#lt;br#gt;", "<br>"],
    ["x</br>y", "x\ny"],
    ["x</BR >y", "x\ny"],
    // Forms that decode to no character keep their source form.
    ["#foo;", "#foo;"],
    ["#fff;", "#fff;"],
    ["#0;", "#0;"],
    ["#1114112;", "#1114112;"],
    ["#55296;", "#55296;"],
  ])("%s", (raw, text) => {
    expect(decodeSeqText(raw)).toBe(text);
  });
});

describe("losesSemicolon", () => {
  test.each([
    ["A->>B: style a:#f00;", true],
    ["A->>B: style: #f00;", false],
    ["style a:x#y; z", true],
    ["style a:#y", false],
    ["classDef a:#f;", true],
    ["a:#f; style", false],
    ["style #:;", false],
    ["style ::#;", true],
    ["style : x:#;", true],
    ["#style:#;", true],
    ["styl a:#f;", false],
    ["", false],
  ])("%s is %s", (line, loses) => {
    expect(losesSemicolon(line)).toBe(loses);
  });

  test("says what the regexes say on any line", () => {
    const bits = [
      "style",
      "classDef",
      ":",
      "#",
      ";",
      " ",
      "x",
      "\t",
      "st",
      "yle",
    ];
    let seed = 11;
    const next = () => {
      seed = (seed * 1_103_515_245 + 12_345) % 2_147_483_648;
      return seed;
    };
    for (let i = 0; i < 20_000; i++) {
      const line = Array.from(
        { length: next() % 9 },
        () => bits[next() % bits.length],
      ).join("");
      expect(losesSemicolon(line), line).toBe(droppedSemicolons(line).size > 0);
    }
  });
});

describe("statements", () => {
  test("s07: each statement's depth follows the nesting", () => {
    const body = fixture("s07");
    const model = understood(body);
    const lines = body.split("\n");
    const seen = model.statements.map((s: SeqStatement) => [
      defined(lines[s.line]).trim(),
      s.depth,
      s.kind,
    ]);
    expect(seen).toEqual([
      ["participant A", 0, "declaration"],
      ["participant B", 0, "declaration"],
      ["loop every minute", 0, "blockStart"],
      ["A->>B: ping", 1, "message"],
      ["alt healthy", 1, "blockStart"],
      ["B-->>A: pong", 2, "message"],
      ["else slow", 1, "blockMiddle"],
      ["opt retry", 2, "blockStart"],
      ["A->>B: ping again", 3, "message"],
      ["end", 2, "blockEnd"],
      ["else", 1, "blockMiddle"],
      ["B--xA: gone", 2, "message"],
      ["end", 1, "blockEnd"],
      ["end", 0, "blockEnd"],
      ["par first branch", 0, "blockStart"],
      ["A->>B: one", 1, "message"],
      ["and second branch", 0, "blockMiddle"],
      ["A->>B: two", 1, "message"],
      ["critical take the lock", 1, "blockStart"],
      ["B->>B: lock", 2, "message"],
      ["option timed out", 1, "blockMiddle"],
      ["break no lock", 2, "blockStart"],
      ["B-->>A: busy", 3, "message"],
      ["end", 2, "blockEnd"],
      ["end", 1, "blockEnd"],
      ["end", 0, "blockEnd"],
      ["rect rgb(240, 240, 255)", 0, "blockStart"],
      ["A->>B: tinted", 1, "message"],
      ["end", 0, "blockEnd"],
    ]);
  });

  test("a box counts as a level, and its end closes it", () => {
    const model = understood(
      `${BODY}  box Aqua x\n  participant A\n  end\n  A->>A: y`,
    );
    expect(model.statements.map((s) => [s.kind, s.depth])).toEqual([
      ["boxStart", 0],
      ["declaration", 1],
      ["boxEnd", 0],
      ["message", 0],
    ]);
  });

  test("the record list counts everything mermaid's list counts", () => {
    const body = `${BODY}  autonumber\n  A->>+B: x\n  Note over A: n\n  activate A\n  deactivate A\n  loop l\n  A->>B: y\n  end\n  alt a\n  else b\n  end\n  par p\n  and q\n  end\n  critical c\n  option o\n  end\n  link A: L @ https://example.com\n  title T\n  accTitle: x`;
    const model = understood(body);
    expect(model.statements.map((s) => s.records)).toEqual([
      [0],
      [1, 2],
      [3],
      [4],
      [5],
      [6],
      [7],
      [8],
      [9],
      [10],
      [11],
      [12],
      [13],
      [14],
      [15],
      [16],
      [17],
      [],
      [],
      [],
    ]);
    expect(model.records.length).toBe(18);
  });

  test("a whole-line # or % and a bare %% are comments here", () => {
    const body = `${BODY}%%\n% single\n# hash\n  A->>B: x\n  %% whole`;
    const model = understood(body);
    expect(model.statements.length).toBe(1);
    expect(model.scan.lines[4]?.kind).toBe("statement");
  });

  test("a title that runs over a line is not understood", () => {
    expect(parseSequence(`${BODY}  title\n  A->>B: x`).understood).toBe(false);
  });

  test("a %% after a message is its text, after end a comment", () => {
    const body = `${BODY}  loop l\n  A->>B: x %% y\n  end %% e`;
    const model = understood(body);
    expect(reads(body, model.messages[0]?.text)).toBe("x %% y");
    expect(model.statements.map((s) => s.kind)).toEqual([
      "blockStart",
      "message",
      "blockEnd",
    ]);
  });

  test("a keyword reads the way the lexer reads it: any case, and at a word boundary", () => {
    const model = understood(
      `${BODY}  NOTE OVER A: x\n  LOOP l\n  Looper->>endpoint: y\n  END\n  PARTICIPANT p AS q`,
    );
    expect(model.statements.map((s) => s.kind)).toEqual([
      "note",
      "blockStart",
      "message",
      "blockEnd",
      "declaration",
    ]);
    expect(model.order).toEqual(["A", "Looper", "endpoint", "p"]);
    expect(parseSequence(`${BODY}  end-point->>B: x`).understood).toBe(false);
  });
});

describe("what stays text", () => {
  test.each(Object.entries(FALLBACK))("%s", (_path, body) => {
    const model = parseSequence(body);
    expect(model.understood).toBe(false);
    expect(model.reasons.length).toBeGreaterThan(0);
  });

  test("a body that is no sequence diagram", () => {
    const model = parseSequence("flowchart TD\n  A --> B");
    expect(model.understood).toBe(false);
    expect(model.scan.keyword).toBe("flowchart");
  });

  test("a body past mermaid's text limit is refused before any scan", () => {
    const body = `${BODY}${"  A->>B: x\n".repeat(5000)}`;
    expect(body.length).toBeGreaterThan(50_000);
    const model = parseSequence(body);
    expect(model.understood).toBe(false);
    expect(model.reasons[0]).toMatch(/too long/);
    expect(model.scan.lines).toEqual([]);
  });

  test("a body at the limit is read", () => {
    const line = "  A->>B: x\n";
    const lines = Math.floor((50_000 - BODY.length) / line.length);
    const body = `${BODY}${line.repeat(lines)}`.padEnd(50_000, " ");
    expect(parseSequence(body).understood).toBe(true);
  });

  test("a line past the line limit", () => {
    const at = `${BODY}  A->>B: ${"x".repeat(MAX_SEQ_LINE - 9)}`;
    expect(at.split("\n")[1]?.length).toBe(MAX_SEQ_LINE);
    expect(parseSequence(at).understood).toBe(true);
    expect(parseSequence(`${at}x`).understood).toBe(false);
  });

  test("it never throws", () => {
    const bits = [
      "sequenceDiagram",
      "\n",
      "A",
      "->>",
      ":",
      ";",
      "#",
      "%%",
      "%%{",
      "}%%",
      "@{",
      "}",
      "as",
      "participant",
      "end",
      "loop",
      "\r",
      "<",
      "+",
      "-",
      "()",
      "accDescr {",
      "---",
      "box",
      ",",
      "note over",
    ];
    let seed = 7;
    const pick = () => {
      seed = (seed * 1_103_515_245 + 12_345) % 2_147_483_648;
      return bits[seed % bits.length] ?? "";
    };
    for (let i = 0; i < 2000; i++) {
      const body = Array.from({ length: seed % 12 }, pick).join(" ");
      expect(() => parseSequence(body)).not.toThrow();
      expect(() => parseSequence(`sequenceDiagram\n${body}`)).not.toThrow();
    }
  });
});

/** The arrows a message can carry, as the sequence grammar names them. */
const MESSAGE_ARROWS = [
  "->",
  "-->",
  "->>",
  "-->>",
  "<<->>",
  "<<-->>",
  "-x",
  "--x",
  "-)",
  "--)",
];

describe("the spans of every corpus file", () => {
  test.each(corpus)("%s", (_name, body) => {
    const model = understood(body);
    for (const message of model.messages) {
      expect(reads(body, message.fromSpan)).toBe(message.from);
      expect(reads(body, message.toSpan)).toBe(message.to);
      const token = defined(reads(body, message.arrow.span));
      expect(MESSAGE_ARROWS).toContain(token.toLowerCase());
      expect(message.span.from).toBe(message.fromSpan.from);
      if (message.arrow.activation) {
        expect(["+", "-"]).toContain(reads(body, message.arrow.activation));
      }
      // No span holds a second statement: a `;` in a text is an entity's.
      expect(
        (reads(body, message.text) ?? "").replace(/#\w+;/g, ""),
      ).not.toMatch(/[;\n]/);
    }
    for (const participant of model.participants.values()) {
      for (const declaration of participant.declarations) {
        expect(reads(body, declaration.nameSpan)).toBe(participant.name);
      }
    }
    for (const statement of model.statements) {
      for (const actor of statement.actors) {
        expect(reads(body, actor.span)).toBe(actor.name);
      }
      expect(model.scan.lines[statement.line]?.from).toBeLessThanOrEqual(
        statement.span.from,
      );
    }
    const listed = model.statements.flatMap((s) => s.records);
    expect(listed).toEqual(model.records.map((_r, i) => i));
  });
});

describe("cost", () => {
  /** The best of three runs, in ms: the least disturbed by a busy machine. */
  function fastest(body: string): number {
    let best = Infinity;
    for (let i = 0; i < 3; i++) {
      const from = performance.now();
      parseSequence(body);
      best = Math.min(best, performance.now() - from);
    }
    return best;
  }
  // Eight times the text: linear work is 8x and quadratic 64x, so the bound sits
  // well apart from both and a busy machine does not reach it (R36).
  const GROWTH_LIMIT = 24;
  const FLOOR_MS = 10;

  /** Filler lines of one shape, as many as fit `size` characters. */
  const fill = (line: string, size: number) =>
    `${BODY}${line.repeat(Math.floor(size / line.length))}`;

  test.each([
    ["plain messages", "  A->>B: some words\n"],
    ["spaces before a colon", `  activate A${" ".repeat(900)}:\n`],
    ["a long name", `  ${"a-b ".repeat(400)}->>B: x\n`],
    ["block starts", "  loop x\n"],
    ["ends", "  end\n"],
    ["empty statements", ";".repeat(900) + "\n"],
    [
      "a run of spaces in a participant line",
      `  participant A${" ".repeat(1500)}x:\n`,
    ],
  ])("%s", (_name, line) => {
    const small = fill(line, 6_000);
    const large = fill(line, 48_000);
    expect(large.length).toBeLessThan(50_000);
    parseSequence(small); // warms the JIT up, so it does not count for `small`
    // One more measurement before it fails: a stall on a busy machine (385 against
    // 351 at a load of 52) does not last, and a slow path fails both.
    const grew = (): boolean =>
      fastest(large) < GROWTH_LIMIT * Math.max(fastest(small), FLOOR_MS);
    expect(grew() || grew()).toBe(true);
  });
});
