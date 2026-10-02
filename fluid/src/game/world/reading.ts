/**
 * What the CRT reader shows for a thing the player reads with Space.
 *
 * Every wall information element and every computer opens the one reader
 * with a `Reading`: a title, markdown and the `##` section to open at.
 * Everything here is pure: a room, a fixture index and the room's place go
 * in, a reading or null comes out. The content is the markdown as written;
 * the reader applies its own inline rules when it draws it.
 *
 * - A terminal reads the place's markdown at its own section.
 * - A machine reads what the room's computers read (`roomReading`): the
 *   place's engram from the top, or a deck's or hangar's listing built from
 *   its own doors (0.22 R15); nothing in the airlock or a room with no
 *   place.
 * - A poster reads every observation of its category, not only the ones on
 *   the wall (0.22 R17); an observation with no category is under `NOTES`,
 *   the poster's own rule.
 * - The placard reads its lines, one paragraph each, under the room's
 *   title.
 * - A screen reads its first line as the title and its other lines as
 *   paragraphs, a line in its `keys` reading `<name> (private)`. The
 *   airlock's directory (`large`) reads every stop of the room's lift
 *   instead, and the lift's note after them, since its `+N MORE` line cuts
 *   the list (0.22 R18).
 *
 * Of the wall elements only fixtures are read (0.22 R12): a prop's painted
 * mark is decoration, and a way's label is already its whole text. With no
 * place, a terminal and a poster read nothing.
 *
 * The computers among the furniture, props, heroes and curios (the kinds in
 * `COMPUTER_DECOR`, `COMPUTER_PROPS`, `COMPUTER_HEROES` and
 * `COMPUTER_CURIOS`, 0.22 R14) read what a machine reads (`roomReading`).
 * Each has one use point (`computersOf`), worked out once per room: a
 * hero's front face, a wall prop's wall point, the centre of anything that
 * stands free. `computerFocus` picks the one the player uses by
 * `focusOf`'s rule, and a thing on a wall is used only from its front
 * (0.22 R16). A room whose computers would read nothing has no use points
 * at all (0.22 R15).
 */

import { FOOTPRINTS, HERO_FRONT, heroFootprint, heroTurn } from "./footprints";
import { posterCategory } from "./generate";
import { HERO_CATALOGUE } from "./heroes";
import { FACING, REACH, relative, type WallPoint } from "./interact";
import type { Player } from "./move";
import type {
  CurioKind,
  DecorKind,
  Fixture,
  Hero,
  HeroKind,
  PlaceInput,
  PropKind,
  RoomSpec,
} from "./types";
import { CELL } from "./units";

/** What the reader shows: a title, markdown, and the `##` section to open at (null: the top). */
export interface Reading {
  title: string;
  content: string;
  section: { heading: string; occurrence: number } | null;
}

/** The suffix a private line reads with. */
const PRIVATE = "(private)";

/** `line` as read: with the private suffix when `key` says so. */
function marked(line: string, key: boolean): string {
  return key ? `${line} ${PRIVATE}` : line;
}

/** Lines as markdown paragraphs, one each. */
function paragraphs(lines: readonly string[]): string {
  return lines.join("\n\n");
}

/**
 * A deck's or hangar's listing: the deck's label as the title, one list
 * line per door in fixture order (the engram each leads to), then the
 * lift's note as its own paragraph when it has one.
 */
function deckListing(room: RoomSpec): Reading {
  const items: string[] = [];
  let note: string | null = null;
  for (const f of room.fixtures) {
    if (f.kind === "door") items.push(`- ${f.label}`);
    else if (f.kind === "lift" && f.note !== null) note = f.note;
  }
  const parts = [items.join("\n")];
  if (note !== null) parts.push(note);
  return {
    title: room.title,
    content: parts.filter((p) => p !== "").join("\n\n"),
    section: null,
  };
}

