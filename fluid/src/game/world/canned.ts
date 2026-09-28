/**
 * The test rooms: places made from engrams that do not exist anywhere but
 * here.
 *
 * `CANNED_BRIDGE` carries what the look demo needs to judge the looks on:
 * two `## ` sections (two terminals), two relations to targets of low and
 * high salience (a sliding door and a blast door), one wikilink into another
 * domain (a portal in the cross-domain colour), two tags (two machines), one
 * observation (a poster) and one inbound reference (a hatch). It is also the
 * golden room the generator is pinned against.
 *
 * `CANNED_HUB` is the other extreme: an engram three hundred others point at,
 * with forty tags and twenty relations, so its room needs a backlink
 * corridor, overflow bays and a placard that names the inbound references
 * past the hatches. Nothing here is fetched.
 *
 * `CANNED_WORKSHOP` is the middle ground the set dressing is judged on: an
 * engineering runbook under construction whose hall is big enough for pipe
 * runs, ducts, corner zones, wall-side floor props, scaffolding and the
 * construction extras, and whose north wall is all doors. It is the second
 * golden room.
 *
 * `galleryRoom` is no place at all but a room built by hand for the dev-only
 * model gallery: one of every model the station draws, machines in bays 1
 * and 2 and set dressing in bays 3 and 4, so each can be walked up to and
 * judged without an engram that happens to need it. It holds one curio on
 * the first host of every non-hero host kind it carries (`galleryCurios`),
 * so a curio can be judged sitting on a terminal, a machine, a piece of
 * decor and a floor prop alike; both under-desk curios sit in a row on the
 * workbench's lower shelf.
 *
 * `heroHallRoom` (H15) is the gallery's second hand-built room, for the
 * `?hall=heroes` dev route: one of every hero kind and variant, hand-placed
 * in a 23 by 24 hall with no fixture but the placard, one round table that
 * carries the 2.6d curios, and no props, so nothing competes with a hero
 * for a screenshot. It holds one of every curio kind and variant,
 * hand-placed on the hero and round-table surfaces they fit
 * (`heroHallCurios`, `row`), for the `?at=prop:<kind>:<n>` shots that frame
 * them close and tilted (`spotView` in `dev/spots.ts`). `row` lays each row
 * along its host's local `a` axis, the wall-parallel one, whatever its own
 * length against the surface's `d` axis, and the rows are grouped to fit
 * that budget.
 *
 * The gallery and the hero hall take a plain finish (`plainFinish`: accent
 * 0, wall pattern 0 everywhere); the variants hall shows every wall
 * pattern (`variantsHallRoom`). The gallery and the hero hall carry no
 * decals (`decals: []`); the variants hall carries one of every decal kind
 * and tile on its south wall and its floor, and both stencils
 * (`variantsHallDecals`).
 */

import { seedFor } from "../core/seed";
import { GAME_VERSION } from "../version";
import {
  CURIO_CATALOGUE,
  CURIO_GAP,
  CURIO_MARGIN,
  CURIO_ORDER,
  curioOn,
  hostSurfaces,
  type HostSurface,
} from "./curios";
import { bayNumber, deckNumber } from "./decals";
import { PROP_ORDER } from "./dress";
import { plainFinish } from "./finish";
import { FOOTPRINTS, PIPE_HALF } from "./footprints";
import { MACHINE_KINDS, NOT_FOUND, NO_ROUTE, scaffoldFor } from "./generate";
import { faceCentre, HERO_ORDER } from "./heroes";
import {
  createSlotPool,
  planLayout,
  wallRuns,
  wallSlots,
  type Layout,
  type SlotPref,
} from "./layout";
import { PROP_CATALOGUE, PROP_KINDS, USE_LANE_DEPTH } from "./props";
import { wallAnchor } from "./sites";
import type {
  Curio,
  CurioKind,
  Decal,
  DecalKind,
  Decor,
  DecorKind,
  Fixture,
  FloorPropKind,
  Hero,
  HeroKind,
  LightZone,
  PlaceAddress,
  PlaceInput,
  Prop,
  PropKind,
  Rect,
  RoomSpec,
  Side,
  SurfaceClass,
  WallSlot,
} from "./types";
import { CELL } from "./units";
import { VARIANT_COUNTS } from "./variants";

/**
 * The station's bridge as the look demo sees it: the place input
 * `generateRoom` turns into the one room milestone 1 renders. Its status is
 * `stable`, the healthy end of the condition scale; the demo's R key swaps it
 * for `archived` to show the derelict end without a second fixture.
 */
export const CANNED_BRIDGE: PlaceInput = {
  domain: "station",
  permalink: "manifest",
  title: "Station Crystalline",
  type: "manifest",
  status: "stable",
  salience: 7,
  validFrom: null,
  validTo: null,
  tags: ["navigation", "reactor"],
  content: [
    "---",
    "type: manifest",
    "status: stable",
    "---",
    "# Station Crystalline",
    "",
    "## Scope",
    "",
    "Everything the crew learned while keeping the station alive.",
    "Decisions, runbooks and the odd warning about the reactor.",
    "",
    "## Routing",
    "",
    "Engineering questions go to the reactor deck.",
    "Cargo questions go through the portal to logistics.",
  ].join("\n"),
  relations: [
    {
      relType: "supersedes",
      target: { domain: null, target: "old-bridge" },
      resolved: true,
      address: { domain: "station", permalink: "old-bridge" },
      targetTitle: "The Old Bridge",
      targetSalience: 2,
    },
    {
      relType: "depends_on",
      target: { domain: null, target: "reactor-core" },
      resolved: true,
      address: { domain: "station", permalink: "reactor-core" },
      targetTitle: "Reactor Core",
      targetSalience: 8,
    },
  ],
  links: [
    {
      relType: null,
      target: { domain: "logistics", target: "Cargo Manifest" },
      resolved: true,
      address: { domain: "logistics", permalink: "cargo-manifest" },
      targetTitle: "Cargo Manifest",
      targetSalience: 5,
    },
  ],
  inbound: [
    {
      address: { domain: "station", permalink: "crew-handbook" },
      title: "Crew Handbook",
      relType: "links_to",
    },
  ],
  inboundTotal: 1,
  observations: [
    {
      category: "warning",
      content: "The reactor runs hot for an hour after every jump.",
    },
  ],
};

/**
 * The workshop: a mid-size engineering runbook under construction, the room
 * the dressing pass is pinned against (the second golden, ruling 18).
 *
 * - **Hall.** Six relations to the located valves `valve-a` to `valve-f` fill
 *   the north wall with doors, every one open, so each gets a keycard reader
 *   and the extinguishers of the north run find every edge near their index
 *   taken; that makes the hall 13 cells wide. Three `## ` sections (west) and
 *   five tags (east) make it 12 deep, and no bay is needed. Its interior
 *   band is 9 by 8 cells, four cluster blocks (E2).
 * - **South wall.** Three inbound references, three hatches beside the
 *   entrance and the placard, each with its sign plate.
 * - **Posters.** Two observations in two categories.
 * - **Dressing.** `runbook` is the engineering archetype (pipe bundles on the
 *   walls, ducts on the ceiling, barrels, crates, trolleys, crate stacks
 *   and drum racks) and `draft` puts it under construction: two scaffold
 *   frames and the construction extras (traffic cones, a ladder, a tool
 *   cart).
 * - **Permalink.** It seeds the room, and is chosen so its span stream
 *   draws a line in every archetype that hangs one (E2).
 */
export const CANNED_WORKSHOP: PlaceInput = {
  domain: "station",
  permalink: "pipe-shop",
  title: "Pump Workshop",
  type: "runbook",
  status: "draft",
  salience: 5,
  validFrom: null,
  validTo: null,
  tags: ["pressure", "valves", "tools", "safety", "gauges"],
  content: [
    "# Pump Workshop",
    "",
    "## Isolate",
    "Close the valve upstream and bleed the line.",
    "## Replace",
    "Swap the seat, torque the bonnet to spec.",
    "## Test",
    "Open slowly and watch the gauge for a minute.",
  ].join("\n"),
  relations: ["a", "b", "c", "d", "e", "f"].map((k) => ({
    relType: "depends_on",
    target: { domain: null, target: `valve-${k}` },
    resolved: true,
    address: { domain: "station", permalink: `valve-${k}` },
    targetTitle: `Valve ${k.toUpperCase()}`,
    targetSalience: 4,
  })),
  links: [],
  inbound: ["boiler-room", "crew-handbook", "pump-deck"].map((permalink) => ({
    address: { domain: "station", permalink },
    title: permalink,
    relType: "links_to",
  })),
  inboundTotal: 3,
  observations: [
    { category: "warning", content: "Valve C sticks when it is cold." },
    { category: "decision", content: "Spare seats live in the east locker." },
  ],
};

const pad = (i: number) => String(i).padStart(2, "0");

/**
 * The hub: every part of the grid at once. Twenty relations (two of them
 * sealed, one never resolved and one resolved but not located) and two
 * wikilinks want the north wall, forty tags the east wall and four sections
 * the west wall, far more than one 24 by 24 hall holds, so the room grows
 * bays. Twenty-four hatches of three hundred inbound references put the
 * hatches in a backlink corridor and `+276 MORE INBOUND` on the placard.
 * Observations in three categories give three posters.
 */
