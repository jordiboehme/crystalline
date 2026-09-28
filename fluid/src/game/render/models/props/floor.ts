/**
 * The floor props' recipes: everything that stands on the floor and
 * collides with the player, the condition extras included.
 *
 * A floor prop stays inside its variant's footprint (`FOOTPRINTS.prop` in
 * `world/footprints.ts`), as it stands at turn 0: `width` along `a`, `depth`
 * along `d`, centred on the anchor, front towards `+d`, and below
 * `FLOOR_TOP`. Every recipe reads its size from `FOOTPRINTS.prop` and never
 * repeats the numbers; most bodies sit a little inside their half-extent so
 * a handle, a wheel or a corner post can protrude past the body's own face
 * without ever crossing the true footprint, the same way a real object
 * rarely touches every edge of its bounding box.
 *
 * Four kinds carry the room's accent (`s.accent()`, 2.7 C9) on one small
 * part, in every variant, so the room's colour repeats on its set dressing
 * without ever taking over a whole prop: the stool's seat, the bench's
 * seat, the trolley's top grip bar and the tool cart's drawer fronts. The
 * barrel's two ribs carry it too, as its middle band, on every barrel of
 * the cluster (variant 1). No other floor prop carries it.
 */

import type { Surface } from "../../geometry";
import { FOOTPRINTS, type FloorSize } from "../../../world/footprints";
import type { RarePropKind } from "../../../world/props";
import type { FloorPropKind } from "../../../world/types";
import { profileAlong, tiltedBar, yawed } from "../common";
import { frameAt, type Kit } from "../../kit";
import type { Rgb } from "../../looks";
import type { PropRecipe } from "./common";

/** A warm hazard red: the tool cart's body. */
const RED: Rgb = [0.74, 0.09, 0.07];

/** A muted plant green: every bush and leaf. */
const PLANT: Rgb = [0.22, 0.42, 0.24];

/** A terracotta clay tone: the planters' tubs. */
const CLAY: Rgb = [0.62, 0.4, 0.3];

/**
 * A pale glass tint: the specimen shelf's jars. Kept fixed across looks,
 * since a jar's glass reads the same regardless of the room's machine or
 * panel colours, unlike a surface that is meant to belong to the room.
 */
const GLASS: Rgb = [0.85, 0.9, 0.86];

/**
 * An amber cap tint: the gas rack's cylinder caps, the standard colour a
 * pressurised gas cylinder's cap carries regardless of the room's own
 * colours.
 */
const GAS_CAP: Rgb = [0.85, 0.55, 0.08];

/**
 * A straight rail leaning from `(d0, h0)` to `(d1, h1)`, `thick` wide along
 * `a` and centred there: a tilted bar drawn in the `(d, h)` side view and
 * extruded along the wall by `profileAlong`, which owns the side frame's
 * sign flip, so the rail leans the same way as the rungs laid along it.
 */
function leaningRail(
  kitAt: Parameters<PropRecipe>[0]["kitAt"],
  a: number,
  thick: number,
  d0: number,
  h0: number,
  d1: number,
  h1: number,
  s: Parameters<Kit["box"]>[6],
): void {
  const cx = (d0 + d1) / 2;
  const cy = (h0 + h1) / 2;
  const angle = Math.atan2(h1 - h0, d1 - d0);
  const length = Math.hypot(d1 - d0, h1 - h0);
  const bar = tiltedBar(cx, cy, angle, length, thick);
  profileAlong(
    kitAt,
    frameAt([0, 0, 0], 0),
    bar,
    a - thick / 2,
    a + thick / 2,
    s,
  );
}

/** A floor prop's size for `kind` and `variant`, half in each direction. */
function halfSize(
  kind: FloorPropKind,
  variant: number,
): { hw: number; hd: number; size: FloorSize } {
  const size = FOOTPRINTS.prop[kind][variant];
  if (!size) throw new Error(`${kind}: no variant ${String(variant)}`);
  return { hw: size.width / 2, hd: size.depth / 2, size };
}

/** The crate's default height, in metres, before a variant scales it. */
const CRATE_H = { small: 0.55, large: 0.85, stackedSmall: 0.45 };

/**
 * Crate: variant 0 a small crate with a bevelled, recessed body between
 * four hazard-taped corner posts (so the posts, not the panel faces, touch
 * the true footprint edge); variant 1 a large plain bevelled crate;
 * variant 2 a small crate stacked, offset, on a large one.
 */
function crate({ k, s, variant }: Parameters<PropRecipe>[0]): void {
  const { hw, hd } = halfSize("crate", variant);
  if (variant === 0) {
    const H = CRATE_H.small;
    const post = 0.045;
    for (const sa of [-1, 1] as const) {
      for (const sd of [-1, 1] as const) {
        k.box(
          sa > 0 ? hw - post : -hw,
          sa > 0 ? hw : -hw + post,
          sd > 0 ? hd - post : -hd,
          sd > 0 ? hd : -hd + post,
          0,
          H,
          s.hazard,
        );
      }
    }
    k.bevelBox(
      -hw + post * 0.7,
      hw - post * 0.7,
      -hd + post * 0.7,
      hd - post * 0.7,
      0,
      H,
      0.03,
      s.body,
    );
    return;
  }
  if (variant === 1) {
    k.bevelBox(-hw, hw, -hd, hd, 0, CRATE_H.large, 0.04, s.body);
    return;
  }
  const H1 = CRATE_H.large * 0.8;
  k.bevelBox(-hw, hw, -hd, hd, 0, H1, 0.03, s.body);
  const sw = hw * 0.55;
  const sd = hd * 0.5;
  const offA = hw - sw - 0.05;
  const offD = -(hd - sd - 0.05);
  k.bevelBox(
    offA - sw,
    offA + sw,
    offD - sd,
    offD + sd,
    H1,
    H1 + CRATE_H.stackedSmall,
    0.025,
    s.body,
  );
}

