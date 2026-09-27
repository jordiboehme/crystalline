/**
 * A mover's per-draw uniforms: the pure half of the renderer's mover loop,
 * so what a door, lamp, hatch lid, portal disc, police box door or rotor
 * is drawn with is tested without a GL context.
 *
 * Every way's moving parts come back from its model as movers (`Mover` in
 * `models/common.ts`): leaves keyed `door:<i>`, and `spark:<i>`,
 * `lamp:<i>`, `lid:<i>` and `disc:<i>`. Each one is drawn with its slide
 * (`axis * travel`) times an open fraction, a scale about its pivot, a turn
 * about its pivot's vertical (`yaw`), a gain and a swirl time. Leaves take the
 * door's fraction from `DoorState` unless a fault frame names their fixture.
 * A lid takes only the frame's.
 * Sparks are drawn only while a frame lights them. The lamp glows at its
 * rest gain and blinks brighter while a frame lights it. A disc scales,
 * flickers and shifts its swirl with the frame, and is skipped at scale 0.
 * A disc's frame gain is scaled by its rest gain, so a sealed disc's run
 * flickers about its dim rest glow and starts and ends with no jump.
 * A wing (a police box's door leaf) turns about its hinge by its swing
 * times its door's fraction, clamped to 0..1; no fault frame names it. The
 * rotor slides by its slide times `rotorPhase`, a cosine ease over
 * `ROTOR_PERIOD` seconds that starts and ends at rest, whatever the doors
 * and frames say. Every part but a wing is drawn at yaw 0, and
 * `swungPoint` is the shader's turn on the CPU, so the tests can follow a
 * wing's points to where the GPU draws them.
 * A null result means the part is not drawn this frame.
 *
 * `restDraw` is the identity every other draw uses: the static room and
 * the prop and hero instances are drawn with it, so the scene shader's
 * pivot, scale, gain and offset change nothing for them. A hero's lights
 * blink through its blink channel (`uBlink`, H11), never through `uGain`,
 * which stays 1 for every instance.
 *
 * Only the fault frame's type is read from `world/malfunction.ts`: the
 * renderer takes the frames the session hands it and never runs the clock.
 */

import type { FaultFrame } from "../world/malfunction";
import type { V3 } from "./geometry";
import type { MoverPart } from "./models/common";

/**
 * What `moverDraw` reads of a mover: its part, its slide (`axis *
 * travel`, the offset when fully open, or at the rotor's top), its pivot
 * (a disc's centre or a wing's hinge in world metres, null for every
 * other part), its rest gain and its swing (the radians a wing turns
 * when fully open, 0 for every other part).
 */
export interface MoverInfo {
  part: MoverPart;
  slide: V3;
  pivot: V3 | null;
  rest: number;
  swing: number;
}

/**
 * One draw's uniforms: `uModelOffset`, `uModelPivot`, `uModelScale`,
 * `uGain`, `uModelYaw` and `uTime`, in that order of the fields. `yaw` is
 * the turn in radians about the vertical through `pivot`, uploaded as its
 * cosine and sine.
 */
export interface MoverDraw {
  offset: V3;
  pivot: V3;
  scale: number;
  gain: number;
  yaw: number;
  time: number;
}

/**
 * How long, in seconds, the console room's rotor takes to rise to its top
 * and fall back to rest.
 */
export const ROTOR_PERIOD = 4;

/**
 * Where the rotor is in its rise and fall at `seconds`: 0 at rest, 1 at
 * the top, `0.5 - 0.5 * cos(2 pi seconds / ROTOR_PERIOD)`. A cosine ease,
 * so it is at rest at 0, slows to a stop at both ends and never leaves
 * 0..1.
 */
export function rotorPhase(seconds: number): number {
  return 0.5 - 0.5 * Math.cos((2 * Math.PI * seconds) / ROTOR_PERIOD);
}

/**
 * Point `p` turned by `yaw` radians about the vertical through `pivot`:
 * the scene shader's `uModelYaw` turn on the CPU, the same numbers as
 * `mat2(c, -s, s, c) * vec2(dx, dz)` in GLSL's column order. Its height
 * is unchanged. A positive yaw turns a point east of the pivot to its
 * north (`-z`).
 */
export function swungPoint(p: V3, pivot: V3, yaw: number): V3 {
  const dx = p[0] - pivot[0];
  const dz = p[2] - pivot[2];
  const c = Math.cos(yaw);
  const s = Math.sin(yaw);
  return [pivot[0] + c * dx + s * dz, p[1], pivot[2] - s * dx + c * dz];
}

/**
 * The uniforms that leave a draw as it is: no offset, scale 1 about the
 * origin, gain 1, no turn, and the frame's own swirl time. The renderer sets
 * them before the static room, again after the movers, before the props and
 * heroes, and once more after those, so every draw pass ends at the
 * identity.
 */
export function restDraw(seconds: number): MoverDraw {
  return {
    offset: [0, 0, 0],
    pivot: [0, 0, 0],
    scale: 1,
    gain: 1,
    yaw: 0,
    time: seconds,
  };
}

/**
 * How mover `m` is drawn this frame, given its door's open fraction
 * (`doorOpen`, from `DoorState` by the mover's key, 0 when missing), the
 * running fault frame of its fixture (undefined while none runs) and the
 * frame's time in seconds (which alone drives a rotor). See the module doc for
 * each part; null means the part is not drawn.
 */
export function moverDraw(
  m: MoverInfo,
  doorOpen: number,
  frame: FaultFrame | undefined,
  seconds: number,
): MoverDraw | null {
  const clamp01 = (v: number) => Math.min(1, Math.max(0, v));
  const at = (k: number): V3 => [
    m.slide[0] * k,
    m.slide[1] * k,
    m.slide[2] * k,
  ];
  const plain = (offset: V3, gain: number): MoverDraw => ({
    offset,
    pivot: [0, 0, 0],
    scale: 1,
    gain,
    yaw: 0,
    time: seconds,
  });
  switch (m.part) {
    case "leaf":
      return plain(at(clamp01(frame?.open ?? doorOpen)), 1);
    case "lid":
      return plain(at(clamp01(frame?.open ?? 0)), 1);
    case "spark":
      return frame !== undefined && frame.spark > 0
        ? plain([0, 0, 0], frame.spark)
        : null;
    case "lamp":
      return plain(
        [0, 0, 0],
        frame !== undefined && frame.lamp > 0 ? frame.lamp : m.rest,
      );
    case "disc": {
      // With no frame the disc is at rest: whole, at its rest gain, with
      // the swirl on the frame's clock.
      const f = frame ?? { scale: 1, gain: m.rest, shift: 0 };
      if (f.scale <= 0) return null;
      return {
        offset: [0, 0, 0],
        pivot: m.pivot ?? [0, 0, 0],
        scale: f.scale,
        gain: frame === undefined ? m.rest : f.gain * m.rest,
        yaw: 0,
        time: seconds + f.shift,
      };
    }
    case "wing":
      // A hero's door: no fault frame names it, so `frame` is ignored.
      return {
        offset: [0, 0, 0],
        pivot: m.pivot ?? [0, 0, 0],
        scale: 1,
        gain: 1,
        yaw: m.swing * clamp01(doorOpen),
        time: seconds,
      };
    case "rotor":
      return plain(at(rotorPhase(seconds)), 1);
  }
}
