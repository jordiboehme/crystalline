/**
 * The milestone 1 test room: a bridge made from an engram that does not
 * exist anywhere but here.
 *
 * It carries exactly what the look demo needs to judge the looks on: two
 * `## ` sections (two terminals), two relations to targets of low and high
 * salience (a sliding door and a blast door), one wikilink into another
 * domain (a portal in the cross-domain colour) and two tags (two machines).
 * Nothing here is fetched; milestone 2 replaces this with real engrams.
 */

import type { PlaceInput } from "./types";

export const CANNED_BRIDGE: PlaceInput = {
  domain: "station",
  permalink: "manifest",
  title: "Station Crystalline",
  type: "manifest",
  status: "stable",
  salience: 7,
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
      targetTitle: "The Old Bridge",
      targetSalience: 2,
    },
    {
      relType: "depends_on",
      target: { domain: null, target: "reactor-core" },
      resolved: true,
      targetTitle: "Reactor Core",
      targetSalience: 8,
    },
  ],
  links: [
    {
      relType: null,
      target: { domain: "logistics", target: "Cargo Manifest" },
      resolved: true,
      targetTitle: "Cargo Manifest",
      targetSalience: 5,
    },
  ],
};