export const CANNED_HUB: PlaceInput = {
  domain: "station",
  permalink: "hub",
  title: "Central Hub",
  type: "reference",
  status: "stable",
  salience: 9,
  validFrom: "2026-01-01",
  validTo: null,
  tags: Array.from({ length: 40 }, (_, i) => `system-${pad(i)}`),
  content: [
    "# Central Hub",
    "",
    "## Decks",
    "Every deck of the station, one door each.",
    "## Crew",
    "Who keeps which deck running.",
    "## Supplies",
    "Where the stores are and who signs for them.",
    "## Drills",
    "What to do when the lights go red.",
  ].join("\n"),
  relations: Array.from({ length: 20 }, (_, i) => {
    const permalink = `deck-${pad(i)}`;
    const resolved = i !== 18;
    const located = resolved && i !== 19;
    return {
      relType: i % 2 === 0 ? "relates_to" : "depends_on",
      target: { domain: null, target: permalink },
      resolved,
      address: located ? { domain: "station", permalink } : null,
      targetTitle: located ? `Deck ${i}` : null,
      targetSalience: located ? i % 11 : null,
    };
  }),
  links: [
    {
      relType: null,
      target: { domain: "logistics", target: "Cargo Manifest" },
      resolved: true,
      address: { domain: "logistics", permalink: "cargo-manifest" },
      targetTitle: "Cargo Manifest",
      targetSalience: 5,
    },
    {
      relType: null,
      target: { domain: null, target: "Lost Deck" },
      resolved: false,
      address: null,
      targetTitle: null,
      targetSalience: null,
    },
  ],
  inbound: Array.from({ length: 24 }, (_, i) => ({
    address: { domain: "station", permalink: `log-${pad(i)}` },
    title: `Log ${i}`,
    relType: "links_to",
  })),
  inboundTotal: 300,
  observations: [
    { category: "decision", content: "Decks are numbered from the bridge." },
    { category: "warning", content: "Deck 13 has no lift." },
    { category: "decision", content: "Every deck keeps its own stores." },
    { category: null, content: "Paint the corridor blue next refit." },
  ],
};

/** The gallery's domain and permalink; no engram lives there. */
const GALLERY: PlaceAddress = { domain: "station", permalink: "gallery" };

/**
 * The need the gallery's floor plan is sized from. It is not what the room
 * carries: eight on the north wall make the hall seventeen cells wide, seven
 * on the west make it sixteen deep, which leaves a band wide enough for all
 * nine pieces of furniture with an aisle up the middle, and the inflated
 * `any` asks for exactly four bays: the first two hold the twelve machines,
 * the last two the one-of-everything set dressing.
 */
const GALLERY_NEED = {
  north: 8,
  west: 7,
  east: 0,
  south: 3,
  any: 55,
  hatches: 2,
};

/**
 * The furniture: every kind once, in two columns either side of the aisle
 * the entrance opens on, with the round table at the head of it. Each piece
 * stands where it keeps clear of the others and of the wall fixtures, and
 * the pipe run hangs from the ceiling beside the generator. Positions in
 * cell units, as `Decor` has them.
 */
const GALLERY_DECOR: readonly (readonly [DecorKind, number, number, number])[] =
  [
    ["command-console", 4.5, 4, 0],
    ["captain-chair", 4.5, 5.25, 0],
    ["round-table", 8.5, 4, 0],
    ["council-chair", 8.5, 5.5, 0],
    ["generator", 12.5, 4, 0],
    ["pipe-run", 12.5, 6, 0],
    ["shelf-row", 4.5, 8.5, 0],
    ["lab-island", 12.5, 8.5, 0],
    ["specimen-tank", 4.5, 11.5, 0],
  ];

/** The gallery's one light level, steady everywhere: a room for judging. */
const GALLERY_LIGHT = 210;

/**
 * The model gallery: a room built by hand rather than generated, holding one
 * of everything the station draws, for judging the models in the dev-only
 * route `/π/dev/gallery`.
 *
 * - **Hall.** Seventeen by sixteen cells, sized by `planLayout` from
 *   `GALLERY_NEED`. On the north wall a door of each style that opens
 *   (sliding, bulkhead, blast), a door sealed `NO ROUTE` and one sealed
 *   `?FILE NOT FOUND`, then a portal into the same domain, one into another
 *   domain and one sealed; two terminals on the west wall; two posters on
 *   the east wall; two hatches beside the entrance and the placard on the
 *   south wall; and every kind of furniture in the middle (`GALLERY_DECOR`).
 * - **Bays.** Four, east of the hall. Bays 1 and 2 hold the twelve machines,
 *   one of each kind in `MACHINE_KINDS` order, six to a bay, labelled with
 *   their kind. Bays 3 and 4 hold one of every prop kind and variant
 *   (`galleryProps`): wall and ceiling kinds on their walls, floor kinds on
 *   their floor, a run kind's variant as a 2-segment run, and a span kind's
 *   variant as a segment hung over row 6. The rare floor kinds stand in the
 *   hall instead, at `GALLERY_HALL_PROPS` (2.6d C19): the canisters, the
 *   console and the marked crates east of the specimen tank, the designer
 *   tower north of the west wall's northern terminal.
 * - **Light.** One steady zone per block of four by four cells, so every
 *   floor cell is lit and nothing flickers while a model is looked at.
 *
 * The doors and portals that open lead to addresses that do not exist; the
 * gallery's session has no client, so walking through one says
 * `SIGNAL LOST` and leaves the player in the gallery. Every field of
 * `RoomSpec` is set, and the same call gives the same room byte for byte.
 */
export function galleryRoom(): RoomSpec {
  const seed = seedFor(GAME_VERSION, "gallery");
  const condition = "clean";
  const layout = planLayout(GALLERY_NEED);
  const pool = createSlotPool(layout);
  const take = (pref: SlotPref): WallSlot => {
    const slot = pool.take(pref);
    if (slot === null) throw new Error(`gallery: no ${pref} slot left`);
    return slot;
  };
  const to = (permalink: string, domain = GALLERY.domain): PlaceAddress => ({
    domain,
    permalink,
  });

  const fixtures: Fixture[] = [
    {
      kind: "placard",
      slot: layout.placard,
      lines: [
        "Model Gallery",
        "EVERY MODEL THE",
        "STATION DRAWS",
        "MACHINES IN BAYS 1 2",
        "PROPS IN BAYS 3 4",
      ],
    },
  ];

  const doors = [
    { style: "sliding", relType: "relates_to", to: "sliding", sealed: null },
    { style: "bulkhead", relType: "depends_on", to: "bulkhead", sealed: null },
    { style: "blast", relType: "supersedes", to: "blast", sealed: null },
    { style: "bulkhead", relType: "relates_to", to: null, sealed: NO_ROUTE },
    { style: "sliding", relType: "relates_to", to: null, sealed: NOT_FOUND },
  ] as const;
  for (const d of doors) {
    const name = d.to ?? d.sealed;
    fixtures.push({
      kind: "door",
      slot: take("north"),
      style: d.style,
      relType: d.relType,
      label: `${d.relType} ${name}`,
      address: d.to === null ? null : to(d.to),
      sealedLabel: d.sealed,
      seed: seedFor(seed, "door", name),
    });
  }

  const portals = [
    { label: "Same Domain", address: to("portal"), crossDomain: false },
    {
      label: "Other Domain",
      address: to("portal", "logistics"),
      crossDomain: true,
    },
    { label: "Lost Portal", address: null, crossDomain: false },
  ] as const;
  for (const p of portals) {
    fixtures.push({
      kind: "portal",
      slot: take("north"),
      label: p.label,
      address: p.address,
      crossDomain: p.crossDomain,
      sealedLabel: p.address === null ? NOT_FOUND : null,
      seed: seedFor(seed, "portal", p.label),
    });
  }

  const terminals = [
    ["Scope", ["Every model the station draws.", "One of each."]],
    ["Keys", ["The arrows or WASD walk.", "Space uses, 1 2 4 pick the look."]],
  ] as const;
  for (const [heading, lines] of terminals) {
    fixtures.push({
      kind: "terminal",
      slot: take("west"),
      heading,
      lines: [...lines],
      section: 0,
      seed: seedFor(seed, "terminal", heading, 0),
    });
  }

  const posters = [
    ["decision", ["Judge a model in every look.", "Then in every condition."]],
    ["warning", ["The doors here lead nowhere."]],
  ] as const;
  for (const [category, lines] of posters) {
    fixtures.push({
      kind: "poster",
      slot: take("east"),
      category,
      lines: [...lines],
      seed: seedFor(seed, "poster", category),
    });
  }

  for (const [permalink, title] of [
    ["crew-handbook", "Crew Handbook"],
    ["reactor-log", "Reactor Log"],
  ] as const) {
    fixtures.push({
      kind: "hatch",
      slot: take("south"),
      label: `${title} links_to`,
      address: to(permalink),
      seed: seedFor(seed, "hatch", GALLERY.domain, permalink),
    });
  }

  // The machines stand in the first two bays' own slots, which the pool
  // would only hand out once the hall's were gone; the last two bays are
  // the set dressing's, below.
  const machineBays = layout.bays.slice(0, 2);
  const baySlots = wallSlots(layout.grid).filter((s) =>
    machineBays.some(
      (b) => s.x >= b.x0 && s.x < b.x1 && s.y >= b.y0 && s.y < b.y1,
    ),
  );
  const perBay = MACHINE_KINDS.length / machineBays.length;
  MACHINE_KINDS.forEach((machine, i) => {
    const bay = machineBays[Math.floor(i / perBay)];
    const own = baySlots.filter(
      (s) => bay !== undefined && s.x >= bay.x0 && s.x < bay.x1,
    );
    const slot = own[i % perBay];
    if (slot === undefined)
      throw new Error(`gallery: no bay slot for ${machine}`);
    fixtures.push({
      kind: "machine",
      slot,
      machine,
      tag: machine,
      hue: seedFor("tag-hue", machine) % 360,
      seed: seedFor(seed, "tag", machine),
    });
  });

  const decor: Decor[] = GALLERY_DECOR.map(([kind, x, y, turn]) => ({
    kind,
    x,
    y,
    turn,
    seed: seedFor(seed, "decor", kind, 0),
  }));

  const propBays = layout.bays.slice(2, 4);
  const bay3 = propBays[0];
  const bay4 = propBays[1];
  if (bay3 === undefined || bay4 === undefined) {
    throw new Error("gallery: needs four bays for the props");
  }
  const props = galleryProps(seed, layout.grid, bay3, bay4);

  const lights: LightZone[] = [];
  for (let y0 = 0; y0 < layout.depth; y0 += 4) {
    for (let x0 = 0; x0 < layout.width; x0 += 4) {
      const x1 = Math.min(layout.width, x0 + 4);
      const y1 = Math.min(layout.depth, y0 + 4);
      const floor = layout.grid
        .slice(y0, y1)
        .some((row) => row.slice(x0, x1).includes("."));
      if (!floor) continue;
      lights.push({
        x0,
        y0,
        x1,
        y1,
        level: GALLERY_LIGHT,
        special: "steady",
        seed: seedFor(seed, "light", x0, y0),
      });
    }
  }

  const base: RoomSpec = {
    version: GAME_VERSION,
    seed,
    domain: GALLERY.domain,
    permalink: GALLERY.permalink,
    title: "Model Gallery",
    archetype: "engineering",
    condition,
    width: layout.width,
    depth: layout.depth,
    grid: layout.grid,
    hall: layout.hall,
    bays: layout.bays,
    corridor: layout.corridor,
    entrance: { x: layout.entrance.x, y: layout.entrance.y },
    ceiling: 4,
    spawn: { x: layout.entrance.x, y: layout.entrance.y, yaw: 0 },
    fixtures,
    decor,
    scaffold: scaffoldFor(condition, layout.hall, decor, seed),
    heroes: [],
    props,
    curios: [],
    finish: plainFinish(layout.bays.length),
    decals: [],
    lights,
    dropped: 0,
    inboundMore: 0,
  };
  return { ...base, curios: galleryCurios(seed, base) };
}

