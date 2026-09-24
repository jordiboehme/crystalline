/**
 * The doors: an outgoing relation, styled by its target's salience.
 *
 * - A sliding door (salience 0 to 3) is two light panels that part
 *   sideways in a thin frame, with seams in the look's door colour and a
 *   track in the floor.
 * - A bulkhead (4 to 6) is one heavy panel that rises into the ceiling, in
 *   a thick bevelled frame over a hazard-striped sill, with a handwheel.
 * - A blast door (7 to 10) is two thick panels with chevron faces that
 *   split at the middle, one up and one down, in a massive frame lined with
 *   a neon border.
 *
 * The frame, sill and label go into the room's static mesh; the panels of
 * an open way come back as movers (`door:<index>`) for the renderer to
 * slide. A sealed door (no address) keeps its panels in the static mesh,
 * returns no movers and carries a hazard plate across them.
 */

import type { Fixture } from "../../world/types";
import { FLAG, createBuilder, type Surface, type V3 } from "../geometry";
import { createKit, frameForSlot, type Frame, type Kit } from "../kit";
import { LAYER } from "../layers";
import {
  label,
  shade,
  surfaces,
  type KitAt,
  type ModelContext,
  type Mover,
  type Surfaces,
} from "./common";

type Door = Extract<Fixture, { kind: "door" }>;

/** How far each sliding panel slides, and how far a bulkhead rises. */
export const SLIDE_TRAVEL = 0.8;
export const BULKHEAD_TRAVEL = 2.3;
/** How far each half of a blast door moves, up and down. */
export const BLAST_TRAVEL = 1.3;

/** The label's width and the depth of its plate. */
const LABEL_HALF = 0.9;

const UP: V3 = [0, 1, 0];
const DOWN: V3 = [0, -1, 0];
const neg = (v: V3): V3 => [-v[0], -v[1], -v[2]];

/**
 * Where a door's panels go: into movers of their own for an open way, or
 * into the static mesh for a sealed one.
 */
interface Panels {
  /** Builds one panel into a kit; for an open door, a mover along `axis`. */
  add(axis: V3, travel: number, build: (k: Kit) => void): void;
  movers: Mover[];
}

function panels(kit: Kit, f: Frame, key: string, sealed: boolean): Panels {
  const movers: Mover[] = [];
  return {
    movers,
    add(axis, travel, build) {
      if (sealed) {
        build(kit);
        return;
      }
      const b = createBuilder();
      build(createKit(b, f));
      movers.push({ key, mesh: b.build(), axis, travel });
    },
  };
}

/** Builds a door against its wall slot and returns its movers. */
export function buildDoor(
  kitAt: KitAt,
  fx: Door,
  index: number,
  ctx: ModelContext,
): Mover[] {
  const f = frameForSlot(fx.slot);
  const k = kitAt(f);
  const key = `door:${index}`;
  const sealed = fx.address === null;
  const out = panels(k, f, key, sealed);
  const style = { sliding, bulkhead, blast }[fx.style];
  style(k, f, out, surfaces(ctx.look), ctx, sealed);
  label(k, ctx, key, -LABEL_HALF, LABEL_HALF, LABEL_BOTTOM, LABEL_DEPTH);
  return out.movers;
}

/** Where every door's label sits: its bottom edge and its plate's depth. */
const LABEL_BOTTOM = 2.62;
const LABEL_DEPTH = 0.1;

/** The sliding door's opening: half width and height. */
const SLIDE_HALF = 0.8;
const SLIDE_TOP = 2.4;
/** The sliding frame's outer half width and depth. */
const SLIDE_JAMB = 0.95;
const SLIDE_FRAME_D = 0.12;

