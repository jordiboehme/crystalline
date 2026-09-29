/**
 * What a change frame means for the station (M4 C13, C14, C18): which of
 * the game's cache keys it makes stale, whether it concerns the place the
 * player stands in, and where a move sends the current engram.
 */

import { describe, expect, it } from "vitest";

import { engramDetailKey } from "../../api/engram";
import type { ChangeEvent, EngramChange } from "../../api/events";
import { graphKey, NEIGHBORHOOD_DEPTH } from "../../api/graph";
import { keysFor } from "../../events/invalidation";
import { CANNED_WORKSHOP } from "../world/canned";
import { generateRoom } from "../world/generate";
import type { Fixture, RoomSpec } from "../world/types";
import {
  changeKeys,
  concerns,
  gameInboundPrefix,
  movedTo,
  watchOf,
  type Watch,
} from "./changes";
import { inboundKey } from "./source";

/** True when `key` is a prefix of `full`, the way TanStack matches a query key. */
function covers(key: readonly unknown[], full: readonly unknown[]): boolean {
  return key.length <= full.length && key.every((v, i) => v === full[i]);
}

/** True when some key of `keys` covers `full`. */
function anyCovers(
  keys: (readonly unknown[])[] | "everything",
  full: readonly unknown[],
): boolean {
  return keys !== "everything" && keys.some((k) => covers(k, full));
}

const change = (o: Partial<EngramChange> = {}): ChangeEvent => ({
  event: "engram",
  change: {
    domain: "eng",
    permalink: "notes/a",
    path: "notes/a.md",
    kind: "modified",
    from: null,
    checksum: "1",
    actor: null,
    draftOf: null,
    ...o,
  },
});

/** A generated room carrying exactly `fixtures`, in the domain `eng`. */
function roomWith(fixtures: Fixture[]): RoomSpec {
  return { ...generateRoom(CANNED_WORKSHOP), domain: "eng", fixtures };
}

const SLOT = { x: 1, y: 1, side: "n" } as const;

