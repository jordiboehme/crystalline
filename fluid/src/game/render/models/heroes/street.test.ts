/**
 * The street heroes' shape tests: the bike's sourced size, its two
 * lights and its sponsor stickers on both sides; the police box's sign
 * layout, its sign on all four sides, the lines of its door notice, and
 * its door leaves handed over as two swinging movers, and its hollow
 * inside lit white behind them.
 */

import { describe, expect, it } from "vitest";

import { boxFront } from "../../../world/box";
import { heroFootprint } from "../../../world/footprints";
import type { Hero } from "../../../world/types";
import { FLAG, blinkFlag, createBuilder, type V3 } from "../../geometry";
import { createKit, frameAt } from "../../kit";
import { LOOK } from "../../looks";
import {
  inked,
  partsOf,
  positions,
  recordingKitAt,
  runsOfLines,
  toLocal,
  type Part,
} from "../../modelChecks";
import { swungPoint } from "../../parts";
import { surfaces } from "../common";
import { MARKS } from "../marks";
import { buildHero, buildHeroMesh } from ".";
import { heroHalf } from "./common";
import {
  BIKE,
  BIKE_STICKERS,
  BOX_SIGN,
  BRAKE_STEEL,
  NOTICE_INK,
  boxDoor,
  boxLeafMovers,
  boxSignLayout,
} from "./street";
import { textRows } from "./pixels";

const local = (p: Part): V3[] =>
  p.points.map((q) => toLocal(frameAt([0, 0, 0], 0), q));

/**
 * The police box's depth range in the recipe's terms: it stands free,
 * centred on its anchor (2.6e), so `d` runs from `BACK` to `FRONT`.
 */
const { d0: BACK, d1: FRONT } = heroHalf("police-box", 0);

/** The height of a panel part: its pixel size, for a mark's run. */
const heightOf = (p: Part): number => {
  const hs = local(p).map((q) => q[2]);
  return Math.max(...hs) - Math.min(...hs);
};

/**
 * The sign of `a` of the door leaf that carries the notice: `boxDoor`
 * builds the left leaf (`facePanels(..., leaf === "left")`) at
 * `sign * edge` with `sign` -1, so its whole column lies at negative `a`.
 */
const LEFT_LEAF_SIGN = -1;

