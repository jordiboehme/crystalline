/**
 * The engrams whose detail and graph an event must leave alone: the one the
 * editor has open. Its room carries the content itself, and a refetch under
 * a live document would be a second answer to a question the room owns.
 */

import { createContext, use, useEffect } from "react";

export interface IgnoredEngrams {
  has(domain: string, permalink: string): boolean;
  /** Register; the returned function unregisters. */
  add(domain: string, permalink: string): () => void;
}

const key = (domain: string, permalink: string) =>
  `${domain}\u0000${permalink}`;

export function createIgnoredEngrams(): IgnoredEngrams {
  const counts = new Map<string, number>();
  return {
    has: (domain, permalink) => (counts.get(key(domain, permalink)) ?? 0) > 0,
    add: (domain, permalink) => {
      const k = key(domain, permalink);
      counts.set(k, (counts.get(k) ?? 0) + 1);
      return () => {
        const left = (counts.get(k) ?? 1) - 1;
        if (left <= 0) counts.delete(k);
        else counts.set(k, left);
      };
    },
  };
}

export const IgnoredEngramsContext = createContext<IgnoredEngrams>(
  createIgnoredEngrams(),
);

/** Keep this engram's detail and graph out of the stream's reach while mounted. */
export function useIgnoredEngram(domain: string, permalink: string): void {
  const ignored = use(IgnoredEngramsContext);
  useEffect(() => ignored.add(domain, permalink), [ignored, domain, permalink]);
}
