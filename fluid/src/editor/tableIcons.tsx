/**
 * The glyphs the icon set has no answer for: the two deletes, and the four
 * marks the alignment menu is built from.
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
 * The alignment four are here for the opposite reason: the set does have marks
 * for this, and they say the wrong thing. A stack of ragged lines is text in a
 * paragraph, and what this menu does is set a table COLUMN, so the lines are
 * drawn between the same two uprights the delete glyph stands in - the reader
 * sees the column first and the alignment second, which is the order the verb
 * happens in. The three menu rows shift the short lines left, centre and right
 * inside that frame; the trigger keeps the frame, puts one line through the
 * middle of it and points an arrow at each wall, because a trigger that has to
 * ask which alignment must not already be wearing one of the three answers.
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

/**
 * The two uprights every alignment mark is drawn between: the delete glyph's
 * own outer columns, at the same coordinates, so the family is recognisably
 * one family and the frame never shifts a hair from row to row.
 */
const COLUMN_FRAME = ["M4 4v16", "M20 4v16"];

/**
 * Left: the three lines start at the same wall.
 *
 * The middle line is the long one in all three of these, which is what keeps
 * the block reading as text rather than as a bar chart; the short lines are
 * the ones that move, and moving them is the entire difference between the
 * three drawings. Everything sits inside the frame with room to spare, so at
 * 16 pixels a line end never touches an upright and fuses into it.
 */
const ALIGN_LEFT_STROKES = [...COLUMN_FRAME, "M7 8h8", "M7 12h10", "M7 16h5"];

/** Centre: the same three lines, each hung off the frame's own midline. */
const ALIGN_CENTER_STROKES = [
  ...COLUMN_FRAME,
  "M8.5 8h7",
  "M7 12h10",
  "M9.5 16h5",
];

/** Right: the three lines end at the same wall. */
const ALIGN_RIGHT_STROKES = [...COLUMN_FRAME, "M9 8h8", "M7 12h10", "M12 16h5"];

/**
 * The trigger: one line across the column with an arrow at either end.
 *
 * It names the axis rather than an answer. The old trigger wore the centre
 * mark, which said "align centre" to anyone who did not already know it was a
 * menu, and the menu it opens offers centre as one of three.
 */
const ALIGN_COLUMN_STROKES = [
  ...COLUMN_FRAME,
  "M7.5 12h9",
  "M10 9l-3 3 3 3",
  "M14 9l3 3-3 3",
];

interface GlyphProps {
  size?: number | string;
  strokeWidth?: number | string;
  className?: string;
  "aria-hidden"?: boolean | "true" | "false";
}

/**
 * The frame every one of these is drawn in, so they differ in their strokes
 * and in nothing else. Defaults match the set's own: 24 units, weight 2, which
 * is what an icon drawn outside this bar would get.
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

/** A column whose lines are flush left. */
export function AlignColumnLeftIcon(props: GlyphProps): ReactElement {
  return (
    <Glyph name="align-column-left" strokes={ALIGN_LEFT_STROKES} {...props} />
  );
}

/** A column whose lines are centred. */
export function AlignColumnCenterIcon(props: GlyphProps): ReactElement {
  return (
    <Glyph
      name="align-column-center"
      strokes={ALIGN_CENTER_STROKES}
      {...props}
    />
  );
}

/** A column whose lines are flush right. */
export function AlignColumnRightIcon(props: GlyphProps): ReactElement {
  return (
    <Glyph name="align-column-right" strokes={ALIGN_RIGHT_STROKES} {...props} />
  );
}

/** A column with an arrow to either wall: which way should this one go? */
export function AlignColumnIcon(props: GlyphProps): ReactElement {
  return (
    <Glyph name="align-column" strokes={ALIGN_COLUMN_STROKES} {...props} />
  );
}
