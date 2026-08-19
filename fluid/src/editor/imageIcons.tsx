/**
 * The eight glyphs the image-format menu is built from: the trigger, the four
 * placements and the three width presets.
 *
 * Drawn here rather than taken from the icon set, for the reason the table
 * glyphs and the paperclip are: they stand in the same row at the same 24-unit
 * canvas with the same round ends, and the set has no mark at all for "this
 * picture floats left inside the prose". The geometry was drawn, rendered and
 * chosen by eye, so every number is pinned in a test - a nudge later should
 * have to be a nudge somebody meant.
 *
 * The family reads on one idea: a frame is the column of prose, and what is
 * inside it is the picture. The placements put the same block in four places
 * with the text lines around it; the widths fill a fraction of the same bar.
 * The trigger is a picture rather than any one of the four answers, because a
 * control that asks which placement must not already be wearing one.
 *
 * Nothing about size or color is decided here. The stroke is `currentColor`
 * and the filled parts take the same color, so a disabled button and a hovered
 * one carry the drawing with them.
 */

import type { ReactElement } from "react";

/** The canvas the whole editor icon family is drawn on. */
const VIEW_BOX = "0 0 24 24";

/** The stem every glyph class in this app hangs off. */
const GLYPH_CLASS = "crystalline-icon";

/** One part of a drawing: a stroked outline, or a solid block of color. */
type Shape =
  | { kind: "path"; d: string }
  | {
      kind: "rect";
      x: number;
      y: number;
      width: number;
      height: number;
      rx: number;
      filled?: true;
    }
  | { kind: "circle"; cx: number; cy: number; r: number; filled?: true };

/** A picture in a frame, with a horizon and a sun: the menu's own trigger. */
const IMAGE_FORMAT_MENU: Shape[] = [
  { kind: "rect", x: 4, y: 5, width: 16, height: 14, rx: 2 },
  { kind: "path", d: "M6.5 15.5l3.5-3.5 2.5 2.5 2-2 3 3" },
  { kind: "circle", cx: 9, cy: 9, r: 1.1, filled: true },
];

/** Centered: a block of its own between two full-width lines of prose. */
const IMAGE_CENTER: Shape[] = [
  { kind: "path", d: "M4 5h16" },
  { kind: "rect", x: 8, y: 8, width: 8, height: 8, rx: 1 },
  { kind: "path", d: "M4 19h16" },
];

/** Full width: the same block, edge to edge. */
const IMAGE_FULL: Shape[] = [
  { kind: "path", d: "M4 5h16" },
  { kind: "rect", x: 4, y: 8, width: 16, height: 8, rx: 1 },
  { kind: "path", d: "M4 19h16" },
];

/** Float left: the picture at the top left, two short lines beside it. */
const IMAGE_FLOAT_LEFT: Shape[] = [
  { kind: "rect", x: 4, y: 5, width: 7, height: 7, rx: 1 },
  { kind: "path", d: "M14 6.5h6" },
  { kind: "path", d: "M14 9.5h6" },
  { kind: "path", d: "M4 15.5h16" },
  { kind: "path", d: "M4 18.5h16" },
];

/** Float right: the same drawing mirrored. */
const IMAGE_FLOAT_RIGHT: Shape[] = [
  { kind: "rect", x: 13, y: 5, width: 7, height: 7, rx: 1 },
  { kind: "path", d: "M4 6.5h6" },
  { kind: "path", d: "M4 9.5h6" },
  { kind: "path", d: "M4 15.5h16" },
  { kind: "path", d: "M4 18.5h16" },
];

/** The bar every width preset fills a share of. */
const WIDTH_FRAME: Shape = {
  kind: "rect",
  x: 4,
  y: 9,
  width: 16,
  height: 6,
  rx: 1,
};

/** A quarter of the column. */
const IMAGE_WIDTH_25: Shape[] = [
  WIDTH_FRAME,
  { kind: "rect", x: 6, y: 11, width: 3, height: 2, rx: 0.5, filled: true },
];

