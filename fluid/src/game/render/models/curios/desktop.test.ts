/**
 * The desktop curios' own shape tests: what `curioModels.test.ts`'s
 * generic checks do not pin for the breadbin computer, the slim computer
 * and the space bricks. Parts are found by their lighting flag, their
 * method and their tint, in the recipe's own local `(a, d, h)` terms.
 * The breadbin slopes from its high rounded back to a low front, keeps
 * four grey function keys in a column on the right and a long space bar
 * at the front, and carries the badge (its lettering, the five stacked
 * stripes between the word and the number) and the red power light on
 * the wall behind its keys. The slim computer is lower and deeper, carries
 * no stripe, and keeps the same lettering and power light. The space
 * bricks put studs on every brick's top, and one red figure with its
 * chest badge stands beside the ship, with nothing lit.
 */

import { describe, expect, it } from "vitest";

import type { CurioKind } from "../../../world/types";
import { createBuilder, FLAG, type V3 } from "../../geometry";
import { frameAt } from "../../kit";
import { LOOKS } from "../../looks";
import { recordingKitAt, toLocal, type Part } from "../../modelChecks";
import { pixelRuns, textRows } from "../heroes/pixels";
import { MARKS } from "../marks";
import { curioHalf } from "./common";
import {
  BADGE_TEXT,
  BADGE_YELLOW,
  FIGURE_RED,
  FUNCTION_GREY,
  KEY_BROWN,
  LOGO_TINT,
  POWER_RED,
  RAINBOW,
  SHIP_BRICKS,
} from "./desktop";
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

/** The parts tinted exactly `c`. */
const tinted = (parts: readonly Part[], c: readonly number[]) =>
  parts.filter((p) => p.tint?.join() === c.join());

