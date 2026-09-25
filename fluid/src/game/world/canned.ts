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
 * model gallery: one of every model the station draws, so each can be
 * walked up to and judged without an engram that happens to need it.
 */

import { seedFor } from "../core/seed";
import { GAME_VERSION } from "../version";
import { MACHINE_KINDS, NOT_FOUND, NO_ROUTE, scaffoldFor } from "./generate";
import { createSlotPool, planLayout, wallSlots, type SlotPref } from "./layout";
import type {
  Decor,
  DecorKind,
  Fixture,
  LightZone,
  PlaceAddress,
  PlaceInput,
  RoomSpec,
  WallSlot,
} from "./types";

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
 *   four tags (east) make it 10 deep, and no bay is needed.
 * - **South wall.** Three inbound references, three hatches beside the
 *   entrance and the placard, each with its sign plate.
 * - **Posters.** Two observations in two categories.
 * - **Dressing.** `runbook` is the engineering archetype (pipe bundles on the
 *   walls, ducts on the ceiling, barrels, crates and trolleys) and `draft`
 *   puts it under construction: two scaffold frames and the construction
 *   extras (traffic cones, a ladder, a tool cart).
 */
export const CANNED_WORKSHOP: PlaceInput = {
  domain: "station",
  permalink: "workshop",
  title: "Valve Workshop",
  type: "runbook",
  status: "draft",
  salience: 5,
  validFrom: null,
  validTo: null,
  tags: ["pressure", "valves", "tools", "safety"],
  content: [
    "# Valve Workshop",
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
 * `any` asks for exactly two bays, where the twelve machines stand.
 */
const GALLERY_NEED = {
  north: 8,
  west: 7,
  east: 0,
  south: 3,
  any: 30,
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
 * route `/game/dev/gallery`.
 *
 * - **Hall.** Seventeen by sixteen cells, sized by `planLayout` from
 *   `GALLERY_NEED`. On the north wall a door of each style that opens
 *   (sliding, bulkhead, blast), a door sealed `NO ROUTE` and one sealed
 *   `?FILE NOT FOUND`, then a portal into the same domain, one into another
 *   domain and one sealed; two terminals on the west wall; two posters on
 *   the east wall; two hatches beside the entrance and the placard on the
 *   south wall; and every kind of furniture in the middle (`GALLERY_DECOR`).
 * - **Bays.** Two, east of the hall, holding the twelve machines, one of
 *   each kind in `MACHINE_KINDS` order, six to a bay, labelled with their
 *   kind.
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
        "MACHINES IN THE BAYS",
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
    ["Keys", ["WASD walks, the arrows turn.", "E uses, 1 2 4 pick the look."]],
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

  // The machines stand in the bays' own slots, which the pool would only
  // hand out once the hall's were gone.
  const baySlots = wallSlots(layout.grid).filter((s) =>
    layout.bays.some(
      (b) => s.x >= b.x0 && s.x < b.x1 && s.y >= b.y0 && s.y < b.y1,
    ),
  );
  const perBay = MACHINE_KINDS.length / layout.bays.length;
  MACHINE_KINDS.forEach((machine, i) => {
    const bay = layout.bays[Math.floor(i / perBay)];
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

  return {
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
    props: [],
    lights,
    dropped: 0,
    inboundMore: 0,
  };
}
