/**
 * The set dressing: the props that make a room look lived in. They are pure
 * decoration and never data; the room's content speaks only through its
 * fixtures. The pass reads a finished room (fixtures, decor and scaffolding,
 * `RoomBase`) and only ever adds props, so a prop never moves a fixture.
 *
 * Where props may go comes from `dressingSites` (`sites.ts`); what goes
 * there comes from the catalogue and palettes in `props.ts`. The pass, in
 * this order:
 *
 * 1. `sites = dressingSites(room)`, `used` (the wall edges a wall prop took),
 *    `floorBoxes` (the boxes of the floor props placed so far) and
 *    `floorCells` (the cells that have a floor prop). `reserved` joins
 *    `sites.taken`.
 * 2. Mandatory wall props (ruling 7), each on a free edge not in `used`: a
 *    keycard reader beside each unsealed door, then a sign plate beside each
 *    door and hatch, both trying the next edge of the fixture's wall run and
 *    then the previous one; then an extinguisher on each run at along-run
 *    index `3 + 6k`, falling back to `+1` and then `-1`. Its variant is the
 *    first draw of `createRng(seed)` (a sign's variant is its pictogram).
 * 3. Wall runs, when the palette has one: `createRng(seedFor(roomSeed,
 *    "prop-runs", "wall"))` draws the count (1 or 2), with 1 the long wall,
 *    then the variant the whole run shares; one segment per long-wall edge
 *    not in `noRun`, token `run-<side>`. Runs do not add to `used`.
 * 4. Optional wall props: every free edge not in `used`, in run order,
 *    draws from `createRng(seed)` a chance of `WALL_SHARE`, then a kind
 *    weighted over the palette's wall picks plus `FILLER`, then a variant.
 * 5. Floor spots: every corner-zone spot, and every wall-side spot whose
 *    wall edge is not in `used`. A zone spot's prop is centred on its cell;
 *    a wall-side spot's is backed to its wall, its centre `WALL_GAP +
 *    depth / 2` metres off it, facing away from it (`turnForSide`). A prop
 *    is accepted when its `propFootprint` fits the floor, overlaps no lane,
 *    no taken box and no floor box placed before, and its cell has no floor
 *    prop yet. Every spot's draws come from `createRng` of its own seed,
 *    `seedFor(roomSeed, "prop", cx, cy, "floor")`: the kind when it is
 *    picked, then the variant, then the turn of a zone spot.
 * 6. Condition extras (ruling 12): `createRng(seedFor(roomSeed, "extras",
 *    condition))` draws each rule's count in `EXTRAS` order. Each floor
 *    extra goes to the first spot in floor-seed order that accepts it, a
 *    wall-backed one (the ladder) only to a wall-side spot. The loose
 *    cables' count is kept for step 9.
 * 7. Corner zones: each zone takes `1` prop on a bridge, otherwise `1 +
 *    (seedFor(roomSeed, "prop-zone", key) % 2)`. Its remaining spots are
 *    walked in seed order, each drawing a kind weighted over the palette's
 *    floor picks that are not wall-backed, a variant and a turn, until that
 *    many are placed or the spots run out, so a cramped zone gets fewer.
 * 8. Wall-side cells: every remaining wall-side spot draws a chance of the
 *    palette's `wallSide`, then a kind weighted over all its floor picks
 *    (wall-backed ones allowed), then a variant, and is placed when
 *    accepted.
 * 9. Ceiling, anchored at wall points like a wall prop: the ceiling run when
 *    the palette has one, drawn as in step 3 from `"ceiling"`, token
 *    `ceiling-<side>`; under each ceiling tray segment a cable loop when
 *    `createRng` of the loop's own seed (token `loop-<side>`) draws
 *    `LOOP_SHARE`; the beacon on the entrance edge, token `beacon-s`; and
 *    the loose cables of step 6 on the hall's wall edges that carry neither
 *    a fixture nor a ceiling segment nor the beacon, in the order of their
 *    own seeds (token `loose-<side>`).
 * 10. The cap: `capProps(candidates, PROP_CAP)`. Readers, door and hatch
 *     signs, the step-2 extinguishers and the beacon are mandatory,
 *     everything else optional.
 * 11. The output: `x` and `y` rounded to three decimals (done as each prop
 *     is made, so acceptance measures exactly the prop that is returned),
 *     sorted by `PROP_ORDER`.
 *
 * Every seed is keyed by the integer anchor cell and a token (ruling 2),
 * never by a position in a list, and every draw comes from the rng of that
 * one prop's seed, so adding a fixture only changes the props near it.
 * That locality holds only below the cap: near `PROP_CAP` a new fixture can
 * change which props the cap drops anywhere in the room.
 *
 * This is the generator side: it imports `props.ts`, `sites.ts`,
 * `footprints.ts`, `types.ts`, `units.ts` and the seeds, and never
 * `move.ts`, `generate.ts` or `interact.ts` (ruling 20). `dress.test.ts`
 * keeps it so.
 */

