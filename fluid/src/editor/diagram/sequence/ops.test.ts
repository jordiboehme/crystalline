// Exact text for each op, then the holes the earlier reviews found: what the
// check after the op has to see, what must refuse and what must stay as written.

import { history, undo } from "@codemirror/commands";
import { EditorState } from "@codemirror/state";
import { describe, expect, test } from "vitest";

import { seqDbDelta, seqDbFor } from "../../../test/mermaidOracle";
import { docText, lineSeparatorFor } from "../../setup";
import { NOTICES, applyEdits, toChanges } from "../edits";
import { encodeSeqText } from "./labels";
import type { SeqTarget } from "./model";
import {
  type SeqOp,
  blocksOf,
  compileSeqOp,
  readingOf,
  sameReading,
} from "./ops";
import { parseSequence } from "./parse";

const FIXTURES = import.meta.glob<string>(
  "../../fixtures/diagrams/sequence/*.mmd",
  { query: "?raw", import: "default", eager: true },
);

function fixture(prefix: string): string {
  const path = Object.keys(FIXTURES).find((p) => p.includes(`/${prefix}-`));
  if (path === undefined) {
    throw new Error(`no fixture ${prefix}`);
  }
  return (FIXTURES[path] ?? "").replace(/\n$/, "");
}

const STARTER =
  "sequenceDiagram\n    participant caller as Caller\n    participant service as Service\n    caller->>service: Request\n    service-->>caller: Reply";

type MessageTarget = SeqTarget & { kind: "message" };

function message(
  from: string,
  to: string,
  text: string,
  occurrence = 0,
): MessageTarget {
  return { kind: "message", from, to, text, occurrence };
}

function run(body: string, op: SeqOp): string {
  const model = parseSequence(body);
  expect(model.reasons).toEqual([]);
  const result = compileSeqOp(model, op);
  if ("refused" in result) {
    throw new Error(`refused: ${result.refused}`);
  }
  return applyEdits(body, result.edits);
}

function code(body: string, op: SeqOp): string {
  const model = parseSequence(body);
  expect(model.reasons).toEqual([]);
  const result = compileSeqOp(model, op);
  return "refused" in result ? result.refused : "applied";
}

function compiled(body: string, op: SeqOp) {
  const result = compileSeqOp(parseSequence(body), op);
  if ("refused" in result) {
    throw new Error(`refused: ${result.refused}`);
  }
  return result;
}

describe("the brief's exact text on the starter", () => {
  test.each<[string, SeqOp, string]>([
    [
      "renameParticipant changes only the alias",
      { op: "renameParticipant", name: "caller", text: "Client" },
      "sequenceDiagram\n    participant caller as Client\n    participant service as Service\n    caller->>service: Request\n    service-->>caller: Reply",
    ],
    [
      "renameMessage changes only the text",
      {
        op: "renameMessage",
        target: message("caller", "service", "Request"),
        text: "Ask",
      },
      "sequenceDiagram\n    participant caller as Caller\n    participant service as Service\n    caller->>service: Ask\n    service-->>caller: Reply",
    ],
    [
      "addParticipant goes after the second declaration",
      { op: "addParticipant" },
      "sequenceDiagram\n    participant caller as Caller\n    participant service as Service\n    participant p1 as New participant\n    caller->>service: Request\n    service-->>caller: Reply",
    ],
    [
      "addMessage appends",
      { op: "addMessage", from: "service", to: "caller" },
      `${STARTER}\n    service->>caller: New message`,
    ],
    [
      "arrowStyle dotted",
      {
        op: "arrowStyle",
        target: message("caller", "service", "Request"),
        line: "dotted",
      },
      "sequenceDiagram\n    participant caller as Caller\n    participant service as Service\n    caller-->>service: Request\n    service-->>caller: Reply",
    ],
    [
      "deleteMessage removes the line",
      {
        op: "deleteMessage",
        target: message("service", "caller", "Reply"),
      },
      "sequenceDiagram\n    participant caller as Caller\n    participant service as Service\n    caller->>service: Request",
    ],
  ])("%s", (_, op, want) => {
    expect(run(STARTER, op)).toBe(want);
  });
});

describe("what each op hands back", () => {
  test("addParticipant creates the participant, with the smallest free id", () => {
    expect(compiled(STARTER, { op: "addParticipant" }).created).toEqual({
      kind: "participant",
      name: "p1",
    });
    const taken =
      "sequenceDiagram\n  participant p1\n  participant p3\n  participant P2";
    expect(run(taken, { op: "addParticipant" })).toBe(
      `${taken}\n  participant p2 as New participant`,
    );
  });

  test("addMessage creates its target, counted among its equals", () => {
    const once = run(STARTER, {
      op: "addMessage",
      from: "caller",
      to: "caller",
    });
    expect(
      compiled(STARTER, { op: "addMessage", from: "caller", to: "caller" })
        .created,
    ).toEqual(message("caller", "caller", "New message"));
    expect(
      compiled(once, { op: "addMessage", from: "caller", to: "caller" })
        .created,
    ).toEqual(message("caller", "caller", "New message", 1));
  });

  test("the same text again is no edit, not a refusal", () => {
    expect(
      compiled(STARTER, {
        op: "renameParticipant",
        name: "caller",
        text: " Caller ",
      }).edits,
    ).toEqual([]);
    expect(
      compiled(STARTER, {
        op: "renameMessage",
        target: message("caller", "service", "Request"),
        text: "Request",
      }).edits,
    ).toEqual([]);
    expect(
      compiled(STARTER, {
        op: "arrowStyle",
        target: message("caller", "service", "Request"),
        line: "solid",
        head: "arrow",
      }).edits,
    ).toEqual([]);
  });

  test.each<[string, SeqOp]>([
    [
      "renameParticipant",
      { op: "renameParticipant", name: "nobody", text: "x" },
    ],
    ["addMessage from", { op: "addMessage", from: "nobody", to: "caller" }],
    ["addMessage to", { op: "addMessage", from: "caller", to: "nobody" }],
    ["deleteParticipant", { op: "deleteParticipant", name: "nobody" }],
    [
      "renameMessage",
      {
        op: "renameMessage",
        target: message("caller", "service", "Nope"),
        text: "x",
      },
    ],
    [
      "renameMessage occurrence",
      {
        op: "renameMessage",
        target: message("caller", "service", "Request", 1),
        text: "x",
      },
    ],
    [
      "arrowStyle",
      {
        op: "arrowStyle",
        target: message("service", "caller", "Request"),
        line: "dotted",
      },
    ],
    [
      "deleteMessage",
      { op: "deleteMessage", target: message("caller", "caller", "Request") },
    ],
  ])("%s of what is not there is gone", (_, op) => {
    expect(code(STARTER, op)).toBe("gone");
  });
});

describe("encodeSeqText", () => {
  test.each<[string, string]>([
    ["plain", "plain"],
    ["one\ntwo", "one<br>two"],
    ["a\r\nb", "a<br>b"],
    ["a\rb", "a<br>b"],
    ["a;b", "a#59;b"],
    // R55: every #, since a bare one cuts the text.
    ["Issue #12", "Issue #35;12"],
    ["#35;", "#35;35#59;"],
    ["a<br>b", "a#lt;br#gt;b"],
    ["A & B", "A #38; B"],
    ["wrap: x", "wrap#58; x"],
    ["nowrap: x", "nowrap#58; x"],
    [":wrap: x", ":wrap#58; x"],
    ["Wrap: x", "Wrap: x"],
    ["a %%{b", "a #37;#37;{b"],
    ["$$x$$", "#36;#36;x#36;#36;"],
    ["a $ b", "a $ b"],
    ["\u00B6\u00DF", "#182;\u00DF"],
    ["\uFB02\u00B0", "#64258;\u00B0"],
    // The entity forms hold the # and ; that encodeEntities reads after style and a colon.
    ["style a:#b; c", "style a#58;#35;b#59; c"],
    ["classDef:#x; y", "classDef#58;#35;x#59; y"],
    ["a:b", "a:b"],
  ])("%j is written %j", (typed, written) => {
    expect(encodeSeqText(typed)).toBe(written);
  });
});

