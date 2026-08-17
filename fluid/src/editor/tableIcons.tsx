/**
 * The two glyphs the icon set has no answer for: delete row and delete column.
 *
 * Every other verb in the format bar wears a mark from the set the rest of the
 * app draws from, and these two used to as well - the same trash can twice,
 * because the set offers nothing that says "a column goes away" and nothing at
 * all for the row. One glyph for two verbs said the destructive half and lost
 * the half that actually tells them apart, so the axis lived only in the label.
 *
 * Drawn here instead, on the one idea a reader gets without being taught: three
 * slots side by side and a cross over the middle one - this one of three goes
 * away. The column glyph stands the slots up, the row glyph lays them down, and
 * the cross sits at the near end of the crossed slot in both, so the eye finds
 * it in the same place either way.
 *
 * Sized for the row they stand in: at 16 pixels with a 1.75 stroke, three thin
 * bars and a six-unit cross stay separate, where an outlined grid with a mark
 * inside it turns to grey. Nothing about weight or color is decided here - the
 * caller passes both, and the stroke is `currentColor`, so a disabled button
 * and a hovered one carry the drawing with them.
 */

import type { ReactElement } from "react";

/**
 * The canvas and the pen, matched to the set these stand beside: a 24-unit
 * box, no fill, round ends and corners. A glyph that differed in any of them
 * would read as a foreign object in the row rather than as another button.
 */
const VIEW_BOX = "0 0 24 24";

/** The stem every one of these classes hangs off, the set's own convention. */
const GLYPH_CLASS = "crystalline-icon";

/**
 * Three columns with the middle one crossed out.
 *
 * The outer two run the full height; the middle is cut back to its lower third,
 * which gives the cross a clear field instead of laying two strokes over a
 * third. The numbers are the ones that survived being drawn at 16 pixels: six
 * units of cross, the smallest that still reads as an X rather than as a
 * smudge, and three units of air below it, because at two the round cap of an
 * arm and the cap of the stub close the gap and the whole middle slot fuses
 * into one mark.
 */
const COLUMN_STROKES = [
  "M4 4v16",
  "M20 4v16",
  "M12 13v7",
  "M9 4L15 10",
  "M15 4L9 10",
];

/** The same drawing turned on its side: three rows, the middle one crossed. */
const ROW_STROKES = [
  "M4 4h16",
  "M4 20h16",
  "M13 12h7",
  "M4 9L10 15",
  "M4 15L10 9",
];

interface GlyphProps {
  size?: number | string;
  strokeWidth?: number | string;
  className?: string;
  "aria-hidden"?: boolean | "true" | "false";
}

/**
 * The frame both glyphs are drawn in, so the two differ in their strokes and
 * in nothing else. Defaults match the set's own: 24 units, weight 2, which is
 * what an icon drawn outside this bar would get.
 */
function Glyph({
  name,
  strokes,
  size = 24,
  strokeWidth = 2,
  className,
  "aria-hidden": ariaHidden,
}: GlyphProps & { name: string; strokes: string[] }): ReactElement {
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
      {strokes.map((d) => (
        <path key={d} d={d} />
      ))}
    </svg>
  );
}

/** Three columns, the middle one crossed out: the column goes away. */
export function DeleteColumnIcon(props: GlyphProps): ReactElement {
  return <Glyph name="delete-column" strokes={COLUMN_STROKES} {...props} />;
}

/** Three rows, the middle one crossed out: the row goes away. */
export function DeleteRowIcon(props: GlyphProps): ReactElement {
  return <Glyph name="delete-row" strokes={ROW_STROKES} {...props} />;
}
