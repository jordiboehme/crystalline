/**
 * Whether this device can run the station, and at what resolution.
 *
 * The game is for a desktop with a keyboard and a mouse and a WebGL2
 * browser. A device without WebGL2, or one with only a touch screen, gets
 * the C64's `?DEVICE NOT PRESENT ERROR` and a way back instead of a canvas
 * that cannot be played.
 *
 * The backbuffer follows the canvas's CSS size and the display's pixel
 * ratio, capped at 1.5: a Retina MacBook Air at full ratio spends most of
 * its GPU on pixels nobody can tell apart in a room with banded,
 * pixel-sharp light.
 */

/** Why a device is turned away: no WebGL2 at all, or no pointer but a finger. */
export type Refusal = "no-webgl2" | "touch";

/**
 * The reason to refuse the station on a device, or null when it can run.
 * WebGL2 is checked first: a touch device without it is refused for the
 * missing GPU path, which is the harder of the two to change.
 */
export function refusalReason(env: {
  webgl2: boolean;
  coarseOnly: boolean;
}): Refusal | null {
  if (!env.webgl2) return "no-webgl2";
  if (env.coarseOnly) return "touch";
  return null;
}

/**
 * The environment as the browser reports it. Call from an effect or a lazy
 * initialiser, not at import: it reads `window`. `hasWebGL2` is passed in
 * so this module stays free of the `gl/` import and cheap for its test; the
 * demo hands it the probe from `gl/context.ts`. A device is touch-only when
 * its primary pointer is coarse and no pointer at all is fine, so a laptop
 * with a touch screen and a trackpad still plays.
 */
export function detectEnvironment(hasWebGL2: () => boolean): {
  webgl2: boolean;
  coarseOnly: boolean;
} {
  const coarse = window.matchMedia("(pointer: coarse)").matches;
  const fine = window.matchMedia("(any-pointer: fine)").matches;
  return { webgl2: hasWebGL2(), coarseOnly: coarse && !fine };
}

/** The highest device pixel ratio the backbuffer follows. */
const MAX_RATIO = 1.5;

/**
 * The backbuffer size in device pixels for a canvas of `cssWidth` by
 * `cssHeight` CSS pixels on a display of ratio `dpr`, the ratio capped at
 * 1.5 and then multiplied by the render `scale` (1 is full size). Never
 * returns a zero side: a canvas that is not laid out yet still gets a 1 by 1
 * buffer, which the renderer's targets can be made for.
 */
export function backbufferSize(
  cssWidth: number,
  cssHeight: number,
  dpr: number,
  scale: number,
): { width: number; height: number } {
  const ratio = Math.min(dpr, MAX_RATIO) * scale;
  return {
    width: Math.max(1, Math.round(cssWidth * ratio)),
    height: Math.max(1, Math.round(cssHeight * ratio)),
  };
}