/** What the room's computers read (spec 3a): the place's engram from the top; a deck's or hangar's listing; null elsewhere. 0.22 R15. */
export function roomReading(
  room: RoomSpec,
  place: PlaceInput | null,
): Reading | null {
  if (room.space === "deck" || room.space === "hangar") {
    return deckListing(room);
  }
  if (room.space === "airlock" || place === null) return null;
  return { title: place.title, content: place.content, section: null };
}

/** A screen's reading; see the module doc. */
function screenReading(
  room: RoomSpec,
  screen: Extract<Fixture, { kind: "screen" }>,
): Reading {
  const keys = new Set(screen.keys);
  const title = marked(screen.lines[0] ?? "", keys.has(0));
  if (screen.large === true) {
    const lift = room.fixtures.find((f) => f.kind === "lift");
    if (lift !== undefined) {
      const lines = lift.stops.map((s) => marked(s.label, s.key));
      if (lift.note !== null) lines.push(lift.note);
      return { title, content: paragraphs(lines), section: null };
    }
  }
  const lines = screen.lines.slice(1).map((l, i) => marked(l, keys.has(i + 1)));
  return { title, content: paragraphs(lines), section: null };
}

/** What the fixture at `index` reads (spec 3b table), or null when it reads nothing. 0.22 R12, R17, R18. */
export function fixtureReading(
  room: RoomSpec,
  index: number,
  place: PlaceInput | null,
): Reading | null {
  const fixture = room.fixtures[index];
  if (fixture === undefined) return null;
  switch (fixture.kind) {
    case "terminal":
      if (place === null) return null;
      return {
        title: place.title,
        content: place.content,
        section: { heading: fixture.heading, occurrence: fixture.section },
      };
    case "machine":
      return roomReading(room, place);
    case "poster": {
      if (place === null) return null;
      const items = place.observations
        .filter((o) => posterCategory(o.category) === fixture.category)
        .map((o) => `- ${o.content}`);
      return {
        title: place.title,
        content: [`## ${fixture.category}`, items.join("\n")]
          .filter((p) => p !== "")
          .join("\n\n"),
        section: null,
      };
    }
    case "placard":
      return {
        title: room.title,
        content: paragraphs(fixture.lines),
        section: null,
      };
    case "screen":
      return screenReading(room, fixture);
    case "door":
    case "portal":
    case "hatch":
    case "lift":
    case "exit":
      return null;
  }
}

/** The computer kinds of the furniture (spec 3a table, 0.22 R14). */
export const COMPUTER_DECOR: ReadonlySet<DecorKind> = new Set<DecorKind>([
  "command-console",
]);

/** The computer kinds of the props (spec 3a table, 0.22 R14). */
export const COMPUTER_PROPS: ReadonlySet<PropKind> = new Set<PropKind>([
  "wall-monitor",
  "designer-tower",
  "gravity-console",
]);

/** The computer kinds of the heroes (spec 3a table, 0.22 R14). */
export const COMPUTER_HEROES: ReadonlySet<HeroKind> = new Set<HeroKind>([
  "core-wall",
  "photo-console",
  "laser-desk",
]);

/** The computer kinds of the curios (spec 3a table, 0.22 R14). */
export const COMPUTER_CURIOS: ReadonlySet<CurioKind> = new Set<CurioKind>([
  "beige-laptop",
  "breadbin-computer",
  "slim-computer",
]);

/**
 * One computer's use point, in metres: its front (`inward` points out of it
 * into the room); `wall` when it stands on a wall. `list` and `index` name
 * the thing in the room's list it stands in.
 */
export interface ComputerPoint {
  list: "decor" | "props" | "heroes" | "curios";
  index: number;
  point: WallPoint;
  wall: boolean;
}

