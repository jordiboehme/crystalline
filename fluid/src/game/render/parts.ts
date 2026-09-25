/**
 * A mover's per-draw uniforms: the pure half of the renderer's mover loop,
 * so what a door, lamp, hatch lid or portal disc is drawn with is tested
 * without a GL context.
 *
 * Every way's moving parts come back from its model as movers (`Mover` in
 * `models/common.ts`): leaves keyed `door:<i>`, and `spark:<i>`,
 * `lamp:<i>`, `lid:<i>` and `disc:<i>`. Each one is drawn with its slide
 * (`axis * travel`) times an open fraction, a scale about its pivot, a
 * gain and a swirl time. Leaves take the door's fraction from `DoorState`
 * unless a fault frame names their fixture. A lid takes only the frame's.
 * Sparks are drawn only while a frame lights them. The lamp glows at its
 * rest gain and blinks brighter while a frame lights it. A disc scales,
 * flickers and shifts its swirl with the frame, and is skipped at scale 0.
 * A disc's frame gain is scaled by its rest gain, so a sealed disc's run
 * flickers about its dim rest glow and starts and ends with no jump.
 * A null result means the part is not drawn this frame.
 *
 * `restDraw` is the identity every other draw uses: the static room and
 * the prop instances are drawn with it, so the scene shader's pivot,
 * scale, gain and offset change nothing for them.
 *
 * Only the fault frame's type is read from `world/malfunction.ts`: the
 * renderer takes the frames the session hands it and never runs the clock.
 */

import type { FaultFrame } from "../world/malfunction";
import type { V3 } from "./geometry";
import type { MoverPart } from "./models/common";

/**
 * What `moverDraw` reads of a mover: its part, its slide (`axis *
 * travel`, the offset when fully open), its pivot (a disc's centre in
 * world metres, null for every other part) and its rest gain.
 */
export interface MoverInfo {
  part: MoverPart;
  slide: V3;
  pivot: V3 | null;
  rest: number;
}

/**
 * One draw's uniforms: `uModelOffset`, `uModelPivot`, `uModelScale`,
 * `uGain` and `uTime`, in that order of the fields.
 */
export interface MoverDraw {
  offset: V3;
  pivot: V3;
  scale: number;
  gain: number;
  time: number;
}

/**
 * The uniforms that leave a draw as it is: no offset, scale 1 about the
 * origin, gain 1, and the frame's own swirl time. The renderer sets them
 * before the static room and again after the movers, before the props.
 */
export function restDraw(seconds: number): MoverDraw {
  return {
    offset: [0, 0, 0],
    pivot: [0, 0, 0],
    scale: 1,
    gain: 1,
    time: seconds,
  };
}

/**
 * How mover `m` is drawn this frame, given its door's open fraction
 * (`doorOpen`, from `DoorState` by the mover's key, 0 when missing), the
 * running fault frame of its fixture (undefined while none runs) and the
 * frame's time in seconds. See the module doc for each part; null means
 * the part is not drawn.
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
        time: seconds + f.shift,
      };
    }
  }
}