describe("renameParticipant", () => {
  test.each<[string, string, string, string, string]>([
    [
      "an alias on a create",
      "sequenceDiagram\n  participant h\n  create actor r as Old\n  h->>r: x",
      "r",
      "New",
      "sequenceDiagram\n  participant h\n  create actor r as New\n  h->>r: x",
    ],
    [
      "the alias drawn, when there are two",
      "sequenceDiagram\n  participant a as One\n  participant a as Two",
      "a",
      "New",
      "sequenceDiagram\n  participant a as One\n  participant a as New",
    ],
    [
      "no alias: after the name of its first declaration",
      "sequenceDiagram\n  actor a\n  a->>a: x\n  participant a",
      "a",
      "New",
      "sequenceDiagram\n  actor a as New\n  a->>a: x\n  participant a",
    ],
    [
      "a name with odd space keeps it",
      "sequenceDiagram\n\tparticipant   a  \n  a->>a: x",
      "a",
      "New",
      "sequenceDiagram\n\tparticipant   a as New  \n  a->>a: x",
    ],
    [
      "no alias, with a config: after the config",
      'sequenceDiagram\n  participant a@{ "type": "queue" }\n  a->>a: x',
      "a",
      "New",
      'sequenceDiagram\n  participant a@{ "type": "queue" } as New\n  a->>a: x',
    ],
    [
      "an alias in a config, quoted",
      'sequenceDiagram\n  participant a@{ "type": "queue", "alias": "Old" }',
      "a",
      "New",
      'sequenceDiagram\n  participant a@{ "type": "queue", "alias": "New" }',
    ],
    [
      'an alias in a config writes ", \\ and } as forms',
      'sequenceDiagram\n  participant a@{ "alias": "Old" }',
      "a",
      'q"b\\c}',
      'sequenceDiagram\n  participant a@{ "alias": "q#quot;b#92;c#125;" }',
    ],
    [
      "an alias in a config, bare: quotes come with it",
      "sequenceDiagram\n  participant a@{ alias: Old }",
      "a",
      "x: y",
      'sequenceDiagram\n  participant a@{ alias: "x: y" }',
    ],
    [
      "a second statement on the line stays",
      "sequenceDiagram\n  participant a; participant b; a->>b: x",
      "a",
      "New",
      "sequenceDiagram\n  participant a as New; participant b; a->>b: x",
    ],
    [
      "undeclared: before its first mention",
      "sequenceDiagram\n  a->>b: x\n  b->>c: y",
      "c",
      "New",
      "sequenceDiagram\n  a->>b: x\n  participant c as New\n  b->>c: y",
    ],
    [
      "undeclared: the first one",
      "sequenceDiagram\n  a->>b: x",
      "a",
      "New",
      "sequenceDiagram\n  participant a as New\n  a->>b: x",
    ],
    [
      "undeclared: before the outermost block, at its indent",
      "sequenceDiagram\n  a->>a: x\n  loop one\n    opt two\n      a->>b: y\n    end\n  end",
      "b",
      "New",
      "sequenceDiagram\n  a->>a: x\n  participant b as New\n  loop one\n    opt two\n      a->>b: y\n    end\n  end",
    ],
    [
      "undeclared in a note",
      "sequenceDiagram\n  a->>a: x\n  Note over b: n",
      "b",
      "New",
      "sequenceDiagram\n  a->>a: x\n  participant b as New\n  Note over b: n",
    ],
  ])("%s", (_, body, name, text, want) => {
    expect(run(body, { op: "renameParticipant", name, text })).toBe(want);
  });

  // R69: a second addActor breaks mermaid's chain of neighbours, and a message then
  // loses its room, so the earlier participants are declared on top with it.
  test.each<[string, string, string, string]>([
    [
      "the receiver of the very first message",
      "sequenceDiagram\n  a->>b: x\n  b->>a: y",
      "b",
      "sequenceDiagram\n  participant a\n  participant b as New\n  a->>b: x\n  b->>a: y",
    ],
    [
      "another undeclared one comes first inside the block",
      "sequenceDiagram\n  loop l\n    x->>y: 1\n    a->>b: 2\n  end",
      "b",
      "sequenceDiagram\n  participant x\n  participant y\n  participant a\n  participant b as New\n  loop l\n    x->>y: 1\n    a->>b: 2\n  end",
    ],
    [
      "only those not read yet",
      "sequenceDiagram\n  a->>a: 1\n  b->>c: 2",
      "c",
      "sequenceDiagram\n  a->>a: 1\n  participant b\n  participant c as New\n  b->>c: 2",
    ],
    [
      "a later statement on the same line",
      "sequenceDiagram\n  a->>b: x;c->>d: y",
      "d",
      "sequenceDiagram\n  participant a\n  participant b\n  participant c\n  participant d as New\n  a->>b: x;c->>d: y",
    ],
    [
      "a sender with a space takes a bare declaration",
      "sequenceDiagram\n  Web App->>B: x",
      "B",
      "sequenceDiagram\n  participant Web App\n  participant B as New\n  Web App->>B: x",
    ],
    [
      "above a destroy, which waits for this message",
      "sequenceDiagram\n  participant w\n  destroy w\n  a->>w: x\n  a->>a: y",
      "a",
      "sequenceDiagram\n  participant w\n  participant a as New\n  destroy w\n  a->>w: x\n  a->>a: y",
    ],
    [
      "a late bare declaration stays as written",
      "sequenceDiagram\n  a->>b: long\n  participant b\n  b->>a: y",
      "b",
      "sequenceDiagram\n  participant a\n  participant b as New\n  a->>b: long\n  participant b\n  b->>a: y",
    ],
    [
      "a late declaration on the mention's own line",
      "sequenceDiagram\n  a->>b: x; participant b",
      "b",
      "sequenceDiagram\n  participant a\n  participant b as New\n  a->>b: x; participant b",
    ],
    [
      "a menu line that comes first, with no declaration, stays read",
      "sequenceDiagram\n  a->>b: 1; link b: D @ https://e.com\n  b->>a: 2",
      "b",
      "sequenceDiagram\n  participant a\n  participant b as New\n  a->>b: 1; link b: D @ https://e.com\n  b->>a: 2",
    ],
  ])("undeclared: %s", async (_, body, name, want) => {
    const after = run(body, { op: "renameParticipant", name, text: "New" });
    expect(after).toBe(want);
    const [before, now] = [await seqDbFor(body), await seqDbFor(after)];
    // Only the label: the chain and the menus are in this delta.
    expect(seqDbDelta(before, now)).toEqual([`actor ${name} description`]);
  });

  test.each<[string, string, string]>([
    [
      "a create's message: its sender goes right of the created one",
      "sequenceDiagram\n  create participant w\n  a->>w: x",
      "a",
    ],
    [
      "an earlier one created inside the block",
      "sequenceDiagram\n  loop l\n    create participant w\n    a->>w: x\n    a->>b: y\n  end",
      "b",
    ],
    [
      "an earlier one with its type inside the block",
      'sequenceDiagram\n  loop l\n    participant x@{ "type": "queue" }\n    x->>b: y\n  end',
      "b",
    ],
    [
      "an earlier actor inside the block",
      "sequenceDiagram\n  loop l\n    actor x\n    x->>b: y\n  end",
      "b",
    ],
    [
      "an earlier one with its alias inside the block",
      "sequenceDiagram\n  loop l\n    participant x as X\n    x->>b: y\n  end",
      "b",
    ],
    [
      "a body with its statement on the header line",
      "sequenceDiagram; a->>b: x",
      "b",
    ],
  ])("undeclared, with no room on top: %s refuses", (_, body, name) => {
    expect(code(body, { op: "renameParticipant", name, text: "New" })).toBe(
      "invalid",
    );
  });

  // R69, finding 2.
  test.each<[string, string]>([
    ["link", "link b: D @ https://e.com"],
    ["links", 'links b: {"D": "https://e.com"}'],
    ["properties", 'properties b: {"class": "k"}'],
    ["details", "details b: d"],
  ])("a %s line before the declaration refuses the alias", (_, menu) => {
    const late = `sequenceDiagram\n  a->>b: 1\n  ${menu}\n  participant b\n  b->>a: 2`;
    expect(
      code(late, { op: "renameParticipant", name: "b", text: "New" }),
    ).toBe("invalid");
    const first = `sequenceDiagram\n  ${menu}\n  participant b`;
    expect(
      code(first, { op: "renameParticipant", name: "b", text: "New" }),
    ).toBe("invalid");
  });

  test("a late declaration with its menu line below it goes on top", async () => {
    const body =
      "sequenceDiagram\n  a->>b: 1\n  participant b\n  link b: D @ https://e.com";
    const after = run(body, {
      op: "renameParticipant",
      name: "b",
      text: "New",
    });
    expect(after).toBe(
      "sequenceDiagram\n  participant a\n  participant b as New\n  a->>b: 1\n  participant b\n  link b: D @ https://e.com",
    );
    expect(seqDbDelta(await seqDbFor(body), await seqDbFor(after))).toEqual([
      "actor b description",
    ]);
  });

  test("a menu line after the declaration stays read", async () => {
    const body =
      "sequenceDiagram\n  participant b\n  link b: D @ https://e.com";
    const after = run(body, {
      op: "renameParticipant",
      name: "b",
      text: "New",
    });
    expect(after).toContain("participant b as New\n");
    expect(seqDbDelta(await seqDbFor(body), await seqDbFor(after))).toEqual([
      "actor b description",
    ]);
  });

  test("s02: every implicit participant keeps the db's order", async () => {
    const body = fixture("s02");
    const order = (await seqDbFor(body)).actors.map((a) => a.name);
    expect(order).toEqual(["browser", "proxy", "daemon"]);
    for (const name of order) {
      const after = run(body, { op: "renameParticipant", name, text: "New" });
      const db = await seqDbFor(after);
      expect(db.actors.map((a) => a.name)).toEqual(order);
      expect(db.actors.find((a) => a.name === name)?.description).toBe("New");
    }
    expect(
      run(body, { op: "renameParticipant", name: "daemon", text: "New" }),
    ).toContain("    participant daemon as New\n    proxy->>daemon: forward");
  });

  // R61: rule 12 takes no spaces and rule 13 takes them all, so this would be one
  // participant named `Web App as L`, and with an undeclared one a second participant.
  test.each<[string, string]>([
    ["declared", "sequenceDiagram\n  participant Web App\n  Web App->>B: x"],
    ["undeclared", "sequenceDiagram\n  Web App->>B: x"],
    ["with a tab", "sequenceDiagram\n  Web\tApp->>B: x"],
  ])("a name with a space cannot take an alias: %s", (_, body) => {
    const name = parseSequence(body).order[0] ?? "";
    expect(name).toMatch(/\s/);
    expect(code(body, { op: "renameParticipant", name, text: "L" })).toBe(
      "unsupported",
    );
  });

  test("that refusal is not a claim about every spaced body: another name still renames", () => {
    const body = "sequenceDiagram\n  participant Web App\n  Web App->>B: x";
    expect(run(body, { op: "renameParticipant", name: "B", text: "L" })).toBe(
      "sequenceDiagram\n  participant Web App\n  participant B as L\n  Web App->>B: x",
    );
  });

  test("a later actor line, which mermaid ignores, stays ignored", async () => {
    // With ` as L` on that line it would count, and turn `c` into an actor.
    const body = "sequenceDiagram\n  c->>a: x\n  actor c";
    expect(parseSequence(body).participants.get("c")?.type).toBe("participant");
    const after = run(body, { op: "renameParticipant", name: "c", text: "L" });
    expect(after).toBe(
      "sequenceDiagram\n  participant c as L\n  c->>a: x\n  actor c",
    );
    expect(seqDbDelta(await seqDbFor(body), await seqDbFor(after))).toEqual([
      "actor c description",
    ]);
  });

  test("a name the declaration form reads otherwise refuses", () => {
    const body = "sequenceDiagram\n  x@y->>b: z";
    expect(parseSequence(body).order[0]).toBe("x@y");
    expect(
      code(body, { op: "renameParticipant", name: "x@y", text: "L" }),
    ).toBe("invalid");
  });

  test.each(["&amp;", "&#35;", "&nbsp;"])(
    "%j cannot be drawn as typed and refuses",
    (text) => {
      expect(
        code(STARTER, { op: "renameParticipant", name: "caller", text }),
      ).toBe("invalid");
    },
  );

  test.each(["", "   ", "\n"])("an empty label %j refuses", (text) => {
    expect(
      code(STARTER, { op: "renameParticipant", name: "caller", text }),
    ).toBe("invalid");
  });

  test("a form in a wrapped alias refuses, plain words apply", () => {
    const body = "sequenceDiagram\n  participant a as wrap: One";
    expect(
      code(body, { op: "renameParticipant", name: "a", text: "x#y" }),
    ).toBe("invalid");
    expect(run(body, { op: "renameParticipant", name: "a", text: "Two" })).toBe(
      "sequenceDiagram\n  participant a as wrap: Two",
    );
  });

  test("a new alias under a config that names wrap: a form refuses too", () => {
    const body =
      '%%{init: {"sequence":{"wrap":true}}}%%\nsequenceDiagram\n  A->>A: x';
    expect(
      code(body, { op: "renameParticipant", name: "A", text: "x#y" }),
    ).toBe("invalid");
  });

  test("the id stays everywhere", () => {
    const after = run(STARTER, {
      op: "renameParticipant",
      name: "caller",
      text: "service",
    });
    expect(parseSequence(after).order).toEqual(["caller", "service"]);
    expect(after).toContain("caller->>service: Request");
  });
});

