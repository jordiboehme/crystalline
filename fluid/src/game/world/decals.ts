/**
 * The decals of a room (2.7 C13 to C19): hazard chevrons, way-out arrows,
 * grime, streaks and rust by condition, and the deck and bay stencils,
 * placed as data. `render/models/decals.ts` draws them; nothing here
 * knows a texture or a colour.
 *
 * `placeDecals` reads a finished room (`DecalBase`: everything but its
 * decals, so after the curios and the finish) and returns its decals in
 * one pass, in this order:
 *
 * 1. **Stencils** (C19). The wall stencil, `DECK n` over `BAY m`, on the
 *    entrance's east neighbour edge `{ x: entrance.x + 1, y: entrance.y,
 *    side: "s" }` (`stencilEdge`) when that is a wall edge and no fixture
 *    edge, 1.5 m up, 0.9 by 0.3 m; the strip in front of it is a lane
 *    (`STENCIL_STRIP`), so no floor prop or hero hides it. The entrance
 *    floor stencil, `BAY m`, at `(entrance.x + 0.5, entrance.y - 1.0)`
 *    reading walking in (turn 0), 1.0 by 0.3 m, or at `entrance.y - 0.5`
 *    when that box is not clear: a lane cell outside the interior band
 *    that nothing takes, so a room always has exactly one (the fallback is
 *    still checked, and the tests pin that it is always clear). Each
 *    overflow bay's `BAY mA` to `BAY mD` at the bay's centre, reading from
 *    the hall (turn 1: every bay lies east of the hall, so its top points
 *    east, away from the reader), when its box is clear. The numbers are `deckNumber` and `bayNumber` (C18). All the
 *    stencils of a room share one seed (they draw nothing), so the sort
 *    orders them by `y`, then `x`. A bay's letter is in `stencil.letter`,
 *    never read from the list order.
 * 2. **Chevrons** (C14), in fixture order: a strip 1.6 by 0.3 m in front
 *    of every bulkhead and blast door, sealed or not, starting
 *    `DOOR_GAP` past the door's own depth (`DOOR_DEPTH`), so the door's
 *    housings, sill and leaves never bury it, and a strip the machine's
 *    width by 0.3 m, from 0.2 to 0.5 m in front of its footprint, before
 *    every `HAZARD_MACHINES` machine; each
 *    only when its box is clear. The tile (variant) is drawn.
 * 3. **Arrows** (C14), 0.6 by 0.9 m, pointing the way out: in each bay one
 *    cell in from its doorway, centred on the doorway's rows, pointing west
 *    (turn 3); in the corridor's cell next to the hall, pointing east
 *    (turn 1); and in a large hall (`isLargeHall`) on the entrance lane
 *    two cells north of the entrance, pointing south (turn 2). Each only
 *    when its box is clear; the corridor's arrow counts the corridor as
 *    floor, every other floor decal the hall and the bays alone.
 * 4. **Streaks** (C14, C15). Each vent grille hangs one from its own edge,
 *    then each pipe-bundle segment whose edge carries no wall prop, in
 *    `PROP_ORDER` (the props' own order), at `DECAL_RATES.streak`; a
 *    second draw under `rust` makes it rust. Its top is the source's
 *    bottom (`STREAK_SOURCES`) less 0.02 m, its length `0.5 + 0.4 * rank`
 *    (rank 0 clean to 3 derelict) but at most its top, and a source whose
 *    bottom is under 0.35 m hangs none. A vent's streak is centred under
 *    it; a pipe segment's is drawn anywhere along its edge. A pipe riser
 *    reaches the floor, so it bleeds at its foot instead, on the same
 *    chance: rust from `dim` on, a streak below, 0.4 by 0.35 m.
 * 5. **Wall smears**: a grime smear low on each free edge (`sites.free`)
 *    that is not reserved and carries no wall prop, at `smear`, 0.8 to 1.6
 *    m wide and 0.4 to 0.7 m tall, anywhere along the edge.
 * 6. **Floor stains**: each floor cell of the hall and then each bay, row
 *    by row, at `stain`; a grime square 0.6 to 1.2 m on the cell's centre,
 *    its turn drawn, kept only when its box is clear.
 * 7. **Faces**: each floor prop with a flat face (`CRATE_FACES`; never the
 *    marked crate, which carries text) on the face its turn points to,
 *    only when a 0.05 m strip in front of that face overlaps no other
 *    prop's footprint, no taken box and no hero. At `band` a chevron band
 *    0.05 to 0.30 m up; at `face` grime (rust at `rust`'s share) above
 *    the band's height (from 0.35 m) or, with no band, from the face's
 *    bottom, 0.25 to 0.5 m tall and never past the face's top, so the two
 *    never share a spot of the face. Both are the face's width less 0.1 m.
 * 8. **The cap** (C17): past `DECAL_CAP`, floor stains are dropped first
 *    (the last placed first), then face decals (bands too), then smears,
 *    then streaks; stencils, chevron strips and arrows never. The list is
 *    then sorted by kind (stencil, chevrons, arrow, streak, rust, grime)
 *    and within a kind by `seed`, `y`, `x`.
 *
 * "Clear" (C16's floor rule): the box stands on the floor (`fitsFloor`),
 * overlaps no taken box of the sites (fixtures, decor, scaffold), no
 * prop's footprint, no hero's footprint and no floor decal placed before
 * it. The wall rule of C16 keeps every wall decal off text: none goes on a
 * fixture edge, a hero's reserved edge or an edge of `noRun` (the wall
 * stencil's own edge aside, which carries nothing by construction), and
 * none on an edge carrying a wall prop, a streak or bleed on its own
 * source aside.
 *
 * Every draw is `createRng` of its anchor's own seed,
 * `seedFor(room.seed, "decal", ...)`, so a decal never moves another and
 * nothing else of the room moves. Every `x`, `y`, `along`, `h`, `width`
 * and `length` is rounded to three places (`round3`) before its box is
 * checked, so the goldens are the same on every engine and the box that
 * was accepted is the box that is returned.
 *
 * This is the generator side: it imports `sites.ts`, `footprints.ts`,
 * `props.ts`, `heroes.ts`, `layout.ts`, `types.ts`, `units.ts` and
 * `core/seed.ts`, never `generate.ts` (which imports it), `dress.ts` or
 * `render/`. The hand tables `STREAK_SOURCES` and `CRATE_FACES` copy their
 * heights from the prop recipes by hand; `decals.test.ts` pins them
 * against the built meshes.
 */

