/**
 * The key hints on the HUD's legend, shared by every station screen so
 * they name the same keys the session reads (`session.ts`).
 *
 * The classic layout comes first, as the spec's controls section asks;
 * WASD and the mouse follow beside it.
 */

/** The classic keys: the arrows, Alt, Space and Shift. */
export const CLASSIC_KEYS = "ARROWS MOVE · ALT STRAFE · SPACE USE · SHIFT RUN";

/** The game route's legend: the classic keys, then the rest. */
export const GAME_LEGEND = `${CLASSIC_KEYS} · WASD MOUSE · F FLUID · I INVERT`;
