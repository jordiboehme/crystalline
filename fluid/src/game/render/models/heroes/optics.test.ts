/**
 * The optics heroes' shape tests: what `heroModels.test.ts` does not check
 * for every kind. Whether the laser desk's catalogue top lies on a real,
 * clear upward face is `heroModels.test.ts`'s check now, for every kind;
 * here, the laser desk's own top stays clear in the recipe's own terms
 * too, the slab keeps its 1 : 4 : 9, the turret's eye looks out of its
 * front and a seam splits its shell, the eye panel is a portrait plate
 * with a small dot at the middle of its lens, the photo console's picture
 * leans back, and the laser's lens hangs over the chair's seat. The marks:
 * the eye panel's two words on its badge, the first on the black and the
 * last on the blue field, and the photo console's wordmark on the lip
 * beside its print slot.
 */

import { describe, expect, it } from "vitest";

import { HERO_CATALOGUE } from "../../../world/heroes";
import { blinkFlag, FLAG, type V3 } from "../../geometry";
import { DECAL_LIFT, frameAt } from "../../kit";
import { LOOK } from "../../looks";
import {
  inked,
  partsOf,
  placeMesh,
  positions,
  runsOfLines,
  toLocal,
  type Part,
} from "../../modelChecks";
import { MARKS } from "../marks";
import { buildHeroMesh } from ".";
import { BADGE_BLUE, BADGE_INK, DESK_BADGE_INK, OFFICE_CHAIR } from "./optics";

/**
 * How high the laser desk's catalogue top must stay clear of a part, in
 * the recipe's own `h`, in metres: below the arm, which passes over it
 * from 1.92.
 */
const CLEAR_TO = 1.9;

/** A part's points in the recipe's local `[a, d, h]`. */
const local = (p: Part): V3[] =>
  p.points.map((q) => toLocal(frameAt([0, 0, 0], 0), q));

/** The height of a panel part: its pixel size, for a mark's run. */
const heightOf = (p: Part): number => {
  const hs = local(p).map((q) => q[2]);
  return Math.max(...hs) - Math.min(...hs);
};

/**
 * The eye panel's name badge, as `EYE_PANEL` builds it: its black plate's
 * `a` and `h` range and its front (the plate's face at 0.05 plus the
 * badge's 0.012), and the blue field's lift over that front.
 */
const BADGE = { a0: -0.17, a1: 0.17, h0: 1.98, h1: 2.07, front: 0.062 };
const BLUE_LIFT = 0.004;

/**
 * The photo console's lip under its screen, as `PHOTO_CONSOLE` builds it:
 * the upright strip of the hood at d 0.45 from the deck's back edge (h
 * 0.95) to the screen face's foot (h 1.0), out to the hood's half width;
 * the print slot takes `a` -0.2 to 0.2 of it.
 */
const LIP = { d: 0.45, h0: 0.95, h1: 1.0, slot: 0.2, half: 0.52 };

/** The one part of a list, failing the test when there is not exactly one. */
function one(parts: readonly Part[]): Part {
  expect(parts).toHaveLength(1);
  const p = parts[0];
  if (!p) throw new Error("no part");
  return p;
}

/** The bounds of a list of local points. */
function bounds(points: readonly V3[]): { lo: V3; hi: V3 } {
  const lo = (k: 0 | 1 | 2) => Math.min(...points.map((p) => p[k]));
  const hi = (k: 0 | 1 | 2) => Math.max(...points.map((p) => p[k]));
  return { lo: [lo(0), lo(1), lo(2)], hi: [hi(0), hi(1), hi(2)] };
}

/** The middle of a list of local points' bounds. */
function centre(points: readonly V3[]): V3 {
  const mid = (k: 0 | 1 | 2) =>
    (Math.min(...points.map((p) => p[k])) +
      Math.max(...points.map((p) => p[k]))) /
    2;
  return [mid(0), mid(1), mid(2)];
}