import { createRng, seedFor } from "../core/seed";
import {
  FOOTPRINTS,
  HERO_FRONT,
  heroFootprint,
  propFootprint,
} from "./footprints";
import { heroReserve } from "./heroes";
import { isFloor } from "./layout";
import { PROP_CATALOGUE } from "./props";
import {
  dressingSites,
  edgeKey,
  edgeOf,
  fitsFloor,
  inside,
  isLargeHall,
  overlaps,
  round3,
  stencilEdge,
  wallAnchor,
  type DecalBase,
} from "./sites";
import type {
  Box,
  Condition,
  Decal,
  DecalKind,
  FloorPropKind,
  MachineKind,
  PropKind,
  Rect,
  StencilText,
  WallSlot,
} from "./types";
import { CELL } from "./units";

/** The most decals a room carries (2.7 C17). */
export const DECAL_CAP = 160;

/**
 * The chance of each draw by condition (2.7 C15): a floor stain per floor
 * cell, a streak under a source, rust instead of grime for a streak or a
 * face, a wall smear per empty free edge, face grime per large crate and
 * a chevron band per large crate.
 */
export const DECAL_RATES: Record<
  Condition,
  {
    stain: number;
    streak: number;
    rust: number;
    smear: number;
    face: number;
    band: number;
  }
