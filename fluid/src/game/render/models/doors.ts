/**
 * The doors: an outgoing relation, styled by its target's salience.
 *
 * - A sliding door (salience 0 to 3) is two light leaves that part
 *   sideways into wide jambs, with seams in the look's door colour and a
 *   track in the floor.
 * - A bulkhead (4 to 6) is one heavy leaf that rises into a door-head
 *   housing, in a thick bevelled frame over a hazard-striped sill, with a
 *   handwheel.
 * - A blast door (7 to 10) is two thick halves with chevron faces, the
 *   upper one rising into a door-head housing and the lower one sinking
 *   into the floor, in a massive frame lined with a neon border.
 *
 * The frame, sill, housing and label go into the room's static mesh, and
 * a dark recess behind the leaves shows as the passage when the door
 * stands open. The leaves of an open way come back as movers
 * (`door:<index>`) for the renderer to slide; wherever a leaf parks it is
 * hidden inside a jamb, a housing or the floor, never above the ceiling
 * and never over the door's own label. A sealed door (no address) keeps
 * its leaves in the static mesh, returns no movers and carries a hazard
 * plate or a locking bar across them.
 */

import type { Fixture } from "../../world/types";
import { FLAG, createBuilder, type Surface, type V3 } from "../geometry";
import { createKit, frameForSlot, type Frame, type Kit } from "../kit";
import { ASPECT, LAYER } from "../layers";
import {
  HEADROOM,
  label,
  shade,
  surfaces,
  textPanel,
  type KitAt,
  type ModelContext,
  type Mover,
  type Surfaces,
} from "./common";

type Door = Extract<Fixture, { kind: "door" }>;

/**
 * How far each sliding leaf slides, in metres: its own width, so a parked
 * leaf sits wholly inside its jamb.
 */
export const SLIDE_TRAVEL = 0.5;
/**
 * How far a bulkhead leaf rises when there is room: its height and a
 * little more, so it clears the opening and parks inside the housing.
 * Under a low ceiling the travel is clamped to what the housing holds (see
 * `riseUnder`), and the bulkhead opens only part of the way.
 */
export const BULKHEAD_TRAVEL = 2.04;
/**
 * How far the upper half of a blast door rises into its housing: its
 * height and a little more. It fits under the lowest ceiling (3.0 m).
 */
export const BLAST_UP_TRAVEL = 0.75;
/**
 * How far the lower half of a blast door sinks into the floor: its height
 * and a little more, so its top ends below the floor.
 */
export const BLAST_DOWN_TRAVEL = 1.49;
/**
 * How far a door-head housing stands out from the wall. Rising leaves
 * pass behind its front face, and the door's label sits on it.
 */
export const HOUSING_DEPTH = 0.29;

const UP: V3 = [0, 1, 0];
const DOWN: V3 = [0, -1, 0];
const neg = (v: V3): V3 => [-v[0], -v[1], -v[2]];

/** The colour of the dark passage behind an open door. */
const RECESS: V3 = [0.02, 0.02, 0.025];

/**
 * How far a rising leaf whose top stands at `top` may travel, up to
 * `travel`: never past the ceiling less `HEADROOM`, where the housing
 * ends.
 */
function riseUnder(ctx: ModelContext, top: number, travel: number): number {
  return Math.max(0, Math.min(travel, ctx.ceiling - HEADROOM - top));
}

/**
 * Where a door's leaves go: into movers of their own for an open way, or
 * into the static mesh for a sealed one.
 */
interface Leaves {
  /** Builds one leaf into a kit; for an open door, a mover along `axis`. */
  add(axis: V3, travel: number, build: (k: Kit) => void): void;
  movers: Mover[];
}