describe("optics hero models", () => {
  it("keeps the laser desk's catalogue top clear in the recipe's own terms", () => {
    // The catalogue's own rectangle and height; the arm passes high over
    // it (from h 1.92), so nothing may stand on it below `CLEAR_TO`,
    // neither a vertex nor a part whose bounds span the area (a long box
    // whose corners all lie outside it).
    const [surf] = HERO_CATALOGUE["laser-desk"].surfaces;
    if (!surf) throw new Error("laser-desk has no catalogue surface");
    const { a0, a1, d0, d1, h: h0 } = surf;
    const h1 = CLEAR_TO;
    const parts = partsOf("laser-desk");
    const inside = (p: V3) =>
      p[0] > a0 &&
      p[0] < a1 &&
      p[1] > d0 &&
      p[1] < d1 &&
      p[2] > h0 + 1e-6 &&
      p[2] < h1;
    expect(parts.flatMap(local).filter(inside)).toEqual([]);
    const over = parts
      .map((p, i) => ({ i, method: p.method, b: bounds(local(p)) }))
      .filter(
        ({ b }) =>
          b.lo[0] < a1 &&
          b.hi[0] > a0 &&
          b.lo[1] < d1 &&
          b.hi[1] > d0 &&
          b.lo[2] < h1 &&
          b.hi[2] > h0 + 1e-6,
      )
      .map(({ i, method }) => `${String(i)}:${method}`);
    expect(over).toEqual([]);
  });

  it("the slab is 1 : 4 : 9", () => {
    const ps = positions(
      placeMesh(buildHeroMesh("black-slab", 0, LOOK), 0, [0, 0, 0]),
    );
    const extent = (k: 0 | 1 | 2) =>
      Math.max(...ps.map((p) => p[k])) - Math.min(...ps.map((p) => p[k]));
    expect(extent(0)).toBeCloseTo(1.2, 6);
    expect(extent(2)).toBeCloseTo(0.3, 6);
    expect(extent(1)).toBeCloseTo(2.7, 6);
  });

  it("the turret's eye faces front", () => {
    const eye = one(partsOf("turret").filter((p) => p.flag === blinkFlag(0)));
    const [, d, h] = centre(local(eye));
    expect(d).toBeGreaterThan(0.2);
    expect(h).toBeGreaterThan(0.95);
    expect(h).toBeLessThan(1.1);
  });

  it("the laser points at the chair", () => {
    const parts = partsOf("laser-desk");
    const lens = one(
      parts.filter((p) => p.method === "cylinder" && p.flag === blinkFlag(0)),
    );
    const [s0, s1] = OFFICE_CHAIR.seat;
    const seat = one(
      parts.filter((p) => {
        if (p.method !== "bevelBox") return false;
        const hs = local(p).map((q) => q[2]);
        return (
          Math.abs(Math.min(...hs) - s0) < 1e-6 &&
          Math.abs(Math.max(...hs) - s1) < 1e-6
        );
      }),
    );
    const [la, ld, lh] = centre(local(lens));
    const [sa, sd] = centre(local(seat));
    expect(Math.hypot(la - sa, ld - sd)).toBeLessThan(0.25);
    expect(lh).toBeGreaterThan(s1);
  });

  it("splits the turret's shell with a seam down its front", () => {
    const seams = partsOf("turret").filter((p) => {
      const ps = local(p);
      const b = bounds(ps);
      return (
        p.flag === FLAG.lit &&
        Math.max(Math.abs(b.lo[0]), Math.abs(b.hi[0])) < 0.006 &&
        b.hi[1] > 0.27 &&
        b.lo[2] < 0.45
      );
    });
    expect(seams.length).toBeGreaterThan(0);
  });

  it("the eye panel is a portrait plate with a small dot in the middle of its lens", () => {
    const parts = partsOf("eye-panel");
    const plate = one(parts.filter((p) => p.method === "bevelBox"));
    const b = bounds(local(plate));
    const ratio = (b.hi[2] - b.lo[2]) / (b.hi[0] - b.lo[0]);
    expect(ratio).toBeGreaterThan(2.8);
    expect(ratio).toBeLessThan(3.2);
    const eye = parts.filter((p) => p.flag === blinkFlag(0)).map(local);
    const mids = eye.map(centre);
    for (const m of mids) {
      expect(m[0]).toBeCloseTo(mids[0]?.[0] ?? NaN, 6);
      expect(m[2]).toBeCloseTo(mids[0]?.[2] ?? NaN, 6);
    }
    const front = eye.reduce((f, p) =>
      bounds(p).hi[1] > bounds(f).hi[1] ? p : f,
    );
    const fb = bounds(front);
    expect(fb.hi[0] - fb.lo[0]).toBeLessThan(0.05);
  });

  it("the photo console's picture leans back", () => {
    const glows = partsOf("photo-console")
      .filter((p) => p.flag === FLAG.emissive)
      .map(local);
    const picture = glows.reduce((f, p) => {
      const area = (q: V3[]) => {
        const b = bounds(q);
        return (b.hi[0] - b.lo[0]) * (b.hi[2] - b.lo[2]);
      };
      return area(p) > area(f) ? p : f;
    });
    const b = bounds(picture);
    const dAt = (h: number) =>
      Math.max(
        ...picture.filter((q) => Math.abs(q[2] - h) < 0.01).map((q) => q[1]),
      );
    expect(dAt(b.lo[2]) - dAt(b.hi[2])).toBeGreaterThan(0.05);
  });

  it("names the eye panel on its badge, the words split as the original splits them (2.6f C13)", () => {
    // Mutation caught: the badge left blank, both words on the black, or
    // the words below the floor.
    expect(runsOfLines(MARKS.panelName)).toBeGreaterThan(3);
    const parts = partsOf("eye-panel");
    const ink = inked(parts, BADGE_INK, "panel");
    expect(ink).toHaveLength(runsOfLines(MARKS.panelName));
    const badge = parts
      .filter((p) => p.tint?.join() === BADGE_BLUE.join())
      .flatMap(local);
    expect(badge.length).toBeGreaterThan(0);
    for (const p of ink)
      expect(heightOf(p)).toBeGreaterThanOrEqual(0.003 - 1e-9);
    // The last word sits on the blue field.
    const last = MARKS.panelName.at(-1) ?? "";
    const onBlue = ink.filter((p) =>
      local(p).every((q) => q[0] >= Math.min(...badge.map((b) => b[0])) - 1e-6),
    );
    expect(onBlue.length).toBeGreaterThanOrEqual(runsOfLines(last));
  });

  it("sets the first word on the black and the last inside the blue field, one lift proud and the same size (2.6f C17)", () => {
    // Mutation caught: both words on the blue, a word off the badge, the
    // ink sunk into or floating off its field, or the two words at two
    // different sizes.
    const [first = "", last = ""] = MARKS.panelName;
    const parts = partsOf("eye-panel");
    const ink = inked(parts, BADGE_INK, "panel");
    const blue = parts
      .filter((p) => p.tint?.join() === BADGE_BLUE.join())
      .flatMap(local);
    const as = blue.map((q) => q[0]);
    const hs = blue.map((q) => q[2]);
    const [b0, b1] = [Math.min(...as), Math.max(...as)];
    const [c0, c1] = [Math.min(...hs), Math.max(...hs)];
    const inside = (q: V3, a0: number, a1: number, h0: number, h1: number) =>
      q[0] >= a0 - 1e-9 &&
      q[0] <= a1 + 1e-9 &&
      q[2] >= h0 - 1e-9 &&
      q[2] <= h1 + 1e-9;
    const onBlue = ink.filter((p) =>
      local(p).every((q) => inside(q, b0, b1, c0, c1)),
    );
    const onBlack = ink.filter((p) =>
      local(p).every((q) => inside(q, BADGE.a0, b0, BADGE.h0, BADGE.h1)),
    );
    expect(onBlue).toHaveLength(runsOfLines(last));
    expect(onBlack).toHaveLength(runsOfLines(first));
    for (const p of onBlack)
      for (const q of local(p))
        expect(q[1]).toBeCloseTo(BADGE.front + DECAL_LIFT, 9);
    for (const p of onBlue)
      for (const q of local(p))
        expect(q[1]).toBeCloseTo(BADGE.front + BLUE_LIFT + DECAL_LIFT, 9);
    const sizes = ink.map(heightOf);
    for (const h of sizes) expect(h).toBeCloseTo(sizes[0] ?? NaN, 9);
  });

  it("prints the photo console's wordmark in its own dark ink on the lip beside the print slot (2.6f C13)", () => {
    // Mutation caught: the wordmark missing or set from the wrong string,
    // over the slot, off the lip, floating or sunk, or below the floor.
    expect(runsOfLines(MARKS.deskBadge)).toBeGreaterThan(3);
    const ink = inked(partsOf("photo-console"), DESK_BADGE_INK, "panel");
    expect(ink).toHaveLength(runsOfLines(MARKS.deskBadge));
    for (const p of ink) {
      expect(heightOf(p)).toBeGreaterThanOrEqual(0.003 - 1e-9);
      for (const q of local(p)) {
        expect(q[1]).toBeCloseTo(LIP.d + DECAL_LIFT, 9);
        expect(q[2]).toBeGreaterThanOrEqual(LIP.h0 - 1e-9);
        expect(q[2]).toBeLessThanOrEqual(LIP.h1 + 1e-9);
        expect(q[0]).toBeGreaterThanOrEqual(LIP.slot - 1e-9);
        expect(q[0]).toBeLessThanOrEqual(LIP.half + 1e-9);
      }
    }
  });
});
