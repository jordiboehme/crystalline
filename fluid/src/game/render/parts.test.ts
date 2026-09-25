import { describe, expect, it } from "vitest";

import { LAMP_ON, REST_FRAME, type FaultFrame } from "../world/malfunction";
import { DISC_SEALED_GAIN, LAMP_IDLE } from "./models";
import { moverDraw, restDraw, type MoverInfo } from "./parts";
import { SCENE_FS, SCENE_VS } from "./shaders";

const SLIDE: [number, number, number] = [0.5, 0, 0];
const T = 12.5;

const info = (
  part: MoverInfo["part"],
  rest = 1,
  pivot: MoverInfo["pivot"] = null,
): MoverInfo => ({ part, slide: SLIDE, pivot, rest });

const frame = (p: Partial<FaultFrame>): FaultFrame => ({ ...REST_FRAME, ...p });

describe("moverDraw", () => {
  describe("a leaf", () => {
    it("slides by the door's fraction with no frame, at gain 1", () => {
      expect(moverDraw(info("leaf"), 0.5, undefined, T)).toEqual({
        offset: [0.25, 0, 0],
        pivot: [0, 0, 0],
        scale: 1,
        gain: 1,
        time: T,
      });
    });

    it("takes the frame's fraction over the door's", () => {
      const d = moverDraw(info("leaf"), 1, frame({ open: 0.4 }), T);
      expect(d?.offset[0]).toBeCloseTo(0.2, 12);
      expect(d?.offset.slice(1)).toEqual([0, 0]);
    });

    it("clamps the fraction to 1", () => {
      expect(moverDraw(info("leaf"), 1.3, undefined, T)?.offset).toEqual(SLIDE);
    });
  });

  describe("a spark cluster", () => {
    it("is not drawn with no frame or a dark one", () => {
      expect(moverDraw(info("spark", 0), 0, undefined, T)).toBeNull();
      expect(moverDraw(info("spark", 0), 0, frame({ spark: 0 }), T)).toBeNull();
    });

    it("glows at the frame's spark gain, in place", () => {
      const d = moverDraw(info("spark", 0), 1, frame({ spark: 2.5 }), T);
      expect(d?.offset).toEqual([0, 0, 0]);
      expect(d?.gain).toBe(2.5);
    });
  });

  describe("a lamp", () => {
    it("glows at its rest gain with no frame or an unlit one", () => {
      const lamp = info("lamp", LAMP_IDLE);
      expect(LAMP_IDLE).toBe(0.3);
      expect(moverDraw(lamp, 0, undefined, T)?.gain).toBe(0.3);
      expect(moverDraw(lamp, 0, frame({ lamp: 0 }), T)?.gain).toBe(0.3);
    });

    it("blinks to LAMP_ON while lit", () => {
      const d = moverDraw(
        info("lamp", LAMP_IDLE),
        0,
        frame({ lamp: LAMP_ON }),
        T,
      );
      expect(d?.gain).toBe(LAMP_ON);
    });
  });

  describe("a lid", () => {
    it("slides the full way at open 1 and stays shut with no frame", () => {
      expect(moverDraw(info("lid"), 0, frame({ open: 1 }), T)?.offset).toEqual(
        SLIDE,
      );
      expect(moverDraw(info("lid"), 1, undefined, T)?.offset).toEqual([
        0, 0, 0,
      ]);
    });
  });

  describe("a disc", () => {
    const pivot: [number, number, number] = [3, 1.35, 4.15];

    it("draws at its rest gain with no frame", () => {
      for (const rest of [1, DISC_SEALED_GAIN]) {
        const d = moverDraw(info("disc", rest, pivot), 0, undefined, T);
        expect(d).toEqual({
          offset: [0, 0, 0],
          pivot,
          scale: 1,
          gain: rest,
          time: T,
        });
      }
      expect(DISC_SEALED_GAIN).toBe(0.5);
    });

    it("scales, flickers and shifts its swirl with the frame", () => {
      const f = frame({ scale: 0.5, gain: 1.6, shift: 0.2 });
      const d = moverDraw(info("disc", 1, pivot), 0, f, T);
      expect(d?.pivot).toEqual(pivot);
      expect(d?.scale).toBe(0.5);
      expect(d?.gain).toBe(1.6);
      expect(d?.time).toBeCloseTo(T + 0.2, 12);
    });

    it("scales a sealed disc's run gains by its rest gain, with no jump", () => {
      const sealed = info("disc", DISC_SEALED_GAIN, pivot);
      const flicker = moverDraw(sealed, 0, frame({ gain: 1.6 }), T);
      expect(flicker?.gain).toBeCloseTo(1.6 * DISC_SEALED_GAIN, 12);
      // A run's calm frame (gain 1) draws the disc at its rest glow, so a
      // run starts and ends where the rest leaves off.
      const calm = moverDraw(sealed, 0, frame({ gain: 1 }), T);
      expect(calm?.gain).toBe(moverDraw(sealed, 0, undefined, T)?.gain);
    });

    it("is not drawn at scale 0", () => {
      const d = moverDraw(info("disc", 1, pivot), 0, frame({ scale: 0 }), T);
      expect(d).toBeNull();
    });
  });

  it("gives every part but a disc pivot 0 and scale 1", () => {
    const f = frame({ open: 0.5, spark: 2, lamp: LAMP_ON, scale: 0.3 });
    for (const part of ["leaf", "spark", "lamp", "lid"] as const) {
      const d = moverDraw(info(part, 1, [9, 9, 9]), 0.5, f, T);
      expect(d?.pivot).toEqual([0, 0, 0]);
      expect(d?.scale).toBe(1);
    }
  });
});