describe("renameMessage", () => {
  test.each<[string, string, MessageTarget, string, string]>([
    [
      "R61: no text, right after the colon",
      "sequenceDiagram\n  A->>B:\n  B->>A: y",
      message("A", "B", ""),
      "new",
      "sequenceDiagram\n  A->>B: new\n  B->>A: y",
    ],
    [
      "R61: no text after a prefix",
      "sequenceDiagram\n  A->>B: wrap:\n  B->>A: y",
      message("A", "B", ""),
      "new",
      "sequenceDiagram\n  A->>B: wrap: new\n  B->>A: y",
    ],
    [
      "R61: no text, a space already there",
      "sequenceDiagram\n  A->>B: \n  B->>A: y",
      message("A", "B", ""),
      "new",
      "sequenceDiagram\n  A->>B: new\n  B->>A: y",
    ],
    [
      "R61: no text, a space before the colon",
      "sequenceDiagram\n  A->>B :\n  B->>A: y",
      message("A", "B", ""),
      "new",
      "sequenceDiagram\n  A->>B : new\n  B->>A: y",
    ],
    [
      "R61: no text, a statement follows",
      "sequenceDiagram\n  A->>B:;B->>A: y",
      message("A", "B", ""),
      "new",
      "sequenceDiagram\n  A->>B: new;B->>A: y",
    ],
    [
      "the prefix stays",
      "sequenceDiagram\n  A->>B: wrap: old",
      message("A", "B", "old"),
      "new",
      "sequenceDiagram\n  A->>B: wrap: new",
    ],
    [
      "odd space stays",
      "sequenceDiagram\n  A  ->>  B  :   old   ",
      message("A", "B", "old"),
      "new",
      "sequenceDiagram\n  A  ->>  B  :   new   ",
    ],
    [
      "the second of two equals",
      "sequenceDiagram\n  A->>B: same\n  A->>B: same",
      message("A", "B", "same", 1),
      "new",
      "sequenceDiagram\n  A->>B: same\n  A->>B: new",
    ],
    [
      "emptied: the colon stays and the space goes",
      "sequenceDiagram\n  A->>B: old\n  B->>A: y",
      message("A", "B", "old"),
      "",
      "sequenceDiagram\n  A->>B:\n  B->>A: y",
    ],
    [
      "a target by its drawn text",
      "sequenceDiagram\n  A->>B: one<br/>t#35;o",
      message("A", "B", "one\nt#o"),
      "new",
      "sequenceDiagram\n  A->>B: new",
    ],
    [
      "a # and a ; stay in the one statement",
      "sequenceDiagram\n  A->>B: old;B->>A: y",
      message("A", "B", "old"),
      "a#b;c",
      "sequenceDiagram\n  A->>B: a#35;b#59;c;B->>A: y",
    ],
  ])("%s", (_, body, target, text, want) => {
    expect(run(body, { op: "renameMessage", target, text })).toBe(want);
  });

  test("an empty text on a message with none is no edit", () => {
    expect(
      compiled("sequenceDiagram\n  A->>B:", {
        op: "renameMessage",
        target: message("A", "B", ""),
        text: "  ",
      }).edits,
    ).toEqual([]);
  });

  // R47's reading for a sequence: drawnLabels decodes `&word;` in the drawn text,
  // so nothing can show these as typed. The field and the drawing must not disagree.
  test.each(["&amp;", "x &quot; y", "&#35;", "&nbsp;"])(
    "%j cannot be drawn as typed and refuses",
    (text) => {
      expect(
        code(STARTER, {
          op: "renameMessage",
          target: message("caller", "service", "Request"),
          text,
        }),
      ).toBe("invalid");
    },
  );

  // mermaid's wrapping can break a word inside a hidden form.
  test.each<[string, string, MessageTarget]>([
    [
      "a wrap: on the message",
      "sequenceDiagram\n  A->>A: wrap: old",
      message("A", "A", "old"),
    ],
    [
      "a wrap: with no text after it",
      "sequenceDiagram\n  A->>A: wrap:",
      message("A", "A", ""),
    ],
    [
      "a config that names wrap",
      "---\nconfig:\n  sequence:\n    wrap: true\n---\nsequenceDiagram\n  A->>A: old",
      message("A", "A", "old"),
    ],
  ])("a form in a wrapped text refuses: %s", (_, body, target) => {
    expect(code(body, { op: "renameMessage", target, text: "a#b" })).toBe(
      "invalid",
    );
    expect(
      run(body, { op: "renameMessage", target, text: "plain words" }),
    ).toContain("plain words");
  });

  test("nowrap: wraps nothing, so a form is fine there", () => {
    expect(
      run("sequenceDiagram\n  A->>A: nowrap: old", {
        op: "renameMessage",
        target: message("A", "A", "old"),
        text: "a#b",
      }),
    ).toBe("sequenceDiagram\n  A->>A: nowrap: a#35;b");
  });

  test("U+2028 and U+2029 refuse, as the parser refuses them; a no-break space applies", () => {
    const target = message("caller", "service", "Request");
    for (const text of ["a\u2028b", "a\u2029b"]) {
      expect(code(STARTER, { op: "renameMessage", target, text })).toBe(
        "invalid",
      );
    }
    expect(
      run(STARTER, { op: "renameMessage", target, text: "a\u00A0b" }),
    ).toContain("caller->>service: a\u00A0b");
  });

  test("occurrence of the other messages follows the new text", () => {
    const body = "sequenceDiagram\n  A->>B: a\n  A->>B: b\n  A->>B: a";
    const after = run(body, {
      op: "renameMessage",
      target: message("A", "B", "b"),
      text: "a",
    });
    expect(parseSequence(after).messages.map((m) => m.occurrence)).toEqual([
      0, 1, 2,
    ]);
  });
});

