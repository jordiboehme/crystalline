/**
 * The retro curios' shape tests: what `curioModels.test.ts` does not check
 * for every kind. The console's two screen pictures never share a quad and
 * its battery light sits left of the screen; the tape drive has five keys
 * along its front edge, one of them the darker record key; the tape player
 * has two foam pads and one orange button; the video tape carries no text,
 * and bare it shows two reels; the laptop's "<=>" is exactly the font's
 * runs, lying on the back plane of its tilted lid, its two drive slots sit
 * either side of the latch and it stays under 900 triangles.
 *
 * Parts are told apart by their colour, so this file records every kit
 * call with its surface's tint (`tintedParts`), which the shared recording
 * kit does not keep.
 */

import { describe, expect, it } from "vitest";

import type { CurioKind } from "../../../world/types";
import {
  FLAG,
  createBuilder,
  type Builder,
  type Surface,
  type V3,
} from "../../geometry";
import { DECAL_LIFT, createKit, frameAt, type Kit } from "../../kit";
import { TEXT_BASE } from "../../layers";
import { LOOKS, type Rgb } from "../../looks";
import { toLocal } from "../../modelChecks";
import type { KitAt } from "../common";
import { pixelRuns, textRows } from "../heroes/pixels";
import { buildCurio, buildCurioMesh } from ".";
import { curioHalf } from "./common";
import {
  CONSOLE_PLAY,
  CONSOLE_TITLE,
  DRIVE_KEY,
  DRIVE_RECORD_KEY,
  LAPTOP_KEY,
  LAPTOP_LATCH,
  LAPTOP_LID,
  LAPTOP_MARK,
  LAPTOP_SLOT,
  MARK_PX,
  PLAYER_FOAM,
  PLAYER_ORANGE,
  TAPE_REEL,
} from "./retro";

/** One kit call: its primitive, its surface's tint, flag and layer, and its points in `[a, d, h]`. */
interface TintedPart {
  method: string;
  tint: Rgb | null;
  flag: number;
  layer: number;
  points: V3[];
}

type Fn = (...args: unknown[]) => void;

/**
 * A curio built at the origin, every kit call recorded with its surface:
 * each call is emitted a second time into a scratch builder to learn its
 * own points, as `recordingKitAt` does, and kept in the recipe's local
 * `[a, d, h]`.
 */
function tintedParts(kind: CurioKind, variant = 0): TintedPart[] {
  const parts: TintedPart[] = [];
  const origin = frameAt([0, 0, 0], 0);
  const kitAt: KitAt = (f) => {
    const wrapped: Record<string, Fn> = {};
    for (const name of Object.keys(createKit(createBuilder(), f))) {
      wrapped[name] = (...args: unknown[]) => {
        const points: V3[] = [];
        const scratch = {
          vertex: (p: V3) => points.push(toLocal(origin, p)),
        } as unknown as Builder;
        (createKit(scratch, f) as unknown as Record<string, Fn | undefined>)[
          name
        ]?.(...args);
        const s = args.find(
          (x): x is Surface =>
            typeof x === "object" && x !== null && "flag" in x,
        );
        parts.push({
          method: name,
          tint: s?.tint ?? null,
          flag: s?.flag ?? -1,
          layer: s?.layer ?? -1,
          points,
        });
      };
    }
    return wrapped as unknown as Kit;
  };
  buildCurio(kitAt, kind, variant, LOOKS.aperture);
  return parts;
}

/** Whether a part is drawn in exactly this colour. */
const inTint = (p: TintedPart, c: Rgb) =>
  p.tint !== null && p.tint.every((x, i) => x === c[i]);

/** A part's bounds in `[a, d, h]`: lowest and highest of each. */
function bounds(p: TintedPart): { lo: V3; hi: V3 } {
  const lo: V3 = [Infinity, Infinity, Infinity];
  const hi: V3 = [-Infinity, -Infinity, -Infinity];
  for (const q of p.points)
    for (const k of [0, 1, 2] as const) {
      lo[k] = Math.min(lo[k], q[k]);
      hi[k] = Math.max(hi[k], q[k]);
    }
  return { lo, hi };
}

/** Whether two parts' `(a, h)` bounds share any area (touching edges do not count). */
function overlapAH(p: TintedPart, q: TintedPart): boolean {
  const e = 1e-9;
  const x = bounds(p);
  const y = bounds(q);
  return (
    x.lo[0] < y.hi[0] - e &&
    y.lo[0] < x.hi[0] - e &&
    x.lo[2] < y.hi[2] - e &&
    y.lo[2] < x.hi[2] - e
  );
}

