/**
 * A room's neighbours, as the discovery pass reads them (2.6f C5 to C8):
 * the rooms its doors, portals and hatches lead to. A neighbour is never
 * generated: its seed and archetype give its raw hero (`rawHero`) and raw
 * curios (`rawCurios`), what its draws would place where everything fits,
 * and `nearOf` gathers them into the sets the hero and curio passes skip.
 * Pure and seeded like the rest of the generator, with no memory of what a
 * player has seen (2.6f C11). `generate.ts` finds the neighbours
 * (`neighboursOf`); this module never imports it.
 *
 * This is the generator side: it imports `curios.ts`, `heroes.ts`,
 * `sites.ts` and `types.ts`, and never `generate.ts`, `move.ts`,
 * `interact.ts`, `malfunction.ts` or anything under `render/`
 * (`neighbours.test.ts` keeps it so).
 */

import { curioDrawsOf, rawCurios } from "./curios";
import { heroDrawsOf, rawHero } from "./heroes";
import type { Near } from "./sites";
import type { Archetype, CurioKind, HeroKind } from "./types";

/**
 * One neighbour: its room seed and its archetype, null when its type is
 * unknown (2.6f C5).
 */
export interface Neighbour {
  seed: number;
  archetype: Archetype | null;
}

/**
 * The neighbour sets of the room with seed `seed` (2.6f C6, C8): each
 * neighbour's raw hero, read as a one-hero hall (its size is unknown), and
 * its raw curios; the `below` sets only from neighbours with a lower seed.
 * A neighbour with the room's own seed is the room itself and is left out.
 */
export function nearOf(seed: number, neighbours: readonly Neighbour[]): Near {
  const heroes = new Set<HeroKind>();
  const heroesBelow = new Set<HeroKind>();
  const curios = new Set<CurioKind>();
  const curiosBelow = new Set<CurioKind>();
  for (const n of neighbours) {
    if (n.seed === seed) continue;
    const below = n.seed < seed;
    const hero = rawHero(heroDrawsOf(n.seed, 1), n.archetype);
    if (hero !== null) {
      heroes.add(hero);
      if (below) heroesBelow.add(hero);
    }
    for (const c of rawCurios(curioDrawsOf(n.seed), n.archetype)) {
      curios.add(c);
      if (below) curiosBelow.add(c);
    }
  }
  return { heroes, heroesBelow, curios, curiosBelow };
}
