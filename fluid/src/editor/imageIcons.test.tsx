/**
 * The eight image-format glyphs, pinned shape for shape.
 *
 * Every number below is a decision somebody made looking at the rendered
 * picture, so this suite is not a test of svg - it is the record of what was
 * approved. A glyph that drifts fails here rather than quietly redrawing
 * itself in somebody's toolbar.
 */

import { render } from "@testing-library/react";
import type { ReactElement } from "react";
import { describe, expect, test } from "vitest";

import {
  ImageCenterIcon,
  ImageFloatLeftIcon,
  ImageFloatRightIcon,
  ImageFormatMenuIcon,
  ImageFullIcon,
  ImageWidth25Icon,
  ImageWidth50Icon,
  ImageWidth75Icon,
} from "./imageIcons";

/** One drawn part, as the attributes that decide where it sits. */
type Part = Record<string, string | null>;

/** Every part of a glyph, in the order it is drawn. */
function parts(glyph: ReactElement): Part[] {
  const { container } = render(glyph);
  const svg = container.querySelector("svg");
  if (!svg) {
    throw new Error("the glyph rendered no svg");
  }
  return [...svg.children].map((node) => {
    const read = (name: string) => node.getAttribute(name);
    if (node.tagName === "path") {
      return { kind: "path", d: read("d") };
    }
    if (node.tagName === "circle") {
      return {
        kind: "circle",
        cx: read("cx"),
        cy: read("cy"),
        r: read("r"),
        fill: read("fill"),
        stroke: read("stroke"),
      };
    }
    return {
      kind: "rect",
      x: read("x"),
      y: read("y"),
      width: read("width"),
      height: read("height"),
      rx: read("rx"),
      fill: read("fill"),
      stroke: read("stroke"),
    };
  });
}

/** An outline: it takes the glyph's own stroke and no fill of its own. */
function outline(rest: Part): Part {
  return { fill: null, stroke: null, ...rest };
}

/** A solid part: the drawing's color, and no outline. */
function solid(rest: Part): Part {
  return { fill: "currentColor", stroke: "none", ...rest };
}

describe("the image-format glyphs", () => {
  test("the trigger is a framed picture with a sun over a horizon", () => {
    expect(parts(<ImageFormatMenuIcon />)).toEqual([
      outline({
        kind: "rect",
        x: "4",
        y: "5",
        width: "16",
        height: "14",
        rx: "2",
      }),
      { kind: "path", d: "M6.5 15.5l3.5-3.5 2.5 2.5 2-2 3 3" },
      solid({ kind: "circle", cx: "9", cy: "9", r: "1.1" }),
    ]);
  });

  test("centered and full share a frame of prose and differ in the block", () => {
    expect(parts(<ImageCenterIcon />)).toEqual([
      { kind: "path", d: "M4 5h16" },
      outline({
        kind: "rect",
        x: "8",
        y: "8",
        width: "8",
        height: "8",
        rx: "1",
      }),
      { kind: "path", d: "M4 19h16" },
    ]);
    expect(parts(<ImageFullIcon />)).toEqual([
      { kind: "path", d: "M4 5h16" },
      outline({
        kind: "rect",
        x: "4",
        y: "8",
        width: "16",
        height: "8",
        rx: "1",
      }),
      { kind: "path", d: "M4 19h16" },
    ]);
  });

  test("the floats are one drawing mirrored", () => {
    expect(parts(<ImageFloatLeftIcon />)).toEqual([
      outline({
        kind: "rect",
        x: "4",
        y: "5",
        width: "7",
        height: "7",
        rx: "1",
      }),
      { kind: "path", d: "M14 6.5h6" },
      { kind: "path", d: "M14 9.5h6" },
      { kind: "path", d: "M4 15.5h16" },
      { kind: "path", d: "M4 18.5h16" },
    ]);
    expect(parts(<ImageFloatRightIcon />)).toEqual([
      outline({
        kind: "rect",
        x: "13",
        y: "5",
        width: "7",
        height: "7",
        rx: "1",
      }),
      { kind: "path", d: "M4 6.5h6" },
      { kind: "path", d: "M4 9.5h6" },
      { kind: "path", d: "M4 15.5h16" },
      { kind: "path", d: "M4 18.5h16" },
    ]);
  });

  test("the widths fill a share of one bar", () => {
    const frame = outline({
      kind: "rect",
      x: "4",
      y: "9",
      width: "16",
      height: "6",
      rx: "1",
    });
    expect(parts(<ImageWidth25Icon />)).toEqual([
      frame,
      solid({
        kind: "rect",
        x: "6",
        y: "11",
        width: "3",
        height: "2",
        rx: "0.5",
      }),
    ]);
    expect(parts(<ImageWidth50Icon />)).toEqual([
      frame,
      solid({
        kind: "rect",
        x: "6",
        y: "11",
        width: "6",
        height: "2",
        rx: "0.5",
      }),
    ]);
    expect(parts(<ImageWidth75Icon />)).toEqual([
      frame,
      solid({
        kind: "rect",
        x: "6",
        y: "11",
        width: "9.5",
        height: "2",
        rx: "0.5",
      }),
    ]);
  });

  test("size and weight come from the caller, color from the button", () => {
    const { container } = render(
      <ImageFormatMenuIcon aria-hidden="true" size={16} strokeWidth={1.75} />,
    );
    const svg = container.querySelector("svg");
    expect(svg?.getAttribute("viewBox")).toBe("0 0 24 24");
    expect(svg?.getAttribute("width")).toBe("16");
    expect(svg?.getAttribute("stroke-width")).toBe("1.75");
    expect(svg?.getAttribute("stroke")).toBe("currentColor");
    expect(svg?.getAttribute("fill")).toBe("none");
    expect(svg?.getAttribute("stroke-linecap")).toBe("round");
    expect(svg?.getAttribute("stroke-linejoin")).toBe("round");
    expect(svg?.getAttribute("aria-hidden")).toBe("true");
  });

  test("the family stands at its own weight when nobody says otherwise", () => {
    const { container } = render(<ImageCenterIcon />);
    expect(container.querySelector("svg")?.getAttribute("stroke-width")).toBe(
      "1.8",
    );
  });
});