describe("addParticipant", () => {
  test.each<[string, string, string]>([
    [
      "after a box's end when the last declaration is inside",
      "sequenceDiagram\n  participant z\n  box Aqua L\n    participant a\n  end\n  z->>a: x",
      "sequenceDiagram\n  participant z\n  box Aqua L\n    participant a\n  end\n  participant p1 as New participant\n  z->>a: x",
    ],
    [
      "at the end when one is undeclared",
      "sequenceDiagram\n  participant a\n  a->>b: x\n\n",
      "sequenceDiagram\n  participant a\n  a->>b: x\n  participant p1 as New participant\n\n",
    ],
    [
      "at the end when one is declared after the first message",
      "sequenceDiagram\n  a->>a: x\n  participant b",
      "sequenceDiagram\n  a->>a: x\n  participant b\n  participant p1 as New participant",
    ],
    [
      "at the end when a create is the last declaration: the line after it waits for its message",
      "sequenceDiagram\n  participant h\n  create participant w\n  h->>w: x",
      "sequenceDiagram\n  participant h\n  create participant w\n  h->>w: x\n  participant p1 as New participant",
    ],
    [
      "at the end after a block, at the top level's indent",
      "sequenceDiagram\n  a->>a: x\n  loop l\n      a->>a: y\n  end",
      "sequenceDiagram\n  a->>a: x\n  loop l\n      a->>a: y\n  end\n  participant p1 as New participant",
    ],
    [
      "a declaration inside a block: at the end, at the top level",
      "sequenceDiagram\n  loop l\n      participant a\n      a->>a: x\n  end",
      "sequenceDiagram\n  loop l\n      participant a\n      a->>a: x\n  end\n  participant p1 as New participant",
    ],
    [
      "a note mentioned first counts as undeclared",
      "sequenceDiagram\n  Note over q: n\n  participant a",
      "sequenceDiagram\n  Note over q: n\n  participant a\n  participant p1 as New participant",
    ],
    [
      "nothing yet: four spaces",
      "sequenceDiagram",
      "sequenceDiagram\n    participant p1 as New participant",
    ],
    [
      "tabs stay tabs",
      "sequenceDiagram\n\tparticipant a\n\ta->>a: x",
      "sequenceDiagram\n\tparticipant a\n\tparticipant p1 as New participant\n\ta->>a: x",
    ],
    [
      "a comment after the end stays above it",
      "sequenceDiagram\n  a->>a: x\n  %% last",
      "sequenceDiagram\n  a->>a: x\n  %% last\n  participant p1 as New participant",
    ],
  ])("%s", async (_, body, want) => {
    const after = run(body, { op: "addParticipant" });
    expect(after).toBe(want);
    const [before, now] = [await seqDbFor(body), await seqDbFor(after)];
    // Rightmost either way.
    expect(now.actors.map((a) => a.name)).toEqual([
      ...before.actors.map((a) => a.name),
      "p1",
    ]);
    expect(now.actors.at(-1)?.description).toBe("New participant");
  });

  test("a new participant does not join a box", async () => {
    const body = "sequenceDiagram\n  box L\n    participant a\n  end";
    const db = await seqDbFor(run(body, { op: "addParticipant" }));
    expect(db.boxes).toEqual([{ actors: ["a"] }]);
  });

  test("hideUnusedParticipants, which would not draw it, is refused before any op", () => {
    const model = parseSequence(
      '%%{init: {"sequence":{"hideUnusedParticipants":true}}}%%\nsequenceDiagram\n  participant a',
    );
    expect(model.understood).toBe(false);
    expect(compileSeqOp(model, { op: "addParticipant" })).toEqual({
      refused: "unsupported",
    });
  });
});

describe("addMessage", () => {
  test.each<[string, string, string, string, string]>([
    [
      "top level after a block, not inside it",
      "sequenceDiagram\n  a->>b: x\n  loop l\n      a->>b: y\n  end",
      "a",
      "b",
      "sequenceDiagram\n  a->>b: x\n  loop l\n      a->>b: y\n  end\n  a->>b: New message",
    ],
    [
      "a name with a space as it is",
      "sequenceDiagram\n  Web App->>B: x",
      "B",
      "Web App",
      "sequenceDiagram\n  Web App->>B: x\n  B->>Web App: New message",
    ],
    [
      "to itself",
      "sequenceDiagram\n\ta->>b: x",
      "b",
      "b",
      "sequenceDiagram\n\ta->>b: x\n\tb->>b: New message",
    ],
    [
      "blank lines after the end stay after it",
      "sequenceDiagram\n  a->>b: x\n\n",
      "a",
      "b",
      "sequenceDiagram\n  a->>b: x\n  a->>b: New message\n\n",
    ],
    [
      "the indent of an accDescr block's text does not count",
      "sequenceDiagram\n  a->>b: x\n  accDescr {\n        text\n  }",
      "a",
      "b",
      "sequenceDiagram\n  a->>b: x\n  accDescr {\n        text\n  }\n  a->>b: New message",
    ],
  ])("%s", (_, body, from, to, want) => {
    expect(run(body, { op: "addMessage", from, to })).toBe(want);
  });

  // It draws without a throw, but its arrow ends at a life line that has stopped.
  test.each<[string, string]>([
    ["host", "worker"],
    ["worker", "host"],
    ["worker", "worker"],
  ])("a message with a destroyed participant refuses: %s to %s", (from, to) => {
    const body =
      "sequenceDiagram\n  participant host\n  participant worker\n  destroy worker\n  host-xworker: stop";
    expect(code(body, { op: "addMessage", from, to })).toBe("participantEnded");
  });

  test("a keyword-like name that the message form reads otherwise refuses", () => {
    const body = "sequenceDiagram\n  participant loop\n  participant b";
    expect(code(body, { op: "addMessage", from: "loop", to: "b" })).toBe(
      "invalid",
    );
  });
});

