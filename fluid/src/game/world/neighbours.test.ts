/**
 * The neighbour sets (2.6f C5 to C8): what a room's neighbours draw raw,
 * from their seeds and archetypes alone, and who may keep a solo draw.
 */

import { describe, expect, it } from "vitest";

import { seedFor } from "../core/seed";
import { curioDrawsOf, rawCurios } from "./curios";
import { heroDrawsOf, rawHero } from "./heroes";
import { nearOf, type Neighbour } from "./neighbours";

const SOURCES = import.meta.glob<string>("./neighbours.ts", {
  query: "?raw",
  import: "default",
  eager: true,
});

describe("nearOf", () => {
  const seeds = Array.from({ length: 400 }, (_, i) => seedFor("near-test", i));

  it("holds exactly the neighbours' raw heroes and curios", () => {
    // Mutation caught: a neighbour read with its full cap, its any draw
    // counted though its pool slot took, or the curios' archetype dropped.
    const ns: Neighbour[] = seeds
      .slice(1, 7)
      .map((s, i) => ({ seed: s, archetype: i % 2 === 0 ? "lab" : null }));
    const near = nearOf(seeds[0] ?? 0, ns);
    const heroes = new Set(
      ns.flatMap((n) => {
        const k = rawHero(heroDrawsOf(n.seed, 1), n.archetype);
        return k === null ? [] : [k];
      }),
    );
    const curios = new Set(
      ns.flatMap((n) => rawCurios(curioDrawsOf(n.seed), n.archetype)),
    );
    expect(near.heroes).toEqual(heroes);
    expect(near.curios).toEqual(curios);
    expect(heroes.size + curios.size).toBeGreaterThan(0);
  });

  it("keeps in the below sets only what a lower-seeded neighbour draws (2.6f C8)", () => {
    // Mutation caught: the comparison turned round, or every neighbour
    // counted as below. Twelve neighbours, so the kinds drawn below and
    // above differ (with hundreds, both sides would draw every kind and
    // the check could not tell them apart).
    const mine = seeds[0] ?? 0;
    const ns: Neighbour[] = seeds
      .slice(1, 13)
      .map((s) => ({ seed: s, archetype: "engineering" }));
    const near = nearOf(mine, ns);
    const below = ns.filter((n) => n.seed < mine);
    const above = ns.filter((n) => n.seed > mine);
    expect(below.length).toBeGreaterThan(0);
    expect(above.length).toBeGreaterThan(0);
    const heroesOf = (list: Neighbour[]) =>
      new Set(
        list.flatMap((n) => {
          const k = rawHero(heroDrawsOf(n.seed, 1), n.archetype);
          return k === null ? [] : [k];
        }),
      );
    const curiosOf = (list: Neighbour[]) =>
      new Set(
        list.flatMap((n) => rawCurios(curioDrawsOf(n.seed), n.archetype)),
      );
    expect(heroesOf(below)).not.toEqual(heroesOf(above));
    expect(heroesOf(below)).not.toEqual(heroesOf(ns));
    expect(near.heroesBelow).toEqual(heroesOf(below));
    expect(near.curiosBelow).toEqual(curiosOf(below));
    for (const k of near.heroesBelow) expect(near.heroes.has(k)).toBe(true);
    for (const k of near.curiosBelow) expect(near.curios.has(k)).toBe(true);
  });

  it("never lists a neighbour with the room's own seed", () => {
    // Mutation caught: the room skipping its own raw picks. The seed is
    // one whose own raw hero is known, so the check is not vacuous.
    const mine = seeds.find((s) => rawHero(heroDrawsOf(s, 1), "lab") !== null);
    if (mine === undefined) throw new Error("no seed with a raw hero");
    const near = nearOf(mine, [{ seed: mine, archetype: "lab" }]);
    expect(near.heroes.size + near.curios.size).toBe(0);
  });

  it("keeps neighbours.ts on the generator side and off generate.ts", () => {
    // Mutation caught: an import of generate, move, interact,
    // malfunction or anything under render.
    const src = Object.values(SOURCES)[0] ?? "";
    expect(src.length).toBeGreaterThan(0);
    expect(
      /\b(?:from|import)\s*\(?\s*["'](?:\.\/(?:move|generate|interact|malfunction)|\.\.\/render(?:\/[^"']*)?)["']/.test(
        src,
      ),
    ).toBe(false);
  });
});