/** A point at `(x, z)` metres facing the way quarter turn `turn` faces. */
function facingPoint(x: number, z: number, turn: number): WallPoint {
  const t = ((Math.round(turn) % 4) + 4) % 4;
  const [fx, fz] = HERO_FRONT[t] ?? [0, -1];
  return { x, z, inward: [fx, fz], along: [fz, -fx] };
}

/**
 * A hero's use point. A hero on a wall (`wall` or `backed`) is used from
 * its front only: the centre of its footprint moved half its depth along
 * its front, so the point sits on the face it runs out from its wall with.
 * A free one is used from any side (0.22 R16): the point is its footprint's
 * centre, which every side reaches. Null for a variant with no footprint.
 */
function heroPoint(h: Hero, wall: boolean): WallPoint | null {
  const size = FOOTPRINTS.hero[h.kind][h.variant];
  if (size === undefined) return null;
  const box = heroFootprint(h);
  const centre = facingPoint(
    (box.x0 + box.x1) / 2,
    (box.z0 + box.z1) / 2,
    heroTurn(h),
  );
  if (!wall) return centre;
  return {
    ...centre,
    x: centre.x + centre.inward[0] * (size.depth / 2),
    z: centre.z + centre.inward[1] * (size.depth / 2),
  };
}

/** The room's computers, or [] when `roomReading(room, place)` is null (0.22 R15). */
export function computersOf(
  room: RoomSpec,
  place: PlaceInput | null,
): ComputerPoint[] {
  if (roomReading(room, place) === null) return [];
  const points: ComputerPoint[] = [];
  for (const [index, d] of room.decor.entries()) {
    if (!COMPUTER_DECOR.has(d.kind)) continue;
    const point = facingPoint(d.x * CELL, d.y * CELL, d.turn);
    points.push({ list: "decor", index, point, wall: false });
  }
  for (const [index, p] of room.props.entries()) {
    if (!COMPUTER_PROPS.has(p.kind)) continue;
    const point = facingPoint(p.x * CELL, p.y * CELL, p.turn);
    points.push({ list: "props", index, point, wall: p.anchor === "wall" });
  }
  for (const [index, h] of room.heroes.entries()) {
    if (!COMPUTER_HEROES.has(h.kind)) continue;
    const placement = HERO_CATALOGUE[h.kind].placement;
    const wall = placement === "wall" || placement === "backed";
    const point = heroPoint(h, wall);
    if (point === null) continue;
    points.push({ list: "heroes", index, point, wall });
  }
  for (const [index, c] of room.curios.entries()) {
    if (!COMPUTER_CURIOS.has(c.kind)) continue;
    const point = facingPoint(c.x * CELL, c.y * CELL, c.turn);
    points.push({ list: "curios", index, point, wall: false });
  }
  return points;
}

/**
 * The computer the player faces and can use, or null: `focusOf`'s rule
 * over the points (the nearest within `REACH` and within `FACING` of the
 * view); a wall point also needs the player in front of it, so nothing is
 * used through a wall, while a free one is used from any side (0.22 R16).
 * The prompt is `SPACE READ <title>`, the room's title.
 */
export function computerFocus(
  points: readonly ComputerPoint[],
  player: Player,
  title: string,
): { point: ComputerPoint; prompt: string } | null {
  const fx = -Math.sin(player.yaw);
  const fz = -Math.cos(player.yaw);
  const cosFacing = Math.cos(FACING);
  let best: ComputerPoint | null = null;
  let bestDistance = Infinity;
  for (const c of points) {
    const w = c.point;
    if (c.wall && relative(w, player.x, player.z).depth <= 0) continue;
    const dx = w.x - player.x;
    const dz = w.z - player.z;
    const distance = Math.hypot(dx, dz);
    if (distance > REACH || distance >= bestDistance) continue;
    if (distance > 0 && (dx * fx + dz * fz) / distance < cosFacing) continue;
    best = c;
    bestDistance = distance;
  }
  return best === null ? null : { point: best, prompt: `SPACE READ ${title}` };
}