describe("the desktop models", () => {
  it("slopes the breadbin's case to the front and keeps its four grey function keys on the right", () => {
    // Mutation caught: a flat box case, or the function keys on the left
    // or brown.
    const parts = recorded("breadbin-computer");
    const pts = parts.flatMap(local);
    const back = Math.max(...pts.filter((q) => q[1] < -0.07).map((q) => q[2]));
    const front = Math.max(...pts.filter((q) => q[1] > 0.08).map((q) => q[2]));
    expect(back).toBeGreaterThan(front + 0.02);
    const fn = tinted(parts, FUNCTION_GREY);
    expect(fn).toHaveLength(4);
    for (const p of fn) expect(span(local(p), 0).lo).toBeGreaterThan(0.1);
    expect(parts.some((p) => p.tint?.join() === KEY_BROWN.join())).toBe(true);
  });

  it("lays the breadbin's keys out in rows with a long space bar at the front", () => {
    // Mutation caught: a keyboard of equal keys, a missing space bar, or
    // a space bar set in the back row.
    const keys = tinted(recorded("breadbin-computer"), KEY_BROWN).map(local);
    expect(keys.length).toBeGreaterThanOrEqual(60);
    const width = (q: V3[]) => span(q, 0).hi - span(q, 0).lo;
    const widest = keys.reduce((w, q) => (width(q) > width(w) ? q : w));
    expect(width(widest)).toBeGreaterThan(0.15);
    const frontmost = Math.max(...keys.map((q) => span(q, 1).mid));
    expect(span(widest, 1).mid).toBeCloseTo(frontmost, 6);
    // More than one key width: the wide keys of the real layout.
    const widths = new Set(keys.map((q) => width(q).toFixed(4)));
    expect(widths.size).toBeGreaterThanOrEqual(4);
  });

  it("gives the breadbin the rainbow badge and a red power light, and the slim one neither stripe", () => {
    // Mutation caught: the stripes dropped, put on the slim computer, or the
    // power light drawn unlit.
    const bread = recorded("breadbin-computer");
    for (const c of RAINBOW)
      expect(
        bread.some((p) => p.tint?.join() === c.join()),
        c.join(),
      ).toBe(true);
    const slim = recorded("slim-computer");
    for (const c of RAINBOW)
      expect(
        slim.some((p) => p.tint?.join() === c.join()),
        c.join(),
      ).toBe(false);
    for (const parts of [bread, slim]) {
      const lights = parts.filter((p) => p.flag === FLAG.signal);
      expect(lights).toHaveLength(1);
      expect(lights[0]?.tint).toEqual(POWER_RED);
    }
  });

  it("stacks the five stripes red over blue between the word and the number, and puts the breadbin's light right and the slim one's left", () => {
    // Mutation caught: the stripes in the wrong order or beside the whole
    // line of text, or a power light on the wrong side or in front of
    // the keys.
    expect(RAINBOW).toHaveLength(5);
    const bread = recorded("breadbin-computer");
    const tops = RAINBOW.map((c) => {
      const [stripe] = tinted(bread, c);
      if (stripe === undefined) throw new Error(`a stripe ${c.join()}`);
      return span(local(stripe), 2).mid;
    });
    for (let i = 1; i < tops.length; i++)
      expect(tops[i]).toBeLessThan(tops[i - 1] ?? 0);
    const stripes = RAINBOW.flatMap((c) => tinted(bread, c)).flatMap(local);
    const text = tinted(bread, BADGE_TEXT["breadbin-computer"]).map(local);
    // Mutation caught: the badge's letters dropped, which would leave the
    // side checks below with nothing to hold.
    expect(text.length).toBeGreaterThan(0);
    const left = text.filter((q) => span(q, 0).hi < span(stripes, 0).lo);
    const right = text.filter((q) => span(q, 0).lo > span(stripes, 0).hi);
    // Mutation caught: the word or the number moved to the stripes' other
    // side, so one side holds no letters.
    expect(left.length).toBeGreaterThan(0);
    expect(right.length).toBeGreaterThan(0);
    expect(left.length + right.length).toBe(text.length);
    for (const [kind, side] of [
      ["breadbin-computer", 1],
      ["slim-computer", -1],
    ] as const) {
      const parts = recorded(kind);
      const light = parts.find((p) => p.flag === FLAG.signal);
      if (light === undefined) throw new Error(`${kind}: a power light`);
      expect(Math.sign(span(local(light), 0).mid), kind).toBe(side);
    }
    const keys = tinted(bread, KEY_BROWN).flatMap(local);
    const light = bread.find((p) => p.flag === FLAG.signal);
    if (light === undefined) throw new Error("a power light");
    expect(span(local(light), 1).hi).toBeLessThan(
      Math.min(...keys.map((q) => q[1])),
    );
  });

  it("sets the badge's text as the font's runs on both computers", () => {
    // Mutation caught: a badge that is a plain bar, not the text.
    const runs = pixelRuns(textRows(MARKS.computerBadge)).length;
    // Mutation caught: a badge line too short to hold a word, which would
    // let a bar of a few runs pass the count below.
    expect(runs).toBeGreaterThan(20);
    for (const kind of ["breadbin-computer", "slim-computer"] as const) {
      const text = recorded(kind).filter(
        (p) =>
          p.method === "panel" && p.tint?.join() === BADGE_TEXT[kind].join(),
      );
      expect(text.length, kind).toBe(runs);
    }
  });

  it("puts the maker's logo left of the word on the breadbin's badge, and none on the slim one", () => {
    // Mutation caught: the logo dropped, drawn after the word, or put on
    // the slim computer, whose original badge carries none.
    const bread = recorded("breadbin-computer");
    const logo = tinted(bread, LOGO_TINT).flatMap(local);
    expect(logo.length).toBeGreaterThan(0);
    const text = tinted(bread, BADGE_TEXT["breadbin-computer"]).flatMap(local);
    expect(span(logo, 0).hi).toBeLessThan(span(text, 0).lo);
    expect(tinted(recorded("slim-computer"), LOGO_TINT)).toHaveLength(0);
  });

  it("makes the slim computer lower and deeper than the breadbin", () => {
    // Mutation caught: the two catalogue sizes swapped or made equal.
    const b = curioHalf("breadbin-computer", 0);
    const s = curioHalf("slim-computer", 0);
    expect(s.top).toBeLessThan(b.top);
    expect(s.hd).toBeGreaterThan(b.hd);
  });

  it("puts studs on every brick's top face, and one red figure with its badge beside the ship", () => {
    // Mutation caught: a brick without studs, or the figure missing its badge.
    expect(SHIP_BRICKS.length).toBeGreaterThanOrEqual(6);
    expect(SHIP_BRICKS.length).toBeLessThanOrEqual(10);
    const parts = recorded("space-bricks");
    const studs = parts.filter((p) => p.method === "lathe");
    expect(studs.length).toBeGreaterThan(0);
    for (const b of SHIP_BRICKS) {
      const on = studs.filter((p) => {
        const q = local(p);
        const a = span(q, 0).mid;
        const d = span(q, 1).mid;
        return (
          Math.abs(span(q, 2).lo - b.h1) < 1e-6 &&
          a > b.a0 &&
          a < b.a1 &&
          d > b.d0 &&
          d < b.d1
        );
      });
      expect(on.length, JSON.stringify(b)).toBeGreaterThan(0);
    }
    // Mutation caught: the figure dropped, or its chest badge dropped.
    expect(parts.some((p) => p.tint?.join() === FIGURE_RED.join())).toBe(true);
    expect(parts.some((p) => p.tint?.join() === BADGE_YELLOW.join())).toBe(
      true,
    );
    // Mutation caught: the canopy or a clear stud drawn glowing or as a
    // signal light; the toy has no light at all.
    expect(parts.some((p) => p.flag !== FLAG.lit)).toBe(false);
  });

  it("stacks the ship in three layers and studs the baseplate on its full grid", () => {
    // Mutation caught: a flat one-layer ship, or a baseplate whose studs
    // are thinned out to a sparse grid.
    const layers = new Set(SHIP_BRICKS.map((b) => b.h0.toFixed(5)));
    expect(layers.size).toBeGreaterThanOrEqual(3);
    const parts = recorded("space-bricks");
    const plate = parts.find((p) => p.method === "box");
    if (plate === undefined) throw new Error("a baseplate");
    const plateTop = span(local(plate), 2).hi;
    const plateStuds = parts
      .filter(
        (p) =>
          p.method === "lathe" &&
          Math.abs(span(local(p), 2).lo - plateTop) < 1e-6,
      )
      .map((p) => [span(local(p), 0).mid, span(local(p), 1).mid] as const);
    expect(plateStuds.length).toBeGreaterThanOrEqual(60);
    const pitch = Math.min(
      ...plateStuds.flatMap(([a, d], i) =>
        plateStuds.slice(i + 1).map(([b, e]) => Math.hypot(a - b, d - e)),
      ),
    );
    // One stud pitch: the narrowest brick is one stud wide.
    const studPitch = Math.min(...SHIP_BRICKS.map((b) => b.a1 - b.a0));
    expect(pitch).toBeLessThanOrEqual(studPitch + 1e-9);
  });

  it("stands the figure beside the ship on the desk, six centimetres tall at the toy's scale, its badge on its chest", () => {
    // Mutation caught: a figure inside the ship, off the desk, a giant,
    // or its badge on its back.
    const parts = recorded("space-bricks");
    const red = tinted(parts, FIGURE_RED).flatMap(local);
    expect(red.length).toBeGreaterThan(0);
    const shipEnd = Math.max(...SHIP_BRICKS.map((b) => b.a1));
    expect(span(red, 0).lo).toBeGreaterThan(shipEnd);
    expect(span(red, 2).lo).toBeCloseTo(0, 6);
    expect(span(red, 2).hi).toBeGreaterThan(0.05);
    expect(span(red, 2).hi).toBeLessThan(0.065);
    const badge = tinted(parts, BADGE_YELLOW).flatMap(local);
    expect(badge.length).toBeGreaterThan(0);
    expect(span(badge, 1).lo).toBeGreaterThan(span(red, 1).mid);
    expect(span(badge, 2).mid).toBeGreaterThan(span(red, 2).mid);
  });
});