/** Ribs, lid and radius shared between the barrel's variants. */
const BARREL = { ribTube: 0.015, lidLift: 0.02 };

/**
 * How far a rib's centre sits in from the barrel's own wall (`radius -
 * inset`), for `oneBarrel`'s `inset` parameter: at `BARREL.ribTube` (its
 * default, `drumRack`'s own) the rib's outer edge lands exactly on the
 * wall, buried in the same surface as the cylinder's own side, so it never
 * shows against it, dark or accented (a fact of this recipe from before
 * 2.7, left as it is for the drum rack). `barrel` passes a smaller inset so
 * its ribs stand a hair proud of the wall instead, where the accent reads.
 */
const RIB_PROUD = 0.008;

/**
 * One upright barrel at `(a, d)`, its base at `base` (0 by default so
 * `barrel`'s own floor-level barrels need not pass it): a metal cylinder
 * with two ribs and a lid, `sides` facets round. `band` colours the two
 * ribs (plain dark metal by default); `barrel` passes the room's accent so
 * every barrel of the cluster carries it, `drumRack` leaves it as it was.
 * `inset` is documented on `RIB_PROUD`.
 */
function oneBarrel(
  k: Kit,
  s: Parameters<PropRecipe>[0]["s"],
  a: number,
  d: number,
  radius: number,
  height: number,
  sides: number,
  base = 0,
  band: Surface = s.dark,
  inset: number = BARREL.ribTube,
): void {
  k.cylinder(a, d, base, base + height, radius, sides, s.metal);
  for (const h of [height * 0.3, height * 0.7]) {
    k.ring(
      a,
      d,
      base + h,
      radius - inset,
      BARREL.ribTube,
      6,
      sides,
      band,
      "up",
    );
  }
  k.cylinder(
    a,
    d,
    base + height,
    base + height + BARREL.lidLift,
    radius * 0.97,
    sides,
    s.dark,
  );
}

/**
 * Barrel: variant 0 one barrel with ribs and a lid at the anchor; variant 1
 * three smaller barrels clustered so the group fills the wider footprint.
 * Every barrel's two ribs carry the room's accent as its middle band
 * (2.7 C9).
 */
function barrel({ k, s, variant }: Parameters<PropRecipe>[0]): void {
  const { hw, hd } = halfSize("barrel", variant);
  const inset = BARREL.ribTube - RIB_PROUD;
  if (variant === 0) {
    oneBarrel(
      k,
      s,
      0,
      0,
      Math.min(hw, hd) - 0.025,
      0.85,
      12,
      0,
      s.accent(),
      inset,
    );
    return;
  }
  const r = 0.28;
  const positions: [number, number][] = [
    [-(hw - r - 0.02), -(hd - r - 0.15)],
    [hw - r - 0.02, -(hd - r - 0.15)],
    [0, hd - r - 0.02],
  ];
  for (const [a, d] of positions) {
    oneBarrel(k, s, a, d, r, 0.8, 10, 0, s.accent(), inset);
  }
}

/** The trolley deck's thickness and the caster's radius. */
const TROLLEY = { deckT: 0.04, caster: 0.07 };

/** One trolley deck panel from `d0` to `d1` at height `h0` to `h0 + deckT`. */
function trolleyDeck(
  k: Kit,
  s: Parameters<PropRecipe>[0]["s"],
  hw: number,
  d0: number,
  d1: number,
  h0: number,
): void {
  k.bevelBox(
    -hw + 0.03,
    hw - 0.03,
    d0,
    d1,
    h0,
    h0 + TROLLEY.deckT,
    0.01,
    s.metal,
  );
}

/**
 * Trolley: a flat cart on four casters with a handle at the back (`-d`).
 * Variant 0 a single deck, variant 1 two decks stacked on corner posts. The
 * handle's top grip bar carries the room's accent (2.7 C9); its two
 * uprights stay plain metal.
 */
function trolley({ k, s, variant }: Parameters<PropRecipe>[0]): void {
  const { hw, hd } = halfSize("trolley", variant);
  const T = TROLLEY;
  const casterD = hd - T.caster - 0.02;
  const casterA = hw - 0.06;
  for (const a of [-casterA, casterA]) {
    for (const d of [-casterD, casterD]) {
      k.cylinderAlong(a - 0.03, a + 0.03, d, T.caster, T.caster, 8, s.dark);
      k.box(
        a - 0.02,
        a + 0.02,
        d - 0.02,
        d + 0.02,
        T.caster,
        T.caster + 0.16,
        s.dark,
      );
    }
  }
  const deckH = T.caster + 0.16;
  trolleyDeck(k, s, hw, -hd + 0.03, hd - 0.03, deckH);
  if (variant === 1) {
    const upperH = deckH + 0.3;
    trolleyDeck(k, s, hw, -hd + 0.03, hd - 0.03, upperH);
    for (const a of [-hw + 0.05, hw - 0.05]) {
      for (const d of [-hd + 0.05, hd - 0.05]) {
        k.box(
          a - 0.015,
          a + 0.015,
          d - 0.015,
          d + 0.015,
          deckH + T.deckT,
          upperH,
          s.metal,
        );
      }
    }
  }
  const handleH0 = deckH + (variant === 1 ? 0.3 : 0);
  const handleTop = Math.min(handleH0 + 0.45, 0.95);
  const railA = hw - 0.1;
  for (const a of [-railA, railA]) {
    k.cylinderAlong(
      a - 0.012,
      a + 0.012,
      -hd + 0.03,
      (handleH0 + handleTop) / 2,
      0.012,
      6,
      s.dark,
    );
  }
  k.cylinderAlong(-railA, railA, -hd + 0.03, handleTop, 0.015, 6, s.accent());
}