/**
 * The gallery's curios (2.6b): one curio on the first host surface
 * of every non-hero host kind the gallery carries (`hostSurfaces`,
 * `curioOn`), at the surface's own centre (`u = v = 0.5`): the terminal's
 * first desk end, the workbench's top, one end of the lab bench, the lab
 * island, the round table's first spot, the lower level of the second
 * storage shelf (v1, at `h` 1.08) and the top of the first (v0), and the
 * tops of both filing cabinets (v0 and v1, told apart by their different
 * `h`). Both under-desk curios (the trap and the fuel case) sit side by
 * side in a row (`row`) on the workbench's lower shelf.
 *
 * The lab bench's top is `cls: "bench"`, and neither ball kind
 * (`CURIO_CATALOGUE["star-ball"|"catch-ball"].classes`) nor the pink
 * gadget's cluster variant (`pink-gadget` 1, 0.36 m wide against the
 * bench's 0.25 m usable span) stands on a bench, so it carries the lit
 * sword's blue stand (`light-sword` 1) instead, the nearest legal kind
 * (`curioFits`) that isn't already shown elsewhere in the gallery.
 *
 * Every single-item seed is `seedFor(seed, "curio", host, j, s.key[0],
 * s.key[1])`, `j` the surface's own index in its host's table and
 * `s.key`'s anchor ints the surface's own position (C9): `host` and `j`
 * alone collide for a kind whose per-variant table always holds one
 * surface (the filing cabinet's top, `j` always 0), since two instances of
 * the same prop kind then share both; the anchor ints tell them apart,
 * since no two hosts stand at the same point. The row's two items share
 * one surface (`machine:workbench`'s under spot, `j` 1), so `j` alone
 * does not tell them apart; each seed adds the curio's own kind as a
 * fifth token instead. No host is added beyond what the gallery already
 * carries.
 */
function galleryCurios(seed: number, room: RoomSpec): Curio[] {
  const surfaces = hostSurfaces(room);
  const firstOf = (host: string, cls: SurfaceClass): HostSurface => {
    const s = surfaces.find((x) => x.host === host && x.cls === cls);
    if (s === undefined) throw new Error(`gallery: no ${host} ${cls} surface`);
    return s;
  };
  const atHeight = (host: string, h: number): HostSurface => {
    const s = surfaces.find((x) => x.host === host && Math.abs(x.h - h) < 1e-9);
    if (s === undefined) {
      throw new Error(`gallery: no ${host} surface at ${String(h)}`);
    }
    return s;
  };
  const place = (
    s: HostSurface,
    kind: CurioKind,
    variant: number,
    j: number,
  ): Curio =>
    curioOn(
      s,
      kind,
      variant,
      0.5,
      0.5,
      seedFor(seed, "curio", s.host, j, s.key[0], s.key[1]),
    );
  const underShelf = firstOf("machine:workbench", "under");

  return [
    place(firstOf("terminal", "desk"), "pocket-console", 0, 0),
    place(firstOf("machine:workbench", "bench"), "beige-laptop", 0, 0),
    ...row(underShelf, [
      {
        kind: "trap-box",
        variant: 0,
        seed: seedFor(
          seed,
          "curio",
          underShelf.host,
          1,
          underShelf.key[0],
          underShelf.key[1],
          "trap-box",
        ),
      },
      {
        kind: "fuel-case",
        variant: 0,
        seed: seedFor(
          seed,
          "curio",
          underShelf.host,
          1,
          underShelf.key[0],
          underShelf.key[1],
          "fuel-case",
        ),
      },
    ]),
    place(firstOf("machine:lab-bench", "bench"), "light-sword", 1, 0),
    place(firstOf("decor:lab-island", "bench"), "green-pistol", 0, 0),
    place(firstOf("decor:round-table", "table"), "video-tape", 0, 0),
    place(atHeight("prop:filing-cabinet", 0.8), "catch-ball", 0, 0),
    place(atHeight("prop:filing-cabinet", 1.4), "pink-gadget", 0, 0),
    place(atHeight("prop:storage-shelf", 1.08), "tape-drive", 0, 1),
    place(atHeight("prop:storage-shelf", 1.6), "light-sword", 0, 0),
  ].sort(CURIO_ORDER);
}

/**
 * Filters the runs `wallRuns` returns to the ones entirely inside `bay`, on
 * one of `sides`: a bay's own straight wall run, never a neighbour's.
 */
function bayRuns(
  grid: readonly string[],
  bay: Rect,
  sides: readonly Side[],
): WallSlot[][] {
  return wallRuns(grid).filter((run) => {
    const first = run[0];
    return (
      first !== undefined &&
      sides.includes(first.side) &&
      run.every(
        (e) => e.x >= bay.x0 && e.x < bay.x1 && e.y >= bay.y0 && e.y < bay.y1,
      )
    );
  });
}

/**
 * A queue over a list of wall runs: `take` hands out one edge at a time and
 * `takePair` two consecutive edges of the same run (a run kind's segments),
 * skipping a run once it cannot serve what is asked. Both throw once the
 * queue is spent, which a bug in the gallery's own bookkeeping is the only
 * way to reach.
 */
function edgeQueue(runsIn: readonly WallSlot[][]): {
  take(): WallSlot;
  takePair(): [WallSlot, WallSlot];
} {
  const runs = runsIn.map((r) => [...r]);
  let cursor = 0;
  const advance = (need: number) => {
    while (cursor < runs.length && (runs[cursor]?.length ?? 0) < need) {
      cursor++;
    }
  };
  return {
    take(): WallSlot {
      advance(1);
      const e = runs[cursor]?.shift();
      if (e === undefined) throw new Error("gallery: ran out of wall edges");
      return e;
    },
    takePair(): [WallSlot, WallSlot] {
      advance(2);
      const pair = runs[cursor]?.splice(0, 2);
      const a = pair?.[0];
      const b = pair?.[1];
      if (a === undefined || b === undefined) {
        throw new Error("gallery: ran out of wall run space");
      }
      return [a, b];
    },
  };
}

