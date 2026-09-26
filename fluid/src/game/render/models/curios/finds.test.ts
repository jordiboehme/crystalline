/**
 * The finds' own shape tests: what `curioModels.test.ts`'s generic checks
 * do not pin for the treasure radar, the capsule case and the reactor
 * case. Parts are found by their lighting flag and by their tint, in the
 * recipe's own local `(a, d, h)` terms. The radar blinks three to five
 * dots on its round screen, one at the centre, each over a steady dim
 * orange base it never shows darker than. The capsule case stands
 * five capsules of five colours in a row, and its lid stands open at the
 * back with the maker's mark on its inner face. The reactor case breathes
 * its core and its segment ring in two groups, all inside the glass
 * lattice, and sets its plaque's line on the brass in three rows.
 */

import { describe, expect, it } from "vitest";

import type { CurioKind } from "../../../world/types";
import type { Rgb } from "../../looks";
import { blinkFlag, createBuilder, FLAG, type V3 } from "../../geometry";
import { frameAt } from "../../kit";
import { LOOKS } from "../../looks";
import { BLINK_LOW } from "../../blink";
import { recordingKitAt, toLocal, type Part } from "../../modelChecks";
import { pixelRuns, textRows } from "../heroes/pixels";
import { MARK_BLUE, MARKS } from "../marks";
import { curioHalf } from "./common";
import { CAPSULE_TINTS, GLASS_EDGE, PLAQUE, PLAQUE_TEXT } from "./finds";
import { buildCurio } from "./index";

/** A curio kind's recorded parts, built once at the origin. */
function recorded(kind: CurioKind, variant = 0): Part[] {
  const builder = createBuilder();
  const parts: Part[] = [];
  buildCurio(recordingKitAt(builder, parts), kind, variant, LOOKS.aperture);
  return parts;
}

/** A part's points in the recipe's local `[a, d, h]`. */
const local = (p: Part): V3[] =>
  p.points.map((q) => toLocal(frameAt([0, 0, 0], 0), q));

/** The low end, high end and middle of some points along axis `i`. */
const span = (pts: readonly V3[], i: 0 | 1 | 2) => {
  const v = pts.map((q) => q[i]);
  return {
    lo: Math.min(...v),
    hi: Math.max(...v),
    mid: (Math.min(...v) + Math.max(...v)) / 2,
  };
};

/** A colour's hue in degrees, 0 to 360: orange lies between about 15 and 50. */
function hue([r, g, b]: Rgb): number {
  const max = Math.max(r, g, b);
  const c = max - Math.min(r, g, b);
  if (c === 0) return 0;
  const h =
    max === r
      ? ((g - b) / c) % 6
      : max === g
        ? (b - r) / c + 2
        : (r - g) / c + 4;
  return (h * 60 + 360) % 360;
}

