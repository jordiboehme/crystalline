/**
 * The paperclip, held to the same contract as the table glyphs: a 24-unit
 * canvas, a stroke that takes the button's color, and a size and weight that
 * arrive from the caller rather than from the drawing.
 *
 * The path is pinned character for character. It was drawn, rendered and
 * chosen by eye, so a change to it is a change to a decision somebody made
 * looking at the picture.
 */

import { render } from "@testing-library/react";
import { describe, expect, test } from "vitest";

import { AttachIcon } from "./attachIcon";

/** The approved geometry, exactly as it was signed off. */
const ATTACH_PATH =
  "M17.7 7.1l-7.6 7.6a2.4 2.4 0 003.4 3.4l7.2-7.2a4.4 4.4 0 00-6.2-6.2l-7.2 7.2a6.4 6.4 0 009 9l5.9-5.9";

/** The glyph as the toolbar draws it: aria-hidden, size 16, stroke 1.75. */
function draw(): SVGSVGElement {
  const { container } = render(
    <AttachIcon aria-hidden="true" size={16} strokeWidth={1.75} />,
  );
  const svg = container.querySelector("svg");
  if (!svg) {
    throw new Error("the glyph rendered no svg");
  }
  return svg;
}

describe("AttachIcon", () => {
  test("draws the approved paperclip, one stroke", () => {
    const paths = [...draw().querySelectorAll("path")].map((path) =>
      path.getAttribute("d"),
    );
    expect(paths).toEqual([ATTACH_PATH]);
  });

  test("takes its size and weight from the caller and its color from the button", () => {
    const svg = draw();
    expect(svg.getAttribute("viewBox")).toBe("0 0 24 24");
    expect(svg.getAttribute("width")).toBe("16");
    expect(svg.getAttribute("height")).toBe("16");
    expect(svg.getAttribute("stroke-width")).toBe("1.75");
    expect(svg.getAttribute("stroke")).toBe("currentColor");
    expect(svg.getAttribute("fill")).toBe("none");
    expect(svg.getAttribute("stroke-linecap")).toBe("round");
    expect(svg.getAttribute("stroke-linejoin")).toBe("round");
    expect(svg.getAttribute("aria-hidden")).toBe("true");
  });

  test("stands at the family's own weight when nobody says otherwise", () => {
    const { container } = render(<AttachIcon />);
    const svg = container.querySelector("svg");
    expect(svg?.getAttribute("stroke-width")).toBe("1.8");
    expect(svg?.getAttribute("width")).toBe("24");
  });
});
