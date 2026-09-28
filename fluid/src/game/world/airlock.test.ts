/**
 * The airlock (M3 C24, C29): the round hand-built room, its lift to every
 * domain's bridge and its domain directory.
 */
import { describe, expect, it } from "vitest";

import { seedFor } from "../core/seed";
import { GAME_VERSION } from "../version";
import {
  AIRLOCK_CEILING,
  AIRLOCK_GRID,
  airlockRoom,
  type AirlockInput,
} from "./airlock";
import airlockSource from "./airlock.ts?raw";
import { CANNED_DOMAINS } from "./canned";
import airlockGolden from "./golden/airlock.json?raw";
import { isFloor } from "./layout";
import { LIFT_WORDS, SCREEN_LINES, byLabel, moreLine } from "./lifts";
import type { Fixture, RoomSpec } from "./types";

type Domains = NonNullable<AirlockInput["domains"]>;

/** `n` domains `d00`, `d01`, ..., every third one private. */
function domainsOf(n: number): Domains {
  return Array.from({ length: n }, (_, i) => ({
    name: `d${String(i).padStart(2, "0")}`,
    private: i % 3 === 0,
  }));
}

/** The room's lift fixture; the test fails when there is none. */
function liftOf(room: RoomSpec): Extract<Fixture, { kind: "lift" }> {
  const lift = room.fixtures.find((f) => f.kind === "lift");
  if (lift?.kind !== "lift") throw new Error("no lift");
  return lift;
}

/** The room's screen fixture; the test fails when there is none. */
function screenOf(room: RoomSpec): Extract<Fixture, { kind: "screen" }> {
  const screen = room.fixtures.find((f) => f.kind === "screen");
  if (screen?.kind !== "screen") throw new Error("no screen");
  return screen;
}

