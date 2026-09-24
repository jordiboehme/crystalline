/**
 * The portal: a prose wikilink, a way through a shimmering surface.
 *
 * A ring emitter stands on a cradle between two pylons, the pylons on a
 * base plinth and joined by a header bar that carries the label. Inside
 * the ring the portal surface shimmers in the portal colour (the cross
 * domain colour for a link into another domain) in front of a dark back
 * plate, and the ring glows with
 * a frame edge in the same colour. A sealed portal has a hazard plate
 * where the surface would be and a padlock-shaped block across it.
 */

import type { Fixture } from "../../world/types";
import { FLAG, type Surface } from "../geometry";
import { frameForSlot } from "../kit";
import { LAYER } from "../layers";
import {
  label,
  shade,
  surfaces,
  type KitAt,
  type ModelContext,
} from "./common";

type Portal = Extract<Fixture, { kind: "portal" }>;

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

/** Builds a portal against its wall slot. */
export function buildPortal(
  kitAt: KitAt,
  fx: Portal,
  index: number,
  ctx: ModelContext,
): void {
  const k = kitAt(frameForSlot(fx.slot));
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
  const disc: [number, number][] = Array.from(
    { length: DISC_SIDES },
    (_, i) => {
      const t = (2 * Math.PI * i) / DISC_SIDES;
      return [Math.sin(t) * DISC_R, RING_H + Math.cos(t) * DISC_R];
    },
  );
  // The emitter's dark back plate, which holds the surface in the ring.
  k.extrude(disc, 0.02, RING_D - 0.01, s.tinted(shade(p.metal, 0.2)));
  if (fx.sealedLabel === null) {
    k.extrude(disc, RING_D - 0.01, RING_D + 0.01, {
      layer: LAYER.portal,
      tint: colour,
      flag: FLAG.portal,
    });
  } else {
    k.extrude(disc, RING_D - 0.01, RING_D + 0.01, s.hazard);
    // The padlock: a body and its shackle.
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
  }
  label(k, ctx, `portal:${index}`, -0.9, 0.9, PYLON_TOP + HEADER + 0.04, 0.08);
}