function leaves(kit: Kit, f: Frame, key: string, sealed: boolean): Leaves {
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

/** What every door style gets. */
interface Style {
  k: Kit;
  f: Frame;
  out: Leaves;
  s: Surfaces;
  ctx: ModelContext;
  sealed: boolean;
  key: string;
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
  const out = leaves(k, f, key, sealed);
  const style = { sliding, bulkhead, blast }[fx.style];
  style({ k, f, out, s: surfaces(ctx.look), ctx, sealed, key });
  return out.movers;
}

/** The dark passage behind the leaves, just in front of the wall. */
function recess(k: Kit, half: number, h0: number, h1: number) {
  k.panel(-half, half, 0.002, h0, h1, {
    layer: LAYER.panel,
    tint: RECESS,
    flag: FLAG.lit,
  });
}

/** The door-head housing's label: its half width and its margin. */
const HOUSING_LABEL_HALF = 0.85;
const HOUSING_LABEL_MARGIN = 0.05;

/**
 * The door-head housing over a rising leaf: a bevelled box from `bottom`
 * up to `top`, `half` wide each way, standing out to `HOUSING_DEPTH`. The
 * door's label sits on its front face near the bottom, squeezed when the
 * housing is short.
 */
function housing(
  st: Style,
  half: number,
  bottom: number,
  top: number,
  body: Surface,
) {
  const { k, ctx, key } = st;
  k.bevelBox(-half, half, 0, HOUSING_DEPTH, bottom, top, 0.03, body);
  const h0 = bottom + HOUSING_LABEL_MARGIN;
  const h1 = Math.min(
    h0 + (2 * HOUSING_LABEL_HALF) / ASPECT.label,
    top - HOUSING_LABEL_MARGIN,
  );
  textPanel(
    k,
    ctx,
    key,
    -HOUSING_LABEL_HALF,
    HOUSING_LABEL_HALF,
    HOUSING_DEPTH + 0.001,
    h0,
    h1,
    { tint: [1, 1, 1], flag: FLAG.emissive },
  );
}

/** The sliding door's opening: half width and height. */
const SLIDE_HALF = 0.5;
const SLIDE_TOP = 2.4;
/** Each leaf's width: a hair under the opening's half, so no face meets the jamb's. */
const SLIDE_LEAF = SLIDE_HALF - 0.01;
/** The jambs' outer edge (the slot's edge), the frame depth and the lintel. */
const SLIDE_JAMB = 1.0;
const SLIDE_FRAME_D = 0.12;
const SLIDE_LINTEL = 0.15;
/** The leaves' depth range, inside the jambs'. */
const SLIDE_D0 = 0.03;
const SLIDE_D1 = 0.09;
/** Where the sliding door's label sits: bottom edge, half width, plate depth. */
const SLIDE_LABEL_BOTTOM = 2.62;
const SLIDE_LABEL_HALF = 0.9;
const SLIDE_LABEL_D = 0.1;

/**
 * The sliding door: two leaves, each half the 1.0 m opening, that part
 * sideways by their own width into jambs covering the rest of the slot,
 * so a parked leaf is hidden inside its jamb. Door-colour seams line the
 * opening, a track runs along the floor and the label sits on a plate
 * over the lintel.
 */
function sliding(st: Style) {
  const { k, f, out, s, ctx, sealed, key } = st;
  const p = ctx.look.palette;
  const seam: Surface = { layer: LAYER.metal, tint: p.door, flag: FLAG.frame };
  const [w, j, t] = [SLIDE_HALF, SLIDE_JAMB, SLIDE_FRAME_D];
  const top = SLIDE_TOP + SLIDE_LINTEL;
  recess(k, w, 0.02, SLIDE_TOP);
  // Jambs, lintel and the floor track.
  k.bevelBox(-j, -w, 0, t, 0, top, 0.02, s.metal);
  k.bevelBox(w, j, 0, t, 0, top, 0.02, s.metal);
  k.bevelBox(-w, w, 0, t, SLIDE_TOP, top, 0.02, s.metal);
  k.box(-j, j, 0, t + 0.02, 0, 0.02, s.dark);
  // Seams in the door colour along the inside of the frame.
  const [s0, s1] = [t - 0.02, t + 0.005];
  k.box(-w - 0.02, -w + 0.02, s0, s1, 0.02, SLIDE_TOP, seam);
  k.box(w - 0.02, w + 0.02, s0, s1, 0.02, SLIDE_TOP, seam);
  k.box(-w - 0.02, w + 0.02, s0, s1, SLIDE_TOP - 0.02, SLIDE_TOP, seam);

  const leaf = s.tinted(p.door, LAYER.metal);
  for (const dir of [-1, 1] as const) {
    const [a0, a1] = dir < 0 ? [-SLIDE_LEAF, 0] : [0, SLIDE_LEAF];
    out.add(dir < 0 ? neg(f.along) : [...f.along], SLIDE_TRAVEL, (m) => {
      m.bevelBox(a0, a1, SLIDE_D0, SLIDE_D1, 0.02, SLIDE_TOP, 0.012, leaf);
      // A window slot, a kick plate and the pull recess by the meeting edge.
      m.box(
        a0 + 0.12,
        a1 - 0.12,
        SLIDE_D1,
        SLIDE_D1 + 0.005,
        1.45,
        1.75,
        s.dark,
      );
      m.box(
        a0 + 0.05,
        a1 - 0.05,
        SLIDE_D1,
        SLIDE_D1 + 0.01,
        0.1,
        0.35,
        s.metal,
      );
      const edge = dir < 0 ? a1 - 0.08 : a0 + 0.04;
      m.box(edge, edge + 0.04, SLIDE_D1, SLIDE_D1 + 0.015, 0.95, 1.25, s.dark);
    });
  }
  if (sealed) {
    k.panel(-0.4, 0.4, SLIDE_D1 + 0.018, 0.9, 1.3, s.hazard, 0.8, 0.4);
  }
  label(
    k,
    ctx,
    key,
    -SLIDE_LABEL_HALF,
    SLIDE_LABEL_HALF,
    SLIDE_LABEL_BOTTOM,
    SLIDE_LABEL_D,
  );
}

/** The bulkhead's opening: half width, sill height and top. */
const BULK_HALF = 0.7;
const BULK_SILL = 0.18;
const BULK_TOP = 2.2;
/** How far the leaf reaches past the opening each side, into the jambs. */
const BULK_LAP = 0.02;
/** The thick frame's outer half width and depth. */
const BULK_JAMB = 0.98;
const BULK_FRAME_D = 0.26;
/** The leaf's depth range. */
const BULK_D0 = 0.1;
const BULK_D1 = 0.2;
/** The handwheel's height, radius and depth. */
const WHEEL_H = 1.25;
const WHEEL_R = 0.2;
const WHEEL_D = 0.26;

/**
 * The bulkhead: one heavy leaf with stiffening ribs and a handwheel that
 * rises into the door-head housing over the opening, in a thick bevelled
 * frame that glows at its edges, over a hazard-striped sill. The housing
 * reaches up to where the leaf parks, or the ceiling less `HEADROOM`,
 * whichever is lower, and carries the label.
 */
function bulkhead(st: Style) {
  const { k, out, s, ctx, sealed } = st;
  const p = ctx.look.palette;
  const frame: Surface = { layer: LAYER.metal, tint: p.door, flag: FLAG.frame };
  const [j, t] = [BULK_JAMB, BULK_FRAME_D];
  const travel = riseUnder(ctx, BULK_TOP, BULKHEAD_TRAVEL);
  recess(k, BULK_HALF, BULK_SILL, BULK_TOP);
  k.bevelBox(-j, -BULK_HALF, 0, t, 0, BULK_TOP, 0.04, frame);
  k.bevelBox(BULK_HALF, j, 0, t, 0, BULK_TOP, 0.04, frame);
  housing(st, j, BULK_TOP, BULK_TOP + travel, s.body);
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
  const lap = BULK_HALF + BULK_LAP;
  out.add(UP, travel, (m) => {
    m.bevelBox(-lap, lap, BULK_D0, BULK_D1, BULK_SILL, BULK_TOP, 0.03, plate);
    // Two stiffening ribs, then the handwheel: hub, spokes and rim.
    for (const h of [0.45, 1.8]) {
      m.bevelBox(
        -BULK_HALF + 0.05,
        BULK_HALF - 0.05,
        BULK_D1,
        BULK_D1 + 0.03,
        h,
        h + 0.1,
        0.01,
        s.metal,
      );
    }
    m.bevelBox(
      -0.05,
      0.05,
      BULK_D1,
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
  if (sealed) {
    k.panel(-0.6, 0.6, BULK_D1 + 0.001, 0.62, 0.98, s.hazard, 1.2, 0.36);
  }
}

/** The blast door's opening: half width, top, and where the halves meet. */
const BLAST_HALF = 0.8;
const BLAST_TOP = 2.2;
const BLAST_SPLIT = 1.47;
/** The massive frame's outer half width and depth. */
const BLAST_JAMB = 1.0;
const BLAST_FRAME_D = 0.28;
/** How far a bolt head stands proud of the jamb. */
const BOLT_PROUD = 0.005;
/** The neon border's depth range, in front of the halves' chevrons. */
const NEON_D0 = 0.23;
/** The two halves' depth range, and how far their chevrons stand out. */
const LEAF_D0 = 0.06;
const LEAF_D1 = 0.2;
const CHEVRON = 0.025;

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

/**
 * The blast door: two thick halves with hazard faces and raised chevrons
 * pointing away from the split, the upper one rising into the door-head
 * housing and the lower, taller one sinking into the floor, so the
 * opening clears under the lowest ceiling. A massive bolted frame lined
 * with a neon border holds them; a sealed one is barred across the split.
 */
function blast(st: Style) {
  const { k, out, s, ctx, sealed } = st;
  const p = ctx.look.palette;
  const neon: Surface = { layer: LAYER.metal, tint: p.door, flag: FLAG.frame };
  const [j, t, w] = [BLAST_JAMB, BLAST_FRAME_D, BLAST_HALF];
  const up = riseUnder(ctx, BLAST_TOP, BLAST_UP_TRAVEL);
  recess(k, w, 0, BLAST_TOP);
  k.bevelBox(-j, -w, 0, t, 0, BLAST_TOP, 0.05, s.metal);
  k.bevelBox(w, j, 0, t, 0, BLAST_TOP, 0.05, s.metal);
  housing(st, j, BLAST_TOP, BLAST_TOP + up, s.metal);
  // Bolt heads down the jambs, standing proud of their faces.
  for (let i = 0; i < 4; i++) {
    const h = 0.25 + i * 0.5;
    k.box(-0.93, -0.87, t - 0.01, t + BOLT_PROUD, h, h + 0.06, s.dark);
    k.box(0.87, 0.93, t - 0.01, t + BOLT_PROUD, h, h + 0.06, s.dark);
  }
  // The neon border lining the opening.
  k.box(-w, -w + 0.02, NEON_D0, t, 0, BLAST_TOP, neon);
  k.box(w - 0.02, w, NEON_D0, t, 0, BLAST_TOP, neon);
  k.box(-w, w, NEON_D0, t, BLAST_TOP - 0.02, BLAST_TOP, neon);

  const leaf = s.tinted(shade(p.metal, 0.8), LAYER.metal);
  const halves: [V3, number, number, number, 1 | -1, number[]][] = [
    [UP, up, BLAST_SPLIT, BLAST_TOP, 1, [1.55]],
    [DOWN, BLAST_DOWN_TRAVEL, 0, BLAST_SPLIT, -1, [1.32, 0.85]],
  ];
  for (const [axis, travel, h0, h1, dir, bases] of halves) {
    out.add(axis, travel, (m) => {
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
      for (const base of bases) {
        m.extrude(chevron(base, dir), LEAF_D1, LEAF_D1 + CHEVRON, s.dark);
      }
    });
  }
  if (sealed) {
    k.bevelBox(
      -0.95,
      0.95,
      LEAF_D1 + CHEVRON,
      t - 0.01,
      BLAST_SPLIT - 0.1,
      BLAST_SPLIT + 0.1,
      0.02,
      s.metal,
    );
  }
}
