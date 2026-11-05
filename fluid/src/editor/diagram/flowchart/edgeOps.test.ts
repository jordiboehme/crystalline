// Real mermaid wherever a test reads a db: a rewritten token can lex as another
// link, and the arrow's meaning is what must not change.

import { EditorState } from "@codemirror/state";
import { describe, expect, test } from "vitest";

import {
  LABEL_TEXTS,
  MARKDOWN_TEXTS,
  PUNCTUATION_TEXTS,
} from "../../../test/labelTable";
import { flowDbFor } from "../../../test/mermaidOracle";
import { docText, lineSeparatorFor } from "../../setup";
import { applyEdits, toChanges } from "../edits";
import { agreesFlow } from "../verdict";
import { decodeLabel, tidyLabel } from "./labels";
import { parseFlowchart } from "./parse";
import { type FlowOp, compileFlowOp } from "./ops";

const TIMEOUT = 120_000;

const KEY = { start: "A", end: "B", occurrence: 0 };

function label(text: string): FlowOp {
  return { op: "edgeLabel", edge: KEY, text };
}

function line(value: "solid" | "dotted" | "thick"): FlowOp {
  return { op: "lineStyle", edge: KEY, line: value };
}

function head(value: "arrow" | "none" | "both"): FlowOp {
  return { op: "arrowhead", edge: KEY, head: value };
}

const REVERSE: FlowOp = { op: "reverse", edge: KEY };

/** The statement after `flowchart TD\n  `, once `op` has run on it. */
function on(statement: string, op: FlowOp): string {
  const body = `flowchart TD\n  ${statement}`;
  const result = compileFlowOp(parseFlowchart(body), op);
  if ("refused" in result) {
    throw new Error(`refused: ${result.refused}`);
  }
  return applyEdits(body, result.edits).slice("flowchart TD\n  ".length);
}

function refusal(statement: string, op: FlowOp): string | null {
  const result = compileFlowOp(
    parseFlowchart(`flowchart TD\n  ${statement}`),
    op,
  );
  return "refused" in result ? result.refused : null;
}

function editCount(statement: string, op: FlowOp): number {
  const result = compileFlowOp(
    parseFlowchart(`flowchart TD\n  ${statement}`),
    op,
  );
  return "refused" in result ? -1 : result.edits.length;
}