describe("arrowStyle", () => {
  const body = "sequenceDiagram\n  A->>B: x";
  const target = message("A", "B", "x");

  test.each<
    ["solid" | "dotted", "arrow" | "open" | "cross" | "async" | "both", string]
  >([
    ["solid", "arrow", "->>"],
    ["solid", "open", "->"],
    ["solid", "cross", "-x"],
    ["solid", "async", "-)"],
    ["solid", "both", "<<->>"],
    ["dotted", "arrow", "-->>"],
    ["dotted", "open", "-->"],
    ["dotted", "cross", "--x"],
    ["dotted", "async", "--)"],
    ["dotted", "both", "<<-->>"],
  ])("%s %s is %s", (line, head, token) => {
    const from = "sequenceDiagram\n  A<<-->>B: x";
    expect(run(from, { op: "arrowStyle", target, line, head })).toBe(
      `sequenceDiagram\n  A${token}B: x`,
    );
  });

  test("the part it does not name stays", () => {
    expect(
      run("sequenceDiagram\n  A--xB: x", {
        op: "arrowStyle",
        target,
        head: "async",
      }),
    ).toBe("sequenceDiagram\n  A--)B: x");
    expect(
      run("sequenceDiagram\n  A--xB: x", {
        op: "arrowStyle",
        target,
        line: "solid",
      }),
    ).toBe("sequenceDiagram\n  A-xB: x");
    expect(
      run("sequenceDiagram\n  A<<-->>B: x", {
        op: "arrowStyle",
        target,
        line: "solid",
      }),
    ).toBe("sequenceDiagram\n  A<<->>B: x");
    expect(
      run("sequenceDiagram\n  A<<-->>B: x", {
        op: "arrowStyle",
        target,
        head: "arrow",
      }),
    ).toBe("sequenceDiagram\n  A-->>B: x");
  });

  test("the activation marker and odd space stay", () => {
    expect(
      run("sequenceDiagram\n  A ->>+ B : x\n  B-->>-A: y", {
        op: "arrowStyle",
        target,
        head: "cross",
      }),
    ).toBe("sequenceDiagram\n  A -x+ B : x\n  B-->>-A: y");
    expect(
      run("sequenceDiagram\n  A->>+B: x\n  B-->>-A: y", {
        op: "arrowStyle",
        target: message("B", "A", "y"),
        head: "both",
      }),
    ).toBe("sequenceDiagram\n  A->>+B: x\n  B<<-->>-A: y");
  });

  test("a capital X in the source reads as the cross it is: no edit", () => {
    expect(
      compiled("sequenceDiagram\n  A-XB: x", {
        op: "arrowStyle",
        target,
        head: "cross",
      }).edits,
    ).toEqual([]);
  });

  test("a receiver that starts with x stays itself", () => {
    expect(
      run("sequenceDiagram\n  A->>xray: x", {
        op: "arrowStyle",
        target: message("A", "xray", "x"),
        head: "cross",
      }),
    ).toBe("sequenceDiagram\n  A-xxray: x");
  });

  test("neither part named is no edit", () => {
    expect(compiled(body, { op: "arrowStyle", target }).edits).toEqual([]);
  });
});