/**
 * Stool: variant 0 a round seat on a post with a foot ring, variant 1 a
 * square seat on four straight legs. The seat carries the room's accent in
 * both variants (2.7 C9).
 */
function stool({ k, s, variant }: Parameters<PropRecipe>[0]): void {
  const { hw, hd } = halfSize("stool", variant);
  const r = Math.min(hw, hd) - 0.03;
  const H = 0.75;
  if (variant === 0) {
    k.cylinder(0, 0, H - 0.04, H, r, 12, s.accent());
    k.cylinder(0, 0, 0.04, H - 0.04, 0.035, 8, s.metal);
    k.cylinder(0, 0, 0, 0.04, r * 0.4, 8, s.dark);
    k.ring(0, 0, 0.24, r * 0.7, 0.014, 6, 12, s.dark, "up");
    return;
  }
  k.bevelBox(-r, r, -r, r, H - 0.04, H, 0.015, s.accent());
  const legA = r - 0.03;
  for (const a of [-legA, legA]) {
    for (const d of [-legA, legA]) {
      k.cylinder(a, d, 0, H - 0.04, 0.02, 6, s.dark);
    }
  }
  for (const a of [-legA, legA]) {
    k.cylinderAlong(-legA, legA, a, 0.18, 0.012, 6, s.dark);
  }
}

/** The filing cabinet's width margin and drawer inset. */
const CABINET = { inset: 0.02, handle: 0.05 };

/**
 * Filing cabinet: a metal body with drawer bands and handles, `drawers`
 * deep, `height` tall.
 */
function filingCabinet({ k, s, variant }: Parameters<PropRecipe>[0]): void {
  const { hw, hd } = halfSize("filing-cabinet", variant);
  const drawers = variant === 0 ? 2 : 4;
  const H = variant === 0 ? 0.8 : 1.4;
  const C = CABINET;
  const front = hd - C.inset;
  k.bevelBox(
    -hw + C.inset,
    hw - C.inset,
    -hd + C.inset,
    front,
    0,
    H,
    0.012,
    s.metal,
  );
  const gap = 0.012;
  const bh = (H - gap * (drawers + 1)) / drawers;
  for (let i = 0; i < drawers; i++) {
    const h0 = gap + i * (bh + gap);
    k.box(
      -hw + 0.04,
      hw - 0.04,
      front - 0.01,
      front + 0.004,
      h0,
      h0 + bh,
      s.dark,
    );
    const hh = h0 + bh * 0.75;
    k.box(
      -C.handle / 2,
      C.handle / 2,
      front + 0.004,
      front + 0.014,
      hh - 0.008,
      hh + 0.008,
      s.metal,
    );
  }
}

/**
 * A cardboard-style box on a shelf, `w` by `dp` by `h` tall, centred at
 * `(a, d)` with its base at `base`.
 */
function shelfBox(
  k: Kit,
  s: Parameters<PropRecipe>[0]["s"],
  a: number,
  d: number,
  base: number,
  w: number,
  dp: number,
  h: number,
): void {
  k.box(a - w / 2, a + w / 2, d - dp / 2, d + dp / 2, base, base + h, s.body);
}

/**
 * Storage shelf: a two-post rack, `shelves` levels between `frame.inset`
 * from the footprint edge. Variant 0 has 4 shelves with boxes on the lower
 * three, variant 1 has 5 bare shelves.
 */
function storageShelf({ k, s, variant }: Parameters<PropRecipe>[0]): void {
  const { hw, hd } = halfSize("storage-shelf", variant);
  const shelves = variant === 0 ? 4 : 5;
  const top = variant === 0 ? 1.6 : 1.8;
  const postA = hw - 0.02;
  const postD = hd - 0.02;
  for (const a of [-postA, postA]) {
    k.box(a - 0.02, a + 0.02, -postD, postD, 0, top, s.metal);
  }
  for (let i = 0; i < shelves; i++) {
    const h = ((i + 1) / shelves) * top;
    k.box(-hw + 0.02, hw - 0.02, -hd + 0.03, hd - 0.03, h - 0.012, h, s.metal);
    if (variant === 0 && i < shelves - 1) {
      shelfBox(
        k,
        s,
        -hw * 0.35,
        0,
        h,
        hw * 0.55,
        hd * 1.2,
        top / shelves - 0.06,
      );
      shelfBox(k, s, hw * 0.35, 0, h, hw * 0.5, hd * 1.1, top / shelves - 0.1);
    }
  }
}

