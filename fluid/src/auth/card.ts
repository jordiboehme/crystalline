/**
 * The two treatments the login card's fields wear.
 *
 * The card holds two different forms now - credentials, and the first-run
 * setup that only an instance with no accounts ever shows - and they have to
 * look like the same screen, because they are: one route, one card, one moment
 * of standing at the door. Held here rather than in either form so neither can
 * drift, and outside both component modules so fast refresh stays happy.
 *
 * These are the card's own sizes, deliberately larger than `primitives`' `FIELD`
 * (which is the in-app control height): this is the one screen with nothing
 * else on it.
 */

import { FOCUS_RING } from "../components/primitives";

/** One text input on the login card. */
export const CARD_FIELD = `rounded border border-slate-300 bg-white px-3 py-2 text-slate-900 outline-none dark:border-slate-700 dark:bg-slate-900 dark:text-slate-100 ${FOCUS_RING}`;

/** The label above one of them. */
export const CARD_LABEL =
  "text-sm font-medium text-slate-700 dark:text-slate-300";