function sliding(
  k: Kit,
  f: Frame,
  out: Panels,
  s: Surfaces,
  ctx: ModelContext,
  sealed: boolean,
) {
  const p = ctx.look.palette;
  const seam: Surface = { layer: LAYER.metal, tint: p.door, flag: FLAG.frame };
  const [j, t] = [SLIDE_JAMB, SLIDE_FRAME_D];
  // Jambs, lintel and the floor track.
  k.bevelBox(-j, -SLIDE_HALF, 0, t, 0, SLIDE_TOP + 0.15, 0.02, s.metal);
  k.bevelBox(SLIDE_HALF, j, 0, t, 0, SLIDE_TOP + 0.15, 0.02, s.metal);
  k.bevelBox(
    -SLIDE_HALF,
    SLIDE_HALF,
    0,
    t,
    SLIDE_TOP,
    SLIDE_TOP + 0.15,
    0.02,
    s.metal,
  );
  k.box(-j, j, 0, t + 0.02, 0, 0.02, s.dark);
  // Seams in the door colour along the inside of the frame.
  k.box(
    -SLIDE_HALF - 0.02,
    -SLIDE_HALF,
    t - 0.02,
    t + 0.005,
    0.02,
    SLIDE_TOP,
    seam,
  );
  k.box(
    SLIDE_HALF,
    SLIDE_HALF + 0.02,
    t - 0.02,
    t + 0.005,
    0.02,
    SLIDE_TOP,
    seam,
  );
  k.box(
    -SLIDE_HALF - 0.02,
    SLIDE_HALF + 0.02,
    t - 0.02,
    t + 0.005,
    SLIDE_TOP,
    SLIDE_TOP + 0.02,
    seam,
  );

  const leaf = s.tinted(p.door, LAYER.metal);
  for (const dir of [-1, 1] as const) {
    const [a0, a1] = dir < 0 ? [-SLIDE_HALF, 0] : [0, SLIDE_HALF];
    out.add(dir < 0 ? neg(f.along) : [...f.along], SLIDE_TRAVEL, (m) => {
      m.bevelBox(a0, a1, 0.03, 0.09, 0.02, SLIDE_TOP, 0.012, leaf);
      // A window slot, a kick plate and the pull recess by the meeting edge.
      m.box(a0 + 0.2, a1 - 0.2, 0.09, 0.095, 1.45, 1.75, s.dark);
      m.box(a0 + 0.05, a1 - 0.05, 0.09, 0.1, 0.1, 0.35, s.metal);
      const edge = dir < 0 ? a1 - 0.09 : a0 + 0.05;
      m.box(edge, edge + 0.04, 0.09, 0.105, 0.95, 1.25, s.dark);
    });
  }
  if (sealed) k.panel(-0.6, 0.6, 0.1, 0.9, 1.3, s.hazard, 1.2, 0.4);
}

/** The bulkhead's opening: half width, sill height and top. */
const BULK_HALF = 0.7;
const BULK_SILL = 0.18;
const BULK_TOP = 2.3;
/** The thick frame's outer half width and depth. */
const BULK_JAMB = 0.98;
const BULK_FRAME_D = 0.26;
/** The handwheel's height, radius and depth. */
const WHEEL_H = 1.25;
const WHEEL_R = 0.2;
const WHEEL_D = 0.26;

function bulkhead(
  k: Kit,
  _f: Frame,
  out: Panels,
  s: Surfaces,
  ctx: ModelContext,
  sealed: boolean,
) {
  const p = ctx.look.palette;
  const frame: Surface = { layer: LAYER.metal, tint: p.door, flag: FLAG.frame };
  const [j, t] = [BULK_JAMB, BULK_FRAME_D];
  k.bevelBox(-j, -BULK_HALF, 0, t, 0, BULK_TOP, 0.04, frame);
  k.bevelBox(BULK_HALF, j, 0, t, 0, BULK_TOP, 0.04, frame);
  k.bevelBox(-j, j, 0, t, BULK_TOP, BULK_TOP + 0.25, 0.04, frame);
  // The hazard-striped sill, with a dark step plate on it.
  k.box(-BULK_HALF, BULK_HALF, 0, t, 0, BULK_SILL, s.hazard);
  k.box(
    -BULK_HALF,
    BULK_HALF,
    t - 0.06,
    t,
    BULK_SILL,
    BULK_SILL + 0.01,
    s.dark,
  );

  const plate = s.tinted(shade(p.metal, 0.85), LAYER.metal);
  out.add(UP, BULKHEAD_TRAVEL, (m) => {
    m.bevelBox(
      -BULK_HALF - 0.02,
      BULK_HALF + 0.02,
      0.1,
      0.2,
      BULK_SILL,
      BULK_TOP,
      0.03,
      plate,
    );
    // Two stiffening ribs, then the handwheel: hub, spokes and rim.
    for (const h of [0.45, 1.85]) {
      m.bevelBox(
        -BULK_HALF + 0.05,
        BULK_HALF - 0.05,
        0.2,
        0.23,
        h,
        h + 0.1,
        0.01,
        s.metal,
      );
    }
    m.bevelBox(
      -0.05,
      0.05,
      0.2,
      WHEEL_D + 0.02,
      WHEEL_H - 0.05,
      WHEEL_H + 0.05,
      0.01,
      s.dark,
    );
    m.box(
      -WHEEL_R,
      WHEEL_R,
      WHEEL_D - 0.01,
      WHEEL_D + 0.01,
      WHEEL_H - 0.012,
      WHEEL_H + 0.012,
      s.metal,
    );
    m.box(
      -0.012,
      0.012,
      WHEEL_D - 0.01,
      WHEEL_D + 0.01,
      WHEEL_H - WHEEL_R,
      WHEEL_H + WHEEL_R,
      s.metal,
    );
    m.ring(0, WHEEL_D, WHEEL_H, WHEEL_R, 0.02, 6, 20, s.metal, "inward");
  });
  if (sealed) k.panel(-0.6, 0.6, 0.201, 0.62, 0.98, s.hazard, 1.2, 0.36);
}