/**
 * Planter: variant 0 a round lathed tub with a single lathed bush, variant
 * 1 a long box tub with three smaller bushes.
 */
function planter({ k, s, variant }: Parameters<PropRecipe>[0]): void {
  const { hw, hd } = halfSize("planter", variant);
  const tub = s.tinted(CLAY);
  const bush = s.tinted(PLANT);
  if (variant === 0) {
    const r = Math.min(hw, hd) - 0.04;
    k.lathe(
      0,
      0,
      [
        [0, 0],
        [r, 0],
        [r * 0.94, 0.42],
        [r * 0.8, 0.45],
        [0, 0.45],
      ],
      12,
      tub,
    );
    k.cylinder(0, 0, 0.42, 0.45, r * 0.75, 10, s.dark);
    k.lathe(
      0,
      0,
      [
        [0, 0.45],
        [r * 0.55, 0.55],
        [r * 0.65, 0.75],
        [r * 0.3, 0.95],
        [0, 1.0],
      ],
      10,
      bush,
    );
    return;
  }
  const boxH = 0.4;
  k.bevelBox(-hw + 0.02, hw - 0.02, -hd + 0.02, hd - 0.02, 0, boxH, 0.02, tub);
  k.box(
    -hw + 0.05,
    hw - 0.05,
    -hd + 0.05,
    hd - 0.05,
    boxH - 0.02,
    boxH,
    s.dark,
  );
  const bushR = Math.min(hw / 3.4, hd) - 0.03;
  for (const a of [-hw * 0.6, 0, hw * 0.6]) {
    k.lathe(
      a,
      0,
      [
        [0, boxH],
        [bushR * 0.5, boxH + 0.1],
        [bushR * 0.6, boxH + 0.28],
        [bushR * 0.28, boxH + 0.42],
        [0, boxH + 0.48],
      ],
      8,
      bush,
    );
  }
}

/**
 * Bench: a slab seat on two legs, front towards `+d`. Variant 1 adds a
 * back panel behind the seat (`-d`). The seat carries the room's accent in
 * both variants (2.7 C9); the legs and the back panel stay plain.
 */
function bench({ k, s, variant }: Parameters<PropRecipe>[0]): void {
  const { hw, hd } = halfSize("bench", variant);
  const seatD0 = variant === 0 ? -hd + 0.02 : -hd * 0.35;
  const seatD1 = hd - 0.02;
  k.bevelBox(
    -hw + 0.02,
    hw - 0.02,
    seatD0,
    seatD1,
    0.42,
    0.48,
    0.012,
    s.accent(),
  );
  const legA = hw - 0.08;
  for (const a of [-legA, legA]) {
    k.box(a - 0.03, a + 0.03, seatD0 + 0.04, seatD1 - 0.04, 0, 0.42, s.metal);
  }
  if (variant === 1) {
    k.bevelBox(
      -hw + 0.04,
      hw - 0.04,
      -hd + 0.02,
      -hd + 0.06,
      0.46,
      0.95,
      0.01,
      s.body,
    );
  }
}

/**
 * A small lathed jar, `radius` wide and `height` tall, base at `base`.
 */
function jar(
  k: Kit,
  s: Parameters<PropRecipe>[0]["s"],
  a: number,
  d: number,
  base: number,
  radius: number,
  height: number,
): void {
  k.lathe(
    a,
    d,
    [
      [0, base],
      [radius, base],
      [radius, base + height * 0.8],
      [radius * 0.55, base + height],
      [0, base + height * 0.9],
    ],
    8,
    s.tinted(GLASS),
  );
}

/**
 * Specimen shelf: the same rack as the storage shelf, shorter, with small
 * lathed jars on every level. Variant 1 adds an emissive strip along the
 * front edge of the top shelf, resting against its shelf lip (so the glow
 * touches a lit part).
 */
function specimenShelf({
  k,
  s,
  ctx,
  variant,
}: Parameters<PropRecipe>[0]): void {
  const { hw, hd } = halfSize("specimen-shelf", variant);
  const shelves = 3;
  const top = 1.7;
  const postA = hw - 0.02;
  for (const a of [-postA, postA]) {
    k.box(a - 0.02, a + 0.02, -hd + 0.02, hd - 0.02, 0, top, s.metal);
  }
  for (let i = 0; i < shelves; i++) {
    const h = ((i + 1) / shelves) * top;
    k.box(-hw + 0.02, hw - 0.02, -hd + 0.03, hd - 0.03, h - 0.012, h, s.metal);
    for (const a of [-hw * 0.5, 0, hw * 0.5]) {
      jar(k, s, a, hd * 0.2, h, Math.min(0.06, hw * 0.18), 0.16);
    }
    if (variant === 1 && i === shelves - 1) {
      const lip = {
        a0: -hw + 0.04,
        a1: hw - 0.04,
        d0: hd - 0.06,
        d1: hd - 0.02,
      };
      k.box(lip.a0, lip.a1, lip.d0, lip.d1, h, h + 0.02, s.dark);
      k.box(
        lip.a0 + 0.01,
        lip.a1 - 0.01,
        lip.d0,
        lip.d1,
        h + 0.002,
        h + 0.014,
        s.glow(ctx.look.palette.lamp),
      );
    }
  }
}

/**
 * Fume cabinet: a tall flat-backed body with a sash window, a top duct
 * stub and a hazard warning strip. Variant 1 is narrower.
 */