describe("the identity draw", () => {
  it("draws the room and the props with offset 0, pivot 0, scale 1, gain 1", () => {
    expect(restDraw(T)).toEqual({
      offset: [0, 0, 0],
      pivot: [0, 0, 0],
      scale: 1,
      gain: 1,
      time: T,
    });
  });

  it("places a vertex exactly where it was under the rest uniforms", () => {
    // The vertex shader's placement, in single precision as the GPU does
    // it: pivot + (placed - pivot) * scale + offset. At the rest values
    // every step is exact, so today's image is unchanged bit for bit.
    const f = Math.fround;
    const d = restDraw(T);
    const place = (p: number, k: 0 | 1 | 2) =>
      f(f(d.pivot[k] + f(f(p - d.pivot[k]) * d.scale)) + d.offset[k]);
    for (const p of [0, 1e-7, -3.14159, 12.345678, 199.99, -0.1]) {
      for (const k of [0, 1, 2] as const) {
        expect(Object.is(place(f(p), k), f(p))).toBe(true);
      }
    }
    for (const c of [0, 0.02, 0.7, 1.4, 3.5]) {
      expect(f(f(c) * d.gain)).toBe(f(c));
    }
  });

  it("is the formula the vertex shader places with", () => {
    expect(SCENE_VS).toContain("uniform vec3 uModelPivot;");
    expect(SCENE_VS).toContain("uniform float uModelScale;");
    expect(SCENE_VS).toContain(
      "vec3 placed = vec3(xz.x, aPosition.y, xz.y) + aInstanceOffset;",
    );
    expect(SCENE_VS).toContain(
      "vec3 world = uModelPivot + (placed - uModelPivot) * uModelScale + uModelOffset;",
    );
  });

  it("scales every exit of the fragment shader by uGain", () => {
    expect(SCENE_FS).toContain("uniform float uGain;");
    const exits = SCENE_FS.split("\n").filter((l) =>
      l.includes("outColour = "),
    );
    // Emissive, lamp, signal, blink, portal and the lit path.
    expect(exits).toHaveLength(6);
    for (const line of exits) expect(line).toContain("uGain");
  });

  it("reads the derivatives and the swirl before any exit", () => {
    const main = SCENE_FS.slice(SCENE_FS.indexOf("void main()"));
    const firstExit = main.indexOf("outColour = ");
    for (const read of ["dFdx(vUv)", "dFdy(vUv)", "float swirl = "]) {
      const at = main.indexOf(read);
      expect(at).toBeGreaterThan(0);
      expect(at).toBeLessThan(firstExit);
    }
  });
});
