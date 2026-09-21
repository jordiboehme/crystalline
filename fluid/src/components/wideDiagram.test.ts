/**
 * The contract of the wide-diagram unclamp.
 *
 * Mermaid's default is scale-to-fit: the root arrives as `width="100%"` with an
 * inline `max-width`, which shrinks a wide diagram until its labels are too
 * small to read. A reader who asks for full width gets the drawing's own size
 * back, never less: `width="100%"` with the clamp gone and an inline
 * `min-width` floor. Mermaid's own `max-width` clamp is the one thing that
 * can be in neither picture.
 */

import { describe, expect, it } from "vitest";

import { unclampDiagram } from "./wideDiagram";

function rootTag(markup: string): string {
  return /<svg[^>]*>/.exec(markup)?.[0] ?? "";
}

function widthAttributes(markup: string): string[] {
  return rootTag(markup).match(/\swidth\s*=/g) ?? [];
}

describe("unclampDiagram", () => {
  function style(markup: string): string {
    return /\sstyle\s*=\s*"([^"]*)"/.exec(rootTag(markup))?.[1] ?? "";
  }

  it("fills the column and floors a narrow diagram at its own width", () => {
    const source =
      '<svg viewBox="0 0 600 400" width="100%" style="max-width: 600px;"><g/></svg>';
    const svg = unclampDiagram(source);
    expect(rootTag(svg)).toContain('width="100%"');
    expect(style(svg)).toContain("min-width: 600px");
    expect(style(svg)).not.toContain("max-width");
  });

  it("says the same thing about a diagram past the threshold", () => {
    // One shape for every diagram: the floor is the drawing's own width, so a
    // 1600px one keeps 1600px in a column that cannot hold it and grows with a
    // column that can.
    const source =
      '<svg viewBox="0 0 1600 400" width="100%" style="max-width: 1600px;"><g/></svg>';
    const svg = unclampDiagram(source);
    expect(rootTag(svg)).toContain('width="100%"');
    expect(style(svg)).toContain("min-width: 1600px");
    expect(style(svg)).not.toContain("max-width");
  });

  it("fills the column with no floor when the natural width cannot be read", () => {
    // Nothing here says how wide the drawing is, so there is no number to hold
    // it up with; filling the column is all that is left to mean.
    const source = '<svg width="100%" style="max-width: 600px;"><g/></svg>';
    const svg = unclampDiagram(source);
    expect(rootTag(svg)).toContain('width="100%"');
    expect(style(svg)).not.toContain("min-width");
    expect(style(svg)).not.toContain("max-width");
  });

  it("takes the floor from a width attribute when there is no viewBox", () => {
    const source = '<svg width="1600px" style="max-width: 1600px;"></svg>';
    const svg = unclampDiagram(source);
    expect(rootTag(svg)).toContain('width="100%"');
    expect(style(svg)).toContain("min-width: 1600px");
  });

  it("replaces the width rather than adding a second one", () => {
    const source =
      '<svg viewBox="0 0 600 400" width="100%" style="max-width: 600px;"></svg>';
    expect(widthAttributes(unclampDiagram(source))).toHaveLength(1);
  });

  it("keeps the rest of an inline style and only trades the clamp for a floor", () => {
    const source =
      '<svg viewBox="0 0 600 400" width="100%" style="max-width: 600px; background-color: transparent;"></svg>';
    expect(style(unclampDiagram(source))).toBe(
      "background-color: transparent; min-width: 600px;",
    );
  });

  it("writes a style attribute where the root had none", () => {
    const svg = unclampDiagram('<svg viewBox="0 0 600 400"></svg>');
    expect(style(svg)).toBe("min-width: 600px;");
    expect(rootTag(svg)).toContain('width="100%"');
  });

  it("is idempotent over its own output", () => {
    // A second press of the same button must not stack a second floor.
    const once = unclampDiagram(
      '<svg viewBox="0 0 1600 400" width="100%" style="max-width: 1600px;"></svg>',
    );
    expect(unclampDiagram(once)).toBe(once);
  });

  it("leaves markup with no root svg exactly as it found it", () => {
    expect(unclampDiagram("")).toBe("");
    expect(unclampDiagram("<p>not a diagram</p>")).toBe("<p>not a diagram</p>");
  });

  it("touches the root tag only", () => {
    const source =
      '<svg viewBox="0 0 600 400" width="100%" style="max-width: 600px;">' +
      "<style>.node { max-width: 40px; }</style>" +
      '<svg width="100%" viewBox="0 0 20 20"></svg></svg>';
    const svg = unclampDiagram(source);
    expect(svg).toContain(".node { max-width: 40px; }");
    expect(svg).toContain('<svg width="100%" viewBox="0 0 20 20">');
  });
});