function fumeCabinet({ k, s, variant }: Parameters<PropRecipe>[0]): void {
  const { hw, hd } = halfSize("fume-cabinet", variant);
  const H = 1.85;
  const front = hd - 0.02;
  k.bevelBox(-hw + 0.02, hw - 0.02, -hd, front, 0, H, 0.02, s.body);
  k.box(-hw * 0.6, hw * 0.6, front - 0.01, front + 0.006, 0.55, 1.5, s.dark);
  k.box(
    -hw + 0.02,
    hw - 0.02,
    front - 0.01,
    front + 0.006,
    1.55,
    1.63,
    s.hazard,
  );
  k.cylinder(0, 0, H, H + 0.15, 0.11, 10, s.metal);
}

/** The traffic cone's colours: an orange body and a hazard band. */
const CONE_ORANGE: Rgb = [0.85, 0.35, 0.05];

/** Traffic cone: a lathed cone on a square base with a hazard band. Variant 1 is taller. */
function trafficCone({ k, s, variant }: Parameters<PropRecipe>[0]): void {
  const { hw, hd } = halfSize("traffic-cone", variant);
  const base = Math.min(hw, hd) - 0.02;
  const H = variant === 0 ? 0.55 : 0.8;
  const h0 = 0.03;
  const r0 = base * 0.85;
  const rTip = r0 * 0.16;
  k.box(-base, base, -base, base, 0, h0, s.tinted(CONE_ORANGE));
  k.lathe(
    0,
    0,
    [
      [0, h0],
      [r0, h0],
      [rTip, H],
      [0, H + 0.02],
    ],
    10,
    s.tinted(CONE_ORANGE),
  );
  /** The cone's own radius at height `h`, linearly tapered from `r0` to `rTip`. */
  const taper = (h: number) => r0 + (rTip - r0) * ((h - h0) / (H - h0));
  const bandH0 = h0 + (H - h0) * 0.4;
  const bandH1 = bandH0 + (H - h0) * 0.12;
  const bandR0 = taper(bandH0) + 0.006;
  const bandR1 = taper(bandH1) + 0.006;
  k.lathe(
    0,
    0,
    [
      [taper(bandH0), bandH0],
      [bandR0, bandH0],
      [bandR1, bandH1],
      [taper(bandH1), bandH1],
    ],
    10,
    s.hazard,
  );
}

/**
 * Ladder: two rails, a few rungs. `inset` clears room for a tilted rail's
 * own thickness (`railThick`), whose corners reach a little past its
 * centreline in both the along and the depth-height directions.
 */
const LADDER = { rungRadius: 0.014, railThick: 0.04, inset: 0.05 };

/**
 * Ladder: variant 0 two rails leaning back from the footprint's front edge
 * to its back edge with rungs between them, topped under 2.2 m; variant 1 a
 * self-supporting step ladder, front rails leaning back to a single brace.
 */
function ladder({ kitAt, s, variant }: Parameters<PropRecipe>[0]): void {
  const { hw, hd } = halfSize("ladder", variant);
  const L = LADDER;
  const railA = hw - L.railThick - L.inset;
  if (variant === 0) {
    const d0 = hd - L.inset;
    const d1 = -hd + L.inset;
    const h0 = 0.03;
    const h1 = 2.05;
    for (const a of [-railA, railA]) {
      leaningRail(kitAt, a, L.railThick, d0, h0, d1, h1, s.metal);
    }
    const rungs = [0.15, 0.35, 0.55, 0.75, 0.9];
    for (const t of rungs) {
      const d = d0 + (d1 - d0) * t;
      const h = h0 + (h1 - h0) * t;
      kitAt(frameAt([0, 0, 0], 0)).cylinderAlong(
        -railA,
        railA,
        d,
        h,
        L.rungRadius,
        6,
        s.dark,
      );
    }
    return;
  }
  const front0 = hd - L.inset;
  const apexD = 0;
  const apexH = 1.55;
  for (const a of [-railA, railA]) {
    leaningRail(kitAt, a, L.railThick, front0, 0.02, apexD, apexH, s.metal);
  }
  leaningRail(kitAt, 0, L.railThick, -hd + L.inset, 0.02, apexD, apexH, s.dark);
  const steps = [0.2, 0.45, 0.7];
  for (const t of steps) {
    const d = front0 + (apexD - front0) * t;
    const h = 0.02 + (apexH - 0.02) * t;
    kitAt(frameAt([0, 0, 0], 0)).cylinderAlong(
      -railA,
      railA,
      d,
      h,
      LADDER.rungRadius,
      6,
      s.metal,
    );
  }
  kitAt(frameAt([0, 0, 0], 0)).box(
    -railA,
    railA,
    apexD - 0.02,
    apexD + 0.02,
    apexH,
    apexH + 0.03,
    s.dark,
  );
}

/** The tool cart's drawer band height and tool props on top. */
const CART = { drawerH: 0.1 };

/**
 * Tool cart: a red body with stacked drawers and small tools on top. Each
 * drawer's front panel carries the room's accent (2.7 C9); its pull handle
 * stays plain metal.
 */
