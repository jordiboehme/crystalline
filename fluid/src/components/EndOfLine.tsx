/**
 * The sign-off at the bottom of a list a reader scrolled to the end of.
 *
 * Twelve characters in the READY voice - monospace, letter-spaced, the C64's
 * light blue - typed out with a block cursor that keeps blinking after the
 * last one. Decoration, so it is hidden from the accessibility tree: a screen
 * reader that reached the end of the list knows it did. It is drawn by the
 * engram list alone and only where the list had to scroll; a list that fit
 * its box ends where it ends.
 */

import type { ReactElement } from "react";

export default function EndOfLine(): ReactElement {
  return (
    <p
      aria-hidden="true"
      className="flex justify-center px-4 pt-6 pb-4 font-mono text-[#6c5eb5] dark:text-[#7c70da]"
    >
      <span className="end-of-line">End of line.</span>
    </p>
  );
}
