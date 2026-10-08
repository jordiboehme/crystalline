import { describe, expect, test } from "vitest";

import { mermaidConfig } from "./mermaid";

describe("mermaidConfig", () => {
  test("the look and the layout engine are named, never inherited", () => {
    // mermaid 12 moved both defaults: `neo` draws gradient node strokes on a
    // theme that sets `useGradient` (base does), and ELK re-lays out most
    // diagram types. Every engram already holding a diagram drew under the
    // old pair, so both are pinned here rather than left to whichever
    // version is installed.
    for (const scheme of [false, true]) {
      expect(mermaidConfig(scheme)).toMatchObject({
        look: "classic",
        layout: "dagre",
        theme: "base",
      });
    }
  });

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

  test("labels are SVG text, wide enough not to break an identifier", () => {
    // HTML labels sit in a foreignObject sized from a measurement Safari
    // takes before its page zoom, so under zoom the text overflows its node.
    // SVG text scales with the node it sits in.
    for (const scheme of [false, true]) {
      const config = mermaidConfig(scheme);
      expect(config.htmlLabels).toBe(false);
      expect(config.flowchart?.wrappingWidth).toBe(2000);
    }
  });
});
