/**
 * Whether the Reflection navigation shows retired pages.
 *
 * Off by default: a retired page is kept for the record, not as part of what
 * a Reflection says now, so the navigation leaves it out. It stays reachable
 * by link and in Reflection search, faded. The choice is remembered in this
 * browser like the other choices of the frame (`fluid.nav`, the width), and
 * shared live by every place the sidebar is drawn (the column and the
 * drawer), so flipping it in one shows in the other at once.
 *
 * A browser that refuses storage gets the default and keeps the choice for
 * the session.
 */

import { useSyncExternalStore } from "react";

export const RETIRED_PAGES_KEY = "fluid.reflection.retired";

const listeners = new Set<() => void>();
let session: boolean | null = null;

function shown(): boolean {
  if (session !== null) {
    return session;
  }
  try {
    return localStorage.getItem(RETIRED_PAGES_KEY) === "shown";
  } catch {
    return false;
  }
}

/** Show or hide retired pages, remembered in this browser. */
export function setRetiredPagesShown(next: boolean): void {
  session = next;
  try {
    if (next) {
      localStorage.setItem(RETIRED_PAGES_KEY, "shown");
    } else {
      localStorage.removeItem(RETIRED_PAGES_KEY);
    }
    // Stored: the store is the truth again, for another tab's change too.
    session = null;
  } catch {
    // Kept for the session only.
  }
  for (const listener of listeners) {
    listener();
  }
}

function subscribe(onChange: () => void): () => void {
  listeners.add(onChange);
  const onStorage = (event: StorageEvent) => {
    if (event.key === RETIRED_PAGES_KEY) {
      onChange();
    }
  };
  window.addEventListener("storage", onStorage);
  return () => {
    listeners.delete(onChange);
    window.removeEventListener("storage", onStorage);
  };
}

export function useRetiredPagesShown(): [boolean, (next: boolean) => void] {
  return [useSyncExternalStore(subscribe, shown), setRetiredPagesShown];
}
