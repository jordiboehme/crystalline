/**
 * The portal: a prose wikilink, a way through a shimmering surface.
 *
 * A ring emitter stands on a cradle between two pylons, the pylons on a
 * base plinth and joined by a header bar that carries the label. Inside
 * the ring the portal surface shimmers in the portal colour (the cross
 * domain colour for a link into another domain) in front of a dark back
 * plate, and the ring glows with a frame edge in the same colour.
 *
 * The surface is a disc of its own, a mover (`disc:<index>`) that a
 * malfunction flickers, stutters and collapses to a point about its centre
 * (`render/parts.ts` turns the fault frame into its scale, gain and swirl
 * time). The back plate stays in the static mesh, so a collapsed disc
 * leaves a dead, dark ring. A sealed portal keeps the same swirl behind a
 * padlock-shaped block, dimmed to `DISC_SEALED_GAIN`, and carries a hazard
 * band on the plinth's front face.
 */

import type { Fixture } from "../../world/types";
import { FLAG, createBuilder, type Surface } from "../geometry";
import { DECAL_LIFT, createKit, frameForSlot } from "../kit";
import { LAYER } from "../layers";
import {
  discOutline,
  label,
  shade,
  surfaces,
  type KitAt,
  type ModelContext,
  type Mover,
} from "./common";

type Portal = Extract<Fixture, { kind: "portal" }>;

/**
 * A sealed portal's swirl gain at rest: half as bright as an open one's,
 * a dim surface behind the padlock.
 */
export const DISC_SEALED_GAIN = 0.5;

/** The ring: centre height and depth, radius to the tube, tube radius. */
const RING_H = 1.35;
const RING_D = 0.15;
const RING_R = 0.76;
const RING_TUBE = 0.12;
/** The surface disc inside it and its facet count. */
const DISC_R = RING_R - RING_TUBE + 0.02;
const DISC_SIDES = 24;
/** Pylons: inner and outer offset along, and their top. */
const PYLON_IN = RING_R + RING_TUBE - 0.04;
const PYLON_OUT = 0.98;
const PYLON_TOP = 2.4;
/** The plinth's height and the header bar's height. */
const PLINTH = 0.3;
const HEADER = 0.1;

/**
 * Builds a portal against its wall slot: its static parts into the kits
 * `kitAt` makes, and its swirl surface returned as the one mover
 * `disc:<index>`, pivoting on the disc's centre.
 */
export function buildPortal(
  kitAt: KitAt,
  fx: Portal,
  index: number,
  ctx: ModelContext,
): Mover[] {
  const f = frameForSlot(fx.slot);
  const k = kitAt(f);
  const s = surfaces(ctx.look);
  const p = ctx.look.palette;
  const colour = fx.crossDomain ? p.portalAlt : p.portal;

  // Plinth, cradle, pylons and the header bar.
  k.bevelBox(-PYLON_OUT, PYLON_OUT, 0, 0.28, 0, PLINTH, 0.03, s.metal);
  k.bevelBox(
    -0.3,
    0.3,
    0.05,
    0.25,
    PLINTH,
    RING_H - RING_R + 0.02,
    0.02,
    s.dark,
  );
  for (const dir of [-1, 1]) {
    const [a0, a1] = [dir * PYLON_IN, dir * PYLON_OUT];
    k.bevelBox(a0, a1, 0.02, 0.26, PLINTH, PYLON_TOP, 0.03, s.body);
    // Three cooling vents and a status light down each pylon.
    for (let i = 0; i < 3; i++) {
      const h = 0.6 + i * 0.25;
      k.box(a0 + dir * 0.03, a1 - dir * 0.03, 0.26, 0.265, h, h + 0.12, s.dark);
    }
    k.box(
      a0 + dir * 0.06,
      a1 - dir * 0.06,
      0.26,
      0.268,
      1.6,
      1.66,
      s.glow(colour),
    );
  }
  k.bevelBox(
    -PYLON_OUT,
    PYLON_OUT,
    0.02,
    0.26,
    PYLON_TOP,
    PYLON_TOP + HEADER,
    0.02,
    s.metal,
  );

  // The emitter ring and what it holds.
  const ring: Surface = { layer: LAYER.metal, tint: colour, flag: FLAG.frame };
  k.ring(0, RING_D, RING_H, RING_R, RING_TUBE, 8, 28, ring, "inward");
  const disc = discOutline(0, RING_H, DISC_R, DISC_SIDES, "top");
  // The emitter's dark back plate, which holds the surface in the ring.
  k.extrude(disc, 0.02, RING_D - 0.01, s.tinted(shade(p.metal, 0.2)));
  // The swirl surface, its own mover, open or sealed.
  const b = createBuilder();
  createKit(b, f).extrude(disc, RING_D - 0.01, RING_D + 0.01, {
    layer: LAYER.portal,
    tint: colour,
    flag: FLAG.portal,
  });
  const sealed = fx.sealedLabel !== null;
  if (sealed) {
    // The padlock in front of the disc: a body and its shackle.
    k.bevelBox(
      -0.18,
      0.18,
      RING_D + 0.01,
      0.27,
      RING_H - 0.25,
      RING_H + 0.05,
      0.02,
      s.metal,
    );
    k.ring(0, 0.21, RING_H + 0.05, 0.11, 0.03, 6, 12, s.metal, "inward");
    // A hazard band across the plinth's front face.
    k.panel(-0.6, 0.6, 0.28 + DECAL_LIFT, 0.08, 0.22, s.hazard, 1.2, 0.14);
  }
  label(k, ctx, `portal:${index}`, -0.9, 0.9, PYLON_TOP + HEADER + 0.04, 0.08);
  return [
    {
      key: `disc:${index}`,
      part: "disc",
      fixture: index,
      mesh: b.build(),
      axis: [...f.inward],
      travel: 0,
      pivot: [
        f.origin[0] + f.inward[0] * RING_D,
        f.origin[1] + f.inward[1] * RING_D + RING_H,
        f.origin[2] + f.inward[2] * RING_D,
      ],
      rest: sealed ? DISC_SEALED_GAIN : 1,
      swing: 0,
    },
  ];
}