/**
 * The gallery's rare floor props (2.6d C19), each at a fixed point, as
 * `[kind, variant, x, y, turn]` in cell units. Bays 3 and 4 offer 44 floor
 * cells, 41 of them used by the regular floor kinds and one closed by the
 * rare poster's keep-clear, so the six rare kind-variants would overflow
 * them: they stand in the hall instead. The canisters, the console and the
 * marked crates stand in two rows east of the specimen tank, turn 2,
 * facing the entrance, each off every lane, fixture and piece of furniture;
 * the designer tower stands where its own rule puts it (C13), the
 * wall-side spot of cell (0, 11) backed to the west wall, its south side
 * `TOWER_DESK_GAP` from the border with the terminal's cell (0, 12). It
 * keeps out of cell (0, 13), between the two terminals, where the west
 * wall stays open to walk along.
 */
export const GALLERY_HALL_PROPS = [
  ["ooze-canisters", 0, 10.5, 10.5, 2],
  ["ooze-canisters", 1, 12.5, 10.5, 2],
  ["gravity-console", 0, 14.5, 10.5, 2],
  ["marked-crate", 0, 10.5, 12.5, 2],
  ["marked-crate", 1, 12.5, 12.5, 2],
  ["designer-tower", 0, 0.1325, 11.9125, 1],
] as const satisfies readonly (readonly [
  FloorPropKind,
  number,
  number,
  number,
  number,
])[];

/**
 * One of every prop kind and variant (ruling 13's six sign-plate pictograms
 * included), hand-placed in bays 3 and 4 rather than drawn from a palette:
 *
 * - wall kinds on the north, south and east walls of both bays, a run kind
 *   (`cable-tray`, `pipe-bundle`) as a 2-segment run per variant, taken from
 *   two consecutive edges of one straight run so a run never turns a corner
 *   or crosses a bay's doorway;
 * - ceiling kinds (spans excepted) along both bays' north wall edges, runs
 *   (`duct`, `ceiling-tray`) the same way; a ceiling prop may share an edge
 *   with a wall prop, since the two hang at different heights, exactly as
 *   the dressing pass allows;
 * - regular floor kinds centred on rows 1, 3 and 5 of the six inner columns of
 *   each bay (`bay.x0 + 1` to `bay.x1 - 2`), turn 0, then row 7 of bay 3 and of
 *   bay 4 in the same columns: the four tall kinds Task 1 adds push the
 *   count past what rows 1, 3 and 5 hold. A row-7 cell is left out when its
 *   south wall edge carries a keep-clear wall prop, since the prop lies
 *   before it in the `wall` list built above and keep-clear holds there as
 *   it does in the real dressing pass. The outer columns are left out
 *   because they sit next to a bay's doorway, which the real dressing pass
 *   always keeps floor props off; rows 0, 2, 4 and 6 stay walkable, row 6
 *   running the length of each bay beside rows 5 and 7.
 * - the rare floor kinds (2.6d C19) not in the bays but at their fixed
 *   points in the hall (`GALLERY_HALL_PROPS`); the rare poster takes the
 *   next bay edge like every other wall kind;
 * - span kinds (`span-duct`, `span-tray`) over row 6: each kind and variant
 *   takes the next of four fixed first cells, `bay3.x0 + 1`, `bay3.x0 + 4`,
 *   `bay4.x0 + 1` and `bay4.x0 + 4`, anchored at `(x + 1, 6.5)`, turn 0.
 *
 * Every seed follows ruling 2, `seedFor(roomSeed, "prop", cx, cy, token)`:
 * the token is the edge's side for a single wall or ceiling prop, `run-` or
 * `ceiling-` plus the side for a run's segments, `"floor"` for a floor prop
 * and `"span"` for a span segment (keyed by its first cell), the same
 * tokens `dress.ts` uses. The output is sorted by `PROP_ORDER`, as a
 * dressed room's `props` always are.
 */
function galleryProps(
  roomSeed: number,
  grid: readonly string[],
  bay3: Rect,
  bay4: Rect,
): Prop[] {
  const atEdge = (
    kind: PropKind,
    e: WallSlot,
    variant: number,
    token: string,
  ): Prop => {
    const a = wallAnchor(e);
    return {
      kind,
      variant,
      anchor: PROP_CATALOGUE[kind].anchor,
      x: a.x,
      y: a.y,
      turn: a.turn,
      seed: seedFor(roomSeed, "prop", e.x, e.y, token),
    };
  };
  const kindsOf = (anchor: "wall" | "floor" | "ceiling") =>
    PROP_KINDS.filter((k) => PROP_CATALOGUE[k].anchor === anchor);

  // Wall kinds: the north, south and east walls of both bays.
  const wallEdges = edgeQueue([
    ...bayRuns(grid, bay3, ["n", "s", "e"]),
    ...bayRuns(grid, bay4, ["n", "s", "e"]),
  ]);
  const wall: Prop[] = [];
  for (const kind of kindsOf("wall")) {
    const entry = PROP_CATALOGUE[kind];
    if (!entry.run) continue;
    for (let variant = 0; variant < entry.variants; variant++) {
      const [a, b] = wallEdges.takePair();
      wall.push(
        atEdge(kind, a, variant, `run-${a.side}`),
        atEdge(kind, b, variant, `run-${b.side}`),
      );
    }
  }
  for (const kind of kindsOf("wall")) {
    const entry = PROP_CATALOGUE[kind];
    if (entry.run) continue;
    for (let variant = 0; variant < entry.variants; variant++) {
      const e = wallEdges.take();
      wall.push(atEdge(kind, e, variant, e.side));
    }
  }

  // Ceiling kinds: both bays' north wall edges only.
  const ceilingEdges = edgeQueue([
    ...bayRuns(grid, bay3, ["n"]),
    ...bayRuns(grid, bay4, ["n"]),
  ]);
  const ceiling: Prop[] = [];
  for (const kind of kindsOf("ceiling")) {
    const entry = PROP_CATALOGUE[kind];
    if (!entry.run) continue;
    for (let variant = 0; variant < entry.variants; variant++) {
      const [a, b] = ceilingEdges.takePair();
      ceiling.push(
        atEdge(kind, a, variant, `ceiling-${a.side}`),
        atEdge(kind, b, variant, `ceiling-${b.side}`),
      );
    }
  }
  for (const kind of kindsOf("ceiling")) {
    const entry = PROP_CATALOGUE[kind];
    if (entry.run || entry.span) continue;
    for (let variant = 0; variant < entry.variants; variant++) {
      const e = ceilingEdges.take();
      ceiling.push(atEdge(kind, e, variant, e.side));
    }
  }

  // Spans: over row 6, the next of four fixed first cells of bay 3 or 4.
  const spanFirstCells = [bay3.x0 + 1, bay3.x0 + 4, bay4.x0 + 1, bay4.x0 + 4];
  let si = 0;
  for (const kind of kindsOf("ceiling")) {
    const entry = PROP_CATALOGUE[kind];
    if (!entry.span) continue;
    for (let variant = 0; variant < entry.variants; variant++) {
      const x = spanFirstCells[si++];
      if (x === undefined) {
        throw new Error("gallery: ran out of span cells");
      }
      ceiling.push({
        kind,
        variant,
        anchor: "ceiling",
        x: x + 1,
        y: 6.5,
        turn: 0,
        seed: seedFor(roomSeed, "prop", x, 6, "span"),
      });
    }
  }

  // Floor kinds: rows 1, 3 and 5 of the six inner columns of each bay, then
  // row 7 of bay 3 and of bay 4, in the same columns, rows 0, 2, 4 and 6
  // stay walkable, row 6 running the length of each bay beside rows 5 and 7.
  const cells: { x: number; y: number }[] = [];
  for (const bay of [bay3, bay4])
    for (const y of [1, 3, 5])
      for (let x = bay.x0 + 1; x <= bay.x1 - 2; x++) cells.push({ x, y });
  // A row-7 cell's south wall edge is the same edge a south-facing wall
  // slot at y = row would anchor on (wallAnchor's "s" case: y: e.y + 1), so
  // it is left out when that edge carries a keep-clear wall prop: the prop
  // lies before it in `wall`, and keep-clear holds as in the real dressing
  // pass.
  const southKeepClear = new Set(
    wall
      .filter((p) => PROP_CATALOGUE[p.kind].keepClear)
      .map((p) => `${String(Math.floor(p.x))},${String(Math.floor(p.y))}`),
  );
  for (const bay of [bay3, bay4])
    for (let x = bay.x0 + 1; x <= bay.x1 - 2; x++)
      if (!southKeepClear.has(`${String(x)},8`)) cells.push({ x, y: 7 });
  let ci = 0;
  const floor: Prop[] = [];
  for (const kind of kindsOf("floor")) {
    const entry = PROP_CATALOGUE[kind];
    if (entry.rare) continue;
    for (let variant = 0; variant < entry.variants; variant++) {
      const cell = cells[ci++];
      if (cell === undefined) {
        throw new Error("gallery: ran out of floor cells for props");
      }
      floor.push({
        kind,
        variant,
        anchor: "floor",
        x: cell.x + 0.5,
        y: cell.y + 0.5,
        turn: 0,
        seed: seedFor(roomSeed, "prop", cell.x, cell.y, "floor"),
      });
    }
  }
  // The rare floor kinds at their fixed points in the hall (2.6d C19).
  for (const [kind, variant, x, y, turn] of GALLERY_HALL_PROPS)
    floor.push({
      kind,
      variant,
      anchor: "floor",
      x,
      y,
      turn,
      seed: seedFor(roomSeed, "prop", Math.floor(x), Math.floor(y), "floor"),
    });

  return [...wall, ...floor, ...ceiling].sort(PROP_ORDER);
}

