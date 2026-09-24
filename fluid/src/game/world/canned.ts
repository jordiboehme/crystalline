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
 */

import type { PlaceInput } from "./types";

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