describe("street hero models", () => {
  it("builds the bike at the original's size: 2.95 long, 0.83 wide, the windscreen at 1.17", () => {
    // Mutation caught: a bike shorter than `BIKE.length`.
    const pts = partsOf("red-bike").flatMap(local);
    const as = pts.map((q) => q[0]);
    expect(Math.max(...as) - Math.min(...as)).toBeCloseTo(BIKE.length, 2);
    const body = partsOf("red-bike")
      .filter((p) => p.method === "extrude")
      .flatMap(local);
    const ds = body.map((q) => q[1]);
    expect(Math.max(...ds) - Math.min(...ds)).toBeLessThanOrEqual(
      BIKE.width + 1e-6,
    );
    expect(Math.max(...pts.map((q) => q[2]))).toBeGreaterThanOrEqual(
      BIKE.screenTop - 1e-6,
    );
  });

  it("lights the bike with a steady headlight at the nose and a breathing tail light at the back", () => {
    // Mutation caught: the tail light made steady (two steady lights),
    // or the two lights swapped end for end.
    const parts = partsOf("red-bike");
    const head = parts.filter((p) => p.flag === FLAG.signal);
    const tail = parts.filter((p) => p.flag === blinkFlag(0));
    expect(head).toHaveLength(1);
    expect(tail).toHaveLength(1);
    expect(
      Math.min(...local(head[0] as Part).map((q) => q[0])),
    ).toBeGreaterThan(1.3);
    expect(Math.max(...local(tail[0] as Part).map((q) => q[0]))).toBeLessThan(
      -1.3,
    );
  });

  it("fits one brake disc, on the front wheel, outboard of its hub", () => {
    // Mutation caught: the disc on the rear wheel, on both wheels, or
    // sunk inside the hub where it cannot be seen. The guard: exactly one
    // part in the disc's steel, so the test cannot pass on none.
    const discs = partsOf("red-bike").filter(
      (p) => p.tint !== null && p.tint.every((c, i) => c === BRAKE_STEEL[i]),
    );
    expect(discs).toHaveLength(1);
    const pts = local(discs[0] as Part);
    const as = pts.map((q) => q[0]);
    expect((Math.max(...as) + Math.min(...as)) / 2).toBeCloseTo(1.0, 2);
    expect(Math.max(...as) - Math.min(...as)).toBeLessThan(0.4);
    // Outboard of the hub's face at 0.17 on the `+d` side.
    expect(Math.min(...pts.map((q) => q[1]))).toBeGreaterThan(0.17);
  });

  it("lays the sign out as on the original: POLICE, PUBLIC over CALL, BOX, inside the band", () => {
    // Mutation caught: the words reordered, the stack side by side, or
    // the sign wider than its band.
    const words = boxSignLayout(1.1, 2.17);
    expect(words.map((w) => w.rows)).toEqual(
      [BOX_SIGN.left, BOX_SIGN.upper, BOX_SIGN.lower, BOX_SIGN.right].map(
        textRows,
      ),
    );
    const [left, upper, lower, right] = words;
    if (!left || !upper || !lower || !right) throw new Error("four words");
    expect(upper.a0).toBeCloseTo(lower.a0, 9);
    expect(upper.h1).toBeGreaterThan(lower.h1);
    expect(upper.px).toBeLessThan(left.px);
    const span = (w: (typeof words)[number]) => [
      w.a0,
      w.a0 + (w.rows[0]?.length ?? 0) * w.px,
    ];
    expect(span(left)[1]).toBeLessThan(upper.a0);
    expect(span(upper)[1]).toBeLessThan(right.a0);
    expect(left.a0).toBeGreaterThanOrEqual(-0.55 - 1e-9);
    expect(span(right)[1]).toBeLessThanOrEqual(0.55 + 1e-9);
  });

  it("puts the sign on all four sides of the box, the back one inside the envelope", () => {
    // Mutation caught: the back sign dropped, or drawn behind the box's
    // footprint.
    const letters = partsOf("police-box").filter(
      (p) => p.flag === FLAG.signal && p.method === "panel",
    );
    const faces = new Set<string>();
    for (const p of letters) {
      const pts = local(p);
      const hs = pts.map((q) => q[2]);
      if (Math.min(...hs) < 2.0) continue; // the windows glow too, lower down
      const as = pts.map((q) => q[0]);
      const ds = pts.map((q) => q[1]);
      if (Math.max(...ds) - Math.min(...ds) < 1e-6)
        faces.add((ds[0] ?? 0) > 0 ? "front" : "back");
      else if (Math.max(...as) - Math.min(...as) < 1e-6)
        faces.add((as[0] ?? 0) > 0 ? "a+" : "a-");
      for (const q of pts) expect(q[1]).toBeGreaterThanOrEqual(BACK - 1e-9);
    }
    expect([...faces].sort()).toEqual(["a+", "a-", "back", "front"]);
    expect(BACK).toBeCloseTo(-0.65, 9);
  });

  it("keeps the box's use point clear: nothing reaches past its front", () => {
    // Mutation caught: a handle, a panel or the sign standing out past the
    // corner posts' front into the use point. The guard: the posts do
    // reach the front, so the test cannot pass on a box built short.
    const ds = partsOf("police-box")
      .flatMap(local)
      .map((q) => q[1]);
    expect(Math.max(...ds)).toBeGreaterThanOrEqual(FRONT - 1e-6);
    for (const d of ds) expect(d).toBeLessThanOrEqual(FRONT + 1e-6);
  });
  it("puts each sponsor sticker on both sides of the bike's shell, from the approved list (2.6f C13)", () => {
    // Mutation caught: a sticker on one side only, a sticker set from the
    // wrong entry, or a sticker off the shell's side faces.
    expect(BIKE_STICKERS.length).toBe(MARKS.bikeStickers.length);
    const parts = partsOf("red-bike");
    for (const s of BIKE_STICKERS) {
      const text = MARKS.bikeStickers[s.text] ?? "";
      expect(runsOfLines(text)).toBeGreaterThan(3);
      const ink = inked(parts, s.ink, "panel");
      expect(ink, text).toHaveLength(2 * runsOfLines(text));
      const sides = new Set(
        ink.map((p) => Math.sign(Math.max(...local(p).map((q) => q[1])))),
      );
      expect([...sides].sort(), text).toEqual([-1, 1]);
      for (const p of ink) {
        const [a0, a1] = s.a;
        for (const q of local(p)) {
          expect(q[0]).toBeGreaterThanOrEqual(a0 - 1e-6);
          expect(q[0]).toBeLessThanOrEqual(a1 + 1e-6);
          expect(Math.abs(q[1])).toBeGreaterThan(0.2);
        }
        expect(heightOf(p)).toBeGreaterThanOrEqual(0.003 - 1e-9);
      }
    }
  });

  it("sets the notice's lines in black on the left door leaf's white notice (2.6f C13)", () => {
    // Mutation caught: the notice left blank, set on the right leaf, or a
    // line dropped.
    expect(runsOfLines(MARKS.boxNotice)).toBeGreaterThan(20);
    const parts = partsOf("police-box");
    const ink = inked(parts, NOTICE_INK, "panel");
    expect(ink).toHaveLength(runsOfLines(MARKS.boxNotice));
    const pts = ink.flatMap(local);
    expect(Math.min(...pts.map((q) => q[2]))).toBeGreaterThanOrEqual(
      1.3 - 1e-6,
    );
    expect(Math.max(...pts.map((q) => q[2]))).toBeLessThanOrEqual(1.52 + 1e-6);
    // The left leaf, seen from the front: its `a` is below 0 in the
    // recipe's frame (turn 0 faces north, `along` runs west).
    const mid =
      (Math.min(...pts.map((q) => q[0])) + Math.max(...pts.map((q) => q[0]))) /
      2;
    expect(Math.sign(mid)).toBe(LEFT_LEAF_SIGN);
    for (const p of ink)
      expect(heightOf(p)).toBeGreaterThanOrEqual(0.003 - 1e-9);
    // Mutation caught: the notice built into the body instead of the left
    // leaf, which the position check above cannot tell apart from the leaf
    // sitting at the same spot. Built apart: the body without its leaves
    // (`movers: false`) carries no notice ink, and the left leaf alone
    // (`boxDoor`) carries all of it.
    const bodyParts: Part[] = [];
    buildHero(
      recordingKitAt(createBuilder(), bodyParts),
      "police-box",
      0,
      LOOK,
      false,
    );
    expect(inked(bodyParts, NOTICE_INK, "panel")).toHaveLength(0);
    const leafParts: Part[] = [];
    boxDoor(recordingKitAt(createBuilder(), leafParts), surfaces(LOOK), "left");
    expect(inked(leafParts, NOTICE_INK, "panel")).toHaveLength(
      runsOfLines(MARKS.boxNotice),
    );
  });
});

