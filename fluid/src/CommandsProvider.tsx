/**
 * The registry the palette's actions live in, held for as long as the app is.
 *
 * The entries are keyed by an owner symbol rather than appended to a list, so
 * a screen that registers again - a re-render with different capabilities, a
 * new engram under the same screen - replaces what it offered instead of
 * doubling it, and unmounting takes exactly its own rows away.
 *
 * They are handed out screens first and the frame last, which is the one
 * ordering registration cannot produce by itself: the frame is mounted before
 * any screen is, and a screen's own actions usually wait on the read that
 * gives them something to act on.
 */

import type { ReactElement, ReactNode } from "react";
import { useCallback, useMemo, useState } from "react";

import { CommandsContext } from "./commands";
import type { CommandScope, PaletteCommand } from "./commands";

interface Entry {
  owner: symbol;
  commands: readonly PaletteCommand[];
  scope: CommandScope;
}

export function CommandsProvider({
  children,
}: {
  children: ReactNode;
}): ReactElement {
  const [entries, setEntries] = useState<Entry[]>([]);
  const register = useCallback(
    (
      owner: symbol,
      commands: readonly PaletteCommand[],
      scope: CommandScope,
    ) => {
      setEntries((current) => [
        ...current.filter((entry) => entry.owner !== owner),
        { owner, commands, scope },
      ]);
    },
    [],
  );
  const unregister = useCallback((owner: symbol) => {
    setEntries((current) => current.filter((entry) => entry.owner !== owner));
  }, []);
  const value = useMemo(
    () => ({
      commands: [...flatten(entries, "screen"), ...flatten(entries, "frame")],
      register,
      unregister,
    }),
    [entries, register, unregister],
  );
  return <CommandsContext value={value}>{children}</CommandsContext>;
}

/** Everything one scope offers, in the order its owners registered. */
function flatten(entries: Entry[], scope: CommandScope): PaletteCommand[] {
  return entries
    .filter((entry) => entry.scope === scope)
    .flatMap((entry) => [...entry.commands]);
}
