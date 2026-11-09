// NO vi.mock("mermaid") here: the verdict asks mermaid 12.2.0's real db.

import { describe, expect, test } from "vitest";

import { defined } from "../../test/assert";
import { flowDbOf } from "./mermaidDb";
import { requestVerdict, verdictFor } from "./verdict";

const FIXTURES = import.meta.glob<string>(
  [
    "../fixtures/diagrams/flowchart/*.mmd",
    "../fixtures/diagrams/fallback/*.mmd",
    "../fixtures/diagrams/sequence/*.mmd",
  ],
  { query: "?raw", import: "default", eager: true },
);

function fixture(prefix: string): string {
  const path = defined(
    Object.keys(FIXTURES).find((key) =>
      key.slice(key.lastIndexOf("/") + 1).startsWith(`${prefix}-`),
    ),
    `fixture ${prefix}`,
  );
  return defined(FIXTURES[path]).replace(/\n$/, "");
}

describe("the diagram verdict", () => {
  test("is undefined until the request settles, then cached", async () => {
    const body = `${fixture("f04")}\n    %% verdict cache probe`;
    expect(verdictFor(body)).toBe(undefined);
    const pending = requestVerdict(body, false);
    expect(verdictFor(body)).toBe(undefined);
    expect(requestVerdict(body, false)).toBe(pending);
    const verdict = await pending;
    expect(verdict.ok).toBe(true);
    expect(verdictFor(body)).toBe(verdict);
    expect(await requestVerdict(body, false)).toBe(verdict);
  });

  test("x04, which mermaid refuses, is not drawable", async () => {
    const verdict = await requestVerdict(fixture("x04"), false);
    expect(verdict.ok).toBe(false);
  });

  test.each(["x01", "x02", "x03"])(
    "%s, which the parser does not understand, is not drawable",
    async (prefix) => {
      const verdict = await requestVerdict(fixture(prefix), false);
      expect(verdict.ok).toBe(false);
      if (!verdict.ok) {
        expect(verdict.reason.length).toBeGreaterThan(0);
      }
    },
  );

  test("f17's domIds map the db domId of step-1 back to step-1", async () => {
    const body = fixture("f17");
    const verdict = await requestVerdict(body, false);
    if (!verdict.ok) {
      throw new Error(verdict.reason);
    }
    expect(verdict.kind).toBe("flowchart");
    const view = await flowDbOf(body, false);
    if ("error" in view) {
      throw new Error("mermaid refused f17");
    }
    const step = defined(view.vertices.find((v) => v.id === "step-1"));
    expect(verdict.domIds.get(step.domId)).toBe("step-1");
    expect(verdict.domIds.size).toBe(3);
  });

  test("a cluster-only vertex gets no dom id (R4)", async () => {
    const verdict = await requestVerdict(fixture("r23"), false);
    if (!verdict.ok) {
      throw new Error(verdict.reason);
    }
    expect([...verdict.domIds.values()]).not.toContain("loaded");
  });

  test("an ellipse is refused before mermaid is asked (R5)", async () => {
    const verdict = await requestVerdict("flowchart LR\n  A(-x-)", false);
    expect(verdict.ok).toBe(false);
  });

  test("a diagram type without an editor is not drawable", async () => {
    const verdict = await requestVerdict('pie\n  "a" : 1', false);
    expect(verdict.ok).toBe(false);
  });

  test("keeps the 256 most recently used bodies", async () => {
    // Refused by the parser, so mermaid is never asked and each settles fast.
    const body = (i: number) => `flowchart LR\n  A(-x-)\n  %% lru ${i}`;
    await requestVerdict(body(0), false);
    for (let i = 1; i < 256; i++) {
      await requestVerdict(body(i), false);
    }
    // A read keeps body 0 alive, so body 1 is now the oldest.
    expect(verdictFor(body(0))).toBeDefined();
    await requestVerdict(body(256), false);
    expect(verdictFor(body(1))).toBe(undefined);
    expect(verdictFor(body(0))).toBeDefined();
    expect(verdictFor(body(256))).toBeDefined();
  });

  test("a duplicate edge id, which mermaid cannot draw, is not drawable", async () => {
    const verdict = await requestVerdict(
      "flowchart TD\n  A_B --> C\n  A --> B_C",
      false,
    );
    expect(verdict.ok).toBe(false);
  });

  test("a real refusal by mermaid's db is a cached verdict", async () => {
    // Understood, and refused by mermaid when it fills its db.
    const body = 'flowchart TD\n  A@{ shape: foo, label: "x" }';
    const verdict = await requestVerdict(body, false);
    expect(verdict.ok).toBe(false);
    if (!verdict.ok) {
      expect(verdict.reason).toMatch(/refuses/);
    }
    expect(verdictFor(body)).toBe(verdict);
    const parseError = await flowDbOf(fixture("x04"), false);
    expect(parseError).not.toHaveProperty("unavailable");
  });

  test("an @{ shape: icon }, which draws no g.node, is not drawable", async () => {
    const verdict = await requestVerdict(
      'flowchart TD\n  A@{ shape: icon, label: "x" }',
      false,
    );
    expect(verdict.ok).toBe(false);
  });

  test("a body past mermaid's text limit is not drawable", async () => {
    const verdict = await requestVerdict(
      `flowchart TD\n  A\n  %% ${"x".repeat(50_000)}`,
      false,
    );
    expect(verdict.ok).toBe(false);
    if (!verdict.ok) {
      expect(verdict.reason).toMatch(/too long/);
    }
  });

  test.each(["s01", "s05", "s07", "s15", "c01"])(
    "%s is a drawable sequence diagram, with its names in domIds",
    async (prefix) => {
      const verdict = await requestVerdict(fixture(prefix), false);
      if (!verdict.ok) {
        throw new Error(verdict.reason);
      }
      expect(verdict.kind).toBe("sequence");
      expect(verdict.domIds.size).toBeGreaterThan(0);
      for (const [dom, name] of verdict.domIds) {
        expect(dom).toBe(name);
      }
    },
  );

  test("y01, which the parser does not understand, is not drawable", async () => {
    const verdict = await requestVerdict(fixture("y01"), false);
    expect(verdict.ok).toBe(false);
    if (!verdict.ok) {
      expect(verdict.reason).toMatch(/half arrow/);
    }
  });

  test("y02, which neither reads, is not drawable", async () => {
    const verdict = await requestVerdict(fixture("y02"), false);
    expect(verdict.ok).toBe(false);
  });

  test("a sequence that mermaid's db refuses is a cached refusal", async () => {
    const body = "sequenceDiagram\n  A->>B: x\n  deactivate A";
    const verdict = await requestVerdict(body, false);
    expect(verdict.ok).toBe(false);
    if (!verdict.ok) {
      expect(verdict.reason).toMatch(/refuses/);
    }
    expect(verdictFor(body)).toBe(verdict);
  });

  test("a sequence that agrees with nothing is refused in words", async () => {
    const verdict = await requestVerdict(
      'sequenceDiagram\n  A->>B: <a href="x">t</a>',
      false,
    );
    expect(verdict.ok).toBe(false);
  });

  test("a sequence past mermaid's text limit is not drawable", async () => {
    const verdict = await requestVerdict(
      `sequenceDiagram\n  A->>B: x\n  %% ${"x".repeat(50_000)}`,
      false,
    );
    expect(verdict.ok).toBe(false);
    if (!verdict.ok) {
      expect(verdict.reason).toMatch(/too long/);
    }
  });

  test("a body that only starts like the keyword goes to neither parser", async () => {
    const verdict = await requestVerdict("sequenceDiagrams\n  A->>B: x", false);
    expect(verdict.ok).toBe(false);
  });
});
