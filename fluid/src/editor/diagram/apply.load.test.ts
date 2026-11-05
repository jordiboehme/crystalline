// mermaid failing to load, on the real path: the op cannot be verified, so
// it writes nothing and does not call the diagram broken, and the next try
// works. Forced as in `verdict.load.test.ts`: a mock that fails once and then
// serves a fake mermaid whose db agrees with the result below.

import { afterEach, expect, test, vi } from "vitest";

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
              getClasses: () => new Map(),
              getTooltip: () => undefined,
            },
          }),
      },
    },
  };
});

import { bytes, cleanupMarkViews, markView } from "../../test/markKit";
import { flowKind, runDiagramOp } from "./apply";
import { NOTICES } from "./edits";

afterEach(cleanupMarkViews);

test("an op that cannot be verified writes nothing, says so, and works next time", async () => {
  const text = "```mermaid\nflowchart TD\n  A\n  B\n```\n";
  const view = markView(text, { raw: true });
  const op = { op: "deleteNode", id: "B" } as const;

  const first = await runDiagramOp(view, 0, flowKind, op);
  expect(first).toEqual({ applied: false, refused: "unchecked" });
  expect(bytes(view.state)).toBe(text);
  expect(NOTICES.unchecked).not.toBe(NOTICES.invalid);

  load.fail = false;
  await expect(runDiagramOp(view, 0, flowKind, op)).resolves.toEqual({
    applied: true,
  });
  expect(bytes(view.state)).toBe("```mermaid\nflowchart TD\n  A\n```\n");
});
