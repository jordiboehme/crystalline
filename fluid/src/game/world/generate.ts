/**
 * A place becomes a room: the pure heart of the station.
 *
 * The generator takes the content of one engram and returns a `RoomSpec`,
 * with no GPU, no DOM and no clock in reach, so the same engram at the same
 * `GAME_VERSION` gives the same room every time it is entered and the tests
 * can pin it byte for byte.
 *
 * The floor plan comes from `layout.ts`: a main hall sized by its doors,
 * terminals and machines, a backlink corridor west of it past eight hatches,
 * and overflow bays east of it for what the hall's walls cannot hold. The
 * fixtures are then placed in this order, each asking the slot pool for the
 * wall it belongs on:
 *
 * 1. the placard, at the entrance;
 * 2. doors for the outgoing relations, north;
 * 3. portals for the prose wikilinks, north;
 * 4. terminals for the `## ` sections, west;
 * 5. machines for the tags, east;
 * 6. hatches for the inbound references, south while there are eight or
 *    fewer, in the corridor beyond;
 * 7. posters for the observations, one per category, anywhere.
 *
 * A fixture whose wall is full takes any free slot of the hall and then of
 * the bays; one that finds none at all is left out and counted in `dropped`.
 *
 * Every fixture's look comes from a seed keyed by a name - a door's or
 * portal's target, a hatch's address, a poster's category, a terminal's
 * heading and occurrence, a machine's tag alone, so the same tag is the same
 * machine in every room - and every list is sorted before placement, so the
 * order the API happened to return things in never moves a door.
 *
 * The hall also gets its archetype's furniture (see `decorFor`), and the
 * grid is lit in blocks of four by four cells.
 *
 * 8. Last, the heroes and the set dressing. The heroes (`placeHeroes`,
 *    `heroes.ts`) stand after the scaffold and before the dressing, which
 *    keeps off what they reserve (`heroReserve`), skipping, by `near`
 *    (`nearFor`, 2.6f C5 to C8), what the rooms its ways lead to draw. Then `dressRoom`
 *    (`dress.ts`) reads the finished room, fixtures, furniture,
 *    scaffolding and heroes included, and adds its props. The dressing runs
 *    after everything else and only ever adds decoration, so a prop never
 *    moves a fixture, and the room states its entrance, bays and corridor
 *    so the dressing never has to work them out again.
 * 9. Last, the curios (`placeCurios`, `curios.ts`), after the dressing,
 *    since shelves and cabinets are props; they read everything and move
 *    nothing, skipping, by `near` (`nearFor`, 2.6f C5 to C8), what the
 *    rooms its ways lead to draw.
 */

import { isRetired } from "../../lifecycle";
import { createRng, seedFor } from "../core/seed";
import { GAME_VERSION } from "../version";
import {
  BAND_MARGIN,
  SOUTH_HATCHES,
  createSlotPool,
  isFloor,
  planLayout,
  type SlotPref,
} from "./layout";
import { dressRoom } from "./dress";
import { curioDraws, placeCurios } from "./curios";
import { decorFootprint } from "./footprints";
import { heroDraws, placeHeroes } from "./heroes";
import { nearOf, type Neighbour } from "./neighbours";
import {
  dressingSites,
  type CurioBase,
  type Near,
  type RoomBase,
} from "./sites";
import { sectionsOf } from "./sections";
import {
  HATCH_CAP,
  type Archetype,
  type Box,
  type Condition,
  type Decor,
  type DecorKind,
  type DoorStyle,
  type Fixture,
  type LightSpecial,
  type LightZone,
  type MachineKind,
  type PlaceAddress,
  type PlaceInbound,
  type PlaceInput,
  type PlaceReference,
  type Rect,
  type RoomSpec,
  type WallSlot,
} from "./types";
import { CELL } from "./units";

/** How many observations a poster shows. */
export const POSTER_LINES = 6;

/** The poster category of an observation that names none. */
export const NOTES = "NOTES";

/** What a sealed way says: the target was never resolved. */
export const NOT_FOUND = "?FILE NOT FOUND";

/** What a sealed way says: resolved, but the graph did not locate it. */
export const NO_ROUTE = "NO ROUTE";

/**
 * What a way says when its target refused the player: the answer to a 403
 * on travel. The generator never seals a way with it (a permission answer
 * comes from a load, not from the index); the session uses it for a way
 * that failed on travel, and `world/malfunction.ts` counts it as broken.
 */
