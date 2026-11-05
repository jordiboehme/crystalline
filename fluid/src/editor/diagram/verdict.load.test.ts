// mermaid failing to load is forced with a mock that fails once and then
// serves a fake mermaid whose db agrees with the body below.

import { describe, expect, test, vi } from "vitest";

const load = vi.hoisted(() => ({ fail: true }));

vi.mock("mermaid", () => {
  if (load.fail) {
    throw new Error("Failed to fetch dynamically imported module");
  }
  return {
    default: {
      initialize: () => undefined,
      mermaidAPI: {
        getDiagramFromText: () =>
          Promise.resolve({
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
            },
          }),
      },
    },
  };
});

import { flowDbOf } from "./mermaidDb";
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
