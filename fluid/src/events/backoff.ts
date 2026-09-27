/**
 * How long the change stream waits before it reopens a source the browser
 * closed for good. A closed source is often a server restarting behind a
 * proxy or the stream cap turning a tab away, so the tab tries again, a
 * little later each time, instead of hammering the server or giving up.
 */

/** The first wait before a closed stream is reopened, doubled per failure. */
export const RECONNECT_BASE_MS = 1_000;

/** The longest wait between two attempts, jitter included. */
export const RECONNECT_MAX_MS = 30_000;

/**
 * The wait before attempt `attempt` (0 for the first): 1, 2, 4 ... seconds,
 * each spread by up to a fifth either way so a restart does not bring every
 * tab back in the same second, and never past the cap.
 */
export function reconnectDelay(
  attempt: number,
  random: () => number = Math.random,
): number {
  const base = RECONNECT_BASE_MS * 2 ** Math.min(attempt, 16);
  const jittered = base * (0.8 + 0.4 * random());
  return Math.min(RECONNECT_MAX_MS, Math.round(jittered));
}
