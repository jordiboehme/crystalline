/**
 * What the reading page says under its title for a minute: the last change
 * the stream announced for the engram it shows. An external store rather
 * than provider state, so one event re-renders the one page that reads its
 * key and nothing else.
 */

import { createContext, use, useSyncExternalStore } from "react";

import type { EngramChange } from "../api/events";

export interface RecentChange {
  /** `moved_away` is what the page at the OLD address sees, with `to` set. */
  kind: "updated" | "moved_here" | "moved_away";
  actor: string | null;
  /** `Date.now()` when the frame arrived. */
  at: number;
  to: string | null;
}

const key = (domain: string, permalink: string) =>
  `${domain}\u0000${permalink}`;

export class RecentChanges {
  private entries = new Map<string, RecentChange>();
  private listeners = new Set<() => void>();

  note(change: EngramChange): void {
    const at = Date.now();
    if (change.kind === "moved" && change.from) {
      this.entries.set(key(change.domain, change.permalink), {
        kind: "moved_here",
        actor: change.actor,
        at,
        to: null,
      });
      this.entries.set(key(change.domain, change.from.permalink), {
        kind: "moved_away",
        actor: change.actor,
        at,
        to: change.permalink,
      });
    } else if (change.kind !== "deleted") {
      this.entries.set(key(change.domain, change.permalink), {
        kind: "updated",
        actor: change.actor,
        at,
        to: null,
      });
    } else {
      this.entries.delete(key(change.domain, change.permalink));
    }
    for (const listener of this.listeners) listener();
  }

  get(domain: string, permalink: string): RecentChange | null {
    return this.entries.get(key(domain, permalink)) ?? null;
  }

  subscribe = (listener: () => void): (() => void) => {
    this.listeners.add(listener);
    return () => {
      this.listeners.delete(listener);
    };
  };
}

export const RecentChangesContext = createContext<RecentChanges>(
  new RecentChanges(),
);

export function useRecentChange(
  domain: string,
  permalink: string,
): RecentChange | null {
  const recent = use(RecentChangesContext);
  return useSyncExternalStore(recent.subscribe, () =>
    recent.get(domain, permalink),
  );
}