> = {
  clean: {
    stain: 0.02,
    streak: 0.25,
    rust: 0,
    smear: 0.1,
    face: 0.1,
    band: 0.25,
  },
  construction: {
    stain: 0.05,
    streak: 0.4,
    rust: 0,
    smear: 0.3,
    face: 0.25,
    band: 0.25,
  },
  dim: {
    stain: 0.08,
    streak: 0.7,
    rust: 0.5,
    smear: 0.5,
    face: 0.5,
    band: 0.25,
  },
  derelict: {
    stain: 0.14,
    streak: 1.0,
    rust: 0.7,
    smear: 0.8,
    face: 0.8,
    band: 0.25,
  },
};

/** The machines a chevron strip runs in front of (2.7 C14). */
export const HAZARD_MACHINES: readonly MachineKind[] = [
  "reactor-coupling",
  "containment",
  "cargo-loader",
  "fabricator",
];

/**
 * The wall props a streak hangs from (2.7 C14): the bottom height, in
 * metres, of each variant's model, copied by hand from the recipes (the
 * vent grille's plate at 1.1 m, the pipe bundle's lowest bracket), since
 * the generator side cannot import `render/`.
 */
export const STREAK_SOURCES: Readonly<
  Partial<Record<PropKind, readonly number[]>>
> = {
  "vent-grille": [1.1, 1.1],
  "pipe-bundle": [2.525, 2.485],
};

/**
 * The flat faces of the large floor props (2.7 C14), per variant: the
 * flat part of the face's height, `h0` to `h1` in metres (the bevelled
 * box's side less its bevel), and `inset`, how far the face's plane lies
 * inside the footprint's side. Null for a variant with no flat face at
 * its footprint's front: the small crate between its posts (crate 0) and
 * the pallet stack (crate stack 1). The marked crate is never listed: it
 * carries text.
 */
export const CRATE_FACES: Readonly<
  Partial<
    Record<
      FloorPropKind,
      readonly (null | { h0: number; h1: number; inset: number })[]
    >
  >
> = {
  crate: [
    null,
    { h0: 0.04, h1: 0.81, inset: 0 },
    { h0: 0.03, h1: 0.65, inset: 0 },
  ],
  "crate-stack": [{ h0: 0.03, h1: 0.77, inset: 0.02 }, null],
};

/**
 * A permalink's deck number (2.7 C18): 1 at the domain's root, else
 * `2 + seedFor("deck", domain, folder) % 98`, 2 to 99, where the folder
 * is everything before the permalink's last `/`. One folder is one deck;
 * two folders may share a number (a hash), since the numbers are
 * decoration, not addresses. Milestone 3 numbers its deck labels with
 * this too, so a deck's label and its rooms' stencils agree.
 */
export function deckNumber(domain: string, permalink: string): number {
  const cut = permalink.lastIndexOf("/");
  if (cut < 0) return 1;
  return 2 + (seedFor("deck", domain, permalink.slice(0, cut)) % 98);
}

/**
 * A room's bay number (2.7 C18): `1 + seedFor("bay", domain, permalink) %
 * 99`, 1 to 99. An overflow bay adds its letter, A to D, on its own floor
 * stencil.
 */
export function bayNumber(domain: string, permalink: string): number {
  return 1 + (seedFor("bay", domain, permalink) % 99);
}

/** How many tiles each kind has in the decal atlas (2.7 C20). */
const TILES = {
  chevrons: 2,
  arrow: 1,
  grime: 3,
  streak: 2,
  rust: 2,
  stencil: 1,
} satisfies Record<DecalKind, number>;

/** The sort order of the kinds (step 8). */
const KIND_ORDER = {
  stencil: 0,
  chevrons: 1,
  arrow: 2,
  streak: 3,
  rust: 4,
  grime: 5,
} satisfies Record<DecalKind, number>;

/** A condition's rank, 0 clean to 3 derelict: a streak's length grows with it. */
const RANK = {
  clean: 0,
  construction: 1,
  dim: 2,
  derelict: 3,
} satisfies Record<Condition, number>;

