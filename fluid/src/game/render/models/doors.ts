/**
 * The doors: an outgoing relation, styled by its target's salience.
 *
 * - A sliding door (salience 0 to 3) is two light leaves that part
 *   sideways into wide jambs, with seams in the look's door colour and a
 *   track in the floor.
 * - A bulkhead (4 to 6) is two heavy leaves with hazard-striped leading
 *   edges that part sideways into bevelled side housings, over a
 *   hazard-striped sill, with a handwheel on the left leaf.
 * - A blast door (7 to 10) is two thick halves with chevron faces, the
 *   upper one rising into a door-head housing and the lower one sinking
 *   into the floor, in a massive frame lined with a neon border.
 *
 * The frame, sill, housing and label go into the room's static mesh, and
 * a dark recess behind the leaves shows as the passage when the door
 * stands open. Every door's leaves come back as movers (`door:<index>`),
 * sealed or not, for the renderer to slide: an open way slides them by
 * its `DoorState`, a broken one by a fault's frames. Wherever a leaf parks
 * it is hidden inside a jamb, a housing or the floor, never above the
 * ceiling and never over the door's own label.
 *
 * A sealed door (no address) carries its marks on the leaves: the sliding
 * door's and the bulkhead's hazard plate is split into one half per leaf
 * and rides with it, so a leaf that jerks open takes its half along. The
 * blast door's locking bar belongs to the frame and stays static across
 * the split, so its halves strain against it.
 *
 * Every door also gets two more movers, which a malfunction drives
 * (`render/parts.ts` turns a fault frame into their draw): a hazard lamp
 * lens (`lamp:<index>`), a flush amber panel near the top of the right
 * jamb or housing that glows dimly at `LAMP_IDLE` and blinks while a fault
 * runs, and a spark cluster (`spark:<index>`), six tiny glowing boxes on
 * the recess at the leaves' meeting line, drawn only while a fault lights
 * them and hidden by the closed leaves.
 */

import type { DoorStyle, Fixture } from "../../world/types";
import { FLAG, createBuilder, type Surface, type V3 } from "../geometry";
import {
  DECAL_LIFT,
  createKit,
  frameForSlot,
  type Frame,
  type Kit,
} from "../kit";
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
 * How far each bulkhead leaf slides sideways, in metres: its own width,
 * so a parked leaf sits wholly inside its side housing and the door opens
 * fully under any ceiling.
 */
export const BULKHEAD_TRAVEL = 0.5;
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
 * How far a door housing stands out from the wall: the blast door's head
 * housing and the bulkhead's side and lintel housings. Moving leaves pass
 * behind its front face, and the door's label sits on it.
 */
export const HOUSING_DEPTH = 0.29;

const UP: V3 = [0, 1, 0];
const DOWN: V3 = [0, -1, 0];
const neg = (v: V3): V3 => [-v[0], -v[1], -v[2]];

/**
 * The colour of the dark passage behind an open door, and of the dark
 * crack behind a hatch lid that pops open.
 */
export const RECESS: V3 = [0.02, 0.02, 0.025];

/**
 * The hazard lamp lens's gain while no fault runs: a dim amber glow, so a
 * blink to `LAMP_ON` (in `world/malfunction.ts`) reads as the lamp
 * lighting up.
 */
export const LAMP_IDLE = 0.3;

/**
 * The hazard lamp lens's colour: amber, a warning light of its own that
 * no look's palette carries, so it reads the same in every look.
 */
export const LAMP_TINT: V3 = [1.0, 0.55, 0.1];

/**
 * The lens's side, in metres: a small square light. On a blast door it is
 * `BLAST_LAMP_SIZE`, so it stays on the narrow flat face of its jamb.
 */
const LAMP_SIZE = 0.12;
const BLAST_LAMP_SIZE = 0.09;

/**
 * The sparks' colour: a hot white-yellow, the colour of welding sparks
 * rather than of any look, drawn emissive so it ignores the room's light.
 */
export const SPARK_TINT: V3 = [1.0, 0.85, 0.55];

/** Each spark box's side, in metres. */
const SPARK_SIZE = 0.02;

/**
 * Where the six sparks sit around the gap, as `[a, dh]`: along the wall
 * from the door's centre line, and up or down from the gap height.
 */
const SPARK_POINTS: readonly (readonly [number, number])[] = [
  [-0.03, 0.05],
  [0.02, -0.04],
  [0.06, 0.1],
  [-0.07, -0.08],
  [0.01, 0.16],
  [-0.02, -0.15],
];

/**
 * How far a rising leaf whose top stands at `top` may travel, up to
 * `travel`: never past the ceiling less `HEADROOM`, where the housing
 * ends.
 */
function riseUnder(ctx: ModelContext, top: number, travel: number): number {
  return Math.max(0, Math.min(travel, ctx.ceiling - HEADROOM - top));
}

