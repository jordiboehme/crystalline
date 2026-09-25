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
 *    `wallKinds` (the kind of the wall prop on each of those edges),
 *    `floorBoxes` (the boxes of the floor props placed so far) and
 *    `floorCells` (the cells that have a floor prop). `reserved` joins
 *    `sites.taken`.
 * 2. Mandatory wall props (ruling 7), each on a free edge not in `used`: a
 *    keycard reader beside each unsealed door, then a sign plate beside each
 *    door and hatch, both trying the next edge of the fixture's wall run and
 *    then the previous one; then an extinguisher on each run at along-run
 *    index `3 + 6k`, falling back to `+1` and then `-1`. Its variant is the
 *    first draw of `createRng(seed)` (a sign's variant is its pictogram).
 *    Each is recorded in `used` and `wallKinds`.
 * 3. Wall runs, when the palette has one: `createRng(seedFor(roomSeed,
 *    "prop-runs", "wall"))` draws the count (1 or 2), with 1 the long wall,
 *    then the variant the whole run shares; one segment per long-wall edge
 *    not in `noRun`, token `run-<side>`. Runs do not add to `used`.
 * 4. Optional wall props: every free edge not in `used`, in run order,
 *    draws from `createRng(seed)` a chance of `WALL_SHARE`, then a kind
 *    weighted over the palette's wall picks plus `FILLER`, then a variant.
 *    Each is recorded in `used` and `wallKinds`.
 * 5. Floor spots and their backing. A spot's backing is what stands behind
 *    it. A zone spot has no wall and is always `bare`, whatever hangs on
 *    its cell's walls. A wall-side spot is `clear` when any of its cell's
 *    four wall edges carries a keep-clear wall prop (`PropEntry.keepClear`),
 *    otherwise `screen` when its own wall edge carries a wall prop,
 *    otherwise `bare`. The floor spots are every corner-zone spot and every
 *    wall-side spot whose backing is not `clear`. A zone spot's prop is
 *    centred on its cell; a wall-side spot's is backed to its wall, facing
 *    away from it (`turnForSide`), its centre `WALL_GAP + depth / 2` metres
 *    off a bare wall and `SCREEN_GAP + depth / 2` off a screen. A prop is
 *    accepted when its `propFootprint` fits the floor, overlaps no lane, no
 *    taken box and no floor box placed before, and its cell has no floor
 *    prop yet. Every spot's draws come from `createRng` of its own seed,
 *    `seedFor(roomSeed, "prop", cx, cy, "floor")`: the kind when it is
 *    picked, then the variant, then the turn of a zone spot.
 * 6. Condition extras (ruling 12): `createRng(seedFor(roomSeed, "extras",
 *    condition))` draws each rule's count in `EXTRAS` order. Each floor
 *    extra goes to the first spot of step 5 in floor-seed order that
 *    accepts it, a wall-backed one (the ladder) only to a wall-side spot
 *    whose backing is `bare`. The loose cables' count is kept for step
 *    10.
 * 7. Corner zones: each zone takes `1` prop on a bridge, otherwise `1 +
 *    (seedFor(roomSeed, "prop-zone", key) % 2)`. Its remaining spots are
 *    walked in seed order, each drawing a kind weighted over the palette's
 *    floor picks that are not wall-backed (`zonePicks`),
 *    a variant and a turn, until that many are placed or the spots run
 *    out, so a cramped zone gets fewer.
 * 8. Wall-side cells: every remaining wall-side spot of step 5 draws a
 *    chance of the palette's `wallSide`, then a kind, then a variant, and
 *    is placed when accepted. The kind is weighted over all the palette's
 *    floor picks when the backing is `bare`, and over `zonePicks` in front
 *    of a screen, since a wall-backed kind 0.35 m off its wall would lean
 *    on air.
 * 9. Mid-hall clusters (D6, D7), in a large hall only: every block of
 *    `sites.clusterBlocks` draws from `createRng(seedFor(roomSeed, "prop",
 *    bx, by, "cluster-block"))`, keyed by its north-west cell, a chance of
 *    `CLUSTER_SHARE` and then a size from `CLUSTER_MIN` to `CLUSTER_MAX`.
 *    Its inner cells are walked in the order of their own seeds,
 *    `seedFor(roomSeed, "prop", cx, cy, "cluster")`, each drawing a kind
 *    weighted over the palette's `cluster` picks, a variant and a turn,
 *    centred on its cell, until that many are placed or the cells run out.
 *    A member is accepted as in step 5, and only when its box grown by
 *    `CLUSTER_CLEAR` (1.0 m) on every side overlaps no taken box and no
 *    floor prop outside its own cluster, so every gap between a cluster
 *    and anything solid is wide enough to walk through. Members of one
 *    cluster may stand side by side, never overlapping. The floor props
 *    the ring is held against are a snapshot of the floor boxes taken
 *    before the block's first member, so a cluster's own members are left
 *    out. Today that check never binds: the other clusters' inner cells,
 *    and the cells of the zone and wall-side props outside the band, all
 *    lie at least 2 cells away. It is a future-proofing guard, kept so that a
 *    later step or a smaller block cannot bring a floor prop within 1.0 m
 *    of a cluster unnoticed.
 * 10. Ceiling, anchored at wall points like a wall prop: the ceiling run
 *     when the palette has one, drawn as in step 3 from `"ceiling"`, token
 *     `ceiling-<side>`; under each ceiling tray segment a cable loop when
 *     `createRng` of the loop's own seed (token `loop-<side>`) draws
 *     `LOOP_SHARE`; then one span line (D9) when the palette has a
 *     `ceilingSpan` and the hall has clear `sites.spanLines` (a large hall
 *     only): `createRng(seedFor(roomSeed, "prop-runs", "span"))` draws a
 *     chance of `SPAN_SHARE` and then the variant every segment shares,
 *     and the line is the first of `sites.spanLines` in the order of
 *     `seedFor(roomSeed, "prop-span", axis, index)`. Each segment is
 *     anchored at its middle under the ceiling, `(x + 1, y + 0.5)` with
 *     turn 0 along a row and `(x + 0.5, y + 1)` with turn 1 along a column,
 *     seeded with token `span` at its first cell. A span line keeps a full
 *     cell (2 m) off every hall wall, clear of the ceiling band along the
 *     walls, and clear of lamps, decor and scaffolding (`sites.ts`); then
 *     the beacon on the entrance edge, token `beacon-s`; and the loose
 *     cables of step 6 on the hall's wall edges that carry neither a
 *     fixture nor a ceiling segment nor the beacon, in the order of their
 *     own seeds (token `loose-<side>`).
 * 11. The cap: `capProps(candidates, PROP_CAP)`. Readers, door and hatch
 *     signs, the step-2 extinguishers and the beacon are mandatory,
 *     everything else optional. A group, the span line, takes the place of
 *     its first member in this order and is kept whole or not at all;
 *     later candidates fill what it leaves.
 * 12. The output: `x` and `y` rounded to three decimals (done as each prop
 *     is made, so acceptance measures exactly the prop that is returned;
 *     a cluster member's cell centre needs no rounding), sorted by
 *     `PROP_ORDER`.
 *
 * Keep-clear and screening (D2 as amended: wall-side spots only, D3). A
 * wall prop a person reads or works by hand at standing height (reader,
 * sign, extinguisher, first-aid box, intercom, wall monitor) keeps a
 * wall-side spot on its cell free of floor props, whether the prop is
 * mandatory or an optional palette pick. A corner-zone spot ignores wall
 * props, keep-clear ones included: its prop stands centred in the corner
 * cell, well off the wall, and hides nothing a person uses. Any other wall
 * prop is a screen: a floor prop may stand in front of it, `SCREEN_GAP`
 * (0.35 m) off the wall, so the two layers never meet; ruling 5's height
 * bands keep them apart above the floor.
 *
 * Every seed is keyed by the integer anchor cell and a token (ruling 2),
 * never by a position in a list, and every draw comes from the rng of that
 * one prop's seed, so adding a fixture only changes the props near it.
 * That locality holds only below the cap: near `PROP_CAP` a new fixture can
 * change which props the cap drops anywhere in the room.
 *
 * This is the generator side: it imports `props.ts`, `sites.ts`,
 * `footprints.ts`, `types.ts`, `units.ts` and the seeds, and never
 * `move.ts`, `generate.ts`, `interact.ts` or anything under `render/`
 * (ruling 20). `dress.test.ts` keeps it so.
 */

import { createRng, seedFor, type Rng } from "../core/seed";
import { FOOTPRINTS, propFootprint } from "./footprints";
import {
  CLUSTER_CLEAR,
  CLUSTER_MAX,
  CLUSTER_MIN,
  CLUSTER_SHARE,
  EXTINGUISHER_EVERY,
  EXTRAS,
  FILLER,
  LOOP_SHARE,
  PALETTES,
  PROP_CAP,
  PROP_CATALOGUE,
  SPAN_CELLS,
  SPAN_SHARE,
  WALL_PROP_DEPTH,
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
  type SpanLine,
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
  /** Segments that the cap keeps or drops together: the one span line. */
  group?: string;
}

/** The order the cap keeps tiers in: walls last to go, ceilings first. */
const TIER: Record<PropAnchor, number> = { ceiling: 0, floor: 1, wall: 2 };

/**
 * Keeps at most `cap` props. Drops the lowest tier first (ceiling, then
 * floor, then wall); within a tier optional props before mandatory ones;
 * within those the highest seed first, with kind, x and y breaking a tie so
 * the result never depends on input order. A group (`Candidate.group`), the
 * span line, takes the place of its first member in this order and is kept
 * whole or not at all; later candidates fill what it leaves.
 */
export function capProps(
  candidates: readonly Candidate[],
  cap: number,
): Prop[] {
  if (candidates.length <= cap) return candidates.map((c) => c.prop);
  const order = [...candidates].sort(
    (a, b) =>
      TIER[b.prop.anchor] - TIER[a.prop.anchor] ||
      Number(b.mandatory) - Number(a.mandatory) ||
      a.prop.seed - b.prop.seed ||
      a.prop.kind.localeCompare(b.prop.kind) ||
      a.prop.x - b.prop.x ||
      a.prop.y - b.prop.y,
  );
  const kept: Prop[] = [];
  const decided = new Set<string>();
  for (const c of order) {
    if (kept.length >= cap) break;
    if (c.group === undefined) {
      kept.push(c.prop);
      continue;
    }
    if (decided.has(c.group)) continue;
    decided.add(c.group);
    const members = order.filter((m) => m.group === c.group);
    if (kept.length + members.length <= cap)
      kept.push(...members.map((m) => m.prop));
  }
  return kept;
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

/**
 * How far a wall-side floor prop stands off its wall when a screenable wall
 * prop hangs on that wall's edge (D3), in metres: the wall band's depth
 * (`WALL_PROP_DEPTH`, 0.3) plus `WALL_GAP`, so 0.35. The floor prop then
 * stands clear of the wall prop behind it; the deepest floor prop (1.4 m)
 * still ends 1.75 m off the wall, inside the 2 m cell.
 */
export const SCREEN_GAP = WALL_PROP_DEPTH + WALL_GAP;

/**
 * What stands behind a floor spot: a bare wall, a screenable wall prop, or
 * something that must stay clear. A corner-zone spot is always bare.
 */
type Backing = "bare" | "screen" | "clear";

/** The four sides of a cell, in the order a spot's backing checks them. */
const SIDES = ["n", "e", "s", "w"] as const;

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
 * mandatory or optional: steps 1 to 10 of the module doc. `reserved` joins
 * the taken boxes and keeps floor props out of what a later pass claimed.
 */
export function dressCandidates(
  room: RoomBase,
  reserved: readonly Box[] = [],
): Candidate[] {
  const sites = dressingSites(room);
  const palette = PALETTES[room.archetype];
  const used = new Set<string>();
  const wallKinds = new Map<string, PropKind>();
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
    wallKinds.set(edgeKey(e), kind);
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

  // Steps 3 and 10: a run along one or two long walls, one segment per edge.
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
    wallKinds.set(k, kind);
    out.push({
      prop: atWall(kind, e, variantOf(kind, rng), seed),
      mandatory: false,
    });
  }

  // Step 5: the floor spots, what stands behind each, and how a prop is
  // accepted on one.
  const backing = (s: FloorSpot): Backing => {
    if (s.wall === null) return "bare";
    for (const side of SIDES) {
      const kind = wallKinds.get(edgeKey({ x: s.cx, y: s.cy, side }));
      if (kind !== undefined && PROP_CATALOGUE[kind].keepClear) return "clear";
    }
    return wallKinds.has(edgeKey({ x: s.cx, y: s.cy, side: s.wall }))
      ? "screen"
      : "bare";
  };
  const wallSide = sites.wallSide.filter((s) => backing(s) !== "clear");
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
      const gap = backing(s) === "screen" ? SCREEN_GAP : WALL_GAP;
      const off = (gap + size.depth / 2) / CELL;
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
        if (!free(s)) continue;
        if (wallBacked && (s.wall === null || backing(s) !== "bare")) continue;
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
    const picks = backing(s) === "bare" ? palette.floor : zonePicks;
    if (picks.length === 0) continue;
    place(onFloor(weighted(rng, picks), s, rng), s);
  }

  // Step 9: mid-hall clusters (D6, D7), large halls only.
  const grow = (b: Box, m: number): Box => ({
    x0: b.x0 - m,
    x1: b.x1 + m,
    z0: b.z0 - m,
    z1: b.z1 + m,
  });
  for (const block of sites.clusterBlocks) {
    const rng = createRng(propSeed(block.x, block.y, "cluster-block"));
    if (!rng.chance(CLUSTER_SHARE)) continue;
    const size = rng.int(CLUSTER_MIN, CLUSTER_MAX);
    // Every floor prop not in this cluster: its members are placed after.
    // A future-proofing guard; today no floor prop outside the cluster comes
    // within the ring (see step 9 of the module doc).
    const outside = [...floorBoxes];
    const clusterSeed = (s: FloorSpot) => propSeed(s.cx, s.cy, "cluster");
    let placed = 0;
    for (const s of [...block.cells].sort(
      (a, b) => clusterSeed(a) - clusterSeed(b) || a.cy - b.cy || a.cx - b.cx,
    )) {
      if (placed >= size) break;
      const r = createRng(clusterSeed(s));
      const kind = weighted(r, palette.cluster);
      const variant = variantOf(kind, r);
      const prop: Prop = {
        kind,
        variant,
        anchor: "floor",
        x: s.cx + 0.5,
        y: s.cy + 0.5,
        turn: r.int(0, 3),
        seed: clusterSeed(s),
      };
      const box = propFootprint(prop);
      if (box === null) continue;
      const ring = grow(box, CLUSTER_CLEAR);
      if (blocked.some((t) => overlaps(ring, t))) continue;
      if (outside.some((o) => overlaps(ring, o))) continue;
      if (place(prop, s)) placed++;
    }
  }

  // Step 10: the ceiling.
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
  const span = palette.ceilingSpan;
  if (span !== null && sites.spanLines.length > 0) {
    const rng = createRng(seedFor(room.seed, "prop-runs", "span"));
    if (rng.chance(SPAN_SHARE)) {
      const variant = variantOf(span, rng);
      const order = (l: SpanLine) =>
        seedFor(room.seed, "prop-span", l.axis, l.index);
      const line = [...sites.spanLines].sort(
        (a, b) => order(a) - order(b) || a.index - b.index,
      )[0];
      if (line !== undefined)
        for (const c of line.segments) {
          const along = line.axis === "x";
          out.push({
            prop: {
              kind: span,
              variant,
              anchor: "ceiling",
              x: along ? c.x + SPAN_CELLS / 2 : c.x + 0.5,
              y: along ? c.y + 0.5 : c.y + SPAN_CELLS / 2,
              turn: along ? 0 : 1,
              seed: propSeed(c.x, c.y, "span"),
            },
            mandatory: false,
            group: "span",
          });
        }
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
