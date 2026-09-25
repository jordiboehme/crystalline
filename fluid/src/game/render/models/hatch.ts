/**
 * The service hatch: an engram that points here, a low hatch that leads
 * back to it.
 *
 * A 1.0 by 1.1 m hatch door sits low in a hazard-striped frame, with a
 * round porthole (a rim that glows dimly in the door colour around dark
 * glass) and a lever handle. Above it a stencil plate carries the
 * "SERVICE" pictogram and the label row with the title of the engram the
 * hatch leads to.
 *
 * The door itself, with its hinges, porthole and lever, is the hatch's lid:
 * a mover (`lid:<index>`) built on its own, which a malfunction pops open
 * `LID_CRACK` along the wall's inward and rattles (`render/parts.ts` turns
 * the fault frame into its offset). A dark recess panel behind it shows as
 * the crack. The frame, the stencil plate, the pictogram and the label
 * stay in the room's static mesh.
 */

import type { Fixture } from "../../world/types";
import { FLAG, createBuilder } from "../geometry";
import { DECAL_LIFT, createKit, frameForSlot } from "../kit";
import { ASPECT, LAYER } from "../layers";
import { PICTOGRAM } from "../text";
import {
  shade,
  surfaces,
  textPanel,
  type KitAt,
  type ModelContext,
  type Mover,
} from "./common";
import { RECESS } from "./doors";

type Hatch = Extract<Fixture, { kind: "hatch" }>;

/**
 * Where the "SERVICE" pictogram sits in the pictogram layer, as a uv
 * rectangle: its tile of the pictogram set, which `PICTOGRAM` in
 * `render/text.ts` lays out and draws, so the plate and the sheet agree.
 */
export const SERVICE_PICTOGRAM = PICTOGRAM.service;

/**
 * How far a broken hatch's lid pops out from the wall, in metres: a crack,
 * which leaves the lever at most 0.233 m out, inside `FLUSH_DEPTH`.
 */
export const LID_CRACK = 0.05;

/** The hatch door: half width, bottom and top. */
const HALF = 0.5;
const BOTTOM = 0.12;
const TOP = BOTTOM + 1.1;
/** The frame around it: its margin and depth. */
const FRAME = 0.1;
const FRAME_D = 0.08;
/** The door's front face. */
const DOOR_D = 0.12;
/** The porthole's centre height and radius. */
const PORT_H = 0.85;
const PORT_R = 0.15;
/** The stencil plate above: bottom, top, half width. */
const PLATE_H0 = TOP + FRAME + 0.08;
const PLATE_HALF = 0.9;
const PLATE_D = 0.03;

/**
 * Builds a service hatch against its wall slot: its static parts into the
 * kits `kitAt` makes, and its lid returned as the one mover `lid:<index>`.
 */
export function buildHatch(
  kitAt: KitAt,
  fx: Hatch,
  index: number,
  ctx: ModelContext,
): Mover[] {
  const f = frameForSlot(fx.slot);
  const k = kitAt(f);
  const s = surfaces(ctx.look);
  const p = ctx.look.palette;

  // The frame, hazard striped, and the door with two hinges.
  k.bevelBox(
    -HALF - FRAME,
    -HALF,
    0,
    FRAME_D,
    BOTTOM - FRAME,
    TOP + FRAME,
    0.015,
    s.hazard,
  );
  k.bevelBox(
    HALF,
    HALF + FRAME,
    0,
    FRAME_D,
    BOTTOM - FRAME,
    TOP + FRAME,
    0.015,
    s.hazard,
  );
  k.bevelBox(-HALF, HALF, 0, FRAME_D, TOP, TOP + FRAME, 0.015, s.hazard);
  k.bevelBox(-HALF, HALF, 0, FRAME_D, BOTTOM - FRAME, BOTTOM, 0.015, s.hazard);
  // The dark recess behind the lid, which shows as the crack.
  k.panel(-HALF, HALF, DECAL_LIFT, BOTTOM, TOP, {
    layer: LAYER.panel,
    tint: RECESS,
    flag: FLAG.lit,
  });

  // The lid: the door, its hinges, the porthole and the lever, one mover.
  const b = createBuilder();
  const lid = createKit(b, f);
  lid.bevelBox(-HALF, HALF, 0.02, DOOR_D, BOTTOM, TOP, 0.02, s.body);
  for (const h of [BOTTOM + 0.15, TOP - 0.25]) {
    lid.box(
      -HALF - 0.04,
      -HALF + 0.1,
      DOOR_D,
      DOOR_D + 0.02,
      h,
      h + 0.1,
      s.dark,
    );
  }

  // The porthole: dark glass inside a dimly glowing rim.
  const glass: [number, number][] = Array.from({ length: 12 }, (_, i) => {
    const t = (2 * Math.PI * i) / 12;
    return [Math.sin(t) * PORT_R, PORT_H + Math.cos(t) * PORT_R];
  });
  lid.extrude(glass, DOOR_D, DOOR_D + 0.005, s.tinted(p.screen));
  lid.ring(
    0,
    DOOR_D + 0.01,
    PORT_H,
    PORT_R + 0.01,
    0.025,
    6,
    16,
    s.glow(shade(p.door, 0.5)),
    "inward",
  );

  // The lever handle on its mount.
  lid.bevelBox(0.22, 0.3, DOOR_D, DOOR_D + 0.06, 0.52, 0.62, 0.01, s.dark);
  lid.cylinderAlong(0.26, 0.44, DOOR_D + 0.045, 0.57, 0.018, 6, s.metal);

  // The stencil plate: the pictogram, then the label row.
  const pictogram = PLATE_HALF * 0.4;
  const labelW = 2 * PLATE_HALF - pictogram - 0.1;
  const labelH = Math.min(labelW / ASPECT.label, pictogram);
  const plateTop = PLATE_H0 + pictogram + 0.04;
  k.bevelBox(
    -PLATE_HALF - 0.02,
    PLATE_HALF + 0.02,
    0,
    PLATE_D,
    PLATE_H0 - 0.02,
    plateTop,
    0.008,
    s.dark,
  );
  const u = SERVICE_PICTOGRAM;
  k.panel(
    -PLATE_HALF,
    -PLATE_HALF + pictogram,
    PLATE_D + DECAL_LIFT,
    PLATE_H0,
    PLATE_H0 + pictogram,
    { layer: LAYER.pictogram, tint: p.panel, flag: FLAG.lit },
    u.uw,
    u.vh,
    u.u0,
    u.v0,
  );
  const mid = PLATE_H0 + pictogram / 2;
  textPanel(
    k,
    ctx,
    `hatch:${index}`,
    PLATE_HALF - labelW,
    PLATE_HALF,
    PLATE_D,
    mid - labelH / 2,
    mid + labelH / 2,
    {
      tint: [1, 1, 1],
      flag: FLAG.emissive,
    },
  );
  return [
    {
      key: `lid:${index}`,
      part: "lid",
      fixture: index,
      mesh: b.build(),
      axis: [...f.inward],
      travel: LID_CRACK,
      pivot: null,
      rest: 1,
    },
  ];
}