export const ACCESS_DENIED = "ACCESS DENIED";

/** Cells a side of one light zone. */
const LIGHT_BLOCK = 4;

/** The machine catalogue, picked by a tag's hash. */
export const MACHINE_KINDS: readonly MachineKind[] = [
  "workbench",
  "lab-bench",
  "server-rack",
  "cryo-pod",
  "fabricator",
  "hydroponics",
  "nav-table",
  "comms-array",
  "reactor-coupling",
  "cargo-loader",
  "med-scanner",
  "containment",
];

const CONSTRUCTION = ["draft", "proposed", "idea", "poc"];

/**
 * The condition a status puts a room in. Retirement is Fluid's own rule
 * (`lifecycle.ts`), so the game and the reading screen agree on it; among
 * the retired statuses, archived and legacy are the derelict ones.
 */
export function conditionFor(status: string | null): Condition {
  const s = (status ?? "").toLowerCase();
  if (isRetired(s))
    return s === "archived" || s === "legacy" ? "derelict" : "dim";
  if (CONSTRUCTION.includes(s)) return "construction";
  return "clean";
}

const ARCHETYPES: Record<string, Archetype> = {
  manifest: "bridge",
  decision: "council",
  runbook: "engineering",
  reference: "archive",
  guide: "lab",
};

const HASHED_ARCHETYPES: readonly Archetype[] = [
  "council",
  "engineering",
  "archive",
  "lab",
];

/** The archetype of a type; an unknown type gets one by its hash. */
export function archetypeFor(type: string | null): Archetype {
  const t = (type ?? "").toLowerCase();
  return (
    ARCHETYPES[t] ?? createRng(seedFor("archetype", t)).pick(HASHED_ARCHETYPES)
  );
}

/** The door a target's salience earns: 0-3 sliding, 4-6 bulkhead, 7-10 blast. */
export function doorStyleFor(salience: number | null): DoorStyle {
  if (salience === null || salience < 4) return "sliding";
  return salience < 7 ? "bulkhead" : "blast";
}

function clamp(v: number, lo: number, hi: number) {
  return Math.max(lo, Math.min(hi, v));
}

function targetKey(ref: PlaceReference) {
  return `${ref.target.domain ?? ""}\u0000${ref.target.target}\u0000${ref.relType ?? ""}`;
}

function byTarget(a: PlaceReference, b: PlaceReference) {
  const ka = targetKey(a);
  const kb = targetKey(b);
  return ka < kb ? -1 : ka > kb ? 1 : 0;
}

function addressKey(a: PlaceAddress) {
  return `${a.domain}\u0000${a.permalink}`;
}

function byAddress(a: PlaceInbound, b: PlaceInbound) {
  const ka = addressKey(a.address);
  const kb = addressKey(b.address);
  return ka < kb ? -1 : ka > kb ? 1 : 0;
}

/**
 * Where a reference leads and, when it leads nowhere, why: an unresolved
 * target is not found, a resolved one the graph did not locate has no route.
 * The address is kept only for a way that is open, so `sealedLabel` is null
 * exactly when there is an address.
 */
function wayOf(ref: PlaceReference): {
  address: PlaceAddress | null;
  sealedLabel: string | null;
} {
  if (!ref.resolved) return { address: null, sealedLabel: NOT_FOUND };
  if (ref.address === null) return { address: null, sealedLabel: NO_ROUTE };
  return { address: ref.address, sealedLabel: null };
}

/**
 * The observations as posters: one per category in order of first
 * appearance, with up to `POSTER_LINES` of its observations. An observation
 * without a category goes on the `NOTES` poster.
 */
function postersOf(place: PlaceInput): { category: string; lines: string[] }[] {
  const posters = new Map<string, string[]>();
  for (const o of place.observations) {
    const trimmed = o.category?.trim() ?? "";
    const category = trimmed === "" ? NOTES : trimmed;
    const lines = posters.get(category) ?? [];
    if (lines.length < POSTER_LINES) lines.push(o.content);
    posters.set(category, lines);
  }
  return [...posters].map(([category, lines]) => ({ category, lines }));
}

