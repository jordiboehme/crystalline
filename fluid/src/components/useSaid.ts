/**
 * The two-second confirmation, in one place.
 *
 * Several controls in this app copy something and then have to say whether it
 * worked. The shape is always the same: a live region that is in the document
 * from the start and empty, text arriving in it, and the text clearing itself
 * shortly after so the region is empty again for the next thing that happens.
 * A region that never empties reads the same sentence back at every later
 * announcement, which is worse than saying nothing.
 *
 * Written out once here rather than in each control, so the three that share
 * the mechanism cannot drift apart on how long it lasts or on whether a
 * refusal clears at all.
 */

import { useEffect, useState } from "react";

/** How long a confirmation stays up, unless a caller asks for otherwise. */
const SAID_FOR_MS = 2000;

/**
 * What is being said right now, and how to say something.
 *
 * `null` is silence. The state IS the text, so saying the same text again
 * while it is still up changes nothing: React sees the same value, the effect
 * does not re-run and the original clock keeps running out. That is what the
 * two controls this was lifted out of already did, and it is the right answer
 * for a live region, which would otherwise repeat a sentence a reader has
 * already been told.
 */
export function useSaid(
  forMs = SAID_FOR_MS,
): [said: string | null, say: (text: string) => void] {
  const [said, setSaid] = useState<string | null>(null);
  useEffect(() => {
    if (said === null) {
      return;
    }
    const timer = setTimeout(() => {
      setSaid(null);
    }, forMs);
    return () => {
      clearTimeout(timer);
    };
  }, [said, forMs]);
  return [said, setSaid];
}