describe("edgeLabel", () => {
  test.each<[string, string, string, string]>([
    ["no label: a pipe after the arrow", "A --> B", "Yes", "A -->|Yes| B"],
    ["with no space around", "A-->B", "Yes", "A-->|Yes|B"],
    ["after a long dotted arrow", "A -..-> B", "Yes", "A -..->|Yes| B"],
    ["after an explicit id", "A e1@--> B", "Yes", "A e1@-->|Yes| B"],
    ["after an open arrow", "A --- B", "Yes", "A ---|Yes| B"],
    ["pipe: only the label", "A -->|old| B", "new", "A -->|new| B"],
    ["pipe, quoted keeps quotes", 'A -->|"old"| B', "new", 'A -->|"new"| B'],
    ["text: only the text", "A -- old --> B", "new", "A -- new --> B"],
    [
      "text, odd spacing kept",
      "A  --   old   -->  B",
      "new",
      "A  --   new   -->  B",
    ],
    [
      "text, quoted keeps quotes",
      'A -- "old" --> B',
      "new",
      'A -- "new" --> B',
    ],
    ["thick text", "A == old ==> B", "new", "A == new ==> B"],
    ["dotted text", "A -. old .-> B", "new", "A -. new .-> B"],
    [
      "markdown keeps the wrapper",
      'A -->|"`old`"| B',
      "new",
      'A -->|"`new`"| B',
    ],
    [
      "a quote goes through the encoder",
      "A --> B",
      'a "b"',
      'A -->|"a #quot;b#quot;"| B',
    ],
    ["a pipe in the text", "A --> B", "a|b", 'A -->|"a#124;b"| B'],
    ["a pipe in a text form", "A -- x --> B", "a|b", 'A -- "a#124;b" --> B'],
    ["a line break", "A --> B", "a\nb", "A -->|a<br>b| B"],
    ["a dash in a text form", "A -- x --> B", "a--b", 'A -- "a--b" --> B'],
    ["an equals in a thick form", "A == x ==> B", "a=b", 'A == "a=b" ==> B'],
    ["a dot in a dotted form", "A -. x .-> B", "a.b", 'A -. "a.b" .-> B'],
    ["tidied", "A --> B", "  a   b ", "A -->|a b| B"],
    [
      "an o before a tight closer gets quotes",
      "A -- old--> B",
      "foo",
      'A -- "foo"--> B',
    ],
  ])("%s", (_, statement, text, expected) => {
    expect(on(statement, label(text))).toBe(expected);
  });

  test.each<[string, string, string]>([
    ["pipe", "A -->|x| B", "A --> B"],
    ["pipe, no spaces", "A-->|x|B", "A-->B"],
    ["pipe, quoted", 'A -->|"x"| B', "A --> B"],
    ["pipe, long", "A --->|x| B", "A ---> B"],
    ["pipe, dotted", "A -.->|x| B", "A -.-> B"],
    ["text", "A -- old --> B", "A --> B"],
    ["text, long", "A -- old ---> B", "A ---> B"],
    ["text, thick", "A == old ==> B", "A ==> B"],
    ["text, thick and open", "A == old === B", "A === B"],
    ["text, dotted", "A -. old .-> B", "A -.-> B"],
    ["text, dotted and long", "A -. old ..-> B", "A -..-> B"],
    ["text, open", "A -- old --- B", "A --- B"],
    ["text, both heads", "A <-- old --> B", "A <--> B"],
    ["text, circles", "A o-- old --o B", "A o--o B"],
    ["text, crosses", "A x-- old --x B", "A x--x B"],
    ["text, an arrow id", "A e1@-- old --> B", "A e1@--> B"],
  ])("an empty text removes it: %s", (_, statement, expected) => {
    expect(on(statement, label(""))).toBe(expected);
    expect(on(statement, label("   "))).toBe(expected);
  });

  test("empty on no label, and the same text, are no edits", () => {
    expect(editCount("A --> B", label(""))).toBe(0);
    expect(editCount("A -->|x| B", label("x"))).toBe(0);
    expect(editCount('A -- "x y" --> B', label("  x   y"))).toBe(0);
  });

  test("a label that would hold a direction refuses (R7)", () => {
    expect(refusal("A --> B", label("go direction LR"))).toBe("invalid");
    expect(refusal("A -->|x| B", label("direction TB"))).toBe("invalid");
    expect(refusal("A -- x --> B", label("direction TB"))).toBe("invalid");
  });

  test("a directive refuses", () => {
    expect(refusal("A --> B", label("a %%{b: c"))).toBe("invalid");
  });

  test("a removal that would change the link refuses", () => {
    const key = { start: "A", end: "xray", occurrence: 0 };
    expect(
      refusal("A---|x|xray", { op: "edgeLabel", edge: key, text: "" }),
    ).toBe("invalid");
  });

  test("a group refuses, an arrow that is not there is gone", () => {
    expect(refusal("A & C --> B", label("x"))).toBe("group");
    expect(refusal("A --> B & C", label("x"))).toBe("group");
    expect(refusal("C --> B", label("x"))).toBe("gone");
  });

  test("a label on one link of a chain touches only that link", async () => {
    const after = on("A --> B --> C", label("Yes"));
    expect(after).toBe("A -->|Yes| B --> C");
    const db = await flowDbFor(`flowchart TD\n  ${after}`);
    expect(db.edges.map((e) => e.text)).toEqual(["Yes", ""]);
  });

  test("a second statement and a click URL on the line stay", () => {
    expect(on("A --> B; C --> D", label("Yes"))).toBe("A -->|Yes| B; C --> D");
    expect(
      refusal('A --> B; click A "https://x.y/?a=1;b=2"', label("style:#b")),
    ).toBe("invalid");
  });

  test(
    "every text of the label table draws as typed or refuses, on each form",
    async () => {
      const forms = [
        "A --> B",
        "A -->|old| B",
        "A -- old --> B",
        'A -->|"`old`"| B',
      ];
      const texts = [
        ...new Set([...LABEL_TEXTS, ...PUNCTUATION_TEXTS, ...MARKDOWN_TEXTS]),
      ];
      let applied = 0;
      const problems: string[] = [];
      const refusals: string[] = [];
      for (const form of forms) {
        for (const text of texts) {
          const typed = tidyLabel(text);
          const body = `flowchart TD\n  ${form}`;
          const result = compileFlowOp(parseFlowchart(body), label(text));
          if ("refused" in result) {
            refusals.push(`${form} ${JSON.stringify(text)} ${result.refused}`);
            continue;
          }
          if (result.edits.length === 0) {
            continue;
          }
          applied++;
          const after = applyEdits(body, result.edits);
          const model = parseFlowchart(after);
          const raw = model.chains[0]?.links[0]?.arrow.label;
          const written = raw ? after.slice(raw.from, raw.to) : "";
          const shown = decodeLabel(written, written.startsWith('"'));
          if (tidyLabel(shown) !== typed) {
            problems.push(`${form} ${JSON.stringify(text)}: shows ${shown}`);
          }
          const db = await flowDbFor(after).catch((error: unknown) =>
            String(error),
          );
          if (typeof db === "string") {
            problems.push(`${form} ${JSON.stringify(text)}: ${db}`);
          } else {
            const difference = agreesFlow(model, db);
            if (difference !== null) {
              problems.push(`${after}: ${difference}`);
            }
          }
        }
      }
      expect(problems).toEqual([]);
      expect(refusals).toEqual([]);
      expect(applied).toBeGreaterThan(texts.length * 3);
    },
    TIMEOUT,
  );
});