/** The gap between a source's bottom and the top of its streak, in metres. */
const STREAK_GAP = 0.02;
/** A source whose bottom is lower than this hangs no streak, in metres. */
const STREAK_MIN = 0.35;
/**
 * How far a bulkhead or blast door stands out from its wall at floor
 * level, in metres: the housing depth of the door models
 * (`HOUSING_DEPTH` in `render/models/doors.ts`, copied by hand since the
 * generator side cannot import `render/`). A door takes no floor
 * (`footprintOf` is null), so the clear check cannot see it; without
 * this a chevron strip would lie mostly under the bulkhead's sill and
 * side housings and the blast door's lower leaf. `decals.test.ts` pins it
 * against the render constant.
 */
export const DOOR_DEPTH = 0.29;
/** The gap between a door's front and its chevron strip, in metres. */
const DOOR_GAP = 0.03;
/** How deep the strip in front of a crate's face must be clear, in metres. */
const FACE_STRIP = 0.05;
/** A face decal is this much narrower than its face, in metres. */
const FACE_MARGIN = 0.1;
/** The chevron band on a crate's face: its bottom and its height, in metres. */
const BAND = { h: 0.05, length: 0.25 };
/** Where face grime starts above a band, in metres, clear of the band's top (0.3). */
const ABOVE_BAND = 0.35;

/** A floor decal's box in metres: `width` across and `length` along its turn. */
function floorBox(d: Decal): Box {
  const sideways = d.turn % 2 === 1;
  const hx = (sideways ? d.length : d.width) / 2;
  const hz = (sideways ? d.width : d.length) / 2;
  const cx = d.x * CELL;
  const cz = d.y * CELL;
  return { x0: cx - hx, x1: cx + hx, z0: cz - hz, z1: cz + hz };
}

/**
 * A decal with every measure rounded to three places, its keys in
 * `Decal`'s order (the goldens' order), `stencil` last and only on a
 * stencil.
 */
function rounded(d: Decal): Decal {
  const out: Decal = {
    kind: d.kind,
    on: d.on,
    x: round3(d.x),
    y: round3(d.y),
    turn: d.turn,
    along: round3(d.along),
    h: round3(d.h),
    width: round3(d.width),
    length: round3(d.length),
    variant: d.variant,
    seed: d.seed,
  };
  if (d.stencil !== undefined) out.stencil = d.stencil;
  return out;
}

/** True when every cell under `box` is floor inside `r`. */
function fitsRect(room: DecalBase, r: Rect, box: Box): boolean {
  const eps = 1e-9;
  const cx0 = Math.floor((box.x0 + eps) / CELL);
  const cx1 = Math.ceil((box.x1 - eps) / CELL) - 1;
  const cy0 = Math.floor((box.z0 + eps) / CELL);
  const cy1 = Math.ceil((box.z1 - eps) / CELL) - 1;
  for (let y = cy0; y <= cy1; y++)
    for (let x = cx0; x <= cx1; x++)
      if (!isFloor(room.grid, x, y) || !inside(r, x, y)) return false;
  return true;
}

/**
 * The decals of a finished room (2.7 C13 to C19). See the module doc for
 * the pass. `cap` is `DECAL_CAP` for every room the generator makes; a
 * test passes `Infinity` to see what the cap cut.
 */