/**
 * The need the hero hall's floor plan is sized from (H15). It is not what
 * the room carries: eleven on the north wall and eleven on the west give
 * `planLayout` a 23 by 24 hall with no bays and its entrance on column 11,
 * the hall's own centre column (checked against `planLayout` directly;
 * `canned.test.ts` does not repin it since only `heroHallRoom`'s own tests
 * read this hall), which is what lets the slab stand exactly where H9
 * places it without dodging the entrance lane.
 */
const HERO_HALL_NEED = {
  north: 11,
  west: 11,
  east: 0,
  south: 3,
  any: 0,
  hatches: 0,
};

/** The hero hall's one light level, steady everywhere, as `galleryRoom`'s. */
const HERO_HALL_LIGHT = 210;

/**
 * One steady light zone per four by four block that holds any floor, as
 * `galleryRoom`'s.
 */
function heroHallLights(roomSeed: number, layout: Layout): LightZone[] {
  const lights: LightZone[] = [];
  for (let y0 = 0; y0 < layout.depth; y0 += 4) {
    for (let x0 = 0; x0 < layout.width; x0 += 4) {
      const x1 = Math.min(layout.width, x0 + 4);
      const y1 = Math.min(layout.depth, y0 + 4);
      const floor = layout.grid
        .slice(y0, y1)
        .some((row) => row.slice(x0, x1).includes("."));
      if (!floor) continue;
      lights.push({
        x0,
        y0,
        x1,
        y1,
        level: HERO_HALL_LIGHT,
        special: "steady",
        seed: seedFor(roomSeed, "light", x0, y0),
      });
    }
  }
  return lights;
}

/**
 * The hero hall (H15): a room built by hand rather than generated, holding
 * one of every hero kind and variant, for judging them in the dev-only
 * route `/π/dev/gallery?hall=heroes` and for the `?at=prop:<kind>:<n>`
 * spots that frame each one (`spotSpawn` in `dev/spots.ts`).
 *
 * `planLayout(HERO_HALL_NEED)` gives a 23 by 24 hall with no bays and its
 * entrance on column 11, the hall's own centre column, so the slab stands at
 * the hall's centre x with its south face touching the entrance lane's north
 * end (H9) with nothing to shift: had the entrance not fallen on column 11
 * the slab would move to the hall's centre x and the laser desk off the
 * column, but it does, so neither moves.
 *
 * Every hero is hand-placed at a fixed point: the wall-anchored ones
 * through `wallAnchor` of a fixed edge (the core wall's two north-wall
 * edges, x 6 and x 7, combined the way `heroEdges`'s two-edge case combines
 * them, into the anchor `(7, 0)` at turn 2), the band, corner and centre
 * ones at fixed coordinates directly, and the turret turned to face the
 * hall's centre the way a generated room's corner hero would (`faceCentre`
 * in `heroes.ts`, H10). The eleven large heroes of 2.6c (C16) fill the rest:
 * the question block, the hoverboard and the flying cloud in a row at y 4 and
 * the stone hand, the thunder hammer and the moon rocket in a row at y 8, at x
 * 15, 18 and 21 in the east half, all turned to face the entrance; the spider
 * tank, the robot head, the red bike and the garden robot down the west
 * interior at x 4, the first three turned east; and the police box standing
 * free by the east wall at (22, 13.5), a walkway all round it, turned to
 * face the hall's centre (2.6e). The 2.6d slab walker (C17) stands
 * at (21, 11), turn 2, a band hero clear of every other one. Each one frames
 * from the front with a clear sight line, and none meets another hero, a
 * pinned hero or prop spot, or a curio host's framing side. Every hero's seed
 * is `seedFor(seed, "hero", kind, variant)`, keyed by its kind and variant
 * since a hand-placed room has no candidate to key a seed by anchor with,
 * and the list is sorted by `HERO_ORDER`, the order a generated room's own
 * heroes keep.
 *
 * No fixture but the placard, which tells the visitor how to frame one hero
 * at a time (`?AT=PROP:KIND:N`); one round table at (20, 16.5), turn 0,
 * that carries the 2.6d curios, and no props, so nothing but the heroes
 * and that table stands between the camera and what is being judged. One
 * steady light zone per four by four block, as `galleryRoom`'s. The same
 * call gives the same room byte for byte.
 */
export function heroHallRoom(): RoomSpec {
  const seed = seedFor(GAME_VERSION, "hero-hall");
  const layout = planLayout(HERO_HALL_NEED);
  const hall = layout.hall;

  const at = (
    kind: HeroKind,
    variant: number,
    x: number,
    y: number,
    turn: number,
  ): Hero => ({
    kind,
    variant,
    x,
    y,
    turn,
    seed: seedFor(seed, "hero", kind, variant),
  });

  const wall = (kind: HeroKind, variant: number, edge: WallSlot): Hero => {
    const a = wallAnchor(edge);
    return at(kind, variant, a.x, a.y, a.turn);
  };

  const heroes: Hero[] = [
    wall("eye-panel", 0, { x: 3, y: 0, side: "n" }),
    // The core wall's two north-wall edges, x 6 and x 7, anchored between
    // them, as `heroEdges`'s two-edge case anchors a two-edge wall hero.
    at("core-wall", 0, 7, 0, 2),
    wall("gun-rack", 0, { x: 10, y: 0, side: "n" }),
    wall("photo-console", 0, { x: 13, y: 0, side: "n" }),
    wall("tube-bench", 0, { x: 16, y: 0, side: "n" }),
    wall("gun-bench", 0, { x: 19, y: 0, side: "n" }),
    wall("arcade-cabinet", 0, { x: 0, y: 6, side: "w" }),
    wall("arcade-cabinet", 1, { x: 0, y: 9, side: "w" }),
    wall("arcade-cabinet", 2, { x: 0, y: 12, side: "w" }),
    wall("recruit-cabinet", 0, { x: 0, y: 15, side: "w" }),
    at("helper-robot", 0, 6.5, 8.5, 1),
    at("laser-desk", 0, 11.5, 9, 2),
    at("dome-planters", 0, 6.5, 13.5, 0),
    at("dome-planters", 1, 16.5, 13.5, 0),
    // H9: the slab's south face is on the hall's centre line.
    at("black-slab", 0, 11.5, 11.925, 2),
    at("sleep-ring", 0, 6.5, 19, 0),
    at("mess-table", 0, 16.5, 19, 0),
    at("turret", 0, 21.5, 22.5, faceCentre(hall, 21.5, 22.5)),
    at("field-pack", 0, 1.5, 22.5, 1),
    at("field-pack", 1, 21.5, 1.5, 2),
    // 2.6c (C16): the east half in two rows facing the entrance, the west
    // interior facing east, the police box free by the east wall. Every spot
    // frames from the front with a clear sight line (spots.test.ts).
    at("question-block", 0, 15, 4, 2),
    at("hoverboard", 0, 18, 4, 2),
    at("flying-cloud", 0, 21, 4, 2),
    at("stone-hand", 0, 15, 8, 2),
    at("thunder-hammer", 0, 18, 8, 2),
    at("moon-rocket", 0, 21, 8, 2),
    at("spider-tank", 0, 4, 5.5, 1),
    at("mech-head", 0, 4, 9.5, 1),
    at("red-bike", 0, 4, 16.5, 1),
    at("garden-robot", 0, 4, 21, 2),
    // 2.6e: free-standing, a walkway all round it, facing the centre.
    at("police-box", 0, 22, 13.5, faceCentre(hall, 22, 13.5)),
    // 2.6d C17: its 1.0 m moat clears every hero, and it frames from the
    // south with a clear sight line.
    at("slab-walker", 0, 21, 11, 2),
  ].sort(HERO_ORDER);

  const base: RoomSpec = {
    version: GAME_VERSION,
    seed,
    domain: "station",
    permalink: "hero-hall",
    title: "Hero Hall",
    archetype: "engineering",
    condition: "clean",
    width: layout.width,
    depth: layout.depth,
    grid: layout.grid,
    hall: layout.hall,
    bays: layout.bays,
    corridor: layout.corridor,
    entrance: { x: layout.entrance.x, y: layout.entrance.y },
    ceiling: 4,
    spawn: { x: layout.entrance.x, y: layout.entrance.y, yaw: 0 },
    fixtures: [
      {
        kind: "placard",
        slot: layout.placard,
        lines: [
          "Hero Hall",
          "ONE OF EVERY",
          "HERO PROPS AND CURIOS",
          "?AT=PROP:KIND:N",
        ],
      },
    ],
    decor: [
      {
        kind: "round-table",
        x: 20,
        y: 16.5,
        turn: 0,
        seed: seedFor(seed, "decor", "round-table", 0),
      },
    ],
    scaffold: [],
    heroes,
    props: [],
    curios: [],
    finish: plainFinish(layout.bays.length),
    decals: [],
    lights: heroHallLights(seed, layout),
    dropped: 0,
    inboundMore: 0,
  };
  return { ...base, curios: heroHallCurios(seed, base) };
}

/**
 * The decor kinds in the order the variants hall stands them, `GALLERY_DECOR`'s
 * own order: a bridge's console and chair, a council chamber's table and
 * chairs, an engineering bay's generator and pipe run, an archive's shelf
 * row, and a lab's island and specimen tank.
 */
