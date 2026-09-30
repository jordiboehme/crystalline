/**
 * The launch hook: the only game module the main chunk carries.
 *
 * The C64 screen (`components/ShatterGem.tsx`) imports this file and nothing
 * else of the game, so it must import nothing itself: every byte it pulls in
 * would ride in the chunk every visitor downloads. `launch.test.ts` reads
 * this file's own text and pins that it has no `import`.
 *
 * Two things live here. `gamePathOf` turns the Fluid page the screen was
 * opened on into the game path it launches into, the page's own path and
 * search under the `π` prefix (M4 C1); the route reads that path back with
 * `addressOfGameLocation` (`paths.ts`), which turns every page outside the
 * station's table into the airlock, so no mapping is needed here.
 *
 * And the sound context primed inside the launching gesture (M4 C4). Some
 * browsers start an `AudioContext` only inside a user gesture, and the game's
 * own code runs after its lazy chunk arrived, outside it. So the click or
 * key that launches calls `primeAudio()`, which creates the context there
 * and keeps it in this module. The game's mixer borrows it with
 * `takePrimedAudio()`, which lends it without clearing it, and never closes
 * it: in development `<StrictMode>` mounts the route, unmounts it and mounts
 * it again, and a hand-over that cleared the context would leave the second
 * mount with none (F29). The next `primeAudio()` closes the older context
 * before it makes a new one, and a route that refuses the device calls
 * `releasePrimedAudio()`, since nothing will ever borrow it (F38).
 */

/** The prefix every game route sits under, percent-encoded as `history` holds it. */
export const GAME_PREFIX = "/%CF%80";

/**
 * The game path a Fluid location launches into (M4 C1): the prefix, the
 * path, the search.
 *
 * The app's root is the bare prefix, never the prefix with a trailing
 * slash, which is the form `gameRouteOf` writes for the airlock. Every other
 * path is appended as it is, so a deck's `?path=` survives and a page the
 * station has no place for lands in the airlock.
 */
export function gamePathOf(pathname: string, search: string): string {
  return pathname === "/" && search === ""
    ? GAME_PREFIX
    : GAME_PREFIX + pathname + search;
}

/** The context the last launch primed, lent to the mixer until the next prime. */
let primed: AudioContext | null = null;

/**
 * Creates an AudioContext inside the launching gesture (M4 C4), closing an
 * older unclaimed one first.
 *
 * Feature-detected and guarded: without an `AudioContext` (jsdom, an old
 * browser) or when the constructor throws, nothing is primed and the game
 * plays silently. The resume is asked for at once, still inside the
 * gesture, and a refusal of it is ignored: the mixer unlocks again on the
 * next click or key.
 */
export function primeAudio(): void {
  releasePrimedAudio();
  const Ctor = (globalThis as { AudioContext?: new () => AudioContext })
    .AudioContext;
  if (!Ctor) return;
  try {
    primed = new Ctor();
    void primed.resume?.().catch(() => {});
  } catch {
    primed = null;
  }
}

/**
 * The primed context, lent until a new prime replaces it; null when none
 * was primed or it failed.
 *
 * Lent, not handed over: it stays here, so a second mount of the route (the
 * development `<StrictMode>` remount) borrows the same one. The borrower
 * never closes it.
 */
export function takePrimedAudio(): AudioContext | null {
  return primed;
}

/**
 * Closes and forgets the primed context (the route calls it when it refuses
 * the device). Safe to call with none primed.
 */
export function releasePrimedAudio(): void {
  const old = primed;
  primed = null;
  if (old !== null) {
    void old.close().catch(() => {});
  }
}