import { createRng, seedFor, type Rng } from "../core/seed";
import { FOOTPRINTS, propFootprint } from "./footprints";
import {
  EXTINGUISHER_EVERY,
  EXTRAS,
  FILLER,
  LOOP_SHARE,
  PALETTES,
  PROP_CAP,
  PROP_CATALOGUE,
  WALL_SHARE,
} from "./props";
import {
  dressingSites,
  edgeKey,
  fitsFloor,
  overlaps,
  turnForSide,
  wallAnchor,
  type FloorSpot,
  type RoomBase,
} from "./sites";
import type {
  Box,
  FloorPropKind,
  Prop,
  PropAnchor,
  PropKind,
  WallSlot,
} from "./types";
import { CELL } from "./units";

/**
 * A prop the pass would place, and whether the cap must keep it as long as
 * its tier allows: readers, door and hatch signs, the extinguishers of the
 * runs and the beacon are mandatory, everything else is optional.
 */
export interface Candidate {
  prop: Prop;
  mandatory: boolean;
}

/** The order the cap keeps tiers in: walls last to go, ceilings first. */
const TIER: Record<PropAnchor, number> = { ceiling: 0, floor: 1, wall: 2 };

/**
 * Keeps at most `cap` props. Drops the lowest tier first (ceiling, then
 * floor, then wall); within a tier optional props before mandatory ones;
 * within those the highest seed first, with kind, x and y breaking a tie so
 * the result never depends on input order.
 */
export function capProps(
  candidates: readonly Candidate[],
  cap: number,
): Prop[] {
  if (candidates.length <= cap) return candidates.map((c) => c.prop);
  const keep = [...candidates].sort(
    (a, b) =>
      TIER[b.prop.anchor] - TIER[a.prop.anchor] ||
      Number(b.mandatory) - Number(a.mandatory) ||
      a.prop.seed - b.prop.seed ||
      a.prop.kind.localeCompare(b.prop.kind) ||
      a.prop.x - b.prop.x ||
      a.prop.y - b.prop.y,
  );
  return keep.slice(0, cap).map((c) => c.prop);
}

/** Where each anchor sorts in the output: wall, floor, ceiling. */
const ANCHOR_ORDER = { wall: 0, floor: 1, ceiling: 2 } satisfies Record<
  PropAnchor,
  number
>;

/**
 * The order of a room's props, and so of `RoomSpec.props` and the goldens:
 * wall props, then floor props, then ceiling props; within an anchor by `y`,
 * then `x`, then `turn`, then `kind` (by code point, never by locale), then
 * `variant`. No two props share all of these, so the order is total.
 */
export const PROP_ORDER = (a: Prop, b: Prop): number =>
  ANCHOR_ORDER[a.anchor] - ANCHOR_ORDER[b.anchor] ||
  a.y - b.y ||
  a.x - b.x ||
  a.turn - b.turn ||
  (a.kind < b.kind ? -1 : a.kind > b.kind ? 1 : 0) ||
  a.variant - b.variant;