describe("lineStyle", () => {
  test.each<[string, string, "solid" | "dotted" | "thick", string]>([
    ["arrow to dotted", "A --> B", "dotted", "A -.-> B"],
    ["arrow to thick", "A --> B", "thick", "A ==> B"],
    ["open to dotted", "A --- B", "dotted", "A -.- B"],
    ["open to thick", "A --- B", "thick", "A === B"],
    ["both to dotted", "A <--> B", "dotted", "A <-.-> B"],
    ["both to thick", "A <--> B", "thick", "A <==> B"],
    ["dotted to solid", "A -.-> B", "solid", "A --> B"],
    ["thick to solid", "A ==> B", "solid", "A --> B"],
    ["dotted open to solid", "A -.- B", "solid", "A --- B"],
    ["thick open to dotted", "A === B", "dotted", "A -.- B"],
    [
      "long arrow keeps its length, with a pipe",
      "A --->|x| B",
      "dotted",
      "A -..->|x| B",
    ],
    ["long dotted to thick", "A -..-> B", "thick", "A ===> B"],
    ["long dotted to solid", "A -...-> B", "solid", "A ----> B"],
    ["long open keeps its length", "A ---- B", "dotted", "A -..- B"],
    ["long thick open", "A ==== B", "solid", "A ---- B"],
    ["circle end kept", "A --o B", "dotted", "A -.-o B"],
    ["cross end kept", "A --x B", "thick", "A ==x B"],
    ["both circles kept", "A o--o B", "thick", "A o==o B"],
    ["both crosses kept", "A x--x B", "dotted", "A x-.-x B"],
    ["no spaces", "A-->B", "thick", "A==>B"],
    ["an arrow id stays", "A e1@--> B", "thick", "A e1@==> B"],
    ["invisible to solid gets an arrow", "A ~~~ B", "solid", "A --> B"],
    ["long invisible to solid", "A ~~~~ B", "solid", "A ---> B"],
    ["invisible to dotted", "A ~~~ B", "dotted", "A -.-> B"],
    ["invisible to thick", "A ~~~ B", "thick", "A ==> B"],
    ["text, to dotted", "A -- x --> B", "dotted", "A -. x -.-> B"],
    ["text, to thick", "A -- x --> B", "thick", "A == x ==> B"],
    ["text, open to thick", "A -- x --- B", "thick", "A == x === B"],
    ["text, both to thick", "A <-- x --> B", "thick", "A <== x ==> B"],
    ["text, both to dotted", "A <-- x --> B", "dotted", "A <-. x -.-> B"],
    ["text, dotted to solid", "A -. x .-> B", "solid", "A -- x --> B"],
    ["text, thick long", "A == x ===> B", "solid", "A -- x ---> B"],
    ["text, circles to thick", "A o-- x --o B", "thick", "A o== x ==o B"],
  ])("%s", (_, statement, value, expected) => {
    expect(on(statement, line(value))).toBe(expected);
  });

  test("the line it already has is no edit", () => {
    expect(editCount("A --> B", line("solid"))).toBe(0);
    expect(editCount("A -.-> B", line("dotted"))).toBe(0);
    expect(editCount("A ==> B", line("thick"))).toBe(0);
    expect(editCount("A -- x --> B", line("solid"))).toBe(0);
    expect(editCount("A -. x .-> B", line("dotted"))).toBe(0);
  });

  test.each<[string, string, "solid" | "dotted" | "thick", string]>([
    // A head stands between the line and the id, so the ends stay.
    ["A-->xray", "xray", "thick", "A==>xray"],
    ["A==>oauth", "oauth", "dotted", "A-.->oauth"],
    // Here the x or o already is the head.
    ["A---xray", "ray", "dotted", "A-..-xray"],
    ["A--oauth", "auth", "thick", "A==oauth"],
  ])(
    "next to an id with an o or an x: %s",
    (statement, end, value, expected) => {
      const op: FlowOp = {
        op: "lineStyle",
        edge: { start: "A", end, occurrence: 0 },
        line: value,
      };
      expect(on(statement, op)).toBe(expected);
      const after = parseFlowchart(`flowchart TD\n  ${expected}`).edges[0];
      expect([after?.start, after?.end]).toEqual(["A", end]);
    },
  );

  test("a label that cannot sit in the new line refuses, it is not re-quoted", () => {
    expect(refusal("A -- a.b --> B", line("dotted"))).toBe("invalid");
    expect(refusal("A -- a=b --> B", line("thick"))).toBe("invalid");
  });

  test("a group refuses, an arrow that is not there is gone", () => {
    expect(refusal("A & C --> B", line("thick"))).toBe("group");
    expect(refusal("A --> B & C", line("thick"))).toBe("group");
    expect(refusal("C --> B", line("thick"))).toBe("gone");
  });

  test("one link of a chain", () => {
    expect(on("A --> B --> C", line("thick"))).toBe("A ==> B --> C");
  });
});

