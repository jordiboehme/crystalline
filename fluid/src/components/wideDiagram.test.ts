/**
 * The contract of the wide-diagram unclamp.
 *
 * Mermaid's default is scale-to-fit: the root arrives as `width="100%"` with an
 * inline `max-width`, which shrinks a wide diagram until its labels are too
 * small to read. Past the threshold this helper hands the diagram back its own
 * width so its container can scroll instead. Everything below the threshold has
 * to come back untouched, byte for byte, because that is every diagram in the
 * app today.
 */

import { describe, expect, it } from "vitest";

import {
  unclampDiagram,
  unclampWideDiagram,
  WIDE_DIAGRAM_PX,
} from "./wideDiagram";

function rootTag(markup: string): string {
  return /<svg[^>]*>/.exec(markup)?.[0] ?? "";
}

function widthAttributes(markup: string): string[] {
  return rootTag(markup).match(/\swidth\s*=/g) ?? [];
}

describe("unclampWideDiagram", () => {
  it("gives a wide diagram its own width and drops the inline clamp", () => {
    const source =
      '<svg id="d" viewBox="0 0 1600 400" width="100%" style="max-width: 1600px;"><g/></svg>';
    const { svg, wide } = unclampWideDiagram(source);
    expect(wide).toBe(true);
    expect(rootTag(svg)).toContain('width="1600px"');
    expect(rootTag(svg)).not.toContain("max-width");
  });

  it("replaces the width mermaid wrote rather than adding a second one", () => {
    // A root carrying two `width` attributes is honored by the browser at the
    // first, which is mermaid's `100%`: the diagram would look exactly as it
    // does today while a substring assertion passed.
    const source =
      '<svg viewBox="0 0 1600 400" width="100%" style="max-width: 1600px;"></svg>';
    const { svg } = unclampWideDiagram(source);
    expect(widthAttributes(svg)).toHaveLength(1);
    expect(svg).not.toContain('width="100%"');
  });

  it("leaves a diagram under the threshold exactly as it found it", () => {
    const source =
      '<svg viewBox="0 0 900 400" width="100%" style="max-width: 900px;"><g/></svg>';
    expect(unclampWideDiagram(source)).toEqual({ svg: source, wide: false });
  });

  it("counts the threshold itself as wide and one pixel under it as narrow", () => {
    const at = `<svg viewBox="0 0 ${WIDE_DIAGRAM_PX} 400" width="100%"></svg>`;
    const under = `<svg viewBox="0 0 ${WIDE_DIAGRAM_PX - 1} 400" width="100%"></svg>`;
    expect(unclampWideDiagram(at).wide).toBe(true);
    expect(unclampWideDiagram(under)).toEqual({ svg: under, wide: false });
  });

  it("falls back to a width attribute in pixels when there is no viewBox", () => {
    const source = '<svg width="1600px" style="max-width: 1600px;"></svg>';
    const { svg, wide } = unclampWideDiagram(source);
    expect(wide).toBe(true);
    expect(widthAttributes(svg)).toHaveLength(1);
    expect(rootTag(svg)).toContain('width="1600px"');
    expect(rootTag(svg)).not.toContain("max-width");
  });

  it("reads a percentage width as no width at all", () => {
    // `width="100%"` is what mermaid writes, and it says nothing about the
    // natural size. It must not parse as 100 pixels, and it must not parse as
    // anything else either: with no viewBox there is no natural width to read,
    // so the markup comes back untouched. Nobody may "fix" this into
    // accepting percentages.
    const source = '<svg width="100%" style="max-width: 1600px;"></svg>';
    expect(unclampWideDiagram(source)).toEqual({ svg: source, wide: false });
  });

  it("gives up on a malformed viewBox, on no measurement at all and on nothing", () => {
    const malformed = '<svg viewBox="0 0 wide 400" width="100%"></svg>';
    const bare = "<svg><g/></svg>";
    expect(unclampWideDiagram(malformed)).toEqual({
      svg: malformed,
      wide: false,
    });
    expect(unclampWideDiagram(bare)).toEqual({ svg: bare, wide: false });
    expect(unclampWideDiagram("")).toEqual({ svg: "", wide: false });
  });

  it("touches the root tag only", () => {
    const source =
      '<svg viewBox="0 0 1600 400" width="100%" style="max-width: 1600px;">' +
      "<style>.node { max-width: 40px; }</style>" +
      '<svg width="100%" viewBox="0 0 20 20"></svg>' +
      '<rect stroke-width="2"/></svg>';
    const { svg } = unclampWideDiagram(source);
    expect(svg).toContain(".node { max-width: 40px; }");
    expect(svg).toContain('<svg width="100%" viewBox="0 0 20 20">');
    expect(svg).toContain('stroke-width="2"');
  });

  it("keeps the rest of an inline style and only drops the clamp", () => {
    const source =
      '<svg viewBox="0 0 1600 400" width="100%" style="max-width: 1600px; background-color: transparent;"></svg>';
    const { svg } = unclampWideDiagram(source);
    expect(rootTag(svg)).toContain("background-color: transparent");
    expect(rootTag(svg)).not.toContain("max-width");
  });
});

/**
 * The same unclamp with the measurement taken out of it.
 *
 * The measured form answers "is this diagram too wide to read scaled down";
 * this one answers a reader who said "show me this one wide" about a diagram
 * that was never too wide for anything. So the threshold has no part in it:
 * whatever the natural width is, the diagram gets it back.
 */
describe("unclampDiagram", () => {
  it("hands a narrow diagram its own width, threshold or no threshold", () => {
    const source =
      '<svg viewBox="0 0 600 400" width="100%" style="max-width: 600px;"><g/></svg>';
    const svg = unclampDiagram(source);
    expect(rootTag(svg)).toContain('width="600px"');
    expect(rootTag(svg)).not.toContain("max-width");
    // The measured form leaves exactly this markup alone, which is the whole
    // difference between the two.
    expect(unclampWideDiagram(source)).toEqual({ svg: source, wide: false });
  });

  it("falls back to the full column when the natural width cannot be read", () => {
    // Nothing here says how wide the drawing is, so there is no number to
    // hand back; filling the column is what "wide" can still mean.
    const source = '<svg width="100%" style="max-width: 600px;"><g/></svg>';
    const svg = unclampDiagram(source);
    expect(rootTag(svg)).toContain('width="100%"');
    expect(rootTag(svg)).not.toContain("max-width");
  });

  it("replaces the width rather than adding a second one", () => {
    const source =
      '<svg viewBox="0 0 600 400" width="100%" style="max-width: 600px;"></svg>';
    expect(widthAttributes(unclampDiagram(source))).toHaveLength(1);
  });

  it("is idempotent, so a wide diagram unclamped twice is unchanged", () => {
    // The toggle runs over markup the measured path may already have rewritten.
    const once = unclampWideDiagram(
      '<svg viewBox="0 0 1600 400" width="100%" style="max-width: 1600px;"></svg>',
    ).svg;
    expect(unclampDiagram(once)).toBe(once);
  });

  it("leaves markup with no root svg exactly as it found it", () => {
    expect(unclampDiagram("")).toBe("");
    expect(unclampDiagram("<p>not a diagram</p>")).toBe("<p>not a diagram</p>");
  });
});
