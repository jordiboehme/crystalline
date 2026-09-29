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
export const GAME_LEGEND = `${CLASSIC_KEYS} · WASD MOUSE · F FLUID · I INVERT · M SOUND`;

/**
 * The pause screen's legend: the game's keys, and M, which turns the
 * sound off and on.
 */
export const PAUSE_LEGEND = `${CLASSIC_KEYS} · WASD MOUSE · F FLUID · I INVERT · M SOUND`;

/** Between two items of a legend, where a line may break. */
export const LEGEND_GAP = " · ";

/**
 * A legend a line breaks only between its items: the spaces inside an
 * item (`I INVERT`) become non-breaking, so a narrow screen never puts a
 * key on one line and its word on the next.
 */
export function unbrokenLegend(legend: string): string {
  return legend
    .split(LEGEND_GAP)
    .map((item) => item.replaceAll(" ", "\u00a0"))
    .join(LEGEND_GAP);
}