/** Half of it. */
const IMAGE_WIDTH_50: Shape[] = [
  WIDTH_FRAME,
  { kind: "rect", x: 6, y: 11, width: 6, height: 2, rx: 0.5, filled: true },
];

/** Three quarters of it. */
const IMAGE_WIDTH_75: Shape[] = [
  WIDTH_FRAME,
  { kind: "rect", x: 6, y: 11, width: 9.5, height: 2, rx: 0.5, filled: true },
];

interface GlyphProps {
  size?: number | string;
  strokeWidth?: number | string;
  className?: string;
  "aria-hidden"?: boolean | "true" | "false";
}

/** A solid part takes the drawing's color and none of its outline. */
const SOLID = { fill: "currentColor", stroke: "none" } as const;

/** One part of a drawing, as the element it is. */
function part(shape: Shape, index: number): ReactElement {
  const key = String(index);
  if (shape.kind === "path") {
    return <path key={key} d={shape.d} />;
  }
  if (shape.kind === "circle") {
    return (
      <circle
        key={key}
        cx={shape.cx}
        cy={shape.cy}
        r={shape.r}
        {...(shape.filled ? SOLID : {})}
      />
    );
  }
  return (
    <rect
      key={key}
      x={shape.x}
      y={shape.y}
      width={shape.width}
      height={shape.height}
      rx={shape.rx}
      {...(shape.filled ? SOLID : {})}
    />
  );
}

/**
 * The frame every one of these is drawn in, so they differ in their shapes and
 * in nothing else. The defaults describe how a glyph stands on its own; the
 * bar renders them at 16 with a 1.75 stroke, like every other icon in the row.
 */
function Glyph({
  name,
  shapes,
  size = 24,
  strokeWidth = 1.8,
  className,
  "aria-hidden": ariaHidden,
}: GlyphProps & { name: string; shapes: Shape[] }): ReactElement {
  return (
    <svg
      xmlns="http://www.w3.org/2000/svg"
      width={size}
      height={size}
      viewBox={VIEW_BOX}
      fill="none"
      stroke="currentColor"
      strokeWidth={strokeWidth}
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden={ariaHidden}
      className={`${GLYPH_CLASS} ${GLYPH_CLASS}-${name}${
        className === undefined ? "" : ` ${className}`
      }`}
    >
      {shapes.map(part)}
    </svg>
  );
}

/** The trigger: a picture, because the menu asks which shape it should take. */
export function ImageFormatMenuIcon(props: GlyphProps): ReactElement {
  return <Glyph name="image-format" shapes={IMAGE_FORMAT_MENU} {...props} />;
}

/** A block centered in the column. */
export function ImageCenterIcon(props: GlyphProps): ReactElement {
  return <Glyph name="image-center" shapes={IMAGE_CENTER} {...props} />;
}

/** A block filling the column. */
export function ImageFullIcon(props: GlyphProps): ReactElement {
  return <Glyph name="image-full" shapes={IMAGE_FULL} {...props} />;
}

/** A block at the left with prose wrapping around it. */
export function ImageFloatLeftIcon(props: GlyphProps): ReactElement {
  return <Glyph name="image-float-left" shapes={IMAGE_FLOAT_LEFT} {...props} />;
}

/** The same at the right. */
export function ImageFloatRightIcon(props: GlyphProps): ReactElement {
  return (
    <Glyph name="image-float-right" shapes={IMAGE_FLOAT_RIGHT} {...props} />
  );
}

/** A quarter-width bar. */
export function ImageWidth25Icon(props: GlyphProps): ReactElement {
  return <Glyph name="image-width-25" shapes={IMAGE_WIDTH_25} {...props} />;
}

/** A half-width bar. */
export function ImageWidth50Icon(props: GlyphProps): ReactElement {
  return <Glyph name="image-width-50" shapes={IMAGE_WIDTH_50} {...props} />;
}

/** A three-quarter-width bar. */
export function ImageWidth75Icon(props: GlyphProps): ReactElement {
  return <Glyph name="image-width-75" shapes={IMAGE_WIDTH_75} {...props} />;
}