describe("the finds' models", () => {
  it("blinks three to five dots on the round screen, one at its centre", () => {
    // Mutation caught: a dot off the screen, no centre dot, or the dots
    // drawn steady.
    const parts = recorded("treasure-radar");
    const screen = parts.find((p) => p.flag === FLAG.emissive);
    if (screen === undefined) throw new Error("a glowing screen");
    const sa = span(local(screen), 0);
    const sd = span(local(screen), 1);
    const r = (sa.hi - sa.lo) / 2;
    const dots = parts.filter((p) => p.flag >= FLAG.blink);
    expect(dots.length).toBeGreaterThanOrEqual(3);
    expect(dots.length).toBeLessThanOrEqual(5);
    const at = dots.map(
      (p) => [span(local(p), 0).mid, span(local(p), 1).mid] as const,
    );
    for (const [a, d] of at)
      expect(Math.hypot(a - sa.mid, d - sd.mid)).toBeLessThan(r);
    expect(
      at.some(([a, d]) => Math.hypot(a - sa.mid, d - sd.mid) < 0.004),
    ).toBe(true);
  });

  it("sets every dot over a steady orange base it never shows darker than", () => {
    // Mutation caught: a dot's base removed, its base not orange or not
    // steady, or a dot whose low phase reads darker than its base.
    const parts = recorded("treasure-radar");
    const dots = parts.filter((p) => p.flag >= FLAG.blink);
    expect(dots.length).toBeGreaterThan(0);
    for (const dot of dots) {
      const q = local(dot);
      const [a, d, h] = [span(q, 0).mid, span(q, 1).mid, span(q, 2)];
      const bases = parts.filter((p) => {
        if (p.flag !== FLAG.signal || p.tint === null) return false;
        const b = local(p);
        return (
          hue(p.tint) >= 15 &&
          hue(p.tint) <= 50 &&
          span(b, 0).lo < a &&
          span(b, 0).hi > a &&
          span(b, 1).lo < d &&
          span(b, 1).hi > d &&
          span(b, 2).lo < h.lo &&
          span(b, 2).hi <= h.hi
        );
      });
      expect(
        bases,
        `a base under the dot at ${a.toFixed(3)}, ${d.toFixed(3)}`,
      ).toHaveLength(1);
      const base = bases[0]?.tint;
      const tint = dot.tint;
      if (!base || !tint) throw new Error("tinted dot and base");
      for (const i of [0, 1, 2] as const)
        expect((tint[i] ?? 0) * BLINK_LOW).toBeGreaterThanOrEqual(
          (base[i] ?? 0) * 0.9,
        );
    }
  });

  it("stands five capsules in a row across the tray, each in its own colour", () => {
    // Mutation caught: a capsule missing, or two of them one colour.
    expect(new Set(CAPSULE_TINTS.map((t) => t.join())).size).toBe(5);
    const parts = recorded("capsule-case");
    const centres = CAPSULE_TINTS.map((t) => {
      const body = parts.filter(
        (p) => p.tint !== null && p.tint.join() === t.join(),
      );
      expect(body.length, t.join()).toBeGreaterThan(0);
      return span(body.flatMap(local), 0).mid;
    });
    const sorted = [...centres].sort((x, y) => x - y);
    for (let i = 1; i < sorted.length; i++)
      expect((sorted[i] ?? 0) - (sorted[i - 1] ?? 0)).toBeGreaterThan(0.025);
  });

  it("opens the lid upright at the back, the mark on its inner face", () => {
    // Mutation caught: the lid shut flat, or the mark on its outside.
    const parts = recorded("capsule-case");
    const { top } = curioHalf("capsule-case", 0);
    const lid = parts.filter(
      (p) =>
        span(local(p), 2).hi > top - 0.01 &&
        p.tint?.join() !== MARK_BLUE.join(),
    );
    expect(lid.length).toBeGreaterThan(0);
    const lidFront = Math.max(...lid.flatMap(local).map((q) => q[1]));
    expect(lidFront).toBeLessThan(-0.03);
    const marks = parts.filter(
      (p) => p.tint?.join() === MARK_BLUE.join() && span(local(p), 2).lo > 0.06,
    );
    expect(marks.length).toBeGreaterThan(0);
    for (const m of marks)
      expect(span(local(m), 1).lo).toBeGreaterThanOrEqual(lidFront - 1e-6);
  });

  it("breathes the reactor's core in group 0 and its segments in group 1, all inside the glass", () => {
    // Mutation caught: the segments in the core's group, fewer than eight,
    // or a light outside the case.
    const parts = recorded("reactor-case");
    const core = parts.filter((p) => p.flag === blinkFlag(0));
    const ring = parts.filter((p) => p.flag === blinkFlag(1));
    expect(core.length).toBeGreaterThan(0);
    expect(ring.length).toBeGreaterThanOrEqual(8);
    const glass = parts
      .filter((p) => p.tint?.join() === GLASS_EDGE.join())
      .flatMap(local);
    expect(glass.length).toBeGreaterThan(0);
    const [ga, gd, gh] = [span(glass, 0), span(glass, 1), span(glass, 2)];
    for (const q of [...core, ...ring].flatMap(local)) {
      expect(q[0]).toBeGreaterThan(ga.lo);
      expect(q[0]).toBeLessThan(ga.hi);
      expect(q[1]).toBeGreaterThan(gd.lo);
      expect(q[1]).toBeLessThan(gd.hi);
      expect(q[2]).toBeLessThan(gh.hi);
    }
  });

  it("sets the plaque's line on the brass in three rows", () => {
    // Mutation caught: the line dropped or cut short, set in one row, or
    // a letter off the brass.
    const parts = recorded("reactor-case");
    const plaque = parts.filter((p) => p.tint?.join() === PLAQUE.join());
    expect(plaque).toHaveLength(1);
    const face = plaque[0];
    if (face === undefined) throw new Error("a plaque");
    const pa = span(local(face), 0);
    const pd = span(local(face), 1);
    const ph = span(local(face), 2);
    const text = parts.filter((p) => p.tint?.join() === PLAQUE_TEXT.join());
    // Every lit run of the whole line, and no more: a run never crosses a
    // glyph's dark column, so cutting the line at its spaces keeps them all.
    expect(text).toHaveLength(pixelRuns(textRows(MARKS.reactorPlaque)).length);
    for (const t of text) {
      const q = local(t);
      expect(span(q, 1).lo).toBeGreaterThanOrEqual(pd.hi - 1e-6);
      expect(span(q, 0).lo).toBeGreaterThanOrEqual(pa.lo - 1e-6);
      expect(span(q, 0).hi).toBeLessThanOrEqual(pa.hi + 1e-6);
      expect(span(q, 2).lo).toBeGreaterThanOrEqual(ph.lo - 1e-6);
      expect(span(q, 2).hi).toBeLessThanOrEqual(ph.hi + 1e-6);
    }
    // Three lines of five pixel rows each: fifteen distinct row tops.
    const tops = new Set(text.map((t) => span(local(t), 2).hi.toFixed(5)));
    expect(tops.size).toBe(15);
  });
});