const VARIANTS_HALL_KIND_ORDER: readonly DecorKind[] = [
  "command-console",
  "captain-chair",
  "round-table",
  "council-chair",
  "generator",
  "pipe-run",
  "shelf-row",
  "lab-island",
  "specimen-tank",
];

/**
 * Every decor kind and variant the variants hall stands once, in hall
 * order: kind by kind (`VARIANTS_HALL_KIND_ORDER`), variant by variant.
 * Read off `VARIANT_COUNTS.decor`, so it follows every count. The stills
 * of the model tasks and of the decals frame these pieces, and
 * `canned.test.ts` pins them against the head of `variantsHallRoom`'s own
 * `room.decor` (the council row, `VARIANTS_HALL_COUNCIL`, follows them).
 */
export const VARIANTS_HALL_DECOR: readonly {
  kind: DecorKind;
  variant: number;
}[] = VARIANTS_HALL_KIND_ORDER.flatMap((kind) =>
  Array.from({ length: VARIANT_COUNTS.decor[kind] }, (_, variant) => ({
    kind,
    variant,
  })),
);

/**
 * The variants hall's council row: one round table and six council chairs
 * round it, as a generated council chamber stands them (`decorFor` in
 * `generate.ts`: 1.25 cells out, each turned to face the table as near as
 * a quarter turn allows), all six chairs in one variant, the set a room
 * draws (2.7 C4). It stands in the hall's southern half, west of the
 * entrance lane, after the one-of-each pieces (`VARIANTS_HALL_DECOR`), so
 * `?at=decor:<kind>:<n>` still frames those by their variant.
 */
export const VARIANTS_HALL_COUNCIL = {
  table: 1,
  chairs: 1,
  /** How far each chair stands from the table's centre, in cells. */
  radius: 1.25,
} as const;

/**
 * How many doors' worth of north wall the variants hall asks `planLayout`
 * for. No door stands there; the need only widens the hall (`2 * north +
 * 1` cells, `layout.ts`) so every decor variant's row fits in its northern
 * half and the council row fits beside the entrance lane in its southern.
 */
const VARIANTS_HALL_NORTH = 5;

/** The variants hall's one light level, steady everywhere, as `galleryRoom`'s. */
const VARIANTS_HALL_LIGHT = 210;

/**
 * How many metres apart a variants-hall decor row's centre stands from the
 * next: the deepest piece's full depth (`DECOR_ROW_REACH` twice) plus
 * `DECOR_ROW_CLEAR`, so two rows never meet.
 */
const DECOR_ROW_PITCH = 3.6;

/**
 * How far a variants-hall decor piece reaches from its row's centre line:
 * half the deepest decor footprint (the round table's).
 */
const DECOR_ROW_REACH = Math.max(
  ...Object.values(FOOTPRINTS.decor).map((s) => (s === null ? 0 : s.depth / 2)),
);

/** How many metres a variants-hall decor piece keeps clear of its row neighbour (2.7 Task 1). */
const DECOR_ROW_CLEAR = 1.2;

/**
 * A little past `USE_LANE_DEPTH`, so a variants-hall decor piece never
 * lands exactly on a terminal's or a machine's use lane's own edge.
 */
const DECOR_ROW_SAFETY = 0.1;

/**
 * Where the variants hall's decor stands (`x` and `y`, cell units), keyed
 * by `"<kind>:<variant>"`: one row every `DECOR_ROW_PITCH` metres, in the
 * northern half of the hall (the entrance lane only ever reaches the
 * southern half, C13's `withHeroes` aside, which this hand-built room never
 * runs), every piece `USE_LANE_DEPTH` plus `DECOR_ROW_SAFETY` clear of the
 * north wall too (machines the east wall cannot hold spill onto it), packed
 * left to right within `USE_LANE_DEPTH` plus `DECOR_ROW_SAFETY` of the west
 * and east walls, so no piece ever reaches a terminal's or a machine's use
 * lane whichever wall row it falls on. Widest first (a largest-fit-first
 * bin pack): a kind's footprint never changes across its variants (2.7
 * C3), so every instance of one kind claims the same width regardless of
 * which variant it draws. `pipe-run`
 * (`decorFootprint` null, C13) packs at `PIPE_HALF * 2`, a piece hanging
 * from the ceiling with no floor box of its own, so it still keeps its row
 * neighbours' clearance without ever being measured against the floor; it
 * stands turned a quarter (`variantsHallRoom`), its length running across
 * the rows, so two runs in one row never cross each other.
 * Throws, naming the kind and variant, when a piece finds no row left: a
 * hall too small for its own decor list is a bug in this file, never a
 * silent overlap (`row`'s own rule, above).
 */
function variantsHallDecorPlacements(
  hall: Rect,
): ReadonlyMap<string, { x: number; y: number }> {
  const xMin = hall.x0 * CELL + USE_LANE_DEPTH + DECOR_ROW_SAFETY;
  const xMax = hall.x1 * CELL - USE_LANE_DEPTH - DECOR_ROW_SAFETY;
  const halfDepth = ((hall.y1 - hall.y0) * CELL) / 2;
  const rows: { z: number; cursor: number }[] = [];
  for (
    let z = USE_LANE_DEPTH + DECOR_ROW_SAFETY + DECOR_ROW_REACH;
    z <= halfDepth - DECOR_ROW_REACH;
    z += DECOR_ROW_PITCH
  ) {
    rows.push({ z, cursor: xMin });
  }

  const items = VARIANTS_HALL_DECOR.map(({ kind, variant }) => {
    const size = FOOTPRINTS.decor[kind];
    return { kind, variant, width: size === null ? PIPE_HALF * 2 : size.width };
  });
  const order = items
    .map((_, i) => i)
    .sort((a, b) => {
      const wa = items[a]?.width ?? 0;
      const wb = items[b]?.width ?? 0;
      return wb - wa || a - b;
    });

  const placed = new Map<string, { x: number; y: number }>();
  for (const i of order) {
    const it = items[i];
    if (it === undefined) continue;
    const row = rows.find((r) => {
      const need = r.cursor === xMin ? it.width : it.width + DECOR_ROW_CLEAR;
      return r.cursor + need <= xMax + 1e-9;
    });
    if (row === undefined) {
      throw new Error(
        `variants hall: no row left for ${it.kind}:${String(it.variant)}`,
      );
    }
    const start =
      row.cursor === xMin ? row.cursor : row.cursor + DECOR_ROW_CLEAR;
    const centreX = start + it.width / 2;
    row.cursor = start + it.width;
    placed.set(`${it.kind}:${String(it.variant)}`, {
      x: centreX / CELL,
      y: row.z / CELL,
    });
  }
  return placed;
}

/**
 * The variants hall's council row (`VARIANTS_HALL_COUNCIL`): the table in
 * the hall's southern half, as far west as its chairs keep
 * `USE_LANE_DEPTH` plus `DECOR_ROW_SAFETY` clear of the west wall, and
 * its six chairs round it.
 */
function variantsHallCouncil(hall: Rect, seed: number): Decor[] {
  const C = VARIANTS_HALL_COUNCIL;
  const chair = FOOTPRINTS.decor["council-chair"];
  const reach = C.radius * CELL + (chair === null ? 0 : chair.width / 2);
  const cx =
    (hall.x0 * CELL + USE_LANE_DEPTH + DECOR_ROW_SAFETY + reach) / CELL;
  const cy = hall.y0 + ((hall.y1 - hall.y0) * 3) / 4;
  const out: Decor[] = [
    {
      kind: "round-table",
      x: cx,
      y: cy,
      turn: 0,
      variant: C.table,
      seed: seedFor(seed, "council", "table"),
    },
  ];
  for (let k = 0; k < 6; k++) {
    const angle = (k * Math.PI) / 3;
    out.push({
      kind: "council-chair",
      x: Math.round((cx + C.radius * Math.sin(angle)) * 1000) / 1000,
      y: Math.round((cy - C.radius * Math.cos(angle)) * 1000) / 1000,
      turn: Math.round((angle + Math.PI) / (Math.PI / 2)) % 4,
      variant: C.chairs,
      seed: seedFor(seed, "council", "chair", k),
    });
  }
  return out;
}

/**
 * The decal kinds and tiles the variants hall lays, one of each on its
 * south wall and one of each on its floor (2.7 C24): every tile of the
 * atlas but the stencils' solid one (`DECAL_TILES` in `render/textures.ts`,
 * whose counts `canned.test.ts` checks this list against).
 */
const VARIANTS_HALL_DECAL_KINDS: readonly {
  kind: Exclude<DecalKind, "stencil">;
  variants: number;
}[] = [
  { kind: "chevrons", variants: 2 },
  { kind: "arrow", variants: 1 },
  { kind: "grime", variants: 3 },
  { kind: "streak", variants: 2 },
  { kind: "rust", variants: 2 },
];

/**
 * The size of each variants-hall sample decal, in metres: `width` across
 * and `length` up the wall or along the floor decal's turn.
 */
const VARIANTS_HALL_DECAL_SIZE: Record<
  Exclude<DecalKind, "stencil">,
  { width: number; length: number }