/** Where a door's leaves go: each into a mover of its own. */
interface Leaves {
  /** Builds one leaf into a kit of its own, a mover along `axis`. */
  add(axis: V3, travel: number, build: (k: Kit) => void): void;
  movers: Mover[];
}

/**
 * The leaves of door `index`, keyed `key`: each one a `leaf` mover built
 * in a fresh builder with a kit on the door's frame, at rest gain 1.
 */
function leaves(f: Frame, key: string, index: number): Leaves {
  const movers: Mover[] = [];
  return {
    movers,
    add(axis, travel, build) {
      const b = createBuilder();
      build(createKit(b, f));
      movers.push({
        key,
        part: "leaf",
        fixture: index,
        mesh: b.build(),
        axis,
        travel,
        pivot: null,
        rest: 1,
      });
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
  index: number;
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
  const out = leaves(f, key, index);
  const style = { sliding, bulkhead, blast }[fx.style];
  const st = { k, f, out, s: surfaces(ctx.look), ctx, sealed, key, index };
  const [lampAt, sparkH] = style(st);
  // Built after the leaves, so the movers come back in the order their
  // kits were first used: leaves, lamp, sparks.
  const lens = lamp(st, ...lampAt);
  return [...out.movers, lens, sparks(st, sparkH)];
}

/**
 * Where a style puts its lamp lens (`[a, h, faceD]`: along, height, and
 * the depth of the face it sits on) and its sparks (the gap height).
 */
type Parts = [
  lamp: [a: number, h: number, faceD: number, size: number],
  sparkH: number,
];

/**
 * The hazard lamp lens, mover `lamp:<index>`: a flush amber panel `size`
 * metres square, centred at `a` along and height `h`, `DECAL_LIFT` in
 * front of the face at depth `faceD`, glowing at `LAMP_IDLE` while no
 * fault runs.
 */
function lamp(
  st: Style,
  a: number,
  h: number,
  faceD: number,
  size: number,
): Mover {
  const b = createBuilder();
  const half = size / 2;
  createKit(b, st.f).panel(
    a - half,
    a + half,
    faceD + DECAL_LIFT,
    h - half,
    h + half,
    { layer: LAYER.panel, tint: LAMP_TINT, flag: FLAG.lamp },
  );
  return {
    key: `lamp:${st.index}`,
    part: "lamp",
    fixture: st.index,
    mesh: b.build(),
    axis: [...st.f.inward],
    travel: 0,
    pivot: null,
    rest: LAMP_IDLE,
  };
}

/**
 * The spark cluster, mover `spark:<index>`: six `SPARK_SIZE` boxes at
 * `SPARK_POINTS` around the door's centre line at gap height `h`, standing
 * on the recess (from `DECAL_LIFT` out), so they touch a lit host and the
 * closed leaves hide them. Drawn only while a fault lights them.
 */
function sparks(st: Style, h: number): Mover {
  const b = createBuilder();
  const m = createKit(b, st.f);
  const hot: Surface = {
    layer: LAYER.panel,
    tint: SPARK_TINT,
    flag: FLAG.emissive,
  };
  const half = SPARK_SIZE / 2;
  for (const [a, dh] of SPARK_POINTS) {
    m.box(
      a - half,
      a + half,
      DECAL_LIFT,
      DECAL_LIFT + SPARK_SIZE,
      h + dh - half,
      h + dh + half,
      hot,
    );
  }
  return {
    key: `spark:${st.index}`,
    part: "spark",
    fixture: st.index,
    mesh: b.build(),
    axis: [...st.f.inward],
    travel: 0,
    pivot: null,
    rest: 0,
  };
}

/** The dark passage behind the leaves, just in front of the wall. */
function recess(k: Kit, half: number, h0: number, h1: number) {
  k.panel(-half, half, DECAL_LIFT, h0, h1, {
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
    HOUSING_DEPTH,
    h0,
    h1,
    { tint: [1, 1, 1], flag: FLAG.emissive },
  );
}

/** The sliding door's opening: half width, bottom and top. */
export const SLIDE_HALF = 0.5;
const SLIDE_SILL = 0.02;
const SLIDE_TOP = 2.4;
/**
 * The gap each leaf keeps from the door's centre line, in metres. A leaf
 * runs from `LEAF_GAP` to its half of the opening less `LEAF_GAP`, so
 * after sliding its own half width it stops just inside the jamb or
 * housing and no leaf face ever lies in the plane of a jamb face.
 */
const LEAF_GAP = 0.005;
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
function sliding(st: Style): Parts {
  const { k, f, out, s, ctx, sealed, key } = st;
  const p = ctx.look.palette;
  const seam: Surface = { layer: LAYER.metal, tint: p.door, flag: FLAG.frame };
  const [w, j, t] = [SLIDE_HALF, SLIDE_JAMB, SLIDE_FRAME_D];
  const top = SLIDE_TOP + SLIDE_LINTEL;
  recess(k, w, SLIDE_SILL, SLIDE_TOP);
  // Jambs, lintel and the floor track.
  k.bevelBox(-j, -w, 0, t, 0, top, 0.02, s.metal);
  k.bevelBox(w, j, 0, t, 0, top, 0.02, s.metal);
  k.bevelBox(-w, w, 0, t, SLIDE_TOP, top, 0.02, s.metal);
  k.box(-j, j, 0, t + 0.02, 0, 0.02, s.dark);
  // Seams in the door colour along the inside of the frame.
  const [s0, s1] = [t - 0.02, t + 0.005];
  k.box(-w - 0.02, -w + 0.02, s0, s1, SLIDE_SILL, SLIDE_TOP, seam);
  k.box(w - 0.02, w + 0.02, s0, s1, SLIDE_SILL, SLIDE_TOP, seam);
  k.box(-w - 0.02, w + 0.02, s0, s1, SLIDE_TOP - 0.02, SLIDE_TOP, seam);

  const leaf = s.tinted(p.door, LAYER.metal);
  for (const dir of [-1, 1] as const) {
    const [a0, a1] =
      dir < 0
        ? [-SLIDE_HALF + LEAF_GAP, -LEAF_GAP]
        : [LEAF_GAP, SLIDE_HALF - LEAF_GAP];
    out.add(dir < 0 ? neg(f.along) : [...f.along], SLIDE_TRAVEL, (m) => {
      m.bevelBox(
        a0,
        a1,
        SLIDE_D0,
        SLIDE_D1,
        SLIDE_SILL,
        SLIDE_TOP,
        0.012,
        leaf,
      );
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
      if (sealed) {
        // This leaf's half of the hazard plate, over the pull recesses,
        // the frontmost faces of the leaves; the right half's stripes go
        // on from where the left half's end.
        const d = SLIDE_D1 + 0.015 + DECAL_LIFT;
        const [p0, p1] = dir < 0 ? [-0.4, -LEAF_GAP] : [LEAF_GAP, 0.4];
        m.panel(p0, p1, d, 0.9, 1.3, s.hazard, 0.4, 0.4, dir < 0 ? 0 : 0.4);
      }
    });
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
  return [[0.75, 2.35, SLIDE_FRAME_D, LAMP_SIZE], 1.2];
}

/** The bulkhead's opening: half width, sill height and top. */
export const BULK_HALF = 0.5;
const BULK_SILL = 0.18;
const BULK_TOP = 2.2;
/** The side housings' outer edge (the slot's edge) and the lintel's top. */
const BULK_JAMB = 1.0;
const BULK_LINTEL = 2.6;
/** The leaves' depth range, inside the housings'. */
const BULK_D0 = 0.1;
const BULK_D1 = 0.2;
/** The hazard-striped leading edge on each leaf: its width and depth. */
const EDGE = 0.07;
const EDGE_D = 0.01;
/** The handwheel on the left leaf: centre along, height, radius, depth. */
const WHEEL_A = -BULK_HALF / 2;
const WHEEL_H = 1.25;
const WHEEL_R = 0.18;
const WHEEL_D = 0.26;
/** The label on the lintel housing: half width and margin. */
const BULK_LABEL_HALF = 0.85;
const BULK_LABEL_MARGIN = 0.05;

/**
 * The bulkhead: the heavy counterpart of the sliding door. Two thick
 * leaves with stiffening ribs and hazard-striped leading edges part
 * sideways by their own width into bevelled side housings that stand out
 * to `HOUSING_DEPTH`, so the door opens fully under any ceiling. A
 * handwheel sits on the left leaf, the sill is hazard striped and the
 * label sits on the lintel housing, in front of the leaves' path. The
 * side housings glow at their edges in the door colour; the lintel is
 * plain, so the label reads on it.
 */
function bulkhead(st: Style): Parts {
  const { k, f, out, s, ctx, sealed, key } = st;
  const p = ctx.look.palette;
  const frame: Surface = { layer: LAYER.metal, tint: p.door, flag: FLAG.frame };
  const [w, j, t] = [BULK_HALF, BULK_JAMB, HOUSING_DEPTH];
  recess(k, w, BULK_SILL, BULK_TOP);
  // The side housings, the lintel housing and the label on its face.
  k.bevelBox(-j, -w, 0, t, 0, BULK_TOP, 0.04, frame);
  k.bevelBox(w, j, 0, t, 0, BULK_TOP, 0.04, frame);
  k.bevelBox(-j, j, 0, t, BULK_TOP, BULK_LINTEL, 0.04, s.body);
  const h0 = BULK_TOP + BULK_LABEL_MARGIN;
  const h1 = Math.min(
    h0 + (2 * BULK_LABEL_HALF) / ASPECT.label,
    BULK_LINTEL - BULK_LABEL_MARGIN,
  );
  textPanel(k, ctx, key, -BULK_LABEL_HALF, BULK_LABEL_HALF, t, h0, h1, {
    tint: [1, 1, 1],
    flag: FLAG.emissive,
  });
  // The hazard-striped sill, with a dark step plate on it.
  k.box(-w, w, 0, t, 0, BULK_SILL, s.hazard);
  k.box(-w, w, t - 0.06, t, BULK_SILL, BULK_SILL + 0.01, s.dark);

  const plate = s.tinted(shade(p.metal, 0.85), LAYER.metal);
  for (const dir of [-1, 1] as const) {
    const [a0, a1] =
      dir < 0
        ? [-BULK_HALF + LEAF_GAP, -LEAF_GAP]
        : [LEAF_GAP, BULK_HALF - LEAF_GAP];
    const [e0, e1] = dir < 0 ? [a1 - EDGE, a1] : [a0, a0 + EDGE];
    out.add(dir < 0 ? neg(f.along) : [...f.along], BULKHEAD_TRAVEL, (m) => {
      m.bevelBox(a0, a1, BULK_D0, BULK_D1, BULK_SILL, BULK_TOP, 0.02, plate);
      m.box(e0, e1, BULK_D1, BULK_D1 + EDGE_D, BULK_SILL, BULK_TOP, s.hazard);
      if (sealed) {
        // This leaf's half of the hazard plate; the stripes run on across
        // the seam.
        const [p0, p1] = dir < 0 ? [-0.4, -LEAF_GAP] : [LEAF_GAP, 0.4];
        m.panel(
          p0,
          p1,
          BULK_D1 + EDGE_D + DECAL_LIFT,
          0.62,
          0.98,
          s.hazard,
          0.4,
          0.36,
          dir < 0 ? 0 : 0.4,
        );
      }
      // Two stiffening ribs across the leaf.
      const [r0, r1] = dir < 0 ? [a0 + 0.04, e0] : [e1, a1 - 0.04];
      for (const h of [0.45, 1.8]) {
        m.bevelBox(r0, r1, BULK_D1, BULK_D1 + 0.03, h, h + 0.1, 0.01, s.metal);
      }
      if (dir > 0) return;
      // The handwheel: hub, spokes and rim.
      const [wa, wh, wr] = [WHEEL_A, WHEEL_H, WHEEL_R];
      m.bevelBox(
        wa - 0.05,
        wa + 0.05,
        BULK_D1,
        WHEEL_D + 0.02,
        wh - 0.05,
        wh + 0.05,
        0.01,
        s.dark,
      );
      m.box(
        wa - wr,
        wa + wr,
        WHEEL_D - 0.01,
        WHEEL_D + 0.01,
        wh - 0.012,
        wh + 0.012,
        s.metal,
      );
      m.box(
        wa - 0.012,
        wa + 0.012,
        WHEEL_D - 0.01,
        WHEEL_D + 0.01,
        wh - wr,
        wh + wr,
        s.metal,
      );
      m.ring(wa, WHEEL_D, wh, wr, 0.02, 6, 20, s.metal, "inward");
    });
  }
  // The lens lands at 0.30 m, the depth of the lintel's label.
  return [[0.75, 2.0, HOUSING_DEPTH, LAMP_SIZE], 1.2];
}

/** The blast door's opening: half width, top, and where the halves meet. */
export const BLAST_HALF = 0.8;
const BLAST_TOP = 2.2;
/** The height where the blast door's two halves meet, in metres. */
export const BLAST_SPLIT = 1.47;
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

/**
 * Each door style's clear opening in its wall frame: `half` the half width
 * along the wall either side of the slot's centre, and `h0` to `h1` the
 * height from the sill to the underside of the lintel. A fully open door
 * clears it (the models test checks that no part of an open door is left
 * inside), which is what makes the doorway something the player can see
 * through and walk into. Read from the same constants the recipes build
 * with, so the test can never check a stale copy.
 */
export const OPENING: Readonly<
  Record<DoorStyle, { half: number; h0: number; h1: number }>
> = {
  sliding: { half: SLIDE_HALF, h0: SLIDE_SILL, h1: SLIDE_TOP },
  bulkhead: { half: BULK_HALF, h0: BULK_SILL, h1: BULK_TOP },
  blast: { half: BLAST_HALF, h0: 0, h1: BLAST_TOP },
};

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
function blast(st: Style): Parts {
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
        LEAF_D1 + DECAL_LIFT,
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
  // The lens on the right jamb above the bolts; the sparks at the split.
  return [[0.9, 2.0, BLAST_FRAME_D, BLAST_LAMP_SIZE], BLAST_SPLIT];
}
