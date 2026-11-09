// The scanner throwing is forced with a mock: no body under mermaid's text
// limit is known to make it throw, and the parser's promise is that it never
// passes a throw on.

import { describe, expect, test, vi } from "vitest";

import { parseFlowchart } from "./parse";

vi.mock("../scan", async (original) => ({
  ...(await original<typeof import("../scan")>()),
  scanFence: () => {
    throw new RangeError("Maximum call stack size exceeded");
  },
}));

describe("parseFlowchart", () => {
  test("turns a throw from the scanner into a reason", () => {
    const model = parseFlowchart("flowchart TD\n  A");
    expect(model.understood).toBe(false);
    expect(model.reasons.join(" ")).toMatch(/call stack/);
  });
});