function toolCart({ k, s, variant }: Parameters<PropRecipe>[0]): void {
  const { hw, hd } = halfSize("tool-cart", variant);
  const drawers = variant === 0 ? 2 : 3;
  const top = 0.55 + (drawers - 2) * CART.drawerH;
  const red = s.tinted(RED);
  k.bevelBox(
    -hw + 0.02,
    hw - 0.02,
    -hd + 0.02,
    hd - 0.02,
    0.14,
    top,
    0.02,
    red,
  );
  // The front panel's outer face stands a hair proud of the body's own
  // face (at `hd - 0.02`), so its accent never sits flush with, or behind,
  // the red body and the two do not fight for the same surface.
  const front = hd - 0.018;
  const gap = 0.01;
  const bh = (top - 0.14 - gap * (drawers + 1)) / drawers;
  for (let i = 0; i < drawers; i++) {
    const h0 = 0.14 + gap + i * (bh + gap);
    k.box(
      -hw + 0.05,
      hw - 0.05,
      front - 0.01,
      front + 0.004,
      h0,
      h0 + bh,
      s.accent(),
    );
    k.box(
      -0.05,
      0.05,
      front + 0.004,
      front + 0.012,
      h0 + bh * 0.4,
      h0 + bh * 0.6,
      s.metal,
    );
  }
  for (const a of [-hw + 0.09, hw - 0.09]) {
    for (const d of [-hd + 0.09, hd - 0.09]) {
      k.cylinderAlong(a - 0.03, a + 0.03, d, 0.07, 0.07, 8, s.dark);
    }
  }
  k.box(-hw * 0.5, -hw * 0.15, -hd * 0.2, hd * 0.2, top, top + 0.05, s.metal);
  k.cylinder(hw * 0.35, 0, top, top + 0.14, 0.03, 8, s.dark);
}

/**
 * Toppled crate: variant 0 a crate lying on its side (a low, wide box) at a
 * scattered yaw; variant 1 two half-crates split apart with a loose lid
 * tilted between them.
 */
function toppledCrate({
  k,
  kitAt,
  s,
  variant,
}: Parameters<PropRecipe>[0]): void {
  const { hw, hd } = halfSize("toppled-crate", variant);
  if (variant === 0) {
    const angle = 0.26;
    const halfW = Math.min(hw, hd) * 0.62;
    const halfT = Math.min(hw, hd) * 0.42;
    const yf = yawed(frameAt([0, 0, 0], 0), 0, 0, angle);
    kitAt(yf).bevelBox(
      -halfW,
      halfW,
      -halfT,
      halfT,
      0,
      halfT * 1.15,
      0.03,
      s.body,
    );
    kitAt(yf).box(
      -halfW * 0.5,
      halfW * 0.9,
      -halfT * 0.9,
      -halfT * 0.5,
      halfT * 1.15,
      halfT * 1.15 + 0.02,
      s.dark,
    );
    return;
  }
  const gap = 0.05;
  const halfEach = hw * 0.42;
  const H = hd * 0.6;
  for (const side of [-1, 1] as const) {
    const cx = side * (halfEach + gap / 2);
    k.bevelBox(
      cx - halfEach,
      cx + halfEach,
      -hd + 0.06,
      hd - 0.06,
      0,
      H,
      0.03,
      s.body,
    );
  }
  const lidW = hw * 0.75;
  const lidD = hd * 0.7;
  const yf = yawed(frameAt([0, 0, 0], 0), 0, 0, -0.35);
  kitAt(yf).box(
    -lidW / 2,
    lidW / 2,
    -lidD / 2,
    lidD / 2,
    H + 0.03,
    H + 0.05,
    s.dark,
  );
}

/** A small tilted crate for the debris pile, `w` by `dp` by `h`, yawed by `angle`. */
function debrisBox(
  kitAt: Parameters<PropRecipe>[0]["kitAt"],
  s: Parameters<PropRecipe>[0]["s"],
  a: number,
  d: number,
  angle: number,
  w: number,
  dp: number,
  h0: number,
  h1: number,
): void {
  const yf = yawed(frameAt([0, 0, 0], 0), a, d, angle);
  kitAt(yf).box(-w / 2, w / 2, -dp / 2, dp / 2, h0, h1, s.body);
}

/**
 * Debris pile: variant 0 a heap of small tilted boxes and a flat plate,
 * variant 1 the same heap with a bent pipe laid across it (two straight
 * segments joined at an angle).
 */
function debrisPile({ k, kitAt, s, variant }: Parameters<PropRecipe>[0]): void {
  const { hw, hd } = halfSize("debris-pile", variant);
  const boxes: [number, number, number, number, number][] = [
    [-hw * 0.5, -hd * 0.35, 0.3, 0.3, 0.2],
    [-hw * 0.15, hd * 0.3, -0.25, 0.32, 0.28],
    [hw * 0.4, -hd * 0.2, 0.5, 0.28, 0.24],
    [hw * 0.1, hd * 0.35, 0.15, 0.3, 0.26],
  ];
  for (const [a, d, angle, w, dp] of boxes) {
    debrisBox(kitAt, s, a, d, angle, w, dp, 0, 0.18);
  }
  const plateW = hw * 0.6;
  const plateD = hd * 0.5;
  const yf = yawed(frameAt([0, 0, 0], 0), -hw * 0.25, hd * 0.05, 0.4);
  kitAt(yf).box(
    -plateW / 2,
    plateW / 2,
    -plateD / 2,
    plateD / 2,
    0.19,
    0.21,
    s.metal,
  );
  if (variant === 1) {
    const bendA = -hw * 0.55;
    const bendD = hd * 0.5;
    k.cylinderAlong(bendA, 0, bendD - hd * 0.3, 0.12, 0.03, 8, s.dark);
    k.cylinderAlong(0, hw * 0.55, bendD, 0.12, 0.03, 8, s.dark);
  }
}