describe("what a frame concerns (M4 C14)", () => {
  it("watches an engram room's domain and the domains its ways reach", () => {
    // Mutation caught: the ways' domains left out (a cross-domain door to a
    // deleted engram would never seal), the domain frame not matched.
    const watch: Watch = {
      kind: "engram",
      domain: "eng",
      permalink: "notes/a",
      wayDomains: ["ops"],
    };
    expect(concerns(change(), watch)).toBe(true);
    expect(concerns(change({ domain: "ops", permalink: "x" }), watch)).toBe(
      true,
    );
    expect(concerns(change({ domain: "far", permalink: "x" }), watch)).toBe(
      false,
    );
    expect(
      concerns(
        { event: "domain", change: { domain: "ops", actor: null } },
        watch,
      ),
    ).toBe(true);
    expect(
      concerns(
        { event: "domain", change: { domain: "far", actor: null } },
        watch,
      ),
    ).toBe(false);
    expect(concerns({ event: "reset" }, watch)).toBe(true);
  });

  it("watches one domain from its bridge and its decks", () => {
    // Mutation caught: a bridge or a deck watching nothing, or every domain.
    const bridge = watchOf({ kind: "bridge", domain: "eng" }, null, false);
    const deck = watchOf(
      { kind: "deck", domain: "eng", folder: "notes", section: null },
      null,
      false,
    );
    expect(bridge).toEqual({ kind: "domain", domain: "eng" });
    expect(deck).toEqual({ kind: "domain", domain: "eng" });
    expect(concerns(change(), bridge)).toBe(true);
    expect(concerns(change({ domain: "ops" }), bridge)).toBe(false);
    expect(
      concerns(
        { event: "domain", change: { domain: "eng", actor: null } },
        deck,
      ),
    ).toBe(true);
    expect(
      concerns(
        { event: "domain", change: { domain: "ops", actor: null } },
        deck,
      ),
    ).toBe(false);
  });

  it("watches the listing from the airlock", () => {
    // Mutation caught: every engram frame taken (a re-check per edit
    // anywhere), or the MANIFEST row missed.
    const airlock: Watch = { kind: "airlock" };
    expect(watchOf({ kind: "airlock" }, null, false)).toEqual(airlock);
    expect(concerns(change(), airlock)).toBe(false);
    expect(concerns(change({ kind: "added" }), airlock)).toBe(true);
    expect(concerns(change({ kind: "deleted" }), airlock)).toBe(true);
    expect(
      concerns(change({ permalink: "manifest", path: "MANIFEST.md" }), airlock),
    ).toBe(true);
    expect(
      concerns(
        { event: "domain", change: { domain: "any", actor: null } },
        airlock,
      ),
    ).toBe(true);
    expect(concerns({ event: "reset" }, airlock)).toBe(true);
  });

  it("watches nothing from the console room", () => {
    // Mutation caught: the room walked in from re-checked while inside.
    expect(
      watchOf({ kind: "engram", domain: "eng", permalink: "a" }, null, true),
    ).toEqual({ kind: "none" });
    expect(watchOf({ kind: "airlock" }, null, true)).toEqual({ kind: "none" });
    expect(watchOf(null, null, false)).toEqual({ kind: "none" });
    expect(concerns({ event: "reset" }, { kind: "none" })).toBe(false);
    expect(concerns(change(), { kind: "none" })).toBe(false);
  });

  it("builds the watch from the room's ways, sorted and unique", () => {
    // Mutation caught: sealed ways (address null) read, duplicates kept,
    // the exit's station address ignored.
    const fixtures: Fixture[] = [
      {
        kind: "door",
        slot: SLOT,
        style: "sliding",
        relType: "depends_on",
        label: "Z",
        address: { domain: "zed", permalink: "z" },
        sealedLabel: null,
        seed: 1,
      },
      {
        kind: "door",
        slot: SLOT,
        style: "sliding",
        relType: "depends_on",
        label: "SEALED",
        address: null,
        sealedLabel: "?FILE NOT FOUND",
        seed: 2,
      },
      {
        kind: "portal",
        slot: SLOT,
        label: "A",
        address: { domain: "alpha", permalink: "a" },
        crossDomain: true,
        sealedLabel: null,
        seed: 3,
      },
      {
        kind: "portal",
        slot: SLOT,
        label: "A AGAIN",
        address: { domain: "alpha", permalink: "b" },
        crossDomain: true,
        sealedLabel: null,
        seed: 4,
      },
      {
        kind: "portal",
        slot: SLOT,
        label: "SEALED",
        address: null,
        crossDomain: true,
        sealedLabel: "NO ROUTE",
        seed: 5,
      },
      {
        kind: "hatch",
        slot: SLOT,
        label: "O",
        address: { domain: "ops", permalink: "o" },
        seed: 6,
      },
      {
        kind: "door",
        slot: SLOT,
        style: "sliding",
        relType: "depends_on",
        label: "OWN",
        address: { domain: "eng", permalink: "b" },
        sealedLabel: null,
        seed: 7,
      },
      {
        kind: "exit",
        slot: SLOT,
        label: "EXIT",
        to: { kind: "bridge", domain: "mid" },
        seed: 8,
      },
      {
        kind: "exit",
        slot: SLOT,
        label: "OUT",
        to: { kind: "airlock" },
        seed: 9,
      },
      { kind: "placard", slot: SLOT, lines: ["ELSEWHERE"] },
    ];
    const watch = watchOf(
      { kind: "engram", domain: "eng", permalink: "notes/a" },
      roomWith(fixtures),
      false,
    );
    expect(watch).toEqual({
      kind: "engram",
      domain: "eng",
      permalink: "notes/a",
      wayDomains: ["alpha", "mid", "ops", "zed"],
    });
    // Before the room is built there are no ways to read.
    expect(
      watchOf(
        { kind: "engram", domain: "eng", permalink: "notes/a" },
        null,
        false,
      ),
    ).toEqual({
      kind: "engram",
      domain: "eng",
      permalink: "notes/a",
      wayDomains: [],
    });
  });

  it("follows a move of the current engram only", () => {
    // Mutation caught: `permalink` compared instead of `from.permalink`,
    // another domain's move taken.
    const moved = change({
      kind: "moved",
      permalink: "b",
      from: { path: "notes/a.md", permalink: "notes/a" },
    });
    expect(
      movedTo(moved, { kind: "engram", domain: "eng", permalink: "notes/a" }),
    ).toEqual({ kind: "engram", domain: "eng", permalink: "b" });
    expect(
      movedTo(moved, { kind: "engram", domain: "eng", permalink: "b" }),
    ).toBeNull();
    expect(
      movedTo(moved, { kind: "engram", domain: "eng", permalink: "notes/c" }),
    ).toBeNull();
    expect(
      movedTo(moved, { kind: "engram", domain: "ops", permalink: "notes/a" }),
    ).toBeNull();
    expect(movedTo(moved, { kind: "bridge", domain: "eng" })).toBeNull();
    expect(
      movedTo(change(), {
        kind: "engram",
        domain: "eng",
        permalink: "notes/a",
      }),
    ).toBeNull();
    expect(movedTo(moved, null)).toBeNull();
  });

  it("makes the game's inbound pages stale with Fluid's own keys", () => {
    // Mutation caught: Fluid's table not reused, the inbound prefix missing
    // for a domain frame, a reset not answered "everything".
    const prefix = gameInboundPrefix("eng");
    expect(prefix.length).toBeGreaterThan(0);
    expect(inboundKey("eng", "notes/a").slice(0, prefix.length)).toEqual(
      prefix,
    );
    expect(prefix).not.toEqual(gameInboundPrefix("ops"));

    const engram = change();
    const fluid = keysFor(engram);
    expect(fluid).not.toBe("everything");
    if (fluid === "everything") return;
    expect(fluid.length).toBeGreaterThan(0);
    const keys = changeKeys(engram);
    expect(keys).not.toBe("everything");
    if (keys === "everything") return;
    expect(keys.slice(0, fluid.length)).toEqual(fluid);

    const domain: ChangeEvent = {
      event: "domain",
      change: { domain: "ops", actor: null },
    };
    const fluidDomain = keysFor(domain);
    expect(fluidDomain).not.toBe("everything");
    if (fluidDomain === "everything") return;
    expect(changeKeys(domain)).toEqual([
      ...fluidDomain,
      gameInboundPrefix("ops"),
    ]);

    expect(changeKeys({ event: "reset" })).toBe("everything");
  });

  it("makes the current room's own detail and graph stale when a neighbour changes (I1)", () => {
    // Mutation caught: an engram frame reaching only the changed engram's
    // own keys (Fluid's table), so the room beside it re-reads a fresh
    // cache and never sees its door's target deleted, moved or retitled;
    // or the prefixes built for every domain rather than the frame's.
    const keys = changeKeys(
      change({ permalink: "notes/b", path: "notes/b.md" }),
    );
    const own = [
      engramDetailKey("eng", "notes/a"),
      graphKey("eng", "notes/a", NEIGHBORHOOD_DEPTH),
    ];
    const far = [
      engramDetailKey("ops", "notes/a"),
      graphKey("ops", "notes/a", NEIGHBORHOOD_DEPTH),
    ];
    expect(own.length).toBeGreaterThan(0);
    for (const key of own) expect(anyCovers(keys, key)).toBe(true);
    for (const key of far) expect(anyCovers(keys, key)).toBe(false);
    // Fluid's own table alone does not cover them: the rows are the game's.
    const fluid = keysFor(change({ permalink: "notes/b", path: "notes/b.md" }));
    for (const key of own) expect(anyCovers(fluid, key)).toBe(false);
  });

  it("makes every domain's inbound pages stale on an engram frame (I2)", () => {
    // Mutation caught: only the frame's own domain's inbound pages made
    // stale, so a new link from another domain never shows as a hatch
    // until the page ages out.
    const keys = changeKeys(
      change({ domain: "far", permalink: "x", path: "x.md" }),
    );
    expect(anyCovers(keys, inboundKey("far", "x"))).toBe(true);
    expect(anyCovers(keys, inboundKey("eng", "notes/a"))).toBe(true);
    // A domain frame keeps to its own domain.
    const domain = changeKeys({
      event: "domain",
      change: { domain: "far", actor: null },
    });
    expect(anyCovers(domain, inboundKey("far", "x"))).toBe(true);
    expect(anyCovers(domain, inboundKey("eng", "notes/a"))).toBe(false);
  });
});