/** The blast door's opening: half width, top, and where the halves meet. */
const BLAST_HALF = 0.8;
const BLAST_TOP = 2.4;
const BLAST_SPLIT = 1.2;
/** The massive frame's outer half width and depth. */
const BLAST_JAMB = 1.0;
const BLAST_FRAME_D = 0.28;
/** The two halves' depth range. */
const LEAF_D0 = 0.06;
const LEAF_D1 = 0.2;

/** A chevron pointing up (`dir` 1) or down (-1) with its base at `h`. */
function chevron(h: number, dir: 1 | -1): [number, number][] {
  return [
    [-0.5, h],
    [0, h + dir * 0.28],
    [0.5, h],
    [0.5, h + dir * 0.12],
    [0, h + dir * 0.4],
    [-0.5, h + dir * 0.12],
  ];
}

function blast(
  k: Kit,
  _f: Frame,
  out: Panels,
  s: Surfaces,
  ctx: ModelContext,
  sealed: boolean,
) {
  const p = ctx.look.palette;
  const neon: Surface = { layer: LAYER.metal, tint: p.door, flag: FLAG.frame };
  const [j, t, w] = [BLAST_JAMB, BLAST_FRAME_D, BLAST_HALF];
  k.bevelBox(-j, -w, 0, t, 0, BLAST_TOP, 0.05, s.metal);
  k.bevelBox(w, j, 0, t, 0, BLAST_TOP, 0.05, s.metal);
  k.bevelBox(-j, j, 0, t, BLAST_TOP, BLAST_TOP + 0.18, 0.05, s.metal);
  // Bolt heads down the jambs.
  for (let i = 0; i < 5; i++) {
    const h = 0.25 + i * 0.48;
    k.box(-0.93, -0.87, t - 0.01, t, h, h + 0.06, s.dark);
    k.box(0.87, 0.93, t - 0.01, t, h, h + 0.06, s.dark);
  }
  // The neon border lining the opening.
  k.box(-w, -w + 0.02, 0.2, t, 0, BLAST_TOP, neon);
  k.box(w - 0.02, w, 0.2, t, 0, BLAST_TOP, neon);
  k.box(-w, w, 0.2, t, BLAST_TOP - 0.02, BLAST_TOP, neon);

  const leaf = s.tinted(shade(p.metal, 0.8), LAYER.metal);
  const halves: [V3, number, number, 1 | -1, number[]][] = [
    [UP, BLAST_SPLIT, BLAST_TOP - 0.02, 1, [1.45, 1.85]],
    [DOWN, 0, BLAST_SPLIT, -1, [1.0, 0.55]],
  ];
  for (const [axis, h0, h1, dir, bases] of halves) {
    out.add(axis, BLAST_TRAVEL, (m) => {
      m.bevelBox(-w + 0.02, w - 0.02, LEAF_D0, LEAF_D1, h0, h1, 0.02, leaf);
      m.panel(
        -w + 0.05,
        w - 0.05,
        LEAF_D1 + 0.001,
        h0 + 0.03,
        h1 - 0.03,
        s.hazard,
        1.5,
        h1 - h0 - 0.06,
      );
      for (const base of bases)
        m.extrude(chevron(base, dir), LEAF_D1, LEAF_D1 + 0.025, s.dark);
    });
  }
  if (sealed)
    k.bevelBox(-0.95, 0.95, LEAF_D1 + 0.025, 0.27, 1.1, 1.3, 0.02, s.metal);
}