/**
 * Cable coil: variant 0 one flat ring coil on the floor, variant 1 two
 * smaller coils side by side.
 */
function cableCoil({ k, s, variant }: Parameters<PropRecipe>[0]): void {
  const { hw, hd } = halfSize("cable-coil", variant);
  if (variant === 0) {
    const r = Math.min(hw, hd) - 0.1;
    k.ring(0, 0, 0.035, r, 0.03, 8, 16, s.dark, "up");
    k.ring(0, 0, 0.08, r * 0.82, 0.026, 8, 16, s.dark, "up");
    return;
  }
  const r = Math.min(hw * 0.42, hd) - 0.08;
  for (const a of [-hw * 0.42, hw * 0.42]) {
    k.ring(a, 0, 0.025, r, 0.025, 8, 14, s.dark, "up");
  }
}

/**
 * Crate stack: variant 0 a large bevelled crate, a medium one stacked on it
 * and offset along `a` (not rotated), then a small one on that, each a
 * little inside the one below; variant 1 a slatted pallet, two crates side
 * by side on it, one crate across both of them, and a small hazard-taped
 * crate on top.
 */
function crateStack({ k, s, variant }: Parameters<PropRecipe>[0]): void {
  const { hw, hd } = halfSize("crate-stack", variant);
  const bevel = 0.03;
  if (variant === 0) {
    const H0 = 0.8;
    const H1 = 1.4;
    const H2 = 1.8;
    k.bevelBox(
      -hw + 0.02,
      hw - 0.02,
      -hd + 0.02,
      hd - 0.02,
      0,
      H0,
      bevel,
      s.body,
    );
    const mw = (hw - 0.02) * 0.78;
    const md = (hd - 0.02) * 0.78;
    const offA = (hw - 0.02 - mw) * 0.5;
    k.bevelBox(offA - mw, offA + mw, -md, md, H0, H1, bevel * 0.85, s.body);
    const sw = mw * 0.62;
    const sd = md * 0.62;
    k.bevelBox(offA - sw, offA + sw, -sd, sd, H1, H2, bevel * 0.7, s.body);
    return;
  }
  const slats = 4;
  const slatW = 0.14;
  const innerHw = hw - 0.03;
  const spacing = (2 * innerHw - slatW) / (slats - 1);
  for (let i = 0; i < slats; i++) {
    const a = -innerHw + slatW / 2 + i * spacing;
    k.box(
      a - slatW / 2,
      a + slatW / 2,
      -hd + 0.03,
      hd - 0.03,
      0,
      0.12,
      s.metal,
    );
  }
  const gap = 0.05;
  for (const side of [-1, 1] as const) {
    const a0 = side > 0 ? gap / 2 : -hw + 0.03;
    const a1 = side > 0 ? hw - 0.03 : -gap / 2;
    k.bevelBox(a0, a1, -hd + 0.04, hd - 0.04, 0.12, 0.8, bevel, s.body);
  }
  k.bevelBox(
    -hw + 0.02,
    hw - 0.02,
    -hd + 0.02,
    hd - 0.02,
    0.8,
    1.45,
    bevel,
    s.body,
  );
  const sw = hw * 0.45;
  const sd = hd * 0.45;
  k.bevelBox(-sw, sw, -sd, sd, 1.45, 1.95, bevel * 0.8, s.body);
  k.box(-sw + 0.02, sw - 0.02, -sd - 0.005, sd + 0.005, 1.68, 1.76, s.hazard);
}

/**
 * Drum rack: variant 0 a two-tier steel rack (four corner posts, deck
 * plates near the bottom and the middle), two upright drums (`oneBarrel`)
 * on each deck; variant 1 drums lying along `a` in cradles
 * (`cylinderAlong`), three tiers of two, two and one, in a frame of four
 * posts.
 */
function drumRack({ k, s, variant }: Parameters<PropRecipe>[0]): void {
  const { hw, hd } = halfSize("drum-rack", variant);
  const postA = hw - 0.03;
  const postD = hd - 0.03;
  const top = variant === 0 ? 1.8 : 1.75;
  for (const a of [-postA, postA]) {
    for (const d of [-postD, postD]) {
      k.box(a - 0.025, a + 0.025, d - 0.025, d + 0.025, 0, top, s.metal);
    }
  }
  if (variant === 0) {
    const decks = [0.05, 0.95];
    for (const h0 of decks) {
      k.box(
        -hw + 0.02,
        hw - 0.02,
        -hd + 0.02,
        hd - 0.02,
        h0,
        h0 + 0.04,
        s.dark,
      );
    }
    const dr = Math.min(hw, hd) * 0.42;
    for (const h0 of decks) {
      for (const a of [-hw * 0.4, hw * 0.4]) {
        oneBarrel(k, s, a, 0, dr, 0.8, 8, h0 + 0.04);
      }
    }
    return;
  }
  const r = 0.15;
  const a0 = -hw + 0.06;
  const a1 = hw - 0.06;
  const tiers: readonly (readonly [number, readonly number[]])[] = [
    [0.3, [-hd * 0.42, hd * 0.42]],
    [0.85, [-hd * 0.42, hd * 0.42]],
    [1.4, [0]],
  ];
  for (const [h, ds] of tiers) {
    for (const d of ds) k.cylinderAlong(a0, a1, d, h, r, 10, s.metal);
  }
}