/** How far a wall-backed floor prop stands off its wall, in metres. */
export const WALL_GAP = 0.05;

/** The along-run index of a run's first extinguisher (ruling 7). */
const EXTINGUISHER_FIRST = 3;

/** Rounds a coordinate, so the golden is the same on every engine. */
function round3(v: number) {
  return Math.round(v * 1000) / 1000;
}

function cellKey(x: number, y: number) {
  return `${String(x)},${String(y)}`;
}

/** One pick of a weighted list; an empty list is a palette bug. */
function weighted<K>(rng: Rng, picks: readonly (readonly [K, number])[]): K {
  let total = 0;
  for (const [, w] of picks) total += w;
  let r = rng.next() * total;
  for (const [k, w] of picks) {
    r -= w;
    if (r < 0) return k;
  }
  const last = picks.at(-1);
  if (last === undefined) throw new Error("weighted pick from an empty list");
  return last[0];
}

function variantOf(kind: PropKind, rng: Rng) {
  return rng.int(0, PROP_CATALOGUE[kind].variants - 1);
}

function isFloorKind(kind: PropKind): kind is FloorPropKind {
  return PROP_CATALOGUE[kind].anchor === "floor";
}

/**
 * Every prop the pass would place in a room, before the cap, each marked
 * mandatory or optional: steps 1 to 9 of the module doc. `reserved` joins
 * the taken boxes and keeps floor props out of what a later pass claimed.
 */