/** The placard: title, type, status, salience, validity and inbound overflow. */
function placardLines(place: PlaceInput, inboundMore: number): string[] {
  const lines = [
    place.title,
    `TYPE ${place.type ?? "-"}`,
    `STATUS ${place.status ?? "-"}`,
    `SALIENCE ${place.salience ?? "-"}`,
  ];
  if (place.validFrom !== null || place.validTo !== null) {
    // An open end is left blank rather than written as a date: absent means
    // always valid before, or valid forever after.
    const range = [place.validFrom, "-", place.validTo].filter(
      (part) => part !== null,
    );
    lines.push(`VALID ${range.join(" ")}`);
  }
  if (inboundMore > 0) lines.push(`+${inboundMore} MORE INBOUND`);
  return lines;
}

/** Rounds a decor coordinate, so the golden is the same on every engine. */
function round3(v: number) {
  return Math.round(v * 1000) / 1000;
}

/**
 * The archetype's furniture, placed in the hall's interior band: the hall
 * without a two-cell margin on every side, which keeps the entrance lane
 * and the doors on the north wall clear and every piece away from the wall
 * fixtures. A hall whose band is smaller than three by three cells gets
 * none. Positions follow `Decor`'s convention (continuous cell units, turn 0
 * facing north with the first footprint dimension along `x`).
 *
 * - bridge: the command console across the hall's north third, and the
 *   captain's chair behind it (south), both facing north;
 * - council: a round table at the centre and six chairs round it, each
 *   turned to face the table as near as a quarter turn allows;
 * - engineering: a generator at the centre and two pipe runs on the ceiling
 *   either side of it, along the hall's longer axis;
 * - archive: rows of shelves two cells long across the width, every third
 *   row, leaving a two-cell aisle up the centre that the entrance opens on;
 * - lab: one island at the centre and a specimen tank in each north corner
 *   of the band.
 *
 * A piece's seed is keyed by its kind and its index among pieces of that
 * kind. That is safe here, unlike for fixtures, because how many pieces
 * there are depends only on the hall's size, never on the content.
 */
function decorFor(archetype: Archetype, hall: Rect, roomSeed: number): Decor[] {
  const band = {
    x0: hall.x0 + BAND_MARGIN,
    x1: hall.x1 - BAND_MARGIN,
    y0: hall.y0 + BAND_MARGIN,
    y1: hall.y1 - BAND_MARGIN,
  };
  if (band.x1 - band.x0 < 3 || band.y1 - band.y0 < 3) return [];
  const cx = (hall.x0 + hall.x1) / 2;
  const cy = (band.y0 + band.y1) / 2;
  const out: Decor[] = [];
  const counts = new Map<DecorKind, number>();
  const put = (kind: DecorKind, x: number, y: number, turn: number) => {
    const n = counts.get(kind) ?? 0;
    counts.set(kind, n + 1);
    out.push({
      kind,
      x: round3(x),
      y: round3(y),
      turn,
      seed: seedFor(roomSeed, "decor", kind, n),
    });
  };
  switch (archetype) {
    case "bridge": {
      const y = clamp((hall.y1 - hall.y0) / 3, band.y0 + 0.5, band.y1 - 1.75);
      put("command-console", cx, y, 0);
      put("captain-chair", cx, y + 1.25, 0);
      break;
    }
    case "council": {
      put("round-table", cx, cy, 0);
      for (let k = 0; k < 6; k++) {
        const angle = (k * Math.PI) / 3;
        // Facing the table is facing back along the angle it sits at.
        const turn = Math.round((angle + Math.PI) / (Math.PI / 2)) % 4;
        put(
          "council-chair",
          cx + 1.25 * Math.sin(angle),
          cy - 1.25 * Math.cos(angle),
          turn,
        );
      }
      break;
    }
    case "engineering": {
      put("generator", cx, cy, 0);
      const alongX = hall.x1 - hall.x0 >= hall.y1 - hall.y0;
      for (const side of [-1.25, 1.25]) {
        if (alongX) put("pipe-run", cx, cy + side, 0);
        else put("pipe-run", cx + side, cy, 1);
      }
      break;
    }
    case "archive": {
      const aisle = Math.floor(cx - 1);
      for (let y = band.y0 + 0.5; y + 0.5 <= band.y1; y += 3) {
        for (let x1 = aisle; x1 - 2 >= band.x0; x1 -= 2) {
          put("shelf-row", x1 - 1, y, 0);
        }
        for (let x0 = aisle + 2; x0 + 2 <= band.x1; x0 += 2) {
          put("shelf-row", x0 + 1, y, 0);
        }
      }
      break;
    }
    case "lab": {
      put("lab-island", cx, cy, 0);
      put("specimen-tank", band.x0 + 0.5, band.y0 + 0.5, 0);
      put("specimen-tank", band.x1 - 0.5, band.y0 + 0.5, 0);
      break;
    }
  }
  return out;
}

