/**
 * The retro curios' shape tests: what `curioModels.test.ts` does not check
 * for every kind. The console's two screen pictures never share a quad,
 * show dark pixels on a pale screen, and its battery light sits left of
 * the screen; the tape drive has five keys along its front edge, one of
 * them the darker record key, and stands 0.045 high there; the tape player
 * has two foam pads, one orange button, its buttons flush with its top end
 * and its label strip along its hubs; the video tape carries no text, and
 * bare it shows two reels; the laptop's "<=>" is exactly the font's runs,
 * lying on the back plane of its tilted lid, its lid and mark keep the
 * brief's numbers, its two drive slots sit either side of the latch, its
 * keys stand in five rows before a palm rest, and it stays under 900
 * triangles. The console, the tape drive and the tape player carry their
 * originals' badges (`MARKS`, 2.6f C13): each rebuilt from its parts
 * cell by cell must be the font's own rows of its word, on the face the
 * original prints it on, one `MARK_PROUD` off that face and at least
 * 1 mm a pixel.
 *
 * Parts are told apart by their colour: `recordingKitAt` keeps each kit
 * call's surface tint.
 */

import { describe, expect, it } from "vitest";

import type { CurioKind } from "../../../world/types";
import { FLAG, createBuilder, type V3 } from "../../geometry";
import { DECAL_LIFT, frameAt } from "../../kit";
import { TEXT_BASE } from "../../layers";
import { LOOK, type Rgb } from "../../looks";
import {
  cellsOf,
  inked,
  recordingKitAt,
  runsOfLines,
  toLocal,
  trimmed,
  type Part,
} from "../../modelChecks";
import { MARK_PROUD, pixelRuns, textRows } from "../heroes/pixels";
import { MARKS } from "../marks";
import { buildCurio, buildCurioMesh } from ".";
import { curioHalf } from "./common";
import {
  CONSOLE_BADGE_INK,
  CONSOLE_LED,
  CONSOLE_PLAY,
  CONSOLE_SCREEN,
  CONSOLE_TITLE,
  DRIVE_BADGE_INK,
  DRIVE_KEY,
  DRIVE_RECORD_KEY,
  DRIVE_WINDOW,
  LAPTOP_KEY,
  LAPTOP_LATCH,
  LAPTOP_LID,
  LAPTOP_MARK,
  LAPTOP_SLOT,
  MARK_PX,
  PLAYER_BADGE_INK,
  PLAYER_FOAM,
  PLAYER_LABEL,
  PLAYER_ORANGE,
  PLAYER_SMOKE,
  TAPE_REEL,
} from "./retro";

/**
 * A curio's recorded parts (`recordingKitAt`, which keeps each surface's
 * tint), built at the origin at turn 0, their points put into the
 * recipe's local `[a, d, h]`.
 */
function partsOf(kind: CurioKind, variant = 0): Part[] {
  const parts: Part[] = [];
  buildCurio(recordingKitAt(createBuilder(), parts), kind, variant, LOOK);
  const origin = frameAt([0, 0, 0], 0);
  return parts.map((p) => ({
    ...p,
    points: p.points.map((q) => toLocal(origin, q)),
  }));
}

/** Whether a part is drawn in exactly this colour. */
const inTint = (p: Part, c: Rgb) =>
  p.tint !== null && p.tint.every((x, i) => x === c[i]);

