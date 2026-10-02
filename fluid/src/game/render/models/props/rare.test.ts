/**
 * The rare props' own shape tests: what `propModels.test.ts`'s generic
 * checks do not pin for the canister cluster, the saucer poster and the
 * designer tower. Parts are found by their lighting flag and their tint,
 * in the recipe's own local `(a, d, h)` terms. The cluster stands its
 * canisters with a glowing crack and the maker's letters each, lays one
 * down with its open mouth glowing and spreads the puddle from that
 * mouth; the poster paints the saucer over the trees and sets its caption
 * in the band under the picture; the tower curves its front out of the
 * box behind it and carries the red display low and the badge high.
 */

import { describe, expect, it } from "vitest";

import type { PropKind } from "../../../world/types";
import { blinkFlag, createBuilder, FLAG, type V3 } from "../../geometry";
import { frameAt } from "../../kit";
import { LOOK } from "../../looks";
import { recordingKitAt, toLocal, type Part } from "../../modelChecks";
import { pixelRuns, textRows } from "../heroes/pixels";
import { MARKS, SIGNATURE } from "../marks";
import { buildProp } from "./index";
import {
  BADGE_GREY,
  CAPTION_WHITE,
  CLOCK_RED,
  LABEL_DARK,
  LID_DARK,
  PIN_METAL,
  SAUCER_PICTURE,
} from "./rare";

/** A prop kind's recorded parts, built once at the origin. */
function recorded(kind: PropKind, variant: number): Part[] {
  const builder = createBuilder();
  const parts: Part[] = [];
  buildProp(recordingKitAt(builder, parts), kind, variant, {
    look: LOOK,
  });
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

describe("the rare props' models", () => {
  it("stands two canisters and lays one down in v0, one and one in v1, the puddle on the floor at the tipped one's mouth", () => {
    // Mutation caught: a canister missing, the puddle off the floor, or the
    // puddle away from the mouth.
    for (const [v, standing] of [
      [0, 2],
      [1, 1],
    ] as const) {
      const parts = recorded("ooze-canisters", v);
      // The ridged cap alone reaches over 0.41 m: one per standing canister.
      const lids = parts.filter(
        (p) =>
          p.tint?.join() === LID_DARK.join() && span(local(p), 2).hi > 0.41,
      );
      expect(lids.length, `lids ${String(v)}`).toBe(standing);
      const glow = parts.filter((p) => p.flag === blinkFlag(0));
      const films = glow.filter((p) => span(local(p), 2).hi <= 0.006);
      expect(films.length, `films ${String(v)}`).toBeGreaterThanOrEqual(2);
      const mouth = glow.filter((p) => {
        const q = local(p);
        // The disc in the tipped canister's mouth: flat across `a`, wide in `d`.
        return (
          span(q, 0).hi - span(q, 0).lo < 0.01 &&
          span(q, 1).hi - span(q, 1).lo > 0.15
        );
      });
      expect(mouth.length, `mouth ${String(v)}`).toBe(1);
      const m = mouth[0];
      if (m === undefined) throw new Error("a mouth");
      const ma = span(local(m), 0).mid;
      for (const f of films)
        expect(span(local(f), 0).lo, `film ${String(v)}`).toBeGreaterThan(
          ma - 0.15,
        );
      const cracks = glow.filter(
        (p) =>
          span(local(p), 2).lo > 0.05 &&
          span(local(p), 2).hi - span(local(p), 2).lo > 0.1,
      );
      expect(cracks.length, `cracks ${String(v)}`).toBe(standing);
      const label = parts.filter((p) => p.tint?.join() === LABEL_DARK.join());
      expect(label.length, `label ${String(v)}`).toBe(
        standing * pixelRuns(textRows(MARKS.canister)).length,
      );
    }
  });

  it("paints the saucer over the trees under a pale sky, with lights only under the disc", () => {
    // Mutation caught: the saucer below the tree line, or lights with no
    // disc above them.
    expect(SAUCER_PICTURE).toHaveLength(26);
    for (const r of SAUCER_PICTURE) expect(r).toHaveLength(24);
    const rowsWith = (ch: string) =>
      SAUCER_PICTURE.flatMap((r, y) => (r.includes(ch) ? [y] : []));
    expect(rowsWith("u").length).toBeGreaterThan(0);
    expect(Math.max(...rowsWith("u"), ...rowsWith("d"))).toBeLessThan(
      Math.min(...rowsWith("t")),
    );
    expect(
      [...(SAUCER_PICTURE[SAUCER_PICTURE.length - 1] ?? "")].every(
        (c) => c === "t",
      ),
    ).toBe(true);
    SAUCER_PICTURE.forEach((r, y) =>
      [...r].forEach((c, x) => {
        if (c !== "g") return;
        expect(
          SAUCER_PICTURE.slice(0, y).some(
            (up) => up[x] === "d" || up[x] === "u",
          ),
          `${String(x)},${String(y)}`,
        ).toBe(true);
      }),
    );
  });

  it("hangs the sheet from 1.1 to 2.0 m, the caption as the font's runs in the band under the picture, four pins and one curled corner", () => {
    // Mutation caught: the caption above the picture, a pin missing, or a
    // flat corner.
    const parts = recorded("saucer-poster", 0);
    const pts = parts.flatMap(local);
    expect(span(pts, 2).lo).toBeGreaterThanOrEqual(1.1 - 1e-6);
    expect(span(pts, 2).hi).toBeLessThanOrEqual(2.0 + 1e-6);
    const caption = parts.filter(
      (p) => p.tint?.join() === CAPTION_WHITE.join(),
    );
    expect(caption).toHaveLength(pixelRuns(textRows(MARKS.poster)).length);
    for (const p of caption) expect(span(local(p), 2).hi).toBeLessThan(1.36);
    expect(
      parts.filter((p) => p.tint?.join() === PIN_METAL.join()),
    ).toHaveLength(4);
    expect(
      parts.some((p) => p.method === "extrude" && span(local(p), 1).hi > 0.015),
    ).toBe(true);
  });

  it("shows the 40 in red low on the curved front, and the badge and signature near the top", () => {
    // Mutation caught: the display unlit, or the badge set as a plain bar.
    const parts = recorded("designer-tower", 0);
    const red = parts.filter(
      (p) => p.flag === FLAG.signal && p.tint?.join() === CLOCK_RED.join(),
    );
    expect(red).toHaveLength(pixelRuns(textRows(MARKS.towerClock)).length);
    for (const p of red) expect(span(local(p), 2).hi).toBeLessThan(0.4);
    const badge = parts.filter((p) => p.tint?.join() === BADGE_GREY.join());
    expect(badge).toHaveLength(
      pixelRuns(textRows(MARKS.towerBadge)).length +
        pixelRuns(SIGNATURE).length,
    );
    for (const p of badge) expect(span(local(p), 2).lo).toBeGreaterThan(0.45);
  });

  it("curves the tower's front out from the box behind it", () => {
    // Mutation caught: a flat box front.
    const pts = recorded("designer-tower", 0).flatMap(local);
    const frontAt = (h0: number, h1: number) =>
      Math.max(...pts.filter((q) => q[2] >= h0 && q[2] <= h1).map((q) => q[1]));
    expect(frontAt(0.1, 0.3)).toBeGreaterThan(frontAt(0.58, 0.62) + 0.03);
  });
});