/**
 * The light zones of a grid: one per block of four by four cells that holds
 * any floor, none over void alone. The blocks are aligned to the hall's
 * north-west corner (a block west of it may be cut short at the grid's
 * edge), and a zone's seed is keyed by its corner relative to that corner,
 * not by a running count or a grid position. So a room that grows keeps the
 * lights it already had, and one whose hatches move into a corridor, which
 * shifts the hall east in the grid, keeps the hall's lights too.
 */
function lightsFor(
  roomSeed: number,
  grid: readonly string[],
  hall: Rect,
  width: number,
  depth: number,
  salience: number,
  condition: Condition,
): LightZone[] {
  const base = Math.min(255, Math.round(150 + salience * 10));
  const zones: LightZone[] = [];
  const startX = hall.x0 - LIGHT_BLOCK * Math.ceil(hall.x0 / LIGHT_BLOCK);
  const startY = hall.y0 - LIGHT_BLOCK * Math.ceil(hall.y0 / LIGHT_BLOCK);
  for (let by = startY; by < depth; by += LIGHT_BLOCK) {
    for (let bx = startX; bx < width; bx += LIGHT_BLOCK) {
      const x0 = Math.max(0, bx);
      const y0 = Math.max(0, by);
      const x1 = Math.min(width, bx + LIGHT_BLOCK);
      const y1 = Math.min(depth, by + LIGHT_BLOCK);
      let floor = false;
      for (let y = y0; y < y1 && !floor; y++)
        for (let x = x0; x < x1 && !floor; x++) floor = isFloor(grid, x, y);
      if (!floor) continue;
      const seed = seedFor(roomSeed, "light", bx - hall.x0, by - hall.y0);
      const rng = createRng(seed);
      let level = base;
      let special: LightSpecial = "steady";
      if (condition === "construction") {
        level = Math.round(base * 0.85);
        if (rng.chance(1 / 3)) special = "flicker";
      } else if (condition === "dim") {
        level = Math.round(base * 0.6);
        special = rng.chance(0.5) ? "flicker" : "glow";
      } else if (condition === "derelict") {
        level = Math.round(base * 0.35);
        const roll = rng.next();
        special = roll < 0.4 ? "failing" : roll < 0.7 ? "strobe" : "steady";
      }
      zones.push({ x0, y0, x1, y1, level, special, seed });
    }
  }
  return zones;
}

/** How many scaffold frames a room under construction gets. */
const SCAFFOLD_FRAMES = 2;
/** A scaffold frame's side, in metres. */
export const SCAFFOLD_SIZE = 1.4;

/**
 * The scaffold frames of a room under construction, and none for any other
 * condition: two frames, `SCAFFOLD_SIZE` metres square, placed from an rng
 * seeded with the room's seed. Each frame's centre is drawn x then z inside
 * the hall's interior band (the hall without a two-cell margin, the band the
 * decor stands in), inset by half a frame so the whole frame stays inside
 * it. The smallest hall's band is one by two cells, still wide enough for a
 * frame.
 *
 * A frame that would overlap a piece of the decor (`decorFootprint`) or the
 * frame kept before it is skipped rather than moved, so a room under
 * construction shows two frames, one or none. A skipped frame still uses its
 * two draws, so whether the first frame is kept never moves the second.
 *
 * `generateRoom` stores the result as `RoomSpec.scaffold`, and the canned
 * rooms call it the same way, so the walking code's blockers and the room
 * mesh's poles are read from exactly these boxes: what the player sees is
 * what blocks the player.
 */
