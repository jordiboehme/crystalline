/**
 * The two hand-drawn table glyphs, held to the contract the icon button calls
 * them under: a 24-unit canvas, a stroke that takes the button's color, and a
 * size and weight that arrive from the caller rather than from the drawing.
 *
 * The bar renders every icon at 16 with a 1.75 stroke, so those two numbers
 * are the ones asserted here: a drawing that baked its own width would look
 * right in isolation and wrong in the only row it ever stands in.
 */

import { render } from "@testing-library/react";
import { describe, expect, test } from "vitest";

import type { IconComponent } from "../components/primitives";
import { DeleteColumnIcon, DeleteRowIcon } from "./tableIcons";

const ICONS: [string, IconComponent][] = [
  ["DeleteColumnIcon", DeleteColumnIcon],
  ["DeleteRowIcon", DeleteRowIcon],
];

/** The glyph as the toolbar draws it: aria-hidden, size 16, stroke 1.75. */
function draw(Icon: IconComponent): SVGSVGElement {
  const { container } = render(
    <Icon aria-hidden="true" size={16} strokeWidth={1.75} />,
  );
  const svg = container.querySelector("svg");
  if (svg === null) {
    throw new Error("the icon drew no svg");
  }
  return svg;
}

/** Every `d` the glyph is made of, in the order it draws them. */
function strokes(svg: SVGSVGElement): string[] {
  return [...svg.querySelectorAll("path")].map(
    (path) => path.getAttribute("d") ?? "",
  );
}

describe("the table delete glyphs", () => {
  for (const [name, Icon] of ICONS) {
    test(`${name} draws on lucide's canvas`, () => {
      const svg = draw(Icon);
      expect(svg.getAttribute("viewBox")).toBe("0 0 24 24");
      expect(svg.getAttribute("stroke")).toBe("currentColor");
      expect(svg.getAttribute("fill")).toBe("none");
      // Round ends and corners, because the row these stand in is lucide's.
      expect(svg.getAttribute("stroke-linecap")).toBe("round");
      expect(svg.getAttribute("stroke-linejoin")).toBe("round");
    });

    test(`${name} takes its size and weight from the caller`, () => {
      const svg = draw(Icon);
      expect(svg.getAttribute("width")).toBe("16");
      expect(svg.getAttribute("height")).toBe("16");
      expect(svg.getAttribute("stroke-width")).toBe("1.75");
    });

    test(`${name} passes aria-hidden through`, () => {
      // The name lives on the button; the drawing must stay out of the
      // accessibility tree rather than announce a second, wordless thing.
      expect(draw(Icon).getAttribute("aria-hidden")).toBe("true");
    });

    test(`${name} draws three bars and a two-stroke X`, () => {
      // Five strokes is the whole idea in a number: three bars for the three
      // slots, two for the cross over the one that goes away. A drawing that
      // lost the cross would still be an icon, and it would say "grid".
      expect(strokes(draw(Icon))).toHaveLength(5);
    });
  }

  test("the two glyphs are different drawings", () => {
    // The one thing a shared trash can could never say: which axis. These are
    // rotations of each other, so nothing but the path data separates them.
    const column = draw(DeleteColumnIcon);
    const row = draw(DeleteRowIcon);
    expect(strokes(column)).not.toEqual(strokes(row));
    expect(column.getAttribute("class")).not.toBe(row.getAttribute("class"));
  });
});