export function placeDecals(room: DecalBase, cap = DECAL_CAP): Decal[] {
  const sites = dressingSites(room);
  const rates = DECAL_RATES[room.condition];
  const rank = RANK[room.condition];
  const reserved = heroReserve(room.heroes).edges;
  const wallEdges = new Set(sites.runs.flat().map(edgeKey));
  const seedOf = (...parts: (string | number)[]) =>
    seedFor(room.seed, "decal", ...parts);

  const solid: Box[] = [...sites.taken];
  for (const p of room.props) {
    const b = propFootprint(p);
    if (b !== null) solid.push(b);
  }
  for (const h of room.heroes) solid.push(heroFootprint(h));
  const laid: Box[] = [];
  const clearOn = (box: Box, floor: (b: Box) => boolean) =>
    floor(box) &&
    !solid.some((s) => overlaps(box, s)) &&
    !laid.some((s) => overlaps(box, s));
  const onFloor = (b: Box) => fitsFloor(room, b);
  /** Lays a floor decal when its box is clear, and says whether it did. */
  const layFloor = (
    out: Decal[],
    d: Decal,
    floor: (b: Box) => boolean = onFloor,
  ): boolean => {
    const r = rounded(d);
    const box = floorBox(r);
    if (!clearOn(box, floor)) return false;
    laid.push(box);
    out.push(r);
    return true;
  };

  // Non-run wall props, by edge: an edge carrying one takes no decal but a
  // streak or bleed from that very prop.
  const wallProps = new Set<string>();
  for (const p of room.props)
    if (p.anchor === "wall" && !PROP_CATALOGUE[p.kind].run)
      wallProps.add(edgeKey(edgeOf(p)));
  const wallOk = (e: WallSlot) => {
    const k = edgeKey(e);
    return (
      wallEdges.has(k) &&
      !sites.fixtureEdges.has(k) &&
      !sites.noRun.has(k) &&
      !reserved.has(k)
    );
  };
  const onWall = (e: WallSlot) => {
    const a = wallAnchor(e);
    return { on: "wall" as const, x: a.x, y: a.y, turn: a.turn };
  };

  // 1. Stencils.
  const fixed: Decal[] = [];
  const stencilSeed = seedOf("stencil");
  const text = (letter: number, lines: 1 | 2): StencilText => ({
    deck: deckNumber(room.domain, room.permalink),
    bay: bayNumber(room.domain, room.permalink),
    letter,
    lines,
  });
  const stencil = {
    kind: "stencil" as const,
    along: 0,
    variant: 0,
    seed: stencilSeed,
  };
  const ent = room.entrance;
  const signEdge = stencilEdge(room);
  const signKey = edgeKey(signEdge);
  if (wallEdges.has(signKey) && !sites.fixtureEdges.has(signKey))
    fixed.push(
      rounded({
        ...stencil,
        ...onWall(signEdge),
        h: 1.5,
        width: 0.9,
        length: 0.3,
        stencil: text(0, 2),
      }),
    );
  const lane = (y: number): Decal => ({
    ...stencil,
    on: "floor",
    x: ent.x + 0.5,
    y,
    turn: 0,
    h: 0,
    width: 1.0,
    length: 0.3,
    stencil: text(0, 1),
  });
  // The fallback is a lane cell outside the interior band, which nothing
  // takes; it is checked all the same, and the tests pin that it is laid.
  if (!layFloor(fixed, lane(ent.y - 1.0))) layFloor(fixed, lane(ent.y - 0.5));
  room.bays.forEach((bay, i) => {
    layFloor(fixed, {
      ...stencil,
      on: "floor",
      x: (bay.x0 + bay.x1) / 2,
      y: (bay.y0 + bay.y1) / 2,
      turn: 1,
      h: 0,
      width: 1.0,
      length: 0.3,
      stencil: text(i + 1, 1),
    });
  });

  // 2. Chevrons.
  const strip = (e: WallSlot, along: number, from: number, token: string) => {
    const seed = seedOf("chevrons", edgeKey(e), token);
    const a = wallAnchor(e);
    const [fx, fz] = HERO_FRONT[a.turn] ?? [0, -1];
    const out = (from + 0.15) / CELL;
    layFloor(fixed, {
      kind: "chevrons",
      on: "floor",
      x: a.x + fx * out,
      y: a.y + fz * out,
      turn: a.turn,
      along: 0,
      h: 0,
      width: along,
      length: 0.3,
      variant: createRng(seed).int(0, TILES.chevrons - 1),
      seed,
    });
  };
  for (const f of room.fixtures) {
    if (f.kind === "door" && f.style !== "sliding")
      strip(f.slot, 1.6, DOOR_DEPTH + DOOR_GAP, "door");
    else if (f.kind === "machine" && HAZARD_MACHINES.includes(f.machine)) {
      const size = FOOTPRINTS.machine[f.machine];
      strip(f.slot, size.along, size.out + 0.2, "machine");
    }
  }

  // 3. Arrows.
  const arrow = (x: number, y: number, turn: number, ...token: string[]) => ({
    kind: "arrow" as const,
    on: "floor" as const,
    x,
    y,
    turn,
    along: 0,
    h: 0,
    width: 0.6,
    length: 0.9,
    variant: 0,
    seed: seedOf("arrow", ...token),
  });
  room.bays.forEach((bay, i) => {
    const rows: number[] = [];
    for (let y = bay.y0; y < bay.y1; y++)
      if (isFloor(room.grid, bay.x0 - 1, y)) rows.push(y);
    const first = rows[0];
    const last = rows.at(-1);
    if (first === undefined || last === undefined) return;
    layFloor(
      fixed,
      arrow(bay.x0 + 0.5, (first + last + 1) / 2, 3, "bay", String(i)),
    );
  });
  const corridor = room.corridor;
  if (corridor !== null)
    layFloor(
      fixed,
      arrow(corridor.x1 - 0.5, (corridor.y0 + corridor.y1) / 2, 1, "corridor"),
      (b) => fitsRect(room, corridor, b),
    );
  if (isLargeHall(room.hall))
    layFloor(fixed, arrow(ent.x + 0.5, ent.y - 1.5, 2, "entrance"));

  // 4. Streaks, and the pipe risers' bleeds.
  const streaks: Decal[] = [];
  const hang = (e: WallSlot, top: number, seed: number, centred: boolean) => {
    const rng = createRng(seed);
    if (!rng.chance(rates.streak)) return;
    const kind: DecalKind = rng.chance(rates.rust) ? "rust" : "streak";
    const width = rng.range(0.35, 0.6);
    const variant = rng.int(0, TILES[kind] - 1);
    const reach = CELL / 2 - width / 2;
    const along = centred ? 0 : rng.range(-reach, reach);
    const length = Math.min(0.5 + 0.4 * rank, top);
    streaks.push(
      rounded({
        kind,
        ...onWall(e),
        along,
        h: top - length,
        width,
        length,
        variant,
        seed,
      }),
    );
  };
  for (const p of room.props) {
    if (p.anchor !== "wall") continue;
    const e = edgeOf(p);
    if (!wallOk(e)) continue;
    if (p.kind === "vent-grille") {
      const bottom = STREAK_SOURCES[p.kind]?.[p.variant];
      if (bottom === undefined || bottom < STREAK_MIN) continue;
      hang(e, bottom - STREAK_GAP, seedOf(edgeKey(e), "streak"), true);
    } else if (p.kind === "pipe-riser") {
      const seed = seedOf(edgeKey(e), "bleed");
      const rng = createRng(seed);
      if (!rng.chance(rates.streak)) continue;
      const kind: DecalKind = rank >= RANK.dim ? "rust" : "streak";
      streaks.push(
        rounded({
          kind,
          ...onWall(e),
          along: 0,
          h: 0,
          width: 0.4,
          length: 0.35,
          variant: rng.int(0, TILES[kind] - 1),
          seed,
        }),
      );
    }
  }
  for (const p of room.props) {
    if (p.kind !== "pipe-bundle") continue;
    const e = edgeOf(p);
    if (!wallOk(e) || wallProps.has(edgeKey(e))) continue;
    const bottom = STREAK_SOURCES[p.kind]?.[p.variant];
    if (bottom === undefined || bottom < STREAK_MIN) continue;
    hang(e, bottom - STREAK_GAP, seedOf(edgeKey(e), "streak-run"), false);
  }

  // 5. Wall smears.
  const smears: Decal[] = [];
  for (const e of sites.runs.flat()) {
    const k = edgeKey(e);
    if (!sites.free.has(k) || reserved.has(k) || wallProps.has(k)) continue;
    const seed = seedOf(k, "smear");
    const rng = createRng(seed);
    if (!rng.chance(rates.smear)) continue;
    const width = rng.range(0.8, 1.6);
    const length = rng.range(0.4, 0.7);
    const reach = CELL / 2 - width / 2;
    const along = rng.range(-reach, reach);
    smears.push(
      rounded({
        kind: "grime",
        ...onWall(e),
        along,
        h: 0,
        width,
        length,
        variant: rng.int(0, TILES.grime - 1),
        seed,
      }),
    );
  }

  // 6. Floor stains.
  const stains: Decal[] = [];
  for (const r of [room.hall, ...room.bays])
    for (let y = r.y0; y < r.y1; y++)
      for (let x = r.x0; x < r.x1; x++) {
        if (!isFloor(room.grid, x, y)) continue;
        const seed = seedOf(x, y, "stain");
        const rng = createRng(seed);
        if (!rng.chance(rates.stain)) continue;
        const size = rng.range(0.6, 1.2);
        layFloor(stains, {
          kind: "grime",
          on: "floor",
          x: x + 0.5,
          y: y + 0.5,
          turn: rng.int(0, 3),
          along: 0,
          h: 0,
          width: size,
          length: size,
          variant: rng.int(0, TILES.grime - 1),
          seed,
        });
      }

  // 7. Faces.
  const faces: Decal[] = [];
  const boxes = room.props.map(propFootprint);
  room.props.forEach((p, i) => {
    if (p.anchor !== "floor") return;
    const face = CRATE_FACES[p.kind as FloorPropKind]?.[p.variant];
    const size = FOOTPRINTS.prop[p.kind as FloorPropKind][p.variant];
    const own = boxes[i];
    if (face === undefined || face === null || size === undefined || !own)
      return;
    const turn = ((p.turn % 4) + 4) % 4;
    const [fx, fz] = HERO_FRONT[turn] ?? [0, -1];
    // The strip in front of the footprint's front side.
    const front: Box =
      fx > 0
        ? { ...own, x0: own.x1, x1: own.x1 + FACE_STRIP }
        : fx < 0
          ? { ...own, x0: own.x0 - FACE_STRIP, x1: own.x0 }
          : fz > 0
            ? { ...own, z0: own.z1, z1: own.z1 + FACE_STRIP }
            : { ...own, z0: own.z0 - FACE_STRIP, z1: own.z0 };
    const blocked =
      boxes.some((b, j) => j !== i && b !== null && overlaps(front, b)) ||
      sites.taken.some((b) => overlaps(front, b)) ||
      room.heroes.some((h) => overlaps(front, heroFootprint(h)));
    if (blocked) return;
    const seed = seedOf(p.x, p.y, "face");
    const rng = createRng(seed);
    const band = rng.chance(rates.band);
    const bandTile = rng.int(0, TILES.chevrons - 1);
    const grime = rng.chance(rates.face);
    const kind: DecalKind = rng.chance(rates.rust) ? "rust" : "grime";
    const tile = rng.int(0, TILES[kind] - 1);
    const tall = rng.range(0.25, 0.5);
    const out = (size.depth / 2 - face.inset) / CELL;
    const at = {
      on: "face" as const,
      x: p.x + fx * out,
      y: p.y + fz * out,
      turn,
      along: 0,
      width: size.width - 2 * face.inset - FACE_MARGIN,
      seed,
    };
    if (band)
      faces.push(
        rounded({
          ...at,
          kind: "chevrons",
          h: BAND.h,
          length: BAND.length,
          variant: bandTile,
        }),
      );
    if (grime) {
      const h = band ? ABOVE_BAND : face.h0;
      const length = Math.min(tall, face.h1 - h);
      if (length > 0)
        faces.push(rounded({ ...at, kind, h, length, variant: tile }));
    }
  });

  // 8. The cap, then the order.
  const drops = [stains, faces, smears, streaks];
  let over = fixed.length + drops.reduce((n, list) => n + list.length, 0) - cap;
  for (const list of drops) {
    if (over <= 0) break;
    const cut = Math.min(over, list.length);
    list.length -= cut;
    over -= cut;
  }
  return [...fixed, ...streaks, ...smears, ...stains, ...faces].sort(
    (a, b) =>
      KIND_ORDER[a.kind] - KIND_ORDER[b.kind] ||
      a.seed - b.seed ||
      a.y - b.y ||
      a.x - b.x,
  );
}
