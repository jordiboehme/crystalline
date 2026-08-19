/**
 * The paperclip the format bar's attach verb wears.
 *
 * Drawn here rather than taken from the icon set for the reason the table
 * glyphs are: this one has to sit in the same row as those, at the same 24-unit
 * canvas with the same round ends, and the set's own clip is a different
 * weight and angle beside them. The geometry is a decision somebody made
 * looking at rendered previews, so the path is pinned in a test - a number
 * nudged later should have to be nudged on purpose.
 *
 * Nothing about size or color is decided here. The stroke is `currentColor`,
 * so a disabled button and a hovered one carry the drawing with them, and the
 * defaults only describe how the glyph stands on its own: the bar renders it
 * at 16 with a 1.75 stroke, like every other icon in the row.
 */

import type { ReactElement } from "react";

/** The canvas the whole editor icon family is drawn on. */
const VIEW_BOX = "0 0 24 24";

/** The stem every glyph class in this app hangs off. */
const GLYPH_CLASS = "crystalline-icon";

/**
 * One clip, one stroke: down the front of the page, round the small return at
 * the bottom, up and over the wide bow, and back down the long side.
 */
const ATTACH_PATH =
  "M17.7 7.1l-7.6 7.6a2.4 2.4 0 003.4 3.4l7.2-7.2a4.4 4.4 0 00-6.2-6.2l-7.2 7.2a6.4 6.4 0 009 9l5.9-5.9";

interface GlyphProps {
  size?: number | string;
  strokeWidth?: number | string;
  className?: string;
  "aria-hidden"?: boolean | "true" | "false";
}

/** A paperclip: something comes with this engram. */
export function AttachIcon({
  size = 24,
  strokeWidth = 1.8,
  className,
  "aria-hidden": ariaHidden,
}: GlyphProps): ReactElement {
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
      className={`${GLYPH_CLASS} ${GLYPH_CLASS}-attach${
        className === undefined ? "" : ` ${className}`
      }`}
    >
      <path d={ATTACH_PATH} />
    </svg>
  );
}