export function dressCandidates(
  room: RoomBase,
  reserved: readonly Box[] = [],
): Candidate[] {
  const sites = dressingSites(room);
  const palette = PALETTES[room.archetype];
  const used = new Set<string>();
  const floorBoxes: Box[] = [];
  const floorCells = new Set<string>();
  const blocked = [...sites.taken, ...reserved];
  const out: Candidate[] = [];
  const propSeed = (x: number, y: number, token: string) =>
    seedFor(room.seed, "prop", x, y, token);

  const atWall = (
    kind: PropKind,
    e: WallSlot,
    variant: number,
    seed: number,
  ): Prop => {
    const a = wallAnchor(e);
    return {
      kind,
      variant,
      anchor: PROP_CATALOGUE[kind].anchor,
      x: round3(a.x),
      y: round3(a.y),
      turn: a.turn,
      seed,
    };
  };

  // Step 2: the mandatory wall props.
  const where = new Map<string, { run: WallSlot[]; i: number }>();
  for (const run of sites.runs)
    run.forEach((e, i) => where.set(edgeKey(e), { run, i }));
  const open = (e: WallSlot | undefined): e is WallSlot =>
    e !== undefined && sites.free.has(edgeKey(e)) && !used.has(edgeKey(e));
  const onWall = (kind: PropKind, e: WallSlot, mandatory: boolean) => {
    const seed = propSeed(e.x, e.y, e.side);
    used.add(edgeKey(e));
    out.push({
      prop: atWall(kind, e, variantOf(kind, createRng(seed)), seed),
      mandatory,
    });
  };
  const beside = (slot: WallSlot) => {
    const w = where.get(edgeKey(slot));
    return w === undefined ? [] : [w.run[w.i + 1], w.run[w.i - 1]];
  };
  for (const f of room.fixtures) {
    if (f.kind !== "door" || f.address === null) continue;
    const e = beside(f.slot).find(open);
    if (e !== undefined) onWall("keycard-reader", e, true);
  }
  for (const f of room.fixtures) {
    if (f.kind !== "door" && f.kind !== "hatch") continue;
    const e = beside(f.slot).find(open);
    if (e !== undefined) onWall("sign-plate", e, true);
  }
  for (const run of sites.runs)
    for (let i = EXTINGUISHER_FIRST; i < run.length; i += EXTINGUISHER_EVERY) {
      const e = [run[i], run[i + 1], run[i - 1]].find(open);
      if (e !== undefined) onWall("extinguisher", e, true);
    }

  // Steps 3 and 9: a run along one or two long walls, one segment per edge.
  const runOf = (kind: PropKind, which: "wall" | "ceiling", token: string) => {
    const rng = createRng(seedFor(room.seed, "prop-runs", which));
    const count = rng.int(1, 2);
    const walls = count === 2 ? sites.longWalls : [rng.pick(sites.longWalls)];
    const variant = variantOf(kind, rng);
    const segments: { e: WallSlot; prop: Prop }[] = [];
    for (const wall of walls)
      for (const e of wall) {
        if (sites.noRun.has(edgeKey(e))) continue;
        const seed = propSeed(e.x, e.y, `${token}-${e.side}`);
        segments.push({ e, prop: atWall(kind, e, variant, seed) });
      }
    return segments;
  };
  if (palette.wallRun !== null)
    for (const { prop } of runOf(palette.wallRun, "wall", "run"))
      out.push({ prop, mandatory: false });

  // Step 4: the optional wall props.
  const wallPicks = [...palette.wall, ...FILLER];
  for (const e of sites.runs.flat()) {
    const k = edgeKey(e);
    if (!sites.free.has(k) || used.has(k)) continue;
    const seed = propSeed(e.x, e.y, e.side);
    const rng = createRng(seed);
    if (!rng.chance(WALL_SHARE)) continue;
    const kind = weighted(rng, wallPicks);
    used.add(k);
    out.push({
      prop: atWall(kind, e, variantOf(kind, rng), seed),
      mandatory: false,
    });
  }

  // Step 5: the floor spots and how a prop is accepted on one.
  const wallSide = sites.wallSide.filter(
    (s) =>
      s.wall !== null && !used.has(edgeKey({ x: s.cx, y: s.cy, side: s.wall })),
  );
  const floorSeed = (s: FloorSpot) => propSeed(s.cx, s.cy, "floor");
  const bySeed = (a: FloorSpot, b: FloorSpot) =>
    floorSeed(a) - floorSeed(b) || a.cy - b.cy || a.cx - b.cx;
  const free = (s: FloorSpot) => !floorCells.has(cellKey(s.cx, s.cy));
  const onFloor = (kind: FloorPropKind, s: FloorSpot, rng: Rng): Prop => {
    const variant = variantOf(kind, rng);
    let x = s.cx + 0.5;
    let y = s.cy + 0.5;
    let turn: number;
    if (s.wall === null) turn = rng.int(0, 3);
    else {
      turn = turnForSide(s.wall);
      const size = FOOTPRINTS.prop[kind][variant];
      if (size === undefined) throw new Error(`${kind} has no variant`);
      const off = (WALL_GAP + size.depth / 2) / CELL;
      if (s.wall === "n") y = s.cy + off;
      else if (s.wall === "s") y = s.cy + 1 - off;
      else if (s.wall === "w") x = s.cx + off;
      else x = s.cx + 1 - off;
    }
    const seed = floorSeed(s);
    return {
      kind,
      variant,
      anchor: "floor",
      x: round3(x),
      y: round3(y),
      turn,
      seed,
    };
  };
  const place = (prop: Prop, s: FloorSpot): boolean => {
    if (!free(s)) return false;
    const box = propFootprint(prop);
    if (box === null || !fitsFloor(room, box)) return false;
    if (sites.lanes.some((l) => overlaps(box, l))) return false;
    if (blocked.some((t) => overlaps(box, t))) return false;
    if (floorBoxes.some((b) => overlaps(box, b))) return false;
    floorBoxes.push(box);
    floorCells.add(cellKey(s.cx, s.cy));
    out.push({ prop, mandatory: false });
    return true;
  };

  // Step 6: the condition extras.
  const spots = [...sites.zones.flatMap((z) => z.spots), ...wallSide].sort(
    bySeed,
  );
  const extras = createRng(seedFor(room.seed, "extras", room.condition));
  let loose = 0;
  for (const rule of EXTRAS[room.condition]) {
    const n = extras.int(rule.min, rule.max);
    const kind: PropKind = rule.kind;
    if (!isFloorKind(kind)) {
      loose += n;
      continue;
    }
    const wallBacked = PROP_CATALOGUE[kind].wallBacked;
    for (let i = 0; i < n; i++)
      for (const s of spots) {
        if (!free(s) || (wallBacked && s.wall === null)) continue;
        if (place(onFloor(kind, s, createRng(floorSeed(s))), s)) break;
      }
  }

  // Step 7: the corner zones.
  const zonePicks = palette.floor.filter(
    ([k]) => !PROP_CATALOGUE[k].wallBacked,
  );
  for (const z of sites.zones) {
    if (zonePicks.length === 0) break;
    const count =
      palette.cornerMax === 1
        ? 1
        : 1 + (seedFor(room.seed, "prop-zone", z.key) % 2);
    let placed = 0;
    for (const s of z.spots.filter(free).sort(bySeed)) {
      if (placed >= count) break;
      const rng = createRng(floorSeed(s));
      if (place(onFloor(weighted(rng, zonePicks), s, rng), s)) placed++;
    }
  }

  // Step 8: the wall-side cells.
  for (const s of wallSide) {
    if (!free(s)) continue;
    const rng = createRng(floorSeed(s));
    if (!rng.chance(palette.wallSide)) continue;
    place(onFloor(weighted(rng, palette.floor), s, rng), s);
  }

  // Step 9: the ceiling.
  const carriers = new Set<string>();
  if (palette.ceilingRun !== null) {
    const tray = palette.ceilingRun === "ceiling-tray";
    for (const { e, prop } of runOf(palette.ceilingRun, "ceiling", "ceiling")) {
      carriers.add(edgeKey(e));
      out.push({ prop, mandatory: false });
      if (!tray) continue;
      const seed = propSeed(e.x, e.y, `loop-${e.side}`);
      const rng = createRng(seed);
      if (!rng.chance(LOOP_SHARE)) continue;
      out.push({
        prop: atWall("cable-loop", e, variantOf("cable-loop", rng), seed),
        mandatory: false,
      });
    }
  }
  const entrance: WallSlot = { ...room.entrance, side: "s" };
  const beaconSeed = propSeed(entrance.x, entrance.y, "beacon-s");
  out.push({
    prop: atWall(
      "beacon",
      entrance,
      variantOf("beacon", createRng(beaconSeed)),
      beaconSeed,
    ),
    mandatory: true,
  });
  if (loose > 0) {
    const h = room.hall;
    const skip = new Set([
      ...sites.fixtureEdges,
      ...carriers,
      edgeKey(entrance),
    ]);
    const looseSeed = (e: WallSlot) => propSeed(e.x, e.y, `loose-${e.side}`);
    const edges = sites.runs
      .flat()
      .filter(
        (e) =>
          e.x >= h.x0 &&
          e.x < h.x1 &&
          e.y >= h.y0 &&
          e.y < h.y1 &&
          !skip.has(edgeKey(e)),
      )
      .sort((a, b) => looseSeed(a) - looseSeed(b));
    for (const e of edges.slice(0, loose)) {
      const seed = looseSeed(e);
      out.push({
        prop: atWall(
          "loose-cable",
          e,
          variantOf("loose-cable", createRng(seed)),
          seed,
        ),
        mandatory: false,
      });
    }
  }
  return out;
}

/**
 * The set dressing of a room: `dressCandidates`, capped at `PROP_CAP`
 * (`capProps`) and sorted by `PROP_ORDER`. See the module doc for the pass.
 * `reserved` keeps floor props out of boxes a later pass has claimed; the
 * generator passes none.
 */
export function dressRoom(
  room: RoomBase,
  reserved: readonly Box[] = [],
): Prop[] {
  return capProps(dressCandidates(room, reserved), PROP_CAP).sort(PROP_ORDER);
}
