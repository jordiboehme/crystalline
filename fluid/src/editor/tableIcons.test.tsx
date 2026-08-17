/**
 * The hand-drawn table glyphs, held to the contract the icon button calls them
 * under: a 24-unit canvas, a stroke that takes the button's color, and a size
 * and weight that arrive from the caller rather than from the drawing.
 *
 * The bar renders every icon at 16 with a 1.75 stroke, so those two numbers
 * are the ones asserted here: a drawing that baked its own width would look
 * right in isolation and wrong in the only row it ever stands in.
 *
 * The path data is pinned stroke for stroke. These were drawn, rendered and
 * chosen by eye, so a number nudged later is a change to a decision somebody
 * made looking at the picture - it should have to be made on purpose.
 */

import { render } from "@testing-library/react";
import { describe, expect, test } from "vitest";

import type { IconComponent } from "../components/primitives";
import {
  AlignColumnCenterIcon,
  AlignColumnIcon,
  AlignColumnLeftIcon,
  AlignColumnRightIcon,
  DeleteColumnIcon,
  DeleteRowIcon,
} from "./tableIcons";

const ICONS: [string, IconComponent][] = [
  ["DeleteColumnIcon", DeleteColumnIcon],
  ["DeleteRowIcon", DeleteRowIcon],
];

/** The alignment family: three menu rows and the trigger that opens them. */
const ALIGN_ICONS: [string, IconComponent][] = [
  ["AlignColumnLeftIcon", AlignColumnLeftIcon],
  ["AlignColumnCenterIcon", AlignColumnCenterIcon],
  ["AlignColumnRightIcon", AlignColumnRightIcon],
  ["AlignColumnIcon", AlignColumnIcon],
];

/** Every glyph this module draws - all of them keep the same frame. */
const ALL_ICONS: [string, IconComponent][] = [...ICONS, ...ALIGN_ICONS];

/** The two uprights every one of these is drawn between. */
const FRAME = ["M4 4v16", "M20 4v16"];

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

describe("the hand-drawn table glyphs", () => {
  for (const [name, Icon] of ALL_ICONS) {
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

    test(`${name} draws five strokes`, () => {
      // Five is the whole idea in a number, whichever family the glyph is in:
      // three bars and a two-stroke cross for a delete, two uprights and three
      // lines of text for an alignment. A drawing that lost one of them would
      // still be an icon, and it would have stopped saying the thing.
      expect(strokes(draw(Icon))).toHaveLength(5);
    });
  }
});

describe("the table delete glyphs", () => {
  test("the two glyphs are different drawings", () => {
    // The one thing a shared trash can could never say: which axis. These are
    // rotations of each other, so nothing but the path data separates them.
    const column = draw(DeleteColumnIcon);
    const row = draw(DeleteRowIcon);
    expect(strokes(column)).not.toEqual(strokes(row));
    expect(column.getAttribute("class")).not.toBe(row.getAttribute("class"));
  });
});

describe("the column-framed alignment glyphs", () => {
  /*
   * The geometry as it was chosen, one test per drawing. The point of the
   * family is that the reader sees a column first and the alignment second,
   * so the two uprights are asserted separately from the lines between them:
   * a glyph that kept its text and lost its frame would pass a bare
   * "five strokes" count and would have left the family.
   */

  const DRAWINGS: [string, IconComponent, string[]][] = [
    [
      "AlignColumnLeftIcon",
      AlignColumnLeftIcon,
      ["M7 8h8", "M7 12h10", "M7 16h5"],
    ],
    [
      "AlignColumnCenterIcon",
      AlignColumnCenterIcon,
      ["M8.5 8h7", "M7 12h10", "M9.5 16h5"],
    ],
    [
      "AlignColumnRightIcon",
      AlignColumnRightIcon,
      ["M9 8h8", "M7 12h10", "M12 16h5"],
    ],
    [
      "AlignColumnIcon",
      AlignColumnIcon,
      ["M7.5 12h9", "M10 9l-3 3 3 3", "M14 9l3 3-3 3"],
    ],
  ];

  for (const [name, Icon, inside] of DRAWINGS) {
    test(`${name} stands its lines in the column frame`, () => {
      expect(strokes(draw(Icon))).toEqual([...FRAME, ...inside]);
    });
  }

  test("the four are four different drawings", () => {
    // Three menu rows plus the trigger above them: the trigger says "which
    // alignment" rather than naming one, so it must not be any of the three.
    const drawn = ALIGN_ICONS.map(([, Icon]) => draw(Icon));
    const paths = drawn.map((svg) => strokes(svg).join(" "));
    const classes = drawn.map((svg) => svg.getAttribute("class"));
    expect(new Set(paths).size).toBe(4);
    expect(new Set(classes).size).toBe(4);
  });

  test("each wears its own name off the set's stem", () => {
    const classOf = (Icon: IconComponent) => draw(Icon).getAttribute("class");
    expect(classOf(AlignColumnLeftIcon)).toBe(
      "crystalline-icon crystalline-icon-align-column-left",
    );
    expect(classOf(AlignColumnCenterIcon)).toBe(
      "crystalline-icon crystalline-icon-align-column-center",
    );
    expect(classOf(AlignColumnRightIcon)).toBe(
      "crystalline-icon crystalline-icon-align-column-right",
    );
    expect(classOf(AlignColumnIcon)).toBe(
      "crystalline-icon crystalline-icon-align-column",
    );
  });
});
