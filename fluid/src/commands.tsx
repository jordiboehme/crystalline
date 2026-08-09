/**
 * The palette's action registry. A screen offers what can be done HERE by
 * registering commands while it is mounted; the palette lists them above its
 * navigation groups. Registration is context state rather than an event bus:
 * commands unregister with their screen, so a stale action can never outlive
 * the place it acted on.
 *
 * A screen registers what it would offer a pointer, gated the same way: an
 * action a session may not run is never registered, so the palette offers no
 * door that will not open.
 *
 * The provider that fills this in lives next door in `CommandsProvider.tsx`,
 * for the reason `AuthContext` and `AuthProvider` are two files: a module that
 * exports a component may export nothing else if fast refresh is to work, and
 * the hooks are what the screens import.
 */

import { createContext, use, useEffect, useState } from "react";

export interface PaletteCommand {
  /** Stable id, also the cmdk value (namespaced "action:<id>"). */
  id: string;
  title: string;
  run: () => void;
}

/**
 * What a screen registers when it has nothing to offer.
 *
 * One frozen array rather than a fresh `[]` per render: the registry effect
 * keys off the array's identity, and a new empty array every render would
 * re-register on every render forever.
 */
export const NO_COMMANDS: readonly PaletteCommand[] = Object.freeze([]);

/**
 * Who is offering: the screen a reader is looking at, or the frame around
 * every screen.
 *
 * It exists because registration order cannot answer the question on its own.
 * The frame mounts first and a screen's actions often wait on the data they
 * act on, so "in the order they registered" would put the app-wide row above
 * whatever the reader is actually looking at, and Enter on a freshly opened
 * palette would run the wrong one.
 */
export type CommandScope = "screen" | "frame";

/** What the provider holds, and the two ways an owner changes it. */
export interface Registry {
  commands: PaletteCommand[];
  register: (
    owner: symbol,
    commands: readonly PaletteCommand[],
    scope: CommandScope,
  ) => void;
  unregister: (owner: symbol) => void;
}

export const CommandsContext = createContext<Registry | null>(null);

/** The registry itself. Throws outside a provider, which is a wiring bug. */
function useRegistry(): Registry {
  const value = use(CommandsContext);
  if (!value) {
    throw new Error("commands were used outside a CommandsProvider");
  }
  return value;
}

/**
 * Register while mounted; unregisters on unmount. Screens come before the
 * frame, and within a scope the order is the order they registered in.
 */
export function useRegisterCommands(
  commands: readonly PaletteCommand[],
  scope: CommandScope = "screen",
): void {
  const { register, unregister } = useRegistry();
  const [owner] = useState(() => Symbol("commands"));
  useEffect(() => {
    register(owner, commands, scope);
    return () => {
      unregister(owner);
    };
    // The caller memoizes `commands`; identity is the dependency on purpose.
  }, [owner, commands, scope, register, unregister]);
}

/** What is currently registered, for the palette. */
export function usePaletteCommands(): PaletteCommand[] {
  return useRegistry().commands;
}
