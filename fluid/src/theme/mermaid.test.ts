import { describe, expect, test } from "vitest";

import { mermaidConfig } from "./mermaid";

describe("mermaidConfig", () => {
  test("arrowheadColor is named in both schemes, never derived", () => {
    // base derives arrowheadColor by channel-inverting the background;
    // inverting the dark scheme's #0f172a lands on a warm cream, which is
    // the user-journey wart this pins shut. A named variable always wins
    // the derivation pass.
    expect(mermaidConfig(false).themeVariables).toMatchObject({
      arrowheadColor: "#475569",
    });
    expect(mermaidConfig(true).themeVariables).toMatchObject({
      arrowheadColor: "#64748b",
    });
  });
});
