/**
 * A place becomes a room: the pure heart of the station.
 *
 * The generator takes the content of one engram and returns a `RoomSpec`,
 * with no GPU, no DOM and no clock in reach, so the same engram at the same
 * `GAME_VERSION` gives the same room every time it is entered and the tests
 * can pin it byte for byte.
 *
 * The layout is deliberately plain for milestone 1: a rectangle of cells
 * whose far (north) wall carries the doors and then the portals, whose west
 * wall carries the terminals and whose east wall carries the machines, with
 * the entrance and its placard on the south wall. Fixtures stand on every
 * second cell so they never touch. The room grows until each wall fits its
 * fixtures, up to `ROOM_CAP` cells a side; a fixture that still finds no
 * slot on its own wall takes the next free one going round the room, and
 * one that finds none at all is left out (annex bays, which make room for
 * those, are milestone 2).
 *
 * Every fixture's look comes from a seed of its own - a machine's from its
 * tag alone, so the same tag is the same machine in every room - and every
 * list is sorted before placement, so the order the API happened to return
 * things in never moves a door.
 */

import { isRetired } from "../../lifecycle";
import { createRng, seedFor } from "../core/seed";
import { GAME_VERSION } from "../version";
import { sectionsOf } from "./sections";
import type {
  Archetype,
  Condition,
  DoorStyle,
  Fixture,
  LightSpecial,
  LightZone,
  MachineKind,
  PlaceInput,
  PlaceReference,
  RoomSpec,
  Side,
  WallSlot,
} from "./types";

/** Metres per cell. */
export const CELL = 2;

/** The largest room, in cells a side. */
export const ROOM_CAP = 24;

/** The smallest room, so even an empty engram is a room to stand in. */
const MIN_WIDTH = 5;
const MIN_DEPTH = 6;

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

/**
 * The free slots of a room, handed out wall by wall. Each wall offers every
 * second cell; `take` serves a wall's own slots first and then any free slot
 * going round north, east, south, west.
 */
function slotPool(width: number, depth: number, reserved: WallSlot[]) {
  const walls: Record<Side, WallSlot[]> = { n: [], e: [], s: [], w: [] };
  for (let x = 1; x < width; x += 2) walls.n.push({ x, y: 0, side: "n" });
  for (let y = 1; y < depth; y += 2)
    walls.e.push({ x: width - 1, y, side: "e" });
  for (let x = width - 2; x >= 0; x -= 2)
    walls.s.push({ x, y: depth - 1, side: "s" });
  for (let y = depth - 2; y >= 0; y -= 2) walls.w.push({ x: 0, y, side: "w" });
  const used = new Set(reserved.map((s) => `${s.x},${s.y},${s.side}`));
  const free = (s: WallSlot) => !used.has(`${s.x},${s.y},${s.side}`);
  const order: Side[] = ["n", "e", "s", "w"];
  return {
    take(side: Side): WallSlot | null {
      const own = walls[side].find(free);
      const slot = own ?? order.flatMap((o) => walls[o]).find(free) ?? null;
      if (slot !== null) used.add(`${slot.x},${slot.y},${slot.side}`);
      return slot;
    },
  };
}

function lightsFor(
  roomSeed: number,
  width: number,
  depth: number,
  salience: number,
  condition: Condition,
): LightZone[] {
  const base = Math.min(255, Math.round(150 + salience * 10));
  const zones: LightZone[] = [];
  for (let y0 = 0; y0 < depth; y0 += LIGHT_BLOCK) {
    for (let x0 = 0; x0 < width; x0 += LIGHT_BLOCK) {
      // Keyed by the zone's corner, not a running count, so a room that
      // grows wider keeps the lights it already had.
      const seed = seedFor(roomSeed, "light", x0, y0);
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
      zones.push({
        x0,
        y0,
        x1: Math.min(width, x0 + LIGHT_BLOCK),
        y1: Math.min(depth, y0 + LIGHT_BLOCK),
        level,
        special,
        seed,
      });
    }
  }
  return zones;
}

/** The room a place becomes. */
export function generateRoom(place: PlaceInput): RoomSpec {
  const seed = seedFor(GAME_VERSION, place.domain, place.permalink);
  const salience = clamp(place.salience ?? 3, 0, 10);
  const condition = conditionFor(place.status);
  const sections = sectionsOf(place.content);
  const relations = place.relations
    .filter((r) => r.relType !== null)
    .slice()
    .sort(byTarget);
  const links = place.links.slice().sort(byTarget);
  const tags = [
    ...new Set(place.tags.map((t) => t.trim()).filter((t) => t !== "")),
  ].sort();

  const north = relations.length + links.length;
  const sides = Math.max(sections.length, tags.length);
  const width = clamp(2 * north + 1, MIN_WIDTH, ROOM_CAP);
  const depth = clamp(2 * sides + 2, MIN_DEPTH, ROOM_CAP);

  const spawnX = Math.floor(width / 2);
  const entrance: WallSlot = { x: spawnX, y: depth - 1, side: "s" };
  const placardSlot: WallSlot = { x: spawnX - 1, y: depth - 1, side: "s" };
  const pool = slotPool(width, depth, [entrance, placardSlot]);
  const fixtures: Fixture[] = [
    {
      kind: "placard",
      slot: placardSlot,
      lines: [
        place.title,
        `TYPE ${place.type ?? "-"}`,
        `STATUS ${place.status ?? "-"}`,
        `SALIENCE ${place.salience ?? "-"}`,
      ],
    },
  ];

  for (const r of relations) {
    const slot = pool.take("n");
    if (slot === null) continue;
    const relType = r.relType ?? "";
    fixtures.push({
      kind: "door",
      slot,
      style: doorStyleFor(r.targetSalience),
      relType,
      label: `${relType} ${r.targetTitle ?? r.target.target}`,
      target: r.target.target,
      seed: seedFor(seed, "door", targetKey(r)),
    });
  }
  for (const l of links) {
    const slot = pool.take("n");
    if (slot === null) continue;
    fixtures.push({
      kind: "portal",
      slot,
      label: l.resolved
        ? (l.targetTitle ?? l.target.target)
        : "?FILE NOT FOUND",
      target: l.target.target,
      crossDomain: l.target.domain !== null && l.target.domain !== place.domain,
      sealed: !l.resolved,
      seed: seedFor(seed, "portal", targetKey(l)),
    });
  }
  // A terminal's seed is its heading plus how many sections of the same
  // heading came before it, never its position, so a section inserted
  // above leaves the terminals below it as they were.
  const headingsSeen = new Map<string, number>();
  for (const s of sections) {
    const nth = headingsSeen.get(s.heading) ?? 0;
    headingsSeen.set(s.heading, nth + 1);
    const slot = pool.take("w");
    if (slot === null) continue;
    fixtures.push({
      kind: "terminal",
      slot,
      heading: s.heading,
      lines: s.lines,
      seed: seedFor(seed, "terminal", s.heading, nth),
    });
  }
  for (const tag of tags) {
    const slot = pool.take("e");
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

  return {
    version: GAME_VERSION,
    seed,
    domain: place.domain,
    permalink: place.permalink,
    title: place.title,
    archetype: archetypeFor(place.type),
    condition,
    width,
    depth,
    ceiling: Math.round((3 + salience * 0.2) * 100) / 100,
    spawn: { x: spawnX, y: depth - 1, yaw: 0 },
    fixtures,
    lights: lightsFor(seed, width, depth, salience, condition),
  };
}