export function scaffoldFor(
  condition: Condition,
  hall: Rect,
  decor: readonly Decor[],
  roomSeed: number,
): Box[] {
  if (condition !== "construction") return [];
  const half = SCAFFOLD_SIZE / 2;
  const x0 = (hall.x0 + BAND_MARGIN) * CELL + half;
  const x1 = (hall.x1 - BAND_MARGIN) * CELL - half;
  const z0 = (hall.y0 + BAND_MARGIN) * CELL + half;
  const z1 = (hall.y1 - BAND_MARGIN) * CELL - half;
  const taken: Box[] = [];
  for (const d of decor) {
    const box = decorFootprint(d);
    if (box !== null) taken.push(box);
  }
  const rng = createRng(roomSeed);
  const out: Box[] = [];
  for (let i = 0; i < SCAFFOLD_FRAMES; i++) {
    const x = rng.range(x0, x1);
    const z = rng.range(z0, z1);
    const frame = { x0: x - half, x1: x + half, z0: z - half, z1: z + half };
    if (taken.some((b) => intersects(frame, b))) continue;
    taken.push(frame);
    out.push(frame);
  }
  return out;
}

/** True when two boxes share floor; touching edges do not count. */
function intersects(a: Box, b: Box) {
  return a.x0 < b.x1 && b.x0 < a.x1 && a.z0 < b.z1 && b.z0 < a.z1;
}

/** A room's seed: the generator's version, its domain and its permalink. What a neighbour's draws start from too (2.6f C5). */
export function roomSeed(domain: string, permalink: string): number {
  return seedFor(GAME_VERSION, domain, permalink);
}

/**
 * The rooms a place's ways lead to (2.6f C5): every relation with a
 * relation type and an address, every link with an address and every
 * hatch, each address once, never the place's own, sorted by address. A
 * neighbour's archetype comes from its type when the way carries one
 * (`targetType`, a hatch's `type`), and is null when unknown; a type
 * learnt from any way beats an unknown one.
 */
export function neighboursOf(place: PlaceInput): Neighbour[] {
  const self = addressKey({ domain: place.domain, permalink: place.permalink });
  const seen = new Map<string, Neighbour>();
  const add = (a: PlaceAddress | null, type: string | null | undefined) => {
    if (a === null) return;
    const key = addressKey(a);
    if (key === self) return;
    const archetype = type === undefined ? null : archetypeFor(type);
    const known = seen.get(key);
    if (known === undefined || (known.archetype === null && archetype !== null))
      seen.set(key, { seed: roomSeed(a.domain, a.permalink), archetype });
  };
  for (const r of place.relations)
    if (r.relType !== null) add(r.address, r.targetType);
  for (const l of place.links) add(l.address, l.targetType);
  for (const h of place.inbound) add(h.address, h.type);
  return [...seen.entries()]
    .sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0))
    .map(([, n]) => n);
}

/** What a place's neighbours draw raw (2.6f C6): `nearOf` its seed and `neighboursOf` it. */
export function nearFor(place: PlaceInput): Near {
  return nearOf(roomSeed(place.domain, place.permalink), neighboursOf(place));
}

