/**
 * The look a dev page starts in: the game always runs in Aperture grid and
 * has no key to switch looks, so the look demo and the model gallery pick
 * one through `?look=` instead.
 */

import { LOOK_ORDER, LOOKS, type LookId } from "../render/looks";

/**
 * The look `?look=` names by its id (any id in `LOOK_ORDER`), or
 * `aperture`, the game's own, when it is absent or names no look.
 */
export function lookParam(params: URLSearchParams): LookId {
  const raw = params.get("look");
  return LOOK_ORDER.find((id) => id === raw) ?? "aperture";
}

/** The look's name as a dev page's legend shows it: `APERTURE GRID`. */
export function lookLabel(id: LookId): string {
  return LOOKS[id].name.toUpperCase();
}
