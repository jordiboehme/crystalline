/**
 * Which variant a machine, a terminal or a decor kind draws (2.7 Variation).
 *
 * A kind's variant is one of `VARIANT_COUNTS[kind]` recipes a model file
 * picks between (`render/models/machines.ts`, `terminal.ts`, `decor.ts`);
 * variant 0 is always the kind's first model, part for part (2.7 C1), and
 * a count of 1 means that model alone. A leaf module (2.7 Global
 * Constraints): it imports `core/seed.ts` and `types.ts` only, so the
 * generator side, the render side and the tests can all read it without
 * pulling in either.
 *
 * A pick never draws from an existing stream (2.7 C4): a machine's variant
 * comes from its tag alone, so the same tag is the same machine and the
 * same variant in every room (2.7 Review Focus 2); a terminal's from its own
 * seed, so two terminals of a room may differ; a decor kind's from the
 * room's seed and its own kind, so every piece of that kind in one room
 * shares its variant (2.7 C4: a set of mismatched chairs would read as a
 * mistake, not as variety).
 *
 * `machineModelSeed` is the seed a machine's own random details (tool
 * lengths, bottle heights, LED colours) are drawn from: the tag alone, never
 * the room, so a tag's machine is identical in every room (2.7 C5). `tagAccent`
 * picks the second accent a machine's model bakes into one trim part
 * (2.7 C10), one of `ACCENT_COUNT` accents every look carries (2.7 C7).
 */

import { seedFor } from "../core/seed";
import type { DecorKind, MachineKind } from "./types";

/**
 * How many variants each kind's recipe builds (2.7 C2): the terminal, every
 * machine kind and every decor kind. A count of 1 is variant 0 alone; a
 * kind's count is raised only once its extra recipes exist, and
 * `variants.test.ts` pins every count between 1 and 4.
 */
export const VARIANT_COUNTS: {
  terminal: number;
  machine: Record<MachineKind, number>;
  decor: Record<DecorKind, number>;
} = {
  terminal: 3,
  machine: {
    workbench: 3,
    "lab-bench": 3,
    "server-rack": 3,
    "cryo-pod": 3,
    fabricator: 3,
    hydroponics: 3,
    "nav-table": 3,
    "comms-array": 3,
    "reactor-coupling": 3,
    "cargo-loader": 3,
    "med-scanner": 3,
    containment: 3,
  },
  decor: {
    "command-console": 3,
    "captain-chair": 2,
    "round-table": 2,
    "council-chair": 3,
    generator: 3,
    "pipe-run": 2,
    "shelf-row": 3,
    "lab-island": 2,
    "specimen-tank": 3,
  },
};

/**
 * A machine's variant, from its tag alone (2.7 C4): the same tag draws the
 * same variant in every room it appears in, alongside the same `kind`
 * (`generate.ts`'s own `seedFor("tag-kind", tag)` pick) and the same model
 * seed (`machineModelSeed`).
 */
export function machineVariant(tag: string, kind: MachineKind): number {
  return seedFor("tag-variant", tag) % VARIANT_COUNTS.machine[kind];
}

/**
 * A terminal's variant, from its own seed (2.7 C4): two terminals of one
 * room may differ, unlike a machine's or a decor kind's variant.
 */
export function terminalVariant(seed: number): number {
  return seedFor(seed, "variant") % VARIANT_COUNTS.terminal;
}

/**
 * A decor kind's variant in one room, shared by every piece of that kind
 * (2.7 C4): keyed by the room's seed and the kind alone, never by a piece's
 * own seed, so six council chairs, both specimen tanks or an archive's shelf
 * rows all match.
 */
export function decorVariant(roomSeed: number, kind: DecorKind): number {
  return seedFor(roomSeed, "decor-variant", kind) % VARIANT_COUNTS.decor[kind];
}

/**
 * The seed a machine's random details are drawn from: its tag alone (2.7
 * C5), so a tag's machine is identical part for part in every room it
 * stands in, down to its tool lengths, bottle heights and LED colours.
 * `buildMachine` reads this instead of the fixture's own `seed`.
 */
export function machineModelSeed(tag: string): number {
  return seedFor("tag-model", tag);
}

/**
 * How many accents every look carries (2.7 C7): five, one per colour family,
 * each at least 0.25 (RGB distance) from its look's door, portal and
 * cross-domain portal colours.
 */
export const ACCENT_COUNT = 5;

/**
 * The index of a machine's second accent in its look's accent set (2.7
 * C10): baked into one trim part of the machine's static mesh, from the
 * tag alone, so the same tag keeps the same second accent in every room.
 * The tag's own hue keeps its glow and its strip.
 */
export function tagAccent(tag: string): number {
  return seedFor("tag-accent", tag) % ACCENT_COUNT;
}
