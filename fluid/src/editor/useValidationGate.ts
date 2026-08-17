/**
 * The dry-run gate: what `/validate` says about the buffer as it stands, and
 * how many hard errors are blocking a save.
 *
 * Its own module because two things now read it. The editor session owns it
 * for the solo transport, and the collab surface needs the same verdict over a
 * buffer whose saves the session server owns - one gate, one debounce, one
 * last-landed rule, whichever transport the text travels on.
 */

import { useQuery } from "@tanstack/react-query";
import { useEffect, useState } from "react";

import type { ValidateResponse } from "../api/model";
import { validateDocument, validateKey } from "../api/writes";

/**
 * How long a pause in typing waits before a dry-run validate fires - one
 * request per pause, never one per keystroke.
 */
export const VALIDATE_DEBOUNCE_MS = 500;

export interface ValidationGate {
  report: ValidateResponse | null;
  hardErrors: number;
  checking: boolean;
  validationUnavailable: boolean;
}

export function useValidationGate(
  domain: string,
  path: string | null,
  buffer: string,
): ValidationGate {
  // The dry run: a pause in typing, not every keystroke, is what fires it -
  // `debouncedBuffer` only catches up with `buffer` once typing has paused
  // for `VALIDATE_DEBOUNCE_MS`, and it is that settled value which becomes
  // part of the query key.
  const [debouncedBuffer, setDebouncedBuffer] = useState(buffer);
  useEffect(() => {
    const timer = setTimeout(() => {
      setDebouncedBuffer(buffer);
    }, VALIDATE_DEBOUNCE_MS);
    return () => {
      clearTimeout(timer);
    };
  }, [buffer]);
  const validation = useQuery({
    queryKey: validateKey(domain, path, debouncedBuffer),
    queryFn: () =>
      validateDocument({
        content: debouncedBuffer,
        domain,
        ...(path !== null ? { path } : {}),
      }),
  });
  // The server does not re-check these rule families on save, so the gate
  // below is the only enforcement there is - it must never blink open just
  // because a fresh keystroke changed the query key and `validation.data`
  // has nothing for the new key yet. `lastLanded` tracks the most recent
  // verdict that actually arrived, independently of whichever key is
  // currently in flight, and `report` falls back to it whenever the live
  // query has nothing of its own.
  //
  // Tracked beside the query with a plain `useState`, updated during render
  // rather than through `placeholderData: keepPreviousData` - react query's
  // own answer to this exact problem - for two reasons: that import lives in
  // a module the editor's lazy route already shares with several
  // eagerly-loaded ones, and pulling in one more named export from it grew
  // the ENTRY bundle even though the code that calls it never leaves the lazy
  // chunk; and updating it from a `useEffect` (the first shape this took) is
  // exactly the "adjust state when a prop changes" case React's own docs say
  // to do in the render body instead - an effect-scheduled update here would
  // let one extra render slip through on the old, wrong verdict.
  // `seen` is what makes that safe: comparing against the previous render's
  // own `validation.data`/`isError` is what stops this from setting state on
  // every render forever.
  const [lastLanded, setLastLanded] = useState<ValidateResponse | null>(null);
  const [seen, setSeen] = useState({
    data: validation.data,
    isError: validation.isError,
  });
  if (validation.data !== seen.data || validation.isError !== seen.isError) {
    setSeen({ data: validation.data, isError: validation.isError });
    if (validation.data !== undefined) {
      setLastLanded(validation.data);
    } else if (validation.isError) {
      // A settled failure, not a still-in-flight revalidation: nothing
      // kept-previous survives a genuine refusal, so a transport failure
      // reopens the gate exactly as it always has - see `report` below.
      setLastLanded(null);
    }
  }
  // A transport failure never blocks writing - the save path has its own
  // errors - so a failed dry run reads as "nothing to report" rather than as
  // a hard error: `report` falls back to null (through `lastLanded`, cleared
  // above), and `hardErrors` falls back to zero right behind it.
  const report = validation.data ?? lastLanded;
  const hardErrors = report?.errors ?? 0;
  // True from the moment a keystroke outruns the last check that landed, not
  // only while a request is actually in flight - a stale clean report never
  // gets to look current while newer, unverified text sits above it.
  const checking = validation.isFetching || buffer !== debouncedBuffer;
  // The dry run failed outright and there is nothing kept-previous to show
  // for it - a refused or unreachable `/validate`, not an ordinary pause in
  // typing. Saves stay allowed regardless, since `hardErrors` is already 0
  // whenever there is no report to read one from.
  const validationUnavailable =
    report === null && validation.isError && !checking;

  return { report, hardErrors, checking, validationUnavailable };
}