describe("deleteMessage", () => {
  test.each<[string, string, MessageTarget, string]>([
    [
      "the last line takes the break before it",
      "sequenceDiagram\n  A->>B: x\n  A->>B: y",
      message("A", "B", "y"),
      "sequenceDiagram\n  A->>B: x",
    ],
    [
      "a middle line takes its own break",
      "sequenceDiagram\n  A->>B: x\n  A->>B: y\n  A->>B: z",
      message("A", "B", "y"),
      "sequenceDiagram\n  A->>B: x\n  A->>B: z",
    ],
    [
      "first of a shared line: its ; goes",
      "sequenceDiagram\n  A->>B: 0\n  A->>B:x;B-->>A:y",
      message("A", "B", "x"),
      "sequenceDiagram\n  A->>B: 0\n  B-->>A:y",
    ],
    [
      "last of a shared line: the ; before it goes",
      "sequenceDiagram\n  A->>B: 0\n  B-->>A: 1\n  A->>B:x;B-->>A:y",
      message("B", "A", "y"),
      "sequenceDiagram\n  A->>B: 0\n  B-->>A: 1\n  A->>B:x",
    ],
    [
      "a ; at the end of the line goes with the line",
      "sequenceDiagram\n  A->>B: 0\n  A->>B: x;\n  B->>A: y",
      message("A", "B", "x"),
      "sequenceDiagram\n  A->>B: 0\n  B->>A: y",
    ],
    [
      "a block on the same line stays",
      "sequenceDiagram\n  A->>B: 0\n  A->>B: x; loop l\n  B->>A: y\n  end",
      message("A", "B", "x"),
      "sequenceDiagram\n  A->>B: 0\n  loop l\n  B->>A: y\n  end",
    ],
    [
      "the + goes with its message",
      "sequenceDiagram\n  A->>B: 0\n  A->>+B: x\n  B->>A: y",
      message("A", "B", "x"),
      "sequenceDiagram\n  A->>B: 0\n  B->>A: y",
    ],
    [
      "a comment beside it stays",
      "sequenceDiagram\n  A->>B: 0\n  %% before\n  A->>B: x\n  %% after",
      message("A", "B", "x"),
      "sequenceDiagram\n  A->>B: 0\n  %% before\n  %% after",
    ],
  ])("%s", (_, body, target, want) => {
    expect(run(body, { op: "deleteMessage", target })).toBe(want);
  });

  test("a later equal takes the occurrence, and its target still finds it", () => {
    const body =
      "sequenceDiagram\n  A->>B: 0\n  A->>B: same\n  B->>A: y\n  A->>B: same";
    const after = run(body, {
      op: "deleteMessage",
      target: message("A", "B", "same"),
    });
    expect(after).toBe(
      "sequenceDiagram\n  A->>B: 0\n  B->>A: y\n  A->>B: same",
    );
    expect(parseSequence(after).messages.at(-1)?.occurrence).toBe(0);
  });

  test("a create or destroy that would lose its message refuses", () => {
    expect(
      code(
        "sequenceDiagram\n  participant h\n  create participant w\n  h->>w: x\n  h->>h: y",
        { op: "deleteMessage", target: message("h", "w", "x") },
      ),
    ).toBe("invalid");
    expect(
      code(
        "sequenceDiagram\n  participant h\n  participant w\n  destroy w\n  h-xw: x\n  h->>h: y",
        { op: "deleteMessage", target: message("h", "w", "x") },
      ),
    ).toBe("invalid");
  });

  // R69: the participants stay, on top, where they were read until now.
  test.each<[string, string, MessageTarget, string]>([
    [
      "the only message",
      "sequenceDiagram\n    Alice->>Bob: hi",
      message("Alice", "Bob", "hi"),
      "sequenceDiagram\n    participant Alice\n    participant Bob",
    ],
    [
      "an undeclared participant would move",
      "sequenceDiagram\n  a->>b: x\n  b->>a: y",
      message("a", "b", "x"),
      "sequenceDiagram\n  participant a\n  participant b\n  b->>a: y",
    ],
    [
      "an undeclared participant would vanish",
      "sequenceDiagram\n  participant a\n  a->>b: x\n  a->>a: y",
      message("a", "b", "x"),
      "sequenceDiagram\n  participant a\n  participant b\n  a->>a: y",
    ],
    [
      "inside a block: above the block",
      "sequenceDiagram\n  loop l\n    x->>y: 1\n    a->>b: 2\n    b->>x: 3\n  end",
      message("x", "y", "1"),
      "sequenceDiagram\n  participant x\n  participant y\n  loop l\n    a->>b: 2\n    b->>x: 3\n  end",
    ],
    [
      "those read before it in the block go on top with it",
      "sequenceDiagram\n  loop l\n    x->>y: 1\n    a->>b: 2\n    b->>x: 3\n  end",
      message("a", "b", "2"),
      "sequenceDiagram\n  participant x\n  participant y\n  participant a\n  participant b\n  loop l\n    x->>y: 1\n    b->>x: 3\n  end",
    ],
    [
      "on a shared line, the rest stays",
      "sequenceDiagram\n  a->>b: 1;b->>a: 2",
      message("a", "b", "1"),
      "sequenceDiagram\n  participant a\n  participant b\n  b->>a: 2",
    ],
  ])(
    "%s: the delete applies and the order stays",
    async (_, body, target, want) => {
      const after = run(body, { op: "deleteMessage", target });
      expect(after).toBe(want);
      const [before, now] = [await seqDbFor(body), await seqDbFor(after)];
      expect(now.actors).toEqual(before.actors);
      expect(now.facts).toEqual(before.facts);
    },
  );

  test("no lines are added when nothing would move", () => {
    expect(
      run("sequenceDiagram\n  a->>b: 1\n  a->>b: 2", {
        op: "deleteMessage",
        target: message("a", "b", "1"),
      }),
    ).toBe("sequenceDiagram\n  a->>b: 2");
  });

  // The source would keep its `+` and the bar would go from the drawing. A `-` whose
  // `+` is on a message goes with that `+` instead (below).
  test.each<[string, string, MessageTarget]>([
    [
      "the - of an activate line",
      "sequenceDiagram\n  participant a\n  participant b\n  activate b\n  b-->>-a: x\n  a->>b: y",
      message("b", "a", "x"),
    ],
  ])(
    "a delete that leaves more open activations refuses: %s",
    (_, body, target) => {
      expect(code(body, { op: "deleteMessage", target })).toBe("leavesActive");
    },
  );

  // A message whose + or - has its other end on a message takes that marker along,
  // paired as mermaid pairs them: a close ends the latest open activation of its
  // participant.
  const ASK =
    "sequenceDiagram\n  participant a\n  participant b\n  a->>+b: ask\n  b-->>-a: reply\n  a->>b: later";
  const STACKED =
    "sequenceDiagram\n  participant a\n  participant b\n  a->>+b: 1\n  a->>+b: 2\n  b-->>-a: 3\n  b-->>-a: 4";
  const BETWEEN =
    "sequenceDiagram\n  participant a\n  participant b\n  participant c\n  a->>+b: ask\n  b->>c: work\n  c-->>b: done\n  b-->>-a: reply";
  const AGAIN =
    "sequenceDiagram\n  participant a\n  participant b\n  a->>+b: one\n  b-->>-a: back\n  a->>+b: two\n  b-->>-a: back again";
  test.each<[string, string, MessageTarget, string]>([
    [
      "the + drops the - of its reply",
      ASK,
      message("a", "b", "ask"),
      "sequenceDiagram\n  participant a\n  participant b\n  b-->>a: reply\n  a->>b: later",
    ],
    [
      "the - drops the + of its opener",
      ASK,
      message("b", "a", "reply"),
      "sequenceDiagram\n  participant a\n  participant b\n  a->>b: ask\n  a->>b: later",
    ],
    [
      "nested: the outer + pairs with the last -",
      STACKED,
      message("a", "b", "1"),
      "sequenceDiagram\n  participant a\n  participant b\n  a->>+b: 2\n  b-->>-a: 3\n  b-->>a: 4",
    ],
    [
      "nested: the inner - pairs with the inner +",
      STACKED,
      message("b", "a", "3"),
      "sequenceDiagram\n  participant a\n  participant b\n  a->>+b: 1\n  a->>b: 2\n  b-->>-a: 4",
    ],
    [
      "nested: the outer - pairs with the outer +",
      STACKED,
      message("b", "a", "4"),
      "sequenceDiagram\n  participant a\n  participant b\n  a->>b: 1\n  a->>+b: 2\n  b-->>-a: 3",
    ],
    [
      "other messages between the + and the -",
      BETWEEN,
      message("a", "b", "ask"),
      "sequenceDiagram\n  participant a\n  participant b\n  participant c\n  b->>c: work\n  c-->>b: done\n  b-->>a: reply",
    ],
    [
      "repeated: the second + pairs with the second -",
      AGAIN,
      message("a", "b", "two"),
      "sequenceDiagram\n  participant a\n  participant b\n  a->>+b: one\n  b-->>-a: back\n  b-->>a: back again",
    ],
    [
      "repeated: the first - pairs with the first +",
      AGAIN,
      message("b", "a", "back"),
      "sequenceDiagram\n  participant a\n  participant b\n  a->>b: one\n  a->>+b: two\n  b-->>-a: back again",
    ],
  ])(
    "an activation pair: %s, and mermaid reads the result",
    async (_, body, target, want) => {
      const after = run(body, { op: "deleteMessage", target });
      expect(after).toBe(want);
      // Throws on mermaid's refusal, as for an inactive participant.
      await expect(seqDbFor(after)).resolves.toBeDefined();
    },
  );

  test.each<[string, string, MessageTarget, string]>([
    [
      "a + with its later - stays: mermaid's verify answers that one",
      "sequenceDiagram\n  participant a\n  participant b\n  a->>+b: 1\n  a->>b: 2",
      message("a", "b", "1"),
      "sequenceDiagram\n  participant a\n  participant b\n  a->>b: 2",
    ],
    [
      "a plain message beside an open activation",
      "sequenceDiagram\n  participant a\n  participant b\n  a->>+b: 1\n  a->>b: 2\n  b-->>-a: 3",
      message("a", "b", "2"),
      "sequenceDiagram\n  participant a\n  participant b\n  a->>+b: 1\n  b-->>-a: 3",
    ],
  ])("no more open than before: %s", (_, body, target, want) => {
    expect(run(body, { op: "deleteMessage", target })).toBe(want);
  });

  test("the notice for it is plain", () => {
    expect(NOTICES.leavesActive).toBe(
      "That would leave an activation open, so nothing was changed.",
    );
  });

  // R56.
  test.each<[string, string, MessageTarget]>([
    [
      "a loop",
      "sequenceDiagram\n  participant A\n  loop l\n    A->>A: x\n  end",
      message("A", "A", "x"),
    ],
    [
      "a rect",
      "sequenceDiagram\n  participant A\n  rect rgb(1,2,3)\n    A->>A: x\n  end",
      message("A", "A", "x"),
    ],
    [
      "the first section of an alt",
      "sequenceDiagram\n  participant A\n  alt a\n    A->>A: x\n  else b\n    A->>A: y\n  end",
      message("A", "A", "x"),
    ],
    [
      "the last section of a par",
      "sequenceDiagram\n  participant A\n  par a\n    A->>A: x\n  and b\n    A->>A: y\n  end",
      message("A", "A", "y"),
    ],
    [
      "only an activate would stay",
      "sequenceDiagram\n  participant A\n  loop l\n    activate A\n    A->>A: x\n    deactivate A\n  end",
      message("A", "A", "x"),
    ],
    [
      "nested",
      "sequenceDiagram\n  participant A\n  loop l\n    opt o\n      A->>A: x\n    end\n  end",
      message("A", "A", "x"),
    ],
  ])(
    "a delete that would empty %s refuses with its notice",
    (_, body, target) => {
      expect(code(body, { op: "deleteMessage", target })).toBe("emptiesBlock");
    },
  );

  test("the notice is plain", () => {
    expect(NOTICES.emptiesBlock).toBe(
      "That would leave a block with nothing in it, so nothing was changed.",
    );
  });

  test.each<[string, string, MessageTarget, string]>([
    [
      "a message stays in the section",
      "sequenceDiagram\n  participant A\n  loop l\n    A->>A: x\n    A->>A: y\n  end",
      message("A", "A", "x"),
      "sequenceDiagram\n  participant A\n  loop l\n    A->>A: y\n  end",
    ],
    [
      "a note stays in the section",
      "sequenceDiagram\n  participant A\n  loop l\n    A->>A: x\n    Note over A: n\n  end",
      message("A", "A", "x"),
      "sequenceDiagram\n  participant A\n  loop l\n    Note over A: n\n  end",
    ],
    [
      "a nested block with a message stays in the section",
      "sequenceDiagram\n  participant A\n  loop l\n    A->>A: x\n    opt o\n      A->>A: y\n    end\n  end",
      message("A", "A", "x"),
      "sequenceDiagram\n  participant A\n  loop l\n    opt o\n      A->>A: y\n    end\n  end",
    ],
    [
      "a section written empty by hand stays understood and stops nothing",
      "sequenceDiagram\n  participant A\n  loop l\n  end\n  A->>A: x\n  A->>A: y",
      message("A", "A", "x"),
      "sequenceDiagram\n  participant A\n  loop l\n  end\n  A->>A: y",
    ],
  ])("R56 does not fire: %s", (_, body, target, want) => {
    expect(run(body, { op: "deleteMessage", target })).toBe(want);
  });
});