/** A part's bounds in `[a, d, h]`: lowest and highest of each. */
function bounds(p: Part): { lo: V3; hi: V3 } {
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
function overlapAH(p: Part, q: Part): boolean {
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

/** Every point of some parts. */
const pointsOf = (parts: readonly Part[]): V3[] =>
  parts.flatMap((p) => p.points);

describe("retro curio models", () => {
  describe("pocket console", () => {
    const parts = partsOf("pocket-console");
    const group = (p: Part) => p.flag - FLAG.blink;
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

    it("shows dark pixels on a pale screen: each pixel the screen's own tint, dark only while its group is low", () => {
      const screens = parts.filter(
        (p) => p.flag === FLAG.signal && inTint(p, CONSOLE_SCREEN),
      );
      expect(screens).toHaveLength(1);
      const pixels = [...title, ...play];
      expect(pixels.length).toBeGreaterThan(0);
      for (const p of pixels) expect(inTint(p, CONSOLE_SCREEN)).toBe(true);
      // The screen is pale, so a pixel at the swap bank's low gain reads dark.
      expect(Math.min(...CONSOLE_SCREEN)).toBeGreaterThan(0.4);
      const screen = bounds(screens[0] as Part);
      for (const p of pixels) {
        const b = bounds(p);
        expect(b.lo[0]).toBeGreaterThanOrEqual(screen.lo[0]);
        expect(b.hi[0]).toBeLessThanOrEqual(screen.hi[0]);
        expect(b.lo[2]).toBeGreaterThanOrEqual(screen.lo[2]);
        expect(b.hi[2]).toBeLessThanOrEqual(screen.hi[2]);
        expect(b.lo[1]).toBeCloseTo(screen.hi[1] + DECAL_LIFT, 9);
      }
    });

    it("lights its battery light left of the screen", () => {
      const leds = parts.filter((p) => inTint(p, CONSOLE_LED));
      expect(leds).toHaveLength(1);
      expect(leds[0]?.flag).toBe(FLAG.signal);
      const led = bounds(leds[0] as Part);
      const pixels = [...title, ...play].map(bounds);
      const screenLeft = Math.min(...pixels.map((b) => b.lo[0]));
      const screenLow = Math.min(...pixels.map((b) => b.lo[2]));
      const screenHigh = Math.max(...pixels.map((b) => b.hi[2]));
      expect(led.hi[0]).toBeLessThan(screenLeft);
      expect(led.lo[2]).toBeGreaterThan(screenLow);
      expect(led.hi[2]).toBeLessThan(screenHigh);
    });

    it("prints the console's name and maker on the grey under its bezel, one mark proud, left to right (2.6f C13)", () => {
      // Mutation caught: the grey left bare, a word dropped, the text over
      // the screen, turned about or mirrored, the words swapped, floating
      // or sunk, or under the 1 mm floor. The console
      // stands upright facing `+d`, so "under the screen" is along `h`.
      expect(runsOfLines(MARKS.consoleBadge)).toBeGreaterThan(3);
      const ink = inked(parts, CONSOLE_BADGE_INK);
      expect(ink).toHaveLength(runsOfLines(MARKS.consoleBadge));
      for (const p of ink) {
        expect(p.method).toBe("panel");
        expect(p.flag).toBe(FLAG.lit);
      }
      const screen = pointsOf(parts.filter((p) => p.flag !== FLAG.lit));
      expect(screen.length).toBeGreaterThan(0);
      const screenBottom = Math.min(...screen.map((q) => q[2]));
      const pts = pointsOf(ink);
      const lo = Math.min(...pts.map((q) => q[2]));
      const hi = Math.max(...pts.map((q) => q[2]));
      expect(hi).toBeLessThanOrEqual(screenBottom + 1e-6);
      // The face the text lies on: the front of every other part that
      // spans the text's whole height.
      const face = Math.max(
        ...parts
          .filter((p) => !ink.includes(p))
          .map(bounds)
          .filter((b) => b.lo[2] <= lo && b.hi[2] >= hi)
          .map((b) => b.hi[1]),
      );
      for (const q of pts) expect(q[1]).toBeCloseTo(face + MARK_PROUD, 9);
      const { rows, px } = cellsOf(ink, (q) => [q[0], -q[2]]);
      expect(px).toBeGreaterThanOrEqual(0.001 - 1e-9);
      const [maker, name] = MARKS.consoleBadge.map((w) => trimmed(textRows(w)));
      if (!maker || !name) throw new Error("two words");
      const gap =
        (rows[0]?.length ?? 0) -
        (maker[0]?.length ?? 0) -
        (name[0]?.length ?? 0);
      expect(gap).toBeGreaterThanOrEqual(2);
      expect(rows).toEqual(
        maker.map((r, i) => r + ".".repeat(gap) + (name[i] ?? "")),
      );
    });
  });

  describe("tape drive", () => {
    it("has five keys along its front edge, the first the darker record key", () => {
      const { hd } = curioHalf("tape-drive", 0);
      const parts = partsOf("tape-drive");
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

    it("stands 0.045 high at its front edge", () => {
      const { hd } = curioHalf("tape-drive", 0);
      const front = partsOf("tape-drive")
        .filter((p) => p.method === "extrude")
        .flatMap((p) => p.points)
        .filter((q) => Math.abs(q[1] - hd) < 1e-9);
      expect(front.length).toBeGreaterThan(0);
      expect(Math.max(...front.map((q) => q[2]))).toBeCloseTo(0.045, 9);
    });

    it("prints its maker's word on the lid, in front of the window and behind the keys at the left, reading from the front (2.6f C13)", () => {
      // Mutation caught: the lid left bare, the word on the key deck or
      // over the window, on the right, turned about or mirrored, floating
      // over the lid or sunk into it, or under the 1 mm floor.
      const parts = partsOf("tape-drive");
      const ink = inked(parts, DRIVE_BADGE_INK);
      expect(ink.length).toBeGreaterThan(0);
      const { rows, px } = cellsOf(ink, (q) => [q[0], q[1]]);
      expect(rows).toEqual(trimmed(textRows(MARKS.driveBadge)));
      expect(px).toBeGreaterThanOrEqual(0.001 - 1e-9);
      const pts = pointsOf(ink);
      const keys = pointsOf(
        parts.filter(
          (p) => inTint(p, DRIVE_KEY) || inTint(p, DRIVE_RECORD_KEY),
        ),
      );
      const window = pointsOf(
        parts.filter((p) => inTint(p, DRIVE_WINDOW) && p.method === "extrude"),
      );
      expect(window.length).toBeGreaterThan(0);
      expect(Math.max(...pts.map((q) => q[1]))).toBeLessThan(
        Math.min(...keys.map((q) => q[1])),
      );
      expect(Math.min(...pts.map((q) => q[1]))).toBeGreaterThan(
        Math.max(...window.map((q) => q[1])),
      );
      expect(Math.max(...pts.map((q) => q[0]))).toBeLessThan(0);
      // The lid: the slab on the slope that spans the window's width and
      // more; its top runs straight from its back edge to its front.
      const lid = parts
        .filter((p) => p.method === "extrude")
        .map(bounds)
        .find(
          (b) =>
            b.lo[0] < Math.min(...window.map((q) => q[0])) - 0.02 &&
            b.hi[0] - b.lo[0] < 0.19 &&
            b.hi[1] < Math.min(...keys.map((q) => q[1])) &&
            b.lo[2] > 0.02,
        );
      if (!lid) throw new Error("the lid");
      const lidParts = parts.filter(
        (p) =>
          p.method === "extrude" &&
          bounds(p).lo[0] === lid.lo[0] &&
          bounds(p).hi[0] === lid.hi[0],
      );
      const lidPts = pointsOf(lidParts);
      const topAt = (d: number) =>
        Math.max(
          ...lidPts.filter((q) => Math.abs(q[1] - d) < 1e-9).map((q) => q[2]),
        );
      const [back, front] = [lid.lo[1], lid.hi[1]];
      const lidTop = (d: number) =>
        topAt(back) +
        ((d - back) / (front - back)) * (topAt(front) - topAt(back));
      for (const p of ink) {
        const off = p.points.map((q) => q[2] - lidTop(q[1]));
        expect(Math.min(...off)).toBeCloseTo(0, 9);
        expect(Math.max(...off)).toBeCloseTo(MARK_PROUD, 9);
      }
    });
  });

  describe("tape player", () => {
    const parts = partsOf("tape-player");

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
      expect(bounds(orange[0] as Part).hi[0]).toBeLessThan(-hw + 0.088);
    });

    it("sets its buttons flush with the top end edge, up to the curio's top", () => {
      const { top } = curioHalf("tape-player", 0);
      const orange = bounds(
        parts.find((p) => inTint(p, PLAYER_ORANGE)) as Part,
      );
      const buttons = parts
        .filter((p) => p.method === "box")
        .map(bounds)
        .filter(
          (b) =>
            Math.abs(b.hi[1] - orange.hi[1]) < 1e-9 &&
            Math.abs(b.hi[2] - top) < 1e-9,
        );
      expect(buttons).toHaveLength(4);
      const body = parts
        .filter((p) => p.method === "box")
        .map(bounds)
        .filter((b) => b.lo[2] === 0);
      const end = Math.max(...body.map((b) => b.hi[1]));
      expect(orange.hi[1]).toBeCloseTo(end, 9);
    });

    it("runs the cassette's label strip along the line of its two hubs", () => {
      const label = bounds(parts.find((p) => inTint(p, PLAYER_LABEL)) as Part);
      expect(label.hi[1] - label.lo[1]).toBeGreaterThan(
        3 * (label.hi[0] - label.lo[0]),
      );
    });

    it("prints the maker's word by the buttons and the model's at the far end, flat on the lid and reading from the front (2.6f C13)", () => {
      // Mutation caught: the lid left bare, a word dropped or over the
      // window, the two words swapped, turned about or mirrored, floating
      // over the lid or sunk into it, or under the 1 mm floor.
      const ink = inked(parts, PLAYER_BADGE_INK);
      const window = bounds(parts.find((p) => inTint(p, PLAYER_SMOKE)) as Part);
      const orange = bounds(
        parts.find((p) => inTint(p, PLAYER_ORANGE)) as Part,
      );
      const lidTop = window.lo[2];
      const mid = (p: Part) => (bounds(p).lo[1] + bounds(p).hi[1]) / 2;
      const maker = ink.filter((p) => mid(p) > 0);
      const model = ink.filter((p) => mid(p) < 0);
      expect(maker.length).toBeGreaterThan(0);
      expect(model.length).toBeGreaterThan(0);
      const [makerWord, modelWord] = MARKS.playerBadge;
      if (!makerWord || !modelWord) throw new Error("two words");
      for (const [part, word] of [
        [maker, makerWord],
        [model, modelWord],
      ] as const) {
        const { rows, px } = cellsOf(part, (q) => [q[0], q[1]]);
        expect(rows).toEqual(trimmed(textRows(word)));
        expect(px).toBeGreaterThanOrEqual(0.001 - 1e-9);
      }
      for (const p of ink) {
        const b = bounds(p);
        expect(p.method).toBe("box");
        expect(b.lo[2]).toBeCloseTo(lidTop, 9);
        expect(b.hi[2]).toBeCloseTo(lidTop + MARK_PROUD, 9);
        expect(b.lo[0]).toBeGreaterThanOrEqual(window.lo[0] - 0.008);
        expect(b.hi[0]).toBeLessThanOrEqual(window.hi[0] + 0.008);
      }
      const body = parts
        .filter((p) => p.method === "box")
        .map(bounds)
        .filter((b) => b.lo[2] === 0);
      const end = Math.min(...body.map((b) => b.lo[1]));
      for (const p of maker) {
        expect(bounds(p).lo[1]).toBeGreaterThan(window.hi[1]);
        expect(bounds(p).hi[1]).toBeLessThan(orange.lo[1]);
      }
      for (const p of model) {
        expect(bounds(p).hi[1]).toBeLessThan(window.lo[1]);
        expect(bounds(p).lo[1]).toBeGreaterThan(end);
      }
    });
  });

  describe("video tape", () => {
    it("carries no text and no decal in either variant", () => {
      for (const v of [0, 1]) {
        const parts = partsOf("video-tape", v);
        expect(parts.length).toBeGreaterThan(0);
        for (const p of parts) {
          expect(p.layer).toBeLessThan(TEXT_BASE);
          expect(p.method).not.toBe("panel");
        }
      }
    });

    it("shows two reels on the bare tape", () => {
      const reels = partsOf("video-tape", 1).filter((p) =>
        inTint(p, TAPE_REEL),
      );
      expect(reels).toHaveLength(2);
      const [a, b] = reels.map(bounds);
      expect(a && b && (a.hi[0] < b.lo[0] || b.hi[0] < a.lo[0])).toBe(true);
    });
  });

  describe("beige laptop", () => {
    const parts = partsOf("beige-laptop");

    it("stays under 900 triangles", () => {
      expect(buildCurioMesh("beige-laptop", 0, LOOK).count / 3).toBeLessThan(
        900,
      );
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

    it("pins the brief's numbers: 12 mm mark pixels, a lid 15 degrees back, hinged at 0.045", () => {
      expect(MARK_PX).toBe(0.012);
      expect((LAPTOP_LID.tilt * 180) / Math.PI).toBeCloseTo(15, 9);
      expect(LAPTOP_LID.hingeH).toBe(0.045);
    });

    it("has five rows of chunky keys and a palm rest in front of them", () => {
      const { hd } = curioHalf("beige-laptop", 0);
      const keys = parts.filter((p) => inTint(p, LAPTOP_KEY)).map(bounds);
      expect(keys.length).toBeGreaterThanOrEqual(48);
      expect(keys.length).toBeLessThanOrEqual(56);
      const rows = new Set(keys.map((b) => b.lo[1].toFixed(6)));
      expect(rows.size).toBe(5);
      const front = Math.max(...keys.map((b) => b.hi[1]));
      expect(hd - front).toBeGreaterThan(0.06);
    });
  });
});