> = {
  chevrons: { width: 1.2, length: 0.3 },
  arrow: { width: 0.6, length: 0.9 },
  grime: { width: 0.9, length: 0.9 },
  streak: { width: 0.6, length: 0.9 },
  rust: { width: 0.6, length: 0.9 },
};

/**
 * The variants hall's decals (2.7 C24), hand-placed:
 *
 * - **The wall**: one of every kind and tile (`VARIANTS_HALL_DECAL_KINDS`)
 *   on the hall's south wall, three to an edge, 0.55 by 0.8 m from 0.3 m
 *   up (under the accent stripe), on the south row's free edges west and
 *   east of the entrance (x 0, 2, 8 and 10, each between two machines),
 *   and the wall stencil, `DECK n` over `BAY m`, on the entrance's east
 *   neighbour edge, where C19 stands it (1.5 m up, 0.9 by 0.3 m). Turning
 *   round at the spawn shows them all.
 * - **The floor**: two rows east of the entrance lane, a little ahead of
 *   the spawn: the chevrons, the arrow (pointing south, to the entrance)
 *   and the floor stencil `BAY m`, then the grime, streaks and rust, each
 *   at its own size (`VARIANTS_HALL_DECAL_SIZE`), clear of every lane and
 *   footprint.
 *
 * The deck and bay numbers are the hall's own address's (`deckNumber`,
 * `bayNumber`: a permalink at the domain's root is deck 1).
 */
function variantsHallDecals(
  room: Pick<RoomSpec, "hall" | "entrance" | "domain" | "permalink">,
  seed: number,
): Decal[] {
  const samples = VARIANTS_HALL_DECAL_KINDS.flatMap(({ kind, variants }) =>
    Array.from({ length: variants }, (_, variant) => ({ kind, variant })),
  );
  const seedOf = (...parts: (string | number)[]) =>
    seedFor(seed, "decal", ...parts);
  const text = {
    deck: deckNumber(room.domain, room.permalink),
    bay: bayNumber(room.domain, room.permalink),
    letter: 0,
  };
  const south = room.hall.y1 - 1;
  const ent = room.entrance;

  // The wall: three to an edge, then the stencil.
  const edges = [0, 2, 8, 10];
  const along = [-0.62, 0, 0.62];
  const out: Decal[] = samples.map(({ kind, variant }, i) => {
    const a = wallAnchor({
      x: edges[Math.floor(i / along.length)] ?? 0,
      y: south,
      side: "s",
    });
    return {
      kind,
      on: "wall",
      x: a.x,
      y: a.y,
      turn: a.turn,
      along: along[i % along.length] ?? 0,
      h: 0.3,
      width: 0.55,
      length: 0.8,
      variant,
      seed: seedOf("wall", kind, variant),
    };
  });
  const sign = wallAnchor({ x: ent.x + 1, y: ent.y, side: "s" });
  out.push({
    kind: "stencil",
    on: "wall",
    x: sign.x,
    y: sign.y,
    turn: sign.turn,
    along: 0,
    h: 1.5,
    width: 0.9,
    length: 0.3,
    variant: 0,
    seed: seedOf("stencil", "wall"),
    stencil: { ...text, lines: 2 },
  });

  // The floor: two rows, left to right from just east of the entrance lane.
  const start = (ent.x + 1) * CELL + 0.4;
  const rows: {
    y: number;
    gap: number;
    items: { kind: DecalKind; variant: number; turn: number }[];
  }[] = [
    {
      y: ent.y - 2.5,
      gap: 0.3,
      items: [
        { kind: "chevrons", variant: 0, turn: 0 },
        { kind: "chevrons", variant: 1, turn: 0 },
        { kind: "arrow", variant: 0, turn: 2 },
        { kind: "stencil", variant: 0, turn: 0 },
      ],
    },
    {
      y: ent.y - 1.3,
      gap: 0.25,
      items: samples
        .filter((d) => d.kind !== "chevrons" && d.kind !== "arrow")
        .map((d) => ({ ...d, turn: 0 })),
    },
  ];
  for (const line of rows) {
    let x = start;
    for (const { kind, variant, turn } of line.items) {
      const size =
        kind === "stencil"
          ? { width: 1.0, length: 0.3 }
          : VARIANTS_HALL_DECAL_SIZE[kind];
      out.push({
        kind,
        on: "floor",
        x: (x + size.width / 2) / CELL,
        y: line.y,
        turn,
        along: 0,
        h: 0,
        width: size.width,
        length: size.length,
        variant,
        seed: seedOf("floor", kind, variant),
        ...(kind === "stencil"
          ? { stencil: { ...text, lines: 1 as const } }
          : {}),
      });
      x += size.width + line.gap;
    }
  }
  return out;
}

/** How many machines the variants hall stands: every kind's own variant count, summed (2.7 C2). */
const VARIANTS_HALL_MACHINES = Object.values(VARIANT_COUNTS.machine).reduce(
  (a, b) => a + b,
  0,
);

/**
 * The variants hall (2.7 C24): a room built by hand rather than generated,
 * standing one of every machine kind and variant along the east wall and
 * the bays that need adds beyond it, one of every terminal variant on the
 * west wall, one of every decor kind and variant on the floor
 * (`VARIANTS_HALL_DECOR`) and a council row of six matching chairs round a
 * table (`VARIANTS_HALL_COUNCIL`), for the dev-only route `?hall=variants`
 * and the `?at=machine:<n>`, `?at=terminal:<n>` and `?at=decor:<kind>:<n>`
 * shots the models and the decals are judged with.
 *
 * `planLayout({ north: VARIANTS_HALL_NORTH, west: VARIANT_COUNTS.terminal,
 * east: VARIANTS_HALL_MACHINES, south: 0, any: 0, hatches: 0 })` sizes the
 * hall: its width from the north need (no door stands there, it only makes
 * room for the decor), its depth from the terminals and machines; the
 * machines and terminals are then handed
 * the slot pool's own `"east"` and `"west"` preferences in turn, the same
 * pool `galleryRoom`'s fixtures draw from, which fills the hall's own wall
 * first and, once a demand outgrows it, the bays the layout added to hold
 * the rest. Every machine carries a tag that names it (`<kind>-v<variant>`,
 * so the tag strip reads it) and its own explicit `variant`, its hue and
 * seed keyed by that tag exactly as a generated room's machine is
 * (`generate.ts`).
 *
 * Placard only: no door, portal, poster or hatch stands here, so nothing
 * but the machines, the terminals and the decor fills the hall. Condition
 * clean, so no scaffold; no heroes, props or curios. One steady light zone
 * per four by four block that holds any floor, as `galleryRoom`'s and
 * `heroHallRoom`'s. Accent 0, and its hall, bays and corridor in wall
 * patterns 0, then 1, 2, 1, 2 bay by bay and 1, so the hall and its bays
 * show all three patterns (2.7 C11). One of every decal kind and tile on
 * its south wall and its floor, and both stencils (`variantsHallDecals`).
 * The same call gives the same room byte for byte.
 */
export function variantsHallRoom(): RoomSpec {
  const seed = seedFor(GAME_VERSION, "variants-hall");
  const condition = "clean";
  const layout = planLayout({
    north: VARIANTS_HALL_NORTH,
    west: VARIANT_COUNTS.terminal,
    east: VARIANTS_HALL_MACHINES,
    south: 0,
    any: 0,
    hatches: 0,
  });
  const pool = createSlotPool(layout);
  const take = (pref: SlotPref): WallSlot => {
    const slot = pool.take(pref);
    if (slot === null) throw new Error(`variants hall: no ${pref} slot left`);
    return slot;
  };

  const fixtures: Fixture[] = [
    {
      kind: "placard",
      slot: layout.placard,
      lines: ["Variants Hall", "EVERY MACHINE TERMINAL", "AND DECOR VARIANT"],
    },
  ];

  for (let variant = 0; variant < VARIANT_COUNTS.terminal; variant++) {
    fixtures.push({
      kind: "terminal",
      slot: take("west"),
      heading: `Variant ${String(variant)}`,
      lines: [`Terminal variant ${String(variant)}.`],
      section: 0,
      seed: seedFor(seed, "terminal", "variant", variant),
      variant,
    });
  }

  for (const kind of MACHINE_KINDS) {
    const count = VARIANT_COUNTS.machine[kind];
    for (let variant = 0; variant < count; variant++) {
      const tag = `${kind}-v${String(variant)}`;
      fixtures.push({
        kind: "machine",
        slot: take("east"),
        machine: kind,
        tag,
        hue: seedFor("tag-hue", tag) % 360,
        seed: seedFor(seed, "tag", tag),
        variant,
      });
    }
  }

  const placements = variantsHallDecorPlacements(layout.hall);
  const decor: Decor[] = VARIANTS_HALL_DECOR.map(({ kind, variant }) => {
    const p = placements.get(`${kind}:${String(variant)}`);
    if (p === undefined) {
      throw new Error(
        `variants hall: no placement for ${kind}:${String(variant)}`,
      );
    }
    return {
      kind,
      x: p.x,
      y: p.y,
      turn: kind === "pipe-run" ? 1 : 0,
      variant,
      seed: seedFor(seed, "decor", kind, variant),
    };
  });
  decor.push(...variantsHallCouncil(layout.hall, seed));

  const lights: LightZone[] = [];
  for (let y0 = 0; y0 < layout.depth; y0 += 4) {
    for (let x0 = 0; x0 < layout.width; x0 += 4) {
      const x1 = Math.min(layout.width, x0 + 4);
      const y1 = Math.min(layout.depth, y0 + 4);
      const floor = layout.grid
        .slice(y0, y1)
        .some((row) => row.slice(x0, x1).includes("."));
      if (!floor) continue;
      lights.push({
        x0,
        y0,
        x1,
        y1,
        level: VARIANTS_HALL_LIGHT,
        special: "steady",
        seed: seedFor(seed, "light", x0, y0),
      });
    }
  }

  return {
    version: GAME_VERSION,
    seed,
    domain: "station",
    permalink: "variants-hall",
    title: "Variants Hall",
    archetype: "engineering",
    condition,
    width: layout.width,
    depth: layout.depth,
    grid: layout.grid,
    hall: layout.hall,
    bays: layout.bays,
    corridor: layout.corridor,
    entrance: { x: layout.entrance.x, y: layout.entrance.y },
    ceiling: 4,
    spawn: { x: layout.entrance.x, y: layout.entrance.y, yaw: 0 },
    fixtures,
    decor,
    scaffold: scaffoldFor(condition, layout.hall, decor, seed),
    heroes: [],
    props: [],
    curios: [],
    finish: {
      accent: 0,
      hallWalls: 0,
      bayWalls: [1, 2, 1, 2].slice(0, layout.bays.length),
      corridorWalls: 1,
    },
    decals: variantsHallDecals(
      {
        hall: layout.hall,
        entrance: layout.entrance,
        domain: "station",
        permalink: "variants-hall",
      },
      seed,
    ),
    lights,
    dropped: 0,
    inboundMore: 0,
  };
}