/**
 * Gas rack: a base plate and a back frame (two posts, rails near the
 * bottom and the top) hold upright gas cylinders, each a plain body, a
 * short dome and a smaller cap under a coloured collar (`GAS_CAP`).
 * Variant 0 three cylinders at one height with a chain bar across the
 * front; variant 1 five cylinders at two alternating heights with a top
 * guard rail instead.
 */
function gasRack({ k, s, variant }: Parameters<PropRecipe>[0]): void {
  const { hw, hd } = halfSize("gas-rack", variant);
  const cap = s.tinted(GAS_CAP);
  const postA = hw - 0.05;
  const backD = -hd + 0.03;
  const frameTop = 1.7;
  k.box(-hw + 0.02, hw - 0.02, -hd + 0.02, hd - 0.02, 0, 0.05, s.metal);
  for (const a of [-postA, postA]) {
    k.box(
      a - 0.02,
      a + 0.02,
      backD - 0.02,
      backD + 0.02,
      0.05,
      frameTop,
      s.dark,
    );
  }
  for (const h of [0.3, 1.7]) {
    k.box(
      -postA,
      postA,
      backD - 0.015,
      backD + 0.015,
      h - 0.02,
      h + 0.02,
      s.dark,
    );
  }
  const positions =
    variant === 0
      ? [-hw * 0.6, 0, hw * 0.6]
      : [-hw * 0.72, -hw * 0.36, 0, hw * 0.36, hw * 0.72];
  const heights = positions.map((_, i) =>
    variant === 0 ? 1.55 : i % 2 === 0 ? 1.45 : 1.65,
  );
  const r = 0.11;
  positions.forEach((a, i) => {
    const top = heights[i] ?? 1.55;
    k.cylinder(a, 0, 0.05, top, r, 10, s.metal);
    k.cylinder(a, 0, top, top + 0.08, r * 0.75, 10, s.metal);
    k.cylinder(a, 0, top + 0.08, top + 0.14, r * 0.4, 8, s.metal);
    k.cylinder(a, 0, top - 0.06, top + 0.01, r * 1.05, 8, cap);
  });
  if (variant === 0) {
    k.cylinderAlong(-postA, postA, 0, 1.2, 0.012, 6, s.dark);
    return;
  }
  k.cylinderAlong(-postA, postA, backD, 1.75, 0.015, 6, s.metal);
}

/**
 * Potted tree: variant 0 a square tub, a thin trunk and a canopy of three
 * stacked bevelled boxes shrinking upwards; variant 1 a round tub and a
 * bundle of six thin stalks of different heights, each topped with a small
 * leaf box, in `PLANT`.
 */
function pottedTree({ k, s, variant }: Parameters<PropRecipe>[0]): void {
  const { hw, hd } = halfSize("potted-tree", variant);
  const tub = s.tinted(CLAY);
  const plant = s.tinted(PLANT);
  if (variant === 0) {
    const tubH = 0.5;
    k.bevelBox(
      -hw + 0.02,
      hw - 0.02,
      -hd + 0.02,
      hd - 0.02,
      0,
      tubH,
      0.02,
      tub,
    );
    k.cylinder(0, 0, tubH - 0.04, 1.1, 0.05, 8, s.dark);
    const tiers = [
      { h0: 1.1, h1: 1.45, half: hw * 0.62 },
      { h0: 1.45, h1: 1.7, half: hw * 0.42 },
      { h0: 1.7, h1: 1.9, half: hw * 0.24 },
    ];
    for (const t of tiers) {
      k.bevelBox(-t.half, t.half, -t.half, t.half, t.h0, t.h1, 0.02, plant);
    }
    return;
  }
  const r = Math.min(hw, hd) - 0.04;
  k.cylinder(0, 0, 0, 0.45, r, 12, tub);
  k.cylinder(0, 0, 0.41, 0.45, r * 0.85, 12, s.dark);
  const sr = r * 0.55;
  const heights = [1.62, 1.74, 1.86, 1.93, 1.79, 1.67];
  for (let i = 0; i < 6; i++) {
    const angle = (i / 6) * Math.PI * 2;
    const a = Math.cos(angle) * sr;
    const d = Math.sin(angle) * sr;
    const top = heights[i] ?? 1.7;
    k.cylinder(a, d, 0.4, top, 0.02, 6, s.dark);
    k.box(a - 0.045, a + 0.045, d - 0.045, d + 0.045, top, top + 0.04, plant);
  }
}

/** The recipe of every floor prop kind, the condition extras included. */
export const FLOOR_RECIPES = {
  crate,
  barrel,
  trolley,
  stool,
  "filing-cabinet": filingCabinet,
  "storage-shelf": storageShelf,
  planter,
  bench,
  "specimen-shelf": specimenShelf,
  "fume-cabinet": fumeCabinet,
  "traffic-cone": trafficCone,
  ladder,
  "tool-cart": toolCart,
  "toppled-crate": toppledCrate,
  "debris-pile": debrisPile,
  "cable-coil": cableCoil,
  "crate-stack": crateStack,
  "drum-rack": drumRack,
  "gas-rack": gasRack,
  "potted-tree": pottedTree,
} satisfies Record<Exclude<FloorPropKind, RarePropKind>, PropRecipe>;