/** The room a place becomes. See the module doc for the rules. */
export function generateRoom(place: PlaceInput): RoomSpec {
  const seed = roomSeed(place.domain, place.permalink);
  const salience = clamp(place.salience ?? 3, 0, 10);
  const condition = conditionFor(place.status);
  const archetype = archetypeFor(place.type);
  const sections = sectionsOf(place.content);
  const relations = place.relations
    .filter((r) => r.relType !== null)
    .slice()
    .sort(byTarget);
  const links = place.links.slice().sort(byTarget);
  const tags = [
    ...new Set(place.tags.map((t) => t.trim()).filter((t) => t !== "")),
  ].sort();
  const inbound = place.inbound.slice().sort(byAddress).slice(0, HATCH_CAP);
  const inboundMore = Math.max(0, place.inboundTotal - inbound.length);
  const posters = postersOf(place);
  const hatchesInCorridor = inbound.length > SOUTH_HATCHES;

  const layout = planLayout({
    north: relations.length + links.length,
    west: sections.length,
    east: tags.length,
    south: hatchesInCorridor ? 0 : inbound.length,
    any: posters.length,
    hatches: inbound.length,
  });
  const pool = createSlotPool(layout);
  const decor = decorFor(archetype, layout.hall, seed);
  let dropped = 0;
  const take = (pref: SlotPref): WallSlot | null => {
    const slot = pool.take(pref);
    if (slot === null) dropped++;
    return slot;
  };

  const fixtures: Fixture[] = [
    {
      kind: "placard",
      slot: layout.placard,
      lines: placardLines(place, inboundMore),
    },
  ];

  for (const r of relations) {
    const slot = take("north");
    if (slot === null) continue;
    const relType = r.relType ?? "";
    fixtures.push({
      kind: "door",
      slot,
      style: doorStyleFor(r.targetSalience),
      relType,
      label: `${relType} ${r.targetTitle ?? r.target.target}`,
      ...wayOf(r),
      seed: seedFor(seed, "door", targetKey(r)),
    });
  }
  for (const l of links) {
    const slot = take("north");
    if (slot === null) continue;
    const way = wayOf(l);
    fixtures.push({
      kind: "portal",
      slot,
      label: l.targetTitle ?? l.target.target,
      address: way.address,
      crossDomain: l.target.domain !== null && l.target.domain !== place.domain,
      sealedLabel: way.sealedLabel,
      seed: seedFor(seed, "portal", targetKey(l)),
    });
  }
  // A terminal's seed is its heading plus how many sections of the same
  // heading came before it, never its position, so a section inserted
  // above leaves the terminals below it as they were. The same occurrence
  // count is what the CRT reader finds the section by.
  const headingsSeen = new Map<string, number>();
  for (const s of sections) {
    const nth = headingsSeen.get(s.heading) ?? 0;
    headingsSeen.set(s.heading, nth + 1);
    const slot = take("west");
    if (slot === null) continue;
    fixtures.push({
      kind: "terminal",
      slot,
      heading: s.heading,
      lines: s.lines,
      section: nth,
      seed: seedFor(seed, "terminal", s.heading, nth),
    });
  }
  for (const tag of tags) {
    const slot = take("east");
    if (slot === null) continue;
    fixtures.push({
      kind: "machine",
      slot,
      machine: createRng(seedFor("tag-kind", tag)).pick(MACHINE_KINDS),
      tag,
      hue: seedFor("tag-hue", tag) % 360,
      seed: seedFor(seed, "tag", tag),
    });
  }
  for (const h of inbound) {
    const slot = take(hatchesInCorridor ? "corridor" : "south");
    if (slot === null) continue;
    fixtures.push({
      kind: "hatch",
      slot,
      label: `${h.title} ${h.relType}`,
      address: h.address,
      seed: seedFor(seed, "hatch", h.address.domain, h.address.permalink),
    });
  }
  for (const p of posters) {
    const slot = take("any");
    if (slot === null) continue;
    fixtures.push({
      kind: "poster",
      slot,
      category: p.category,
      lines: p.lines,
      seed: seedFor(seed, "poster", p.category),
    });
  }

  const base: RoomBase = {
    version: GAME_VERSION,
    seed,
    domain: place.domain,
    permalink: place.permalink,
    title: place.title,
    archetype,
    condition,
    width: layout.width,
    depth: layout.depth,
    grid: layout.grid,
    hall: layout.hall,
    bays: layout.bays,
    corridor: layout.corridor,
    entrance: { x: layout.entrance.x, y: layout.entrance.y },
    ceiling: Math.round((3 + salience * 0.2) * 100) / 100,
    spawn: { x: layout.entrance.x, y: layout.entrance.y, yaw: 0 },
    fixtures,
    decor,
    scaffold: scaffoldFor(condition, layout.hall, decor, seed),
    heroes: [],
    lights: lightsFor(
      seed,
      layout.grid,
      layout.hall,
      layout.width,
      layout.depth,
      salience,
      condition,
    ),
    dropped,
    inboundMore,
  };
  // The heroes stand before the dressing, which keeps off what they
  // reserve. They replace the empty list in place, so the keys (and the
  // goldens) read fixtures, decor, scaffold, heroes, props, curios, lights.
  const near = nearFor(place);
  const room: RoomBase = {
    ...base,
    heroes: placeHeroes(base, heroDraws(base), dressingSites(base), near),
  };
  // The curios come last, on the dressed room, since shelves and filing
  // cabinets are props; they read everything and move nothing.
  const dressed: CurioBase = { ...room, props: dressRoom(room) };
  const { lights, dropped: left, inboundMore: more, ...head } = room;
  return {
    ...head,
    props: dressed.props,
    curios: placeCurios(dressed, curioDraws(dressed), near),
    lights,
    dropped: left,
    inboundMore: more,
  };
}