describe("deleteParticipant", () => {
  test.each<[string, string, string, string]>([
    [
      "the starter",
      STARTER,
      "caller",
      "sequenceDiagram\n    participant service as Service",
    ],
    [
      "each kind of line that names it",
      [
        "sequenceDiagram",
        "  participant a",
        "  participant b",
        "  participant c",
        "  participant a as A again",
        "  link a: L @ https://e.com",
        "  links a: {}",
        '  properties a: {"class": "x"}',
        "  details a: d",
        "  b->>c: keep",
        "  activate a",
        "  deactivate a",
        "  a->>b: from",
        "  b->>a: to",
        "  a->>a: self",
        "  Note over a: only",
        "  Note left of a: left",
        "  Note right of a: right",
        "  Note over a,a: twice",
        "  c->>b: keep too",
      ].join("\n"),
      "a",
      "sequenceDiagram\n  participant b\n  participant c\n  b->>c: keep\n  c->>b: keep too",
    ],
    [
      "create and destroy with their messages",
      "sequenceDiagram\n  participant h\n  h->>h: 0\n  create participant w\n  h->>w: 1\n  destroy w\n  w-xh: 2\n  h->>h: 3",
      "w",
      "sequenceDiagram\n  participant h\n  h->>h: 0\n  h->>h: 3",
    ],
    [
      "a note over two loses the name and one comma",
      "sequenceDiagram\n  participant a\n  participant b\n  Note over a,b: 1\n  Note over b,a: 2\n  Note  over  a , b :  3\n  b->>b: x",
      "a",
      "sequenceDiagram\n  participant b\n  Note over b: 1\n  Note over b: 2\n  Note  over  b :  3\n  b->>b: x",
    ],
    [
      "statements on a shared line go one by one",
      "sequenceDiagram\n  participant b\n  participant a\n  a->>b: 1;b->>b: 2;a->>b: 3;a->>b: 4;b->>b: 5",
      "a",
      "sequenceDiagram\n  participant b\n  b->>b: 2;b->>b: 5",
    ],
    [
      "a box left empty stays",
      "sequenceDiagram\n  box L\n    participant a\n  end\n  participant b\n  b->>b: x",
      "a",
      "sequenceDiagram\n  box L\n  end\n  participant b\n  b->>b: x",
    ],
    [
      "a box keeps its others",
      "sequenceDiagram\n  box L\n    participant a\n    participant b\n  end\n  b->>b: x",
      "a",
      "sequenceDiagram\n  box L\n    participant b\n  end\n  b->>b: x",
    ],
    [
      "comments stay",
      "sequenceDiagram\n  %% one\n  participant a\n  %% two\n  participant b\n  %% three\n  a->>b: x\n  %% four\n  b->>b: y",
      "a",
      "sequenceDiagram\n  %% one\n  %% two\n  participant b\n  %% three\n  %% four\n  b->>b: y",
    ],
    [
      "the only one: an empty diagram",
      "sequenceDiagram\n  participant a\n  a->>a: x",
      "a",
      "sequenceDiagram",
    ],
  ])("%s", async (_, body, name, want) => {
    const after = run(body, { op: "deleteParticipant", name });
    expect(after).toBe(want);
    const db = await seqDbFor(after);
    expect(db.actors.map((a) => a.name)).not.toContain(name);
  });

  test("autonumber's records stay where they were", async () => {
    const body =
      "sequenceDiagram\n  participant a\n  participant b\n  autonumber\n  a->>b: 1\n  b->>b: 2\n  autonumber 10\n  b->>b: 3";
    const after = run(body, { op: "deleteParticipant", name: "a" });
    const db = await seqDbFor(after);
    expect(db.records.map((r) => r.autonumber?.start ?? r.message)).toEqual([
      "",
      "2",
      10,
      "3",
    ]);
  });

  test("create and destroy of another stay on their own messages", async () => {
    const body =
      "sequenceDiagram\n  participant a\n  participant h\n  a->>h: gone\n  create participant w\n  h->>w: keep\n  destroy w\n  h-xw: stop";
    const after = run(body, { op: "deleteParticipant", name: "a" });
    const db = await seqDbFor(after);
    expect(db.created).toEqual([["w", 0]]);
    expect(db.destroyed).toEqual([["w", 1]]);
  });

  test("another's activation goes with the + on a message it loses", () => {
    const body =
      "sequenceDiagram\n  participant a\n  participant b\n  a->>+b: x\n  b->>b: y";
    expect(run(body, { op: "deleteParticipant", name: "a" })).toBe(
      "sequenceDiagram\n  participant b\n  b->>b: y",
    );
  });

  test.each<[string, string, string, string]>([
    [
      "a create would lose its message",
      "sequenceDiagram\n  participant a\n  create participant b\n  a->>b: x\n  Note over b: y",
      "a",
      "invalid",
    ],
    [
      "the - that closes another's +",
      "sequenceDiagram\n  participant a\n  participant b\n  participant c\n  a->>+b: 1\n  b-->>-c: 2\n  a->>b: 3",
      "c",
      "leavesActive",
    ],
    [
      "R56: its message is the last in a block",
      "sequenceDiagram\n  participant a\n  participant b\n  loop l\n    a->>b: x\n  end",
      "a",
      "emptiesBlock",
    ],
    [
      "R56: its note is the last in a section",
      "sequenceDiagram\n  participant a\n  participant b\n  alt x\n    Note over a: n\n  else y\n    b->>b: z\n  end",
      "a",
      "emptiesBlock",
    ],
  ])("%s", (_, body, name, want) => {
    expect(code(body, { op: "deleteParticipant", name })).toBe(want);
  });

  // R69: the others stay, on top.
  test.each<[string, string, string, string]>([
    [
      "the only message",
      "sequenceDiagram\n    Alice->>Bob: hi",
      "Alice",
      "sequenceDiagram\n    participant Bob",
    ],
    [
      "another undeclared would move",
      "sequenceDiagram\n  a->>b: 1\n  c->>b: 2",
      "a",
      "sequenceDiagram\n  participant b\n  c->>b: 2",
    ],
    [
      "s02's proxy: the others would vanish",
      fixture("s02"),
      "proxy",
      "sequenceDiagram\n    %% nobody is declared, the order comes from the first mention\n    participant browser\n    participant daemon",
    ],
  ])("%s: the delete applies", (_, body, name, want) => {
    expect(run(body, { op: "deleteParticipant", name })).toBe(want);
  });

  test("a note over two in a block keeps the section", () => {
    const body =
      "sequenceDiagram\n  participant a\n  participant b\n  loop l\n    Note over a,b: n\n  end";
    expect(run(body, { op: "deleteParticipant", name: "a" })).toBe(
      "sequenceDiagram\n  participant b\n  loop l\n    Note over b: n\n  end",
    );
  });
});

describe("R51: every remaining line keeps its own break, through a real state", () => {
  const DOC =
    "sequenceDiagram\r\n    participant caller as Caller\n    participant service as Service\r\n    caller->>service: Request\n    service-->>caller: Reply\r\n    caller->>caller: Last";

  /** The doc saved through a real state, with what `toChanges` makes of the edits. */
  function saved(doc: string, op: SeqOp): string {
    const state = EditorState.create({
      doc,
      extensions: lineSeparatorFor(doc),
    });
    const body = state.doc.sliceString(0, state.doc.length);
    const result = compileSeqOp(parseSequence(body), op);
    if ("refused" in result) {
      throw new Error(`refused: ${result.refused}`);
    }
    return docText(
      state.update({ changes: toChanges(0, result.edits, state.lineBreak) })
        .state,
    );
  }

  test("the document keeps its breaks when nothing happens", () => {
    const state = EditorState.create({
      doc: DOC,
      extensions: lineSeparatorFor(DOC),
    });
    expect(docText(state)).toBe(DOC);
    expect(state.lineBreak).toBe("\r\n");
  });

  test("addParticipant between lines: its own break is the state's", () => {
    expect(saved(DOC, { op: "addParticipant" })).toBe(
      "sequenceDiagram\r\n    participant caller as Caller\n    participant service as Service\r\n    participant p1 as New participant\r\n    caller->>service: Request\n    service-->>caller: Reply\r\n    caller->>caller: Last",
    );
  });

  test("addMessage on the last line, with no break of its own", () => {
    expect(
      saved(DOC, { op: "addMessage", from: "caller", to: "service" }),
    ).toBe(`${DOC}\r\n    caller->>service: New message`);
  });

  test("a declaration inserted before a line", () => {
    const doc = "sequenceDiagram\r\n  a->>a: x\n  a->>b: y\r\n  b->>a: z";
    expect(
      saved(doc, { op: "renameParticipant", name: "b", text: "New" }),
    ).toBe(
      "sequenceDiagram\r\n  a->>a: x\n  participant b as New\r\n  a->>b: y\r\n  b->>a: z",
    );
  });

  test("declarations on top, each with the state's break", () => {
    const doc = "sequenceDiagram\r\n  a->>b: x\n  b->>a: y\r\n  b->>b: z";
    expect(
      saved(doc, { op: "renameParticipant", name: "b", text: "New" }),
    ).toBe(
      "sequenceDiagram\r\n  participant a\r\n  participant b as New\r\n  a->>b: x\n  b->>a: y\r\n  b->>b: z",
    );
  });

  test("removeBlock: each keyword line goes with its own break", () => {
    const doc =
      "sequenceDiagram\r\n  participant a\n  loop l\r\n    a->>a: x\n  end\r\n  a->>a: y";
    expect(saved(doc, { op: "removeBlock", block: 0 })).toBe(
      "sequenceDiagram\r\n  participant a\n    a->>a: x\n  a->>a: y",
    );
  });

  test("a rename in a line touches no break", () => {
    expect(
      saved(DOC, {
        op: "renameMessage",
        target: message("caller", "service", "Request"),
        text: "Ask",
      }),
    ).toBe(DOC.replace("Request", "Ask"));
    expect(
      saved(DOC, { op: "renameParticipant", name: "caller", text: "Client" }),
    ).toBe(DOC.replace("Caller", "Client"));
  });

  test("deleteMessage in the middle: the line goes with its own break", () => {
    expect(
      saved(DOC, {
        op: "deleteMessage",
        target: message("caller", "service", "Request"),
      }),
    ).toBe(DOC.replace("    caller->>service: Request\n", ""));
    expect(
      saved(DOC, {
        op: "deleteMessage",
        target: message("service", "caller", "Reply"),
      }),
    ).toBe(DOC.replace("    service-->>caller: Reply\r\n", ""));
  });

  test("deleteMessage on the last line: only the break before it goes", () => {
    expect(
      saved(DOC, {
        op: "deleteMessage",
        target: message("caller", "caller", "Last"),
      }),
    ).toBe(DOC.replace("\r\n    caller->>caller: Last", ""));
  });

  test("deleteParticipant: every other line keeps its own break", () => {
    expect(saved(DOC, { op: "deleteParticipant", name: "service" })).toBe(
      "sequenceDiagram\r\n    participant caller as Caller\n    caller->>caller: Last",
    );
  });
});

