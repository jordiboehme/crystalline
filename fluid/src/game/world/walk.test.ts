/**
 * The walk (M3 C7, C9, C12, C24, C28): a whole station, served from
 * in-memory trees through `roomFor`, walked breadth first from the airlock
 * over every lift stop, every deck door and every exit, the way a player
 * can walk it. Every engram must be the target of exactly one deck door,
 * every engram room's exit must lead back up to a deck that holds its
 * door, and every deck and bridge must lead back towards the airlock.
 *
 * Nothing here reads the API: the loader answers each station address the
 * way `data/station.ts` does, from a flat list of engrams whose folders
 * come from their paths (`folderOfPath`), so the trees are worked out, not
 * written down twice.
 */

import { describe, expect, it } from "vitest";

import { sameStation } from "../paths";
import {
  folderName,
  folderOfPath,
  isManifestPermalink,
  parentFolder,
} from "./folders";
import { ARRIVAL_DISTANCE, arrivalSpawn, wallPoint } from "./interact";
import { LIFT_WORDS } from "./lifts";
import { roomFor, type StationRoom, type StationRoomInput } from "./station";
import type { Arrival } from "./interact";
import type { PlaceInput, StationAddress } from "./types";

/** One engram of the fake station: its domain and its file path. */
interface FakeEngram {
  domain: string;
  permalink: string;
  path: string;
}

const DOMAINS = [
  { name: "station", private: false },
  { name: "vault", private: true },
] as const;

/** `count` engrams in `folder` of domain `station`, named `<stem><i>`. */
function many(folder: string, stem: string, count: number): FakeEngram[] {
  return Array.from({ length: count }, (_, i) => {
    const name = `${stem}${String(i).padStart(2, "0")}`;
    const permalink = folder === "" ? name : `${folder}/${name}`;
    return { domain: "station", permalink, path: `${permalink}.md` };
  });
}

const ENGRAMS: readonly FakeEngram[] = [
  { domain: "station", permalink: "MANIFEST", path: "MANIFEST.md" },
  ...many("", "root-", 3),
  ...many("crew", "crew-", 30),
  ...many("crew/drills", "drill-", 2),
  ...many("empty/sub", "deep-", 1),
  { domain: "vault", permalink: "MANIFEST", path: "MANIFEST.md" },
  { domain: "vault", permalink: "vault-note", path: "vault-note.md" },
];

const folderOf = (e: FakeEngram) => folderOfPath(e.path);

/** Every folder of `domain`, the root and every ancestor included. */
function foldersOf(domain: string): Set<string> {
  const out = new Set<string>([""]);
  for (const e of ENGRAMS) {
    if (e.domain !== domain) continue;
    let f: string | null = folderOf(e);
    while (f !== null && f !== "") {
      out.add(f);
      f = parentFolder(f);
    }
  }
  return out;
}

/** The names of the folders directly below `folder` in `domain`. */
function subfoldersOf(domain: string, folder: string): string[] {
  return [...foldersOf(domain)]
    .filter((f) => f !== "" && parentFolder(f) === folder)
    .map(folderName);
}

/** The engrams directly in `folder` of `domain`, the MANIFEST included. */
function rowsOf(domain: string, folder: string) {
  return ENGRAMS.filter(
    (e) => e.domain === domain && folderOf(e) === folder,
  ).map((e) => ({
    permalink: e.permalink,
    title: `Title ${e.permalink}`,
    type: "engram",
    status: "stable",
  }));
}

/** A place with no ways out but its exit: nothing leads off the walk. */
function plainPlace(domain: string, permalink: string): PlaceInput {
  return {
    domain,
    permalink,
    title: `Title ${permalink}`,
    type: "engram",
    status: "stable",
    salience: null,
    validFrom: null,
    validTo: null,
    tags: [],
    content: "",
    relations: [],
    links: [],
    inbound: [],
    inboundTotal: 0,
    observations: [],
  };
}

/** What `loadStation` would answer for `a`, read from the fake trees. */
function load(a: StationAddress): StationRoomInput {
  switch (a.kind) {
    case "airlock":
      return {
        kind: "airlock",
        input: { domains: DOMAINS.map((d) => ({ ...d })), here: null },
      };
    case "bridge": {
      const root = rowsOf(a.domain, "");
      const manifest = root.find((r) => isManifestPermalink(r.permalink));
      return {
        kind: "bridge",
        place: plainPlace(a.domain, manifest?.permalink ?? "manifest"),
        bridge: {
          domain: a.domain,
          display: a.domain,
          engrams: ENGRAMS.filter((e) => e.domain === a.domain).length,
          private: DOMAINS.find((d) => d.name === a.domain)?.private ?? false,
          folders: subfoldersOf(a.domain, ""),
          rootDeck: root.length - (manifest === undefined ? 0 : 1) > 0,
        },
      };
    }
    case "deck": {
      const rows = rowsOf(a.domain, a.folder);
      return {
        kind: "deck",
        input: {
          domain: a.domain,
          folder: a.folder,
          rows,
          subfolders: subfoldersOf(a.domain, a.folder),
          total: rows.length,
          truncated: false,
        },
        section: a.section,
      };
    }
    case "engram": {
      const e = ENGRAMS.find(
        (x) => x.domain === a.domain && x.permalink === a.permalink,
      );
      if (e === undefined) throw new Error(`no engram ${a.permalink}`);
      return {
        kind: "engram",
        place: plainPlace(a.domain, a.permalink),
        folder: folderOf(e),
      };
    }
  }
}