describe("the police box's doors (2.6e C10, C16)", () => {
  const hero: Hero = {
    kind: "police-box",
    variant: 0,
    x: 4.5,
    y: 6,
    turn: 0,
    seed: 1,
  };

  it("leaves the leaves out of the instanced mesh and hands them over as two wings", () => {
    // Mutation caught: buildHeroMesh still building the leaves (they would
    // be drawn twice, one copy never opening), or a leaf mover missing.
    const whole = createBuilder();
    buildHero((f) => createKit(whole, f), "police-box", 0, LOOK, true);
    const body = buildHeroMesh("police-box", 0, LOOK);
    const wings = boxLeafMovers(hero, 7, LOOK);
    expect(wings.map((m) => m.part)).toEqual(["wing", "wing"]);
    expect(new Set(wings.map((m) => m.key))).toEqual(new Set(["box:7"]));
    expect(wings.every((m) => m.fixture === -1)).toBe(true);
    expect(body.count + wings.reduce((n, m) => n + m.mesh.count, 0)).toBe(
      whole.build().count,
    );
  });

  it("swings each leaf's inner edge deep into the box, never out of its front", () => {
    // Mutation caught: a swing sign that opens a leaf outward, both leaves
    // turning the same way, or a swing of 0 (the doors never open).
    const wings = boxLeafMovers(hero, 0, LOOK);
    expect(wings.length).toBe(2);
    expect((wings[0]?.swing ?? 0) * (wings[1]?.swing ?? 0)).toBeLessThan(0);
    const box = heroFootprint(hero);
    for (const m of wings) {
      const pivot = m.pivot ?? [0, 0, 0];
      const points = positions(m.mesh);
      expect(points.length).toBeGreaterThan(0);
      for (const p of points) {
        const q = swungPoint(p, pivot, m.swing);
        // turn 0: the front faces north (-z), so the front plane is box.z0
        // and "deeper into the box" is a larger z.
        expect(q[2]).toBeGreaterThanOrEqual(box.z0 - 1e-6);
        expect(q[0]).toBeGreaterThanOrEqual(box.x0 - 1e-6);
        expect(q[0]).toBeLessThanOrEqual(box.x1 + 1e-6);
      }
      // The leaf's inner edge: its vertex farthest from the hinge along x.
      const inner = points.reduce((a, b) =>
        Math.abs(b[0] - pivot[0]) > Math.abs(a[0] - pivot[0]) ? b : a,
      );
      const swung = swungPoint(inner, pivot, m.swing);
      expect(swung[2] - inner[2]).toBeGreaterThanOrEqual(0.3);
    }
  });

  it("keeps both leaves inside the free-standing box's footprint at every turn, swinging deeper in (2.6e)", () => {
    // Mutation caught: a leaf's frame or pivot that ignores the box's turn
    // or its centring on the anchor (a leaf drawn or hinged half a box
    // away, or outside the box at turns 1 to 3), or a swing that opens a
    // leaf outward at some turn.
    for (const turn of [0, 1, 2, 3]) {
      const h: Hero = { ...hero, turn };
      const box = heroFootprint(h);
      const front = boxFront(h);
      const depth = (q: V3) =>
        (q[0] - front.x) * front.inward[0] + (q[2] - front.z) * front.inward[1];
      const wings = boxLeafMovers(h, 0, LOOK);
      expect(wings.length).toBe(2);
      for (const m of wings) {
        const pivot = m.pivot ?? [0, 0, 0];
        const points = positions(m.mesh);
        expect(points.length).toBeGreaterThan(0);
        for (const p of points)
          for (const q of [p, swungPoint(p, pivot, m.swing)]) {
            expect(q[0], `turn ${String(turn)}`).toBeGreaterThanOrEqual(
              box.x0 - 1e-6,
            );
            expect(q[0], `turn ${String(turn)}`).toBeLessThanOrEqual(
              box.x1 + 1e-6,
            );
            expect(q[2], `turn ${String(turn)}`).toBeGreaterThanOrEqual(
              box.z0 - 1e-6,
            );
            expect(q[2], `turn ${String(turn)}`).toBeLessThanOrEqual(
              box.z1 + 1e-6,
            );
          }
        // The hinge stands on the box's front, just behind the door plane.
        const hinge: V3 = [pivot[0], 0, pivot[2]];
        expect(depth(hinge)).toBeLessThan(0);
        expect(depth(hinge)).toBeGreaterThan(-0.1);
        const inner = points.reduce((a, b) =>
          Math.hypot(b[0] - pivot[0], b[2] - pivot[2]) >
          Math.hypot(a[0] - pivot[0], a[2] - pivot[2])
            ? b
            : a,
        );
        const swung = swungPoint(inner, pivot, m.swing);
        expect(
          depth(inner) - depth(swung),
          `turn ${String(turn)}`,
        ).toBeGreaterThanOrEqual(0.3);
      }
    }
  });

  it("lights the hollow inside white behind the doors", () => {
    // Mutation caught: the body left solid (the open doorway would show a
    // blue wall), or the inside lit with a lit surface that reads dark.
    // Only a light inside the body counts: behind the door leaves, in
    // front of the back wall and between the side walls, under the sign
    // band, so the sign's white words on the four faces never pass it.
    const inside = partsOf("police-box").filter(
      (p) =>
        p.flag === FLAG.signal &&
        p.tint !== null &&
        p.tint[2] > 0.85 &&
        p.tint[0] > 0.9 &&
        local(p).every(
          (q) =>
            Math.abs(q[0]) < 0.6 &&
            q[1] > BACK &&
            q[1] < FRONT - 0.1 &&
            q[2] < 2.08,
        ),
    );
    expect(inside.length).toBeGreaterThan(0);
  });
});