describe("arrowhead", () => {
  test.each<[string, string, "arrow" | "none" | "both", string]>([
    ["arrow to both", "A --> B", "both", "A <--> B"],
    ["arrow to none", "A --> B", "none", "A --- B"],
    ["none to arrow", "A --- B", "arrow", "A --> B"],
    ["none to both", "A --- B", "both", "A <--> B"],
    ["both to arrow", "A <--> B", "arrow", "A --> B"],
    ["both to none", "A <--> B", "none", "A --- B"],
    ["dotted, none", "A -.-> B", "none", "A -.- B"],
    ["dotted, both", "A -.-> B", "both", "A <-.-> B"],
    ["thick, none", "A ==> B", "none", "A === B"],
    ["thick, both", "A ==> B", "both", "A <==> B"],
    ["length kept to none", "A ---> B", "none", "A ---- B"],
    ["length kept from none", "A ---- B", "arrow", "A ---> B"],
    ["length kept to both", "A ---> B", "both", "A <---> B"],
    ["dotted length kept", "A -..-> B", "none", "A -..- B"],
    ["thick length kept", "A ===> B", "none", "A ==== B"],
    ["a circle is replaced", "A --o B", "arrow", "A --> B"],
    ["a cross is replaced", "A --x B", "none", "A --- B"],
    ["both circles are replaced", "A o--o B", "both", "A <--> B"],
    ["both crosses to arrow", "A x--x B", "arrow", "A --> B"],
    ["with a pipe label", "A -->|x| B", "both", "A <-->|x| B"],
    ["text, to both", "A -- x --> B", "both", "A <-- x --> B"],
    ["text, to none", "A -- x --> B", "none", "A -- x --- B"],
    ["text, none to arrow", "A -- x --- B", "arrow", "A -- x --> B"],
    ["text, both to arrow", "A <-- x --> B", "arrow", "A -- x --> B"],
    ["text, thick", "A == x ==> B", "both", "A <== x ==> B"],
    [
      "text, dotted keeps its closer's form",
      "A -. x .-> B",
      "none",
      "A -. x .- B",
    ],
    ["text, dotted both", "A -. x .-> B", "both", "A <-. x .-> B"],
    ["text, long", "A -- x ---> B", "none", "A -- x ---- B"],
    ["text, circles", "A o-- x --o B", "arrow", "A -- x --> B"],
  ])("%s", (_, statement, value, expected) => {
    expect(on(statement, head(value))).toBe(expected);
  });

  test("the heads it already has are no edit", () => {
    expect(editCount("A --> B", head("arrow"))).toBe(0);
    expect(editCount("A --- B", head("none"))).toBe(0);
    expect(editCount("A <--> B", head("both"))).toBe(0);
  });

  test("the arrow token trap (proposal 11): A-->xray to none refuses", () => {
    const key = { start: "A", end: "xray", occurrence: 0 };
    expect(parseFlowchart("flowchart TD\n  A---xray").edges[0]?.end).toBe(
      "ray",
    );
    for (const statement of ["A-->xray", "A-.->xray", "A==>xray"]) {
      expect(
        refusal(statement, { op: "arrowhead", edge: key, head: "none" }),
        statement,
      ).toBe("invalid");
    }
    const oauth = { start: "A", end: "oauth", occurrence: 0 };
    expect(
      refusal("A==>oauth", { op: "arrowhead", edge: oauth, head: "none" }),
    ).toBe("invalid");
  });

  test("a token before such an id with a space between applies", () => {
    expect(
      on("A--> xray", {
        op: "arrowhead",
        edge: { start: "A", end: "xray", occurrence: 0 },
        head: "none",
      }),
    ).toBe("A--- xray");
  });

  test("an invisible link has no heads to change", () => {
    expect(editCount("A ~~~ B", head("none"))).toBe(0);
    expect(refusal("A ~~~ B", head("arrow"))).toBe("unsupported");
    expect(refusal("A ~~~ B", head("both"))).toBe("unsupported");
  });

  test("a group refuses, an arrow that is not there is gone", () => {
    expect(refusal("A & C --> B", head("both"))).toBe("group");
    expect(refusal("C --> B", head("both"))).toBe("gone");
  });
});