const keyOf = (a: StationAddress) => JSON.stringify(a);

/**
 * Every room the walk reaches from the airlock, by its resolved address,
 * and how many deck doors lead to each engram.
 */
function walk() {
  const rooms = new Map<string, StationRoom>();
  const doorTargets = new Map<string, number>();
  const queue: { to: StationAddress; arrival: Arrival | null }[] = [
    { to: { kind: "airlock" }, arrival: null },
  ];
  while (queue.length > 0) {
    const { to, arrival } = queue.shift()!;
    const built = roomFor(load(to), arrival, null);
    const key = keyOf(built.address);
    if (rooms.has(key)) continue;
    rooms.set(key, built);
    const from = built.address;
    for (const f of built.room.fixtures) {
      if (f.kind === "lift") {
        for (const stop of f.stops)
          queue.push({ to: stop.to, arrival: { via: "lift", from } });
      } else if (f.kind === "door" && f.address !== null) {
        const target: StationAddress = { kind: "engram", ...f.address };
        doorTargets.set(
          keyOf(target),
          (doorTargets.get(keyOf(target)) ?? 0) + 1,
        );
        queue.push({ to: target, arrival: { via: "door", from } });
      } else if (f.kind === "exit") {
        queue.push({ to: f.to, arrival: { via: "exit", from } });
      }
    }
  }
  return { rooms, doorTargets };
}

describe("the walk (M3 C7, C12, C28)", () => {
  it("walks from the airlock to every engram and back", () => {
    // Mutation caught: a subfolder missing from its parent's lift, the
    // root deck missing from the bridge, a section unreachable, an exit
    // to the wrong section.
    const engrams: StationAddress[] = ENGRAMS.filter(
      (e) => !isManifestPermalink(e.permalink),
    ).map((e) => ({
      kind: "engram",
      domain: e.domain,
      permalink: e.permalink,
    }));
    expect(engrams.length).toBeGreaterThan(0);
    const { rooms, doorTargets } = walk();

    // The airlock lists every domain, the private one with the key.
    const airlock = rooms.get(keyOf({ kind: "airlock" }));
    const panel = airlock?.room.fixtures.find((f) => f.kind === "lift");
    if (panel?.kind !== "lift") throw new Error("the airlock has no lift");
    for (const d of DOMAINS) {
      const stop = panel.stops.find(
        (s) => s.to.kind === "bridge" && s.to.domain === d.name,
      );
      expect(stop?.key, d.name).toBe(d.private);
    }

    // Every engram is the target of exactly one deck door.
    for (const e of engrams) {
      expect(doorTargets.get(keyOf(e)), keyOf(e)).toBe(1);
      expect(rooms.has(keyOf(e)), keyOf(e)).toBe(true);
    }
    expect(doorTargets.size).toBe(engrams.length);

    // Every engram room's exit leads up to a deck that holds its door,
    // and the player stands in front of that door.
    for (const e of engrams) {
      const room = rooms.get(keyOf(e))!.room;
      const exits = room.fixtures.filter((f) => f.kind === "exit");
      expect(exits, keyOf(e)).toHaveLength(1);
      const exit = exits[0]!;
      if (exit.kind !== "exit") throw new Error("not an exit");
      const up = roomFor(load(exit.to), { from: e }, null);
      expect(up.address.kind, keyOf(e)).toBe("deck");
      const back = up.room.fixtures.find(
        (f) =>
          f.kind === "door" &&
          f.address !== null &&
          sameStation({ kind: "engram", ...f.address }, e),
      );
      expect(back, keyOf(e)).toBeDefined();
      if (back === undefined) continue;
      const w = wallPoint(back.slot);
      const at = arrivalSpawn(up.room, { via: "exit", from: e });
      expect(Math.hypot(at.x - w.x, at.z - w.z), keyOf(e)).toBeCloseTo(
        ARRIVAL_DISTANCE,
      );
    }

    // Every deck's lift reaches its bridge, and every bridge's the
    // airlock.
    const decks = [...rooms.values()].filter((r) => r.address.kind === "deck");
    const bridges = [...rooms.values()].filter(
      (r) => r.address.kind === "bridge",
    );
    expect(decks.length).toBeGreaterThan(0);
    expect(bridges.map((b) => keyOf(b.address)).sort()).toEqual(
      DOMAINS.map((d) => keyOf({ kind: "bridge", domain: d.name })).sort(),
    );
    const stopsOf = (r: StationRoom) => {
      const lift = r.room.fixtures.find((f) => f.kind === "lift");
      return lift?.kind === "lift" ? lift.stops : [];
    };
    for (const d of decks) {
      if (d.address.kind !== "deck") continue;
      const bridge: StationAddress = {
        kind: "bridge",
        domain: d.address.domain,
      };
      expect(
        stopsOf(d).some((s) => sameStation(s.to, bridge)),
        keyOf(d.address),
      ).toBe(true);
    }
    for (const b of bridges) {
      const stops = stopsOf(b);
      expect(
        stops.some(
          (s) => s.to.kind === "airlock" && s.label === LIFT_WORDS.airlock,
        ),
        keyOf(b.address),
      ).toBe(true);
    }

    // Every folder of the station was a deck on the walk.
    const deckFolders = new Set(
      decks.map((d) =>
        d.address.kind === "deck"
          ? `${d.address.domain}:${d.address.folder}`
          : "",
      ),
    );
    for (const folder of foldersOf("station"))
      expect(deckFolders.has(`station:${folder}`), folder).toBe(true);
  }, 60_000);
});