describe("the airlock (M3 C24)", () => {
  it("is round, lit and bare", () => {
    // Mutation caught: a square grid, the dressing run on the airlock, a
    // dim or flickering zone, a floor cell left outside every zone.
    const room = airlockRoom({ domains: CANNED_DOMAINS, here: null });
    expect(AIRLOCK_GRID).toEqual([
      "  ....  ",
      " ...... ",
      "........",
      "........",
      "........",
      "........",
      " ...... ",
      "  ....  ",
    ]);
    expect(room.grid).toEqual(AIRLOCK_GRID);
    expect([room.width, room.depth]).toEqual([8, 8]);
    expect(room.entrance).toEqual({ x: 3, y: 7 });
    expect(room.ceiling).toBe(AIRLOCK_CEILING);
    expect(room.space).toBe("airlock");
    expect(room.condition).toBe("clean");
    expect(room.seed).toBe(seedFor(GAME_VERSION, "airlock"));
    expect([room.domain, room.permalink]).toEqual(["", ""]);
    expect(room.props).toEqual([]);
    expect(room.heroes).toEqual([]);
    expect(room.curios).toEqual([]);
    expect(room.decals).toEqual([]);
    expect(room.decor).toEqual([]);
    expect(room.scaffold).toEqual([]);
    expect(room.fixtures.map((f) => f.kind)).toEqual(["lift", "screen"]);
    expect(room.lights.length).toBeGreaterThan(0);
    for (const z of room.lights) {
      expect(z.special).toBe("steady");
      expect(z.level).toBeGreaterThanOrEqual(208);
    }
    let floor = 0;
    for (let y = 0; y < room.depth; y++)
      for (let x = 0; x < room.width; x++) {
        if (!isFloor(room.grid, x, y)) continue;
        floor += 1;
        const lit = room.lights.some(
          (z) => x >= z.x0 && x < z.x1 && y >= z.y0 && y < z.y1,
        );
        expect(lit, `cell ${String(x)},${String(y)}`).toBe(true);
      }
    expect(floor).toBe(52);
  });

  it("lists every domain on the lift and on the directory, keys on the private ones", () => {
    // Mutation caught: the directory not capped, the lift capped too, the
    // cap one line off, the key on the wrong line.
    const three: Domains = [
      { name: "vault", private: true },
      { name: "Atlas", private: false },
      { name: "mill", private: true },
    ];
    const small = airlockRoom({ domains: three, here: "mill" });
    const lift = liftOf(small);
    expect(lift.slot).toEqual({ x: 3, y: 7, side: "s" });
    expect(lift.note).toBeNull();
    expect(lift.stops).toEqual([
      {
        label: "Atlas",
        to: { kind: "bridge", domain: "Atlas" },
        key: false,
        here: false,
      },
      {
        label: "mill",
        to: { kind: "bridge", domain: "mill" },
        key: true,
        here: true,
      },
      {
        label: "vault",
        to: { kind: "bridge", domain: "vault" },
        key: true,
        here: false,
      },
    ]);
    const screen = screenOf(small);
    expect(screen.slot).toEqual({ x: 3, y: 0, side: "n" });
    expect(screen.lines).toEqual([
      LIFT_WORDS.airlock,
      "Atlas",
      "mill",
      "vault",
    ]);
    expect(screen.keys).toEqual([2, 3]);

    // Eleven names and the heading fill the screen exactly; twelve do not.
    const eleven = screenOf(
      airlockRoom({ domains: domainsOf(11), here: null }),
    );
    expect(eleven.lines).toHaveLength(SCREEN_LINES);
    expect(eleven.lines.at(-1)).toBe("d10");
    const twelve = screenOf(
      airlockRoom({ domains: domainsOf(12), here: null }),
    );
    expect(twelve.lines).toHaveLength(SCREEN_LINES);
    expect(twelve.lines.at(-1)).toBe(moreLine(2));

    const thirty = domainsOf(30);
    const big = airlockRoom({ domains: thirty, here: null });
    const names = thirty.map((d) => d.name).sort(byLabel);
    const directory = screenOf(big);
    expect(directory.lines).toEqual([
      LIFT_WORDS.airlock,
      ...names.slice(0, 10),
      moreLine(20),
    ]);
    expect(directory.lines).toHaveLength(SCREEN_LINES);
    // d00, d03, d06 and d09 are private: lines 1, 4, 7 and 10.
    expect(directory.keys).toEqual([1, 4, 7, 10]);
    expect(liftOf(big).stops.map((s) => s.label)).toEqual(names);
  });

  it("says when the listing failed", () => {
    // Mutation caught: a failed listing drawn as an empty station (no
    // line), the note missing from the lift (the lift's retry reads it).
    const room = airlockRoom({ domains: null, here: null });
    const lift = liftOf(room);
    expect(lift.stops).toEqual([]);
    expect(lift.note).toBe(LIFT_WORDS.domainError);
    const screen = screenOf(room);
    expect(screen.lines).toEqual([LIFT_WORDS.airlock, LIFT_WORDS.domainError]);
    expect(screen.keys).toEqual([]);
    // An empty listing is no failure: the heading alone, no note.
    const empty = airlockRoom({ domains: [], here: null });
    expect(liftOf(empty).note).toBeNull();
    expect(screenOf(empty).lines).toEqual([LIFT_WORDS.airlock]);
  });

  it("keeps airlock.ts on the generator side: no ui, render or session imports", () => {
    // Mutation caught: an import of `ui/`, `render/` or a session module
    // into the generator side.
    expect(airlockSource).not.toMatch(
      /\b(?:from|import)\s*\(?\s*["'](?:\.\.\/(?:ui|render)(?:\/[^"']*)?|\.\/(?:move|malfunction|interact|box|arrival|station))["']/,
    );
  });

  it("builds the same airlock twice and matches the golden byte for byte", () => {
    // Mutation caught: any change to the builder's output, key order included.
    const input: AirlockInput = { domains: CANNED_DOMAINS, here: null };
    expect(airlockRoom(input)).toEqual(airlockRoom(input));
    expect(JSON.stringify(airlockRoom(input), null, 2) + "\n").toBe(
      airlockGolden,
    );
  });
});