describe("reverse", () => {
  test.each<[string, string, string]>([
    ["refs keep their bytes", "A[S] --> B", "B --> A[S]"],
    ["both shaped", 'A["a b"]:::x -->|l| B{c}', 'B{c} -->|l| A["a b"]:::x'],
    ["odd spacing", "A   -->   B", "B   -->   A"],
    ["no spaces", "A-->B", "B-->A"],
    ["an arrow id stays on the arrow", "A e1@--> B", "B e1@--> A"],
    ["text label", "A -- x --> B", "B -- x --> A"],
    ["a second statement stays", "A --> B; C --> D", "B --> A; C --> D"],
  ])("%s", (_, statement, expected) => {
    expect(on(statement, REVERSE)).toBe(expected);
  });

  test("a chain and a group refuse", () => {
    expect(refusal("A --> B --> C", REVERSE)).toBe("chain");
    expect(refusal("C --> A --> B", REVERSE)).toBe("chain");
    expect(refusal("A & C --> B", REVERSE)).toBe("group");
    expect(refusal("A --> B & C", REVERSE)).toBe("group");
    expect(refusal("A & C --> B --> D", REVERSE)).toBe("chain");
  });

  test("an arrow that is not there is gone", () => {
    expect(refusal("C --> B", REVERSE)).toBe("gone");
  });

  test("a loop is no edit", () => {
    expect(
      editCount("A --> A", {
        op: "reverse",
        edge: { start: "A", end: "A", occurrence: 0 },
      }),
    ).toBe(0);
  });

  test("a swap that would lex as another link refuses", () => {
    expect(
      refusal("xray---A", {
        op: "reverse",
        edge: { start: "xray", end: "A", occurrence: 0 },
      }),
    ).toBe("invalid");
  });

  test("the linkStyle stays on the arrow", async () => {
    const body =
      "flowchart TD\n  B --> A\n  A --> B\n  linkStyle 1 stroke:#ff0000";
    const result = compileFlowOp(parseFlowchart(body), REVERSE);
    if ("refused" in result) {
      throw new Error(result.refused);
    }
    const after = applyEdits(body, result.edits);
    expect(after).toBe(
      "flowchart TD\n  B --> A\n  B --> A\n  linkStyle 1 stroke:#ff0000",
    );
    const db = await flowDbFor(after);
    expect(db.edges[1]?.style).toContain("stroke:#ff0000");
    expect(db.edges.map((e) => e.id)).toEqual(["L_B_A_0", "L_B_A_2"]);
  });

  test("a box's label and subgraph stay", async () => {
    const body =
      "flowchart TD\n  subgraph s\n    A[S] --> B\n  end\n  subgraph t\n    B --> C\n  end";
    const result = compileFlowOp(parseFlowchart(body), REVERSE);
    if ("refused" in result) {
      throw new Error(result.refused);
    }
    const db = await flowDbFor(applyEdits(body, result.edits));
    expect(db.subgraphs.find((g) => g.id === "s")?.nodes.toSorted()).toEqual([
      "A",
      "B",
    ]);
    expect(db.vertices.find((v) => v.id === "A")?.text).toBe("S");
  });
});

describe("the arrow ops on a document with CRLF", () => {
  test.each<[string, FlowOp, string]>([
    ["A --> B", label("Yes"), "A -->|Yes| B"],
    ["A --> B", line("thick"), "A ==> B"],
    ["A --> B", head("both"), "A <--> B"],
    ["A[S] --> B", REVERSE, "B --> A[S]"],
  ])("%s", (statement, op, expected) => {
    const doc = `flowchart TD\r\n  ${statement}\r\n  B --> C`;
    const state = EditorState.create({
      doc,
      extensions: lineSeparatorFor(doc),
    });
    const body = state.doc.sliceString(0, state.doc.length);
    const result = compileFlowOp(parseFlowchart(body), op);
    if ("refused" in result) {
      throw new Error(result.refused);
    }
    expect(
      docText(
        state.update({ changes: toChanges(0, result.edits, state.lineBreak) })
          .state,
      ),
    ).toBe(`flowchart TD\r\n  ${expected}\r\n  B --> C`);
  });
});