/**
 * Lays `items` along surface `s`'s wall-parallel axis, the host's own local
 * `a` axis (`turnedPoint` in `footprints.ts`), whatever its own length
 * against the surface's `d` axis (the one running from the wall out into the
 * room): at an even turn `a` is world x and `d` is world z, at an odd turn
 * the two swap (`turnedPoint`'s `front`/`along` split), and a curio placed
 * at its own host's turn shares that same split (`curioBox`'s width/depth
 * swap), so an item's extent along the row is always its `width`, regardless
 * of turn. `curioOn` places each item's centre at the cumulative offset
 * along `a`, centred on `d`. Every curio in a row takes `turn`, the
 * surface's own by default; a turn of the same parity as the surface's
 * (the round table's places, turned to face out from its centre) keeps
 * each item's extent along the row equal to its width, and `row` throws
 * when the two differ in parity, since a row turned across itself is a
 * bug in this file. Laying along `a` rather than whichever axis a
 * surface's box happens to span more matters because a surface can be
 * narrower along `a` than along `d`, as the tube bench and the gun bench
 * both are: a row run into the room instead of along the wall would stand
 * one item's box behind the next on the only line a player can stand, so
 * the one behind would never be visible. Throws, naming the host and the
 * kind, when an item does not fit the row's own budget (`surfaceLen - 2 *
 * CURIO_MARGIN`), whether because the item alone is too big for the
 * surface or because the items before it in the row already used the
 * space: a hand-built row is trusted arithmetic (H15's own "fits by the
 * Baselines numbers"), so a row that does not fit is a bug in this file,
 * never a silent overlap. Exported for `canned.test.ts`'s own direct test
 * of the throw; `heroHallCurios` is its only production caller.
 */
export function row(
  s: HostSurface,
  items: readonly { kind: CurioKind; variant: number; seed: number }[],
  turn: number = s.turn,
): Curio[] {
  const parity = (t: number) => (((Math.round(t) % 4) + 4) % 4) % 2;
  if (parity(turn) !== parity(s.turn)) {
    throw new Error(
      `row: turn ${String(turn)} runs across ${s.host}'s row (turn ${String(s.turn)})`,
    );
  }
  const aIsX = parity(s.turn) === 0;
  const surfaceLen = aIsX ? s.box.x1 - s.box.x0 : s.box.z1 - s.box.z0;
  const budget = surfaceLen - 2 * CURIO_MARGIN;
  const extentOf = (kind: CurioKind, variant: number): number => {
    const size = CURIO_CATALOGUE[kind].sizes[variant];
    if (size === undefined) {
      throw new Error(`row: ${kind} has no variant ${String(variant)}`);
    }
    return size.width;
  };
  let pos = 0;
  const out: Curio[] = [];
  for (const it of items) {
    const len = extentOf(it.kind, it.variant);
    if (pos + len > budget) {
      throw new Error(
        `row: ${it.kind} does not fit ${s.host} (over budget by ${(pos + len - budget).toFixed(3)} m)`,
      );
    }
    const slack = surfaceLen - len - 2 * CURIO_MARGIN;
    const frac = slack > 0 ? pos / slack : 0;
    out.push(
      curioOn(
        s,
        it.kind,
        it.variant,
        aIsX ? frac : 0.5,
        aIsX ? 0.5 : frac,
        it.seed,
        turn,
      ),
    );
    pos += len + 2 * CURIO_GAP;
  }
  return out;
}

/**
 * The hero hall's curios (2.6b, 2.6d): one of every curio kind and
 * variant, hand-placed on the hero and round-table surfaces they fit
 * (`hostSurfaces`, `curioOn` and `row`). `row` lays each row along its
 * host's local `a` axis (wall-parallel), and the tube bench and the gun
 * bench are both narrower
 * along `a` than along `d`, so the rows are grouped to fit those budgets:
 *
 * - the mess table's top, in a row: the laptop, the tape drive, the tape
 *   player, both video tapes, the pocket console, both balls and the lit
 *   sword's two upright variants (1 and 2), since every sword variant
 *   stands on a table too (C19's classes) and the mess table's `a` budget
 *   (its length) is by far the roomiest in the hall;
 * - the laser desk's top: the pistol, then the meter (its surface is longer
 *   along `a` than along `d`);
 * - the tube bench's top: the gadget's two variants, side by side (a
 *   "bench" surface, which both include, C19's classes);
 * - the gun bench's top: the sword lying in its cradle, alone (its `a`
 *   budget, 0.31 m, is too narrow for a second item);
 * - the laser desk's under spot: the trap;
 * - the gun bench's under spot: the fuel case;
 * - the space bricks at the end of the mess table's row (2.94 m of its
 *   2.96 m budget);
 * - the round table's four places (2.6d C19), each curio turned to face
 *   out from the table's centre: place 0 (south, turn 2) the radar and
 *   the capsule case side by side, place 1 (west, turn 3) the breadbin
 *   computer, place 2 (north, turn 0) the reactor case and the drone side
 *   by side, place 3 (east, turn 1) the slim computer, and the south under
 *   spot (turn 2) both soot puff variants in a row.
 *
 * Every seed is `seedFor(seed, "curio", kind, variant)`, and the output is
 * sorted by `CURIO_ORDER`.
 */
function heroHallCurios(seed: number, room: RoomSpec): Curio[] {
  const surfaces = hostSurfaces(room);
  const at = (token: string): HostSurface => {
    const s = surfaces.find((x) => x.key[2] === token);
    if (s === undefined) throw new Error(`hero hall: no ${token} surface`);
    return s;
  };
  const item = (kind: CurioKind, variant: number) => ({
    kind,
    variant,
    seed: seedFor(seed, "curio", kind, variant),
  });
  const one = (
    token: string,
    kind: CurioKind,
    variant: number,
    turn?: number,
  ): Curio => {
    const s = at(token);
    return curioOn(
      s,
      kind,
      variant,
      0.5,
      0.5,
      seedFor(seed, "curio", kind, variant),
      turn ?? s.turn,
    );
  };

  return [
    ...row(at("hero-mess-table-0"), [
      item("beige-laptop", 0),
      item("tape-drive", 0),
      item("tape-player", 0),
      item("video-tape", 0),
      item("video-tape", 1),
      item("pocket-console", 0),
      item("star-ball", 0),
      item("catch-ball", 0),
      item("light-sword", 1),
      item("light-sword", 2),
      item("space-bricks", 0),
    ]),
    ...row(at("hero-laser-desk-0"), [
      item("green-pistol", 0),
      item("wing-meter", 0),
    ]),
    ...row(at("hero-tube-bench-0"), [
      item("pink-gadget", 0),
      item("pink-gadget", 1),
    ]),
    ...row(at("hero-gun-bench-0"), [item("light-sword", 0)]),
    one("hero-laser-desk-under-0", "trap-box", 0),
    one("hero-gun-bench-under-0", "fuel-case", 0),
    // 2.6d C19: the round table's four places, each turned out from its
    // centre (place 0 south, 1 west, 2 north, 3 east), and its south
    // under spot.
    ...row(
      at("decor-round-table-0"),
      [item("treasure-radar", 0), item("capsule-case", 0)],
      2,
    ),
    one("decor-round-table-1", "breadbin-computer", 0, 3),
    ...row(
      at("decor-round-table-2"),
      [item("reactor-case", 0), item("hover-drone", 0)],
      0,
    ),
    one("decor-round-table-3", "slim-computer", 0, 1),
    ...row(
      at("decor-round-table-4"),
      [item("soot-puffs", 0), item("soot-puffs", 1)],
      2,
    ),
  ].sort(CURIO_ORDER);
}
