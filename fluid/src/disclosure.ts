/**
 * A section that remembers whether it was open.
 *
 * The folded sections on a reading page are second ways of reading it, and
 * somebody who wants one of them wants it on every engram they open, not once.
 * The choice therefore outlives the screen it was made on and the session too,
 * under a key of the section's own.
 *
 * Closed is the default, so a first visit still pays for nothing it did not
 * ask for. A browser that refuses storage - a private window with cookies off,
 * an embedded view - is not a reason to fail to draw the section: it gets the
 * default and keeps whatever it chooses for the session.
 */

import { useState } from "react";

/** What a remembered-open section writes down. */
const OPEN = "open";

/** Whether this section was left open, defaulting to closed. */
function storedOpen(key: string): boolean {
  try {
    return localStorage.getItem(key) === OPEN;
  } catch {
    return false;
  }
}

/**
 * The open state of one remembered section, and the toggle that writes it
 * down. Read in a lazy initializer, so an open section is open on the first
 * paint rather than after one.
 */
export function useRememberedDisclosure(key: string): [boolean, () => void] {
  const [open, setOpen] = useState(() => storedOpen(key));
  const toggle = () => {
    setOpen((was) => {
      const next = !was;
      try {
        localStorage.setItem(key, next ? OPEN : "closed");
      } catch {
        // A browser that refuses storage still gets the session's choice.
      }
      return next;
    });
  };
  return [open, toggle];
}
