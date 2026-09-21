/**
 * Which way a domain's or a folder's engrams are listed: newest recorded
 * first, oldest first, or by name either way.
 *
 * A frame-level choice like the width, for the same reason: a reader who
 * sorts one listing by name wants the next folder sorted the same way, and
 * the next visit too. It applies to every domain and folder listing,
 * filtered or not, and never to search results, which are ranked by
 * relevance and have no order to choose.
 *
 * Kept out of `Layout.tsx` for the reason `layoutWidth.ts` is: a module that
 * exports a component may export nothing else if fast refresh is to work,
 * and the hook is what the screens import.
 */

import { createContext, use } from "react";

import type { ListingOrder } from "./api/engrams";

/** The four orders, as they are remembered and offered. */
export type EngramsOrder = "newest" | "oldest" | "name-asc" | "name-desc";

/** Every order, in the order the menu offers them. */
export const ENGRAMS_ORDERS: readonly EngramsOrder[] = [
  "newest",
  "oldest",
  "name-asc",
  "name-desc",
];

/**
 * Where the choice is remembered. Anything but one of the four, including
 * nothing at all, reads as newest first.
 */
export const ENGRAMS_ORDER_KEY = "fluid.engrams.order";

/** The order a listing opens on when nobody has chosen: what the domain page opened on before there was a choice. */
export const DEFAULT_ENGRAMS_ORDER: EngramsOrder = "newest";

/** Whether a stored or chosen value names one of the four orders. */
export function isEngramsOrder(value: unknown): value is EngramsOrder {
  return (
    typeof value === "string" &&
    (ENGRAMS_ORDERS as readonly string[]).includes(value)
  );
}

/**
 * How the listing was last left, read once at mount.
 *
 * A browser that refuses storage - a private window with cookies off, an
 * embedded view - is not a reason to fail to draw a listing, so it gets the
 * default and keeps whatever it chooses for the session.
 */
export function storedEngramsOrder(): EngramsOrder {
  try {
    const stored = localStorage.getItem(ENGRAMS_ORDER_KEY);
    return isEngramsOrder(stored) ? stored : DEFAULT_ENGRAMS_ORDER;
  } catch {
    return DEFAULT_ENGRAMS_ORDER;
  }
}

/** What a screen reads: the order that is on, and how to change it. */
export interface EngramsOrderChoice {
  order: EngramsOrder;
  /** Choose another order and remember it. */
  setOrder: (next: EngramsOrder) => void;
}

/**
 * The default is newest first with nothing to press, which is what a screen
 * rendered outside the frame gets: a preview, a test of one panel. There is
 * no wiring bug to throw over here, so this context has a value rather than
 * a null the hook has to guard.
 */
export const EngramsOrderContext = createContext<EngramsOrderChoice>({
  order: DEFAULT_ENGRAMS_ORDER,
  setOrder: () => undefined,
});

/** The order the frame is in, wherever a listing is drawn inside it. */
export function useEngramsOrder(): EngramsOrderChoice {
  return use(EngramsOrderContext);
}

/** The menu's words for an order, and the trigger's. */
export function orderLabel(order: EngramsOrder): string {
  switch (order) {
    case "newest":
      return "Newest first";
    case "oldest":
      return "Oldest first";
    case "name-asc":
      return "Name A to Z";
    case "name-desc":
      return "Name Z to A";
  }
}

/** What the listing request carries for an order. Name means the path. */
export function orderQuery(order: EngramsOrder): ListingOrder {
  switch (order) {
    case "newest":
      return { sort: "recorded", dir: "desc" };
    case "oldest":
      return { sort: "recorded", dir: "asc" };
    case "name-asc":
      return { sort: "path", dir: "asc" };
    case "name-desc":
      return { sort: "path", dir: "desc" };
  }
}