describe("removeBlock (R66)", () => {
  const P = "sequenceDiagram\n  participant a\n  participant b\n";

  test.each<[string, string, string]>([
    ["loop", `${P}  loop l\n    a->>b: x\n  end\n`, `${P}    a->>b: x\n`],
    ["opt", `${P}  opt l\n    a->>b: x\n  end\n`, `${P}    a->>b: x\n`],
    ["break", `${P}  break l\n    a->>b: x\n  end\n`, `${P}    a->>b: x\n`],
    [
      "rect",
      `${P}  rect rgb(1,2,3)\n    a->>b: x\n  end\n`,
      `${P}    a->>b: x\n`,
    ],
    [
      "alt with sections",
      `${P}  alt one\n    a->>b: x\n  else two\n    Note over a: n\n  else\n    b->>a: y\n  end\n`,
      `${P}    a->>b: x\n    Note over a: n\n    b->>a: y\n`,
    ],
    [
      "par with sections",
      `${P}  par one\n    a->>b: x\n  and two\n    b->>a: y\n  end\n`,
      `${P}    a->>b: x\n    b->>a: y\n`,
    ],
    [
      "par over",
      `${P}  par_over one\n    a->>b: x\n  and two\n    b->>a: y\n  end\n`,
      `${P}    a->>b: x\n    b->>a: y\n`,
    ],
    [
      "critical with sections",
      `${P}  critical one\n    a->>b: x\n  option two\n    b->>a: y\n  end\n`,
      `${P}    a->>b: x\n    b->>a: y\n`,
    ],
    ["empty by hand", `${P}  loop l\n  end\n  a->>b: x`, `${P}  a->>b: x`],
    [
      "on one line with its messages",
      `${P}  loop l; a->>b: z; b->>a: w; end\n  a->>b: x`,
      `${P}  a->>b: z; b->>a: w\n  a->>b: x`,
    ],
    [
      "comments inside stay",
      `${P}  loop l\n    %% one\n    a->>b: x\n    %% two\n  end %% tail\n`,
      `${P}    %% one\n    a->>b: x\n    %% two\n   %% tail\n`,
    ],
  ])("%s", async (_, body, want) => {
    const after = run(body, { op: "removeBlock", block: 0 });
    expect(after).toBe(want);
    const [before, now] = [await seqDbFor(body), await seqDbFor(after)];
    const kept = (list: typeof before.records) =>
      list
        .filter((r) => r.from !== undefined)
        .map((r) => [r.from, r.to, r.message, r.type, r.placement]);
    expect(kept(now.records)).toEqual(kept(before.records));
    expect(now.actors).toEqual(before.actors);
  });

  test("nested: only the one named goes, in source order of the starts", () => {
    const body = `${P}  loop outer\n    a->>b: 1\n    opt inner\n      a->>b: 2\n    end\n    a->>b: 3\n  end`;
    expect(blocksOf(parseSequence(body))).toHaveLength(2);
    expect(run(body, { op: "removeBlock", block: 0 })).toBe(
      `${P}    a->>b: 1\n    opt inner\n      a->>b: 2\n    end\n    a->>b: 3`,
    );
    expect(run(body, { op: "removeBlock", block: 1 })).toBe(
      `${P}  loop outer\n    a->>b: 1\n      a->>b: 2\n    a->>b: 3\n  end`,
    );
  });

  test("blocksOf holds each block's own statements, with the records the drawing names", () => {
    const body = `${P}  alt one\n    a->>b: 1\n  else two\n    a->>b: 2\n  end\n  loop l\n    a->>b: 3\n  end`;
    const model = parseSequence(body);
    const blocks = blocksOf(model);
    expect(blocks.map((own) => own.map((s) => s.kind))).toEqual([
      ["blockStart", "blockMiddle", "blockEnd"],
      ["blockStart", "blockEnd"],
    ]);
    expect(blocks.map((own) => own.flatMap((s) => s.records))).toEqual([
      [0, 2, 4],
      [5, 7],
    ]);
  });

  test.each([1, -1, 0.5])("block %d is gone", (block) => {
    expect(
      code(`${P}  loop l\n    a->>b: x\n  end`, { op: "removeBlock", block }),
    ).toBe("gone");
  });

  test("a box is no block", () => {
    const body = "sequenceDiagram\n  box L\n    participant a\n  end";
    expect(code(body, { op: "removeBlock", block: 0 })).toBe("gone");
  });

  test("the route R56 promises: the block goes first, then its last message", () => {
    const body = `${P}  loop l\n    a->>b: x\n  end`;
    const target = message("a", "b", "x");
    expect(code(body, { op: "deleteMessage", target })).toBe("emptiesBlock");
    const open = run(body, { op: "removeBlock", block: 0 });
    expect(run(open, { op: "deleteMessage", target })).toBe(
      "sequenceDiagram\n  participant a\n  participant b",
    );
  });

  test("one undo step", () => {
    const doc = `${P}  alt one\n    a->>b: x\n  else two\n    b->>a: y\n  end`;
    const state = EditorState.create({ doc, extensions: [history()] });
    const { edits } = compiled(doc, { op: "removeBlock", block: 0 });
    expect(edits.length).toBeGreaterThan(1);
    const after = state.update({
      changes: toChanges(0, edits, state.lineBreak),
    }).state;
    expect(after.doc.toString()).toBe(`${P}    a->>b: x\n    b->>a: y`);
    let undone = after;
    undo({
      state: after,
      dispatch: (tr) => {
        undone = tr.state;
      },
    });
    expect(undone.doc.toString()).toBe(doc);
  });
});

describe("sameReading compares each field", () => {
  const BODY =
    "sequenceDiagram\n  box L\n    participant a as A\n  end\n  participant b\n  autonumber\n  a->>b: x\n  Note over a: n\n  create participant c\n  a->>c: y\n  destroy c\n  a-xc: z";

  test.each<[string, (r: ReturnType<typeof readingOf>) => void]>([
    [
      "a label",
      (r) => (
        r.people.set("a", { label: "Z", type: "participant" }),
        undefined
      ),
    ],
    [
      "a type",
      (r) => (r.people.set("a", { label: "A", type: "actor" }), undefined),
    ],
    [
      "the order",
      (r) => {
        r.people = new Map([...r.people].reverse());
      },
    ],
    ["one person fewer", (r) => void r.people.delete("b")],
    ["one record fewer", (r) => void r.records.pop()],
    ["a record's from", (r) => recordAt(r, "x", (x) => (x.from = "b"))],
    ["a record's to", (r) => recordAt(r, "x", (x) => (x.to = "a"))],
    ["a record's type", (r) => recordAt(r, "x", (x) => (x.type = 1))],
    ["a record's text", (r) => recordAt(r, "x", (x) => (x.text = "other"))],
    ["a record's placement", (r) => recordAt(r, "n", (x) => (x.placement = 0))],
    [
      "an autonumber",
      (r) => {
        const row = r.records[0];
        if (row) {
          row.autonumber = "null";
        }
      },
    ],
    ["a box's members", (r) => void r.boxes[0]?.push("b")],
    ["a box fewer", (r) => void r.boxes.pop()],
    ["who is created", (r) => void r.created.delete("c")],
    ["where it is created", (r) => void r.created.set("c", 0)],
    ["who is destroyed", (r) => void r.destroyed.delete("c")],
    ["where it is destroyed", (r) => void r.destroyed.set("c", 0)],
  ])("%s", (_, tamper) => {
    const model = parseSequence(BODY);
    expect(model.reasons).toEqual([]);
    expect(sameReading(readingOf(model), readingOf(model))).toBe(true);
    const tampered = readingOf(model);
    tamper(tampered);
    expect(sameReading(tampered, readingOf(model))).toBe(false);
  });

  function recordAt(
    reading: ReturnType<typeof readingOf>,
    text: string,
    change: (row: ReturnType<typeof readingOf>["records"][number]) => void,
  ): void {
    const row = reading.records.find((r) => r.text === text);
    expect(row).toBeDefined();
    if (row) {
      change(row);
    }
  }
});
