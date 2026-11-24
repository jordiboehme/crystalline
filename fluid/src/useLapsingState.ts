/**
 * State that holds a value only for a while: a token armed for something
 * that should follow soon (the focus a retire hands on once the page shows
 * the new state) and that must not fire much later, when the person has
 * moved on.
 */

import { useEffect, useState } from "react";

/**
 * A value, or null, that goes back to null `lapseMs` after it was set when
 * nothing cleared it first. Setting a new value starts the time again; the
 * timer goes with the component.
 */
export function useLapsingState<T>(
  lapseMs: number,
): [T | null, (value: T | null) => void] {
  const [value, setValue] = useState<T | null>(null);
  useEffect(() => {
    if (value === null) {
      return undefined;
    }
    const timer = setTimeout(() => {
      setValue(null);
    }, lapseMs);
    return () => {
      clearTimeout(timer);
    };
  }, [value, lapseMs]);
  return [value, setValue];
}