describe("retro curio models", () => {
  describe("pocket console", () => {
    const parts = tintedParts("pocket-console");
    const group = (p: TintedPart) => p.flag - FLAG.blink;
    const title = parts.filter((p) => group(p) >= 0 && group(p) < 4);
    const play = parts.filter((p) => group(p) >= 4 && group(p) < 8);

    it("has two pictures of one size, never lit in the same cell", () => {
      const width = CONSOLE_TITLE[0]?.length ?? 0;
      expect(width).toBeGreaterThan(0);
      expect(CONSOLE_PLAY).toHaveLength(CONSOLE_TITLE.length);
      for (const r of [...CONSOLE_TITLE, ...CONSOLE_PLAY])
        expect(r).toHaveLength(width);
      CONSOLE_TITLE.forEach((r, y) =>
        [...r].forEach((ch, x) => {
          if (ch === "#") expect(CONSOLE_PLAY[y]?.[x]).not.toBe("#");
        }),
      );
      const word = textRows("BLOK").map((r) => `.${r}.`);
      expect(CONSOLE_TITLE.join("\n")).toContain(word.join("\n"));
    });

    it("draws the title in groups 0 to 3 and the play picture in 4 to 7, never on one quad", () => {
      expect(new Set(title.map(group))).toEqual(new Set([0, 1, 2, 3]));
      expect(new Set(play.map(group))).toEqual(new Set([4, 5, 6, 7]));
      expect([...title, ...play].every((p) => p.method === "panel")).toBe(true);
      for (const t of title)
        for (const q of play) expect(overlapAH(t, q)).toBe(false);
    });

    it("lights its battery light left of the screen", () => {
      const leds = parts.filter((p) => p.flag === FLAG.signal);
      expect(leds).toHaveLength(1);
      const led = bounds(leds[0] as TintedPart);
      const pixels = [...title, ...play].map(bounds);
      const screenLeft = Math.min(...pixels.map((b) => b.lo[0]));
      const screenLow = Math.min(...pixels.map((b) => b.lo[2]));
      const screenHigh = Math.max(...pixels.map((b) => b.hi[2]));
      expect(led.hi[0]).toBeLessThan(screenLeft);
      expect(led.lo[2]).toBeGreaterThan(screenLow);
      expect(led.hi[2]).toBeLessThan(screenHigh);
    });
  });

  describe("tape drive", () => {
    it("has five keys along its front edge, the first the darker record key", () => {
      const { hd } = curioHalf("tape-drive", 0);
      const parts = tintedParts("tape-drive");
      const keys = parts
        .filter(
          (p) =>
            p.method === "box" &&
            (inTint(p, DRIVE_KEY) || inTint(p, DRIVE_RECORD_KEY)),
        )
        .sort((p, q) => bounds(p).lo[0] - bounds(q).lo[0]);
      expect(keys).toHaveLength(5);
      expect(keys.map((k) => inTint(k, DRIVE_RECORD_KEY))).toEqual([
        true,
        false,
        false,
        false,
        false,
      ]);
      for (const [i, k] of keys.entries()) {
        const b = bounds(k);
        expect(b.hi[1]).toBeGreaterThan(hd - 0.002);
        expect(b.lo[1]).toBeGreaterThan(0);
        const next = keys[i + 1];
        if (next) expect(b.hi[0]).toBeLessThan(bounds(next).lo[0]);
      }
    });
  });

  describe("tape player", () => {
    const parts = tintedParts("tape-player");

    it("has two foam pads lying flat, one on each earcup", () => {
      const pads = parts.filter((p) => inTint(p, PLAYER_FOAM));
      expect(pads).toHaveLength(2);
      const centre = (b: { lo: V3; hi: V3 }) => [
        (b.lo[0] + b.hi[0]) / 2,
        (b.lo[1] + b.hi[1]) / 2,
      ];
      for (const p of pads) {
        const b = bounds(p);
        expect(p.method).toBe("cylinder");
        // A 10-sided pad of radius 0.022: at most 0.044 across.
        expect(b.hi[0] - b.lo[0]).toBeGreaterThan(0.04);
        expect(b.hi[0] - b.lo[0]).toBeLessThanOrEqual(0.044 + 1e-9);
        expect(b.hi[2] - b.lo[2]).toBeLessThan(b.hi[0] - b.lo[0]);
        const [ca, cd] = centre(b);
        const cups = parts.filter((q) => {
          if (q.method !== "cylinder" || inTint(q, PLAYER_FOAM)) return false;
          const c = bounds(q);
          const [qa, qd] = centre(c);
          return (
            Math.abs((qa ?? 0) - (ca ?? 0)) < 1e-3 &&
            Math.abs((qd ?? 0) - (cd ?? 0)) < 1e-3 &&
            Math.abs(c.hi[2] - b.lo[2]) < 1e-9 &&
            c.lo[2] === 0
          );
        });
        expect(cups).toHaveLength(1);
      }
    });

    it("has exactly one orange button, on the player", () => {
      const orange = parts.filter((p) => inTint(p, PLAYER_ORANGE));
      expect(orange).toHaveLength(1);
      const { hw } = curioHalf("tape-player", 0);
      expect(bounds(orange[0] as TintedPart).hi[0]).toBeLessThan(-hw + 0.088);
    });
  });

  describe("video tape", () => {
    it("carries no text and no decal in either variant", () => {
      for (const v of [0, 1]) {
        const parts = tintedParts("video-tape", v);
        expect(parts.length).toBeGreaterThan(0);
        for (const p of parts) {
          expect(p.layer).toBeLessThan(TEXT_BASE);
          expect(p.method).not.toBe("panel");
        }
      }
    });

    it("shows two reels on the bare tape", () => {
      const reels = tintedParts("video-tape", 1).filter((p) =>
        inTint(p, TAPE_REEL),
      );
      expect(reels).toHaveLength(2);
      const [a, b] = reels.map(bounds);
      expect(a && b && (a.hi[0] < b.lo[0] || b.hi[0] < a.lo[0])).toBe(true);
    });
  });

  describe("beige laptop", () => {
    const parts = tintedParts("beige-laptop");

    it("stays under 900 triangles", () => {
      expect(
        buildCurioMesh("beige-laptop", 0, LOOKS.aperture).count / 3,
      ).toBeLessThan(900);
    });

    it("lays the mark on the lid's back plane as exactly the font's runs of <=>", () => {
      const L = LAPTOP_LID;
      /** A local point in the lid's terms: `s` up it, `o` through it from its back face. */
      const lid = (q: V3) => {
        const d = q[1] - L.hingeD;
        const h = q[2] - L.hingeH;
        return {
          s: d * L.up[0] + h * L.up[1],
          o: d * L.out[0] + h * L.out[1],
        };
      };
      const mark = parts.filter((p) => inTint(p, LAPTOP_MARK));
      expect(mark.length).toBeGreaterThan(0);
      const boxes = mark.map((p) => {
        const pts = p.points.map((q) => ({ a: q[0], ...lid(q) }));
        const os = pts.map((q) => q.o);
        // Sunk 1 mm into the lid, standing DECAL_LIFT proud of its back.
        expect(Math.max(...os)).toBeCloseTo(0.001, 9);
        expect(Math.min(...os)).toBeCloseTo(-DECAL_LIFT, 9);
        return {
          a0: Math.min(...pts.map((q) => q.a)),
          a1: Math.max(...pts.map((q) => q.a)),
          s0: Math.min(...pts.map((q) => q.s)),
          s1: Math.max(...pts.map((q) => q.s)),
        };
      });
      const rows = textRows("<=>");
      const start = Math.max(...boxes.map((b) => b.a1));
      const sTop = Math.max(...boxes.map((b) => b.s1));
      // Read from behind, column 0 is at +a: centred across the lid, and
      // centred on the middle of its upper half.
      expect(start).toBeCloseTo(((rows[0]?.length ?? 0) * MARK_PX) / 2, 9);
      expect(Math.min(...boxes.map((b) => b.a0))).toBeCloseTo(-start, 9);
      expect(sTop - (rows.length * MARK_PX) / 2).toBeCloseTo(
        0.75 * L.length,
        9,
      );
      const runs = boxes
        .map((b) => ({
          col: Math.round((start - b.a1) / MARK_PX),
          row: Math.round((sTop - b.s1) / MARK_PX),
          len: Math.round((b.a1 - b.a0) / MARK_PX),
          height: (b.s1 - b.s0) / MARK_PX,
        }))
        .sort((x, y) => x.row - y.row || x.col - y.col);
      for (const r of runs) expect(r.height).toBeCloseTo(1, 9);
      expect(runs.map(({ col, row, len }) => ({ col, row, len }))).toEqual(
        pixelRuns(rows),
      );
    });

    it("puts its two drive slots in the band's front face, either side of the latch", () => {
      const slots = parts
        .filter((p) => inTint(p, LAPTOP_SLOT))
        .map(bounds)
        .sort((x, y) => x.lo[0] - y.lo[0]);
      const latches = parts.filter((p) => inTint(p, LAPTOP_LATCH)).map(bounds);
      expect(slots).toHaveLength(2);
      expect(latches).toHaveLength(1);
      const [left, right] = slots;
      const latch = latches[0];
      if (!left || !right || !latch) throw new Error("missing parts");
      expect(left.hi[0]).toBeLessThan(latch.lo[0]);
      expect(latch.hi[0]).toBeLessThan(right.lo[0]);
      for (const s of slots) {
        expect(s.hi[0] - s.lo[0]).toBeCloseTo(0.095, 6);
        expect(s.hi[2] - s.lo[2]).toBeCloseTo(0.004, 6);
        expect(s.lo[1]).toBeCloseTo(latch.lo[1], 9);
      }
    });

    it("has about 45 chunky keys", () => {
      const keys = parts.filter((p) => inTint(p, LAPTOP_KEY));
      expect(keys.length).toBeGreaterThanOrEqual(42);
      expect(keys.length).toBeLessThanOrEqual(48);
    });
  });
});
