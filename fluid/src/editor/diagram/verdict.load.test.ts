// mermaid failing to load is forced with a mock that fails once and then
// serves a fake mermaid whose db agrees with the body below.

import { describe, expect, test, vi } from "vitest";

interface Load {
  fail: boolean;
  /** What the next `getDiagramFromText` rejects with, once. */
  rejectOnce: Error | null;
  reads: number;
}

const load = vi.hoisted((): Load => ({
  fail: true,
  rejectOnce: null,
  reads: 0,
}));

vi.mock("mermaid", () => {
  if (load.fail) {
    throw new Error("Failed to fetch dynamically imported module");
  }
  return {
    default: {
      initialize: () => undefined,
      mermaidAPI: {
        getDiagramFromText: () => {
          load.reads++;
          if (load.rejectOnce !== null) {
            const error = load.rejectOnce;
            load.rejectOnce = null;
            return Promise.reject(error);
          }
          return Promise.resolve({
            type: "flowchart-v2",
            db: {
              getDirection: () => "TB",
              getVertices: () =>
                new Map([
                  [
                    "A",
                    {
                      id: "A",
                      domId: "flowchart-A-0",
                      text: "A",
                      classes: [],
                      styles: [],
                    },
                  ],
                ]),
              getEdges: () => [],
              getSubGraphs: () => [],
              getClasses: () => new Map(),
              getTooltip: () => undefined,
            },
          });
        },
      },
    },
  };
});

import { flowDbOf, isImportFailure } from "./mermaidDb";
import { requestVerdict, verdictFor } from "./verdict";

describe("mermaid failing to load", () => {
  test("is told apart from mermaid refusing a body", async () => {
    const view = await flowDbOf("flowchart TD\n  A", false);
    expect(view).toMatchObject({ unavailable: true });
  });

  test("is no verdict: not cached, named, and a later request tries again", async () => {
    const body = "flowchart TD\n  A";
    const first = await requestVerdict(body, false);
    expect(first.ok).toBe(false);
    if (!first.ok) {
      expect(first.reason).toMatch(/could not be loaded/);
      // vitest wraps the factory's error, so the message's text is its own.
      expect(first.reason).not.toMatch(/refuses/);
    }
    expect(verdictFor(body)).toBe(undefined);

    load.fail = false;
    const second = await requestVerdict(body, false);
    expect(second.ok).toBe(true);
    expect(verdictFor(body)).toBe(second);
  });
});

describe("the diagram chunk failing to load inside mermaid", () => {
  test.each([
    "Failed to fetch dynamically imported module: https://x/flowDiagram.js",
    "error loading dynamically imported module: https://x/flowDiagram.js",
    "Importing a module script failed.",
  ])(
    "%s is no verdict: not cached, and a later request tries again",
    async (message) => {
      load.fail = false;
      const body = `flowchart TD\n  A\n  %% ${message}`;
      load.rejectOnce = new TypeError(message);
      const first = await requestVerdict(body, false);
      expect(first.ok).toBe(false);
      if (!first.ok) {
        expect(first.reason).toMatch(/could not be loaded/);
      }
      expect(verdictFor(body)).toBe(undefined);
      const second = await requestVerdict(body, false);
      expect(second.ok).toBe(true);
      expect(verdictFor(body)).toBe(second);
    },
  );

  test("the sequence chunk failing is no verdict either", async () => {
    load.fail = false;
    const body = "sequenceDiagram\n  A->>B: x";
    load.rejectOnce = new TypeError(
      "Failed to fetch dynamically imported module: https://x/sequenceDiagram.js",
    );
    const first = await requestVerdict(body, false);
    expect(first.ok).toBe(false);
    if (!first.ok) {
      expect(first.reason).toMatch(/could not be loaded/);
    }
    expect(verdictFor(body)).toBe(undefined);
  });

  test("a refusal from the parse is a cached verdict, even one quoting those words", async () => {
    load.fail = false;
    const body = "flowchart TD\n  B";
    // mermaid's parser puts `hash` on its errors and quotes the source.
    load.rejectOnce = Object.assign(
      new Error("Parse error: Failed to fetch dynamically imported module"),
      { hash: { line: 1 } },
    );
    const first = await requestVerdict(body, false);
    expect(first.ok).toBe(false);
    if (!first.ok) {
      expect(first.reason).toMatch(/refuses/);
    }
    expect(verdictFor(body)).toBe(first);
    const reads = load.reads;
    await requestVerdict(body, false);
    expect(load.reads).toBe(reads);
  });

  test("any other error from mermaid is a refusal", async () => {
    load.fail = false;
    const body = "flowchart TD\n  C";
    load.rejectOnce = new Error("No such shape: foo.");
    const verdict = await requestVerdict(body, false);
    expect(verdict.ok).toBe(false);
    expect(verdictFor(body)).toBe(verdict);
  });

  test("isImportFailure reads errors of any type", () => {
    expect(isImportFailure("Failed to fetch dynamically imported module")).toBe(
      false,
    );
    expect(isImportFailure(new Error("Unable to preload CSS for /x.css"))).toBe(
      true,
    );
    expect(isImportFailure(new TypeError("Cannot read properties"))).toBe(
      false,
    );
    expect(isImportFailure(null)).toBe(false);
  });
});
