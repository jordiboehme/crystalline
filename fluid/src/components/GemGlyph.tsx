/**
 * The mark, in its two sizes.
 *
 * One drawing with two callers: the top bar's `ShatterGem` and the login
 * card. The cell is the flat-top hexagon from the banner's lattice, split
 * into three rhombic faces - the lit face upper left, the side face right,
 * the far face lower left - in three stops of the accent ramp so the cube
 * reads on either theme.
 *
 * The trio is the mark itself: three cells tiled the way the banner's own
 * lattice tiles them, which is why the offsets below are the cell's own
 * geometry rather than round numbers. A cell is 21 wide and 18.18 tall at
 * this viewBox; a neighbour to the upper right sits half a height up and
 * three quarters of a width across.
 */
import type { ReactElement } from "react";

/** One cube's three faces, drawn at the origin of a 24 by 24 cell. */
function Cell(): ReactElement {
  return (
    <>
      <polygon
        points="12,12 1.5,12 6.75,2.91 17.25,2.91"
        className="fill-accent-300 dark:fill-accent-200"
      />
      <polygon
        points="12,12 17.25,2.91 22.5,12 17.25,21.09"
        className="fill-accent-500 dark:fill-accent-400"
      />
      <polygon
        points="12,12 17.25,21.09 6.75,21.09 1.5,12"
        className="fill-accent-800 dark:fill-accent-700"
      />
    </>
  );
}

/** The single cell, as the top bar and the favicon draw it. */
export function GemGlyph({ size = 18 }: { size?: number }): ReactElement {
  return (
    <svg
      aria-hidden="true"
      width={size}
      height={size}
      viewBox="0 0 24 24"
      className="shrink-0"
    >
      <Cell />
    </svg>
  );
}

/** Three cells tiled as the mark, for the one screen outside the app frame. */
export function GemTrio({ size = 96 }: { size?: number }): ReactElement {
  return (
    <svg
      aria-hidden="true"
      width={size}
      height={(size * 30) / 45}
      viewBox="0 0 45 30"
      className="shrink-0"
    >
      <g transform="translate(0, 6)">
        <Cell />
      </g>
      <g transform="translate(15.75, -3)">
        <Cell />
      </g>
      <g transform="translate(15.75, 15)">
        <Cell />
      </g>
    </svg>
  );
}
