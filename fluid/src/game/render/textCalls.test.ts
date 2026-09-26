/**
 * The readable-text guard at run time (2.6d C16): every string a hero,
 * curio or prop recipe hands `textRows` while it builds is on the
 * allowlist `heroText.test.ts` keeps. The static scans there read the
 * sources and see literals; this one records the calls themselves, so a
 * constant, a computed string, a template or a title defined outside the
 * recipe files is caught whatever its case. `textRows` is wrapped with
 * `vi.mock` (hoisted per file, so this guard has a file of its own): the
 * real module stays, and the wrapper records each argument before it sets
 * it. Every kind and variant of the three families is built, in every
 * look, at turn 0. Mutation caught: a recipe that sets a lower-case or
 * mixed-case constant (`textRows(LABEL)` with `LABEL = "Some word"`), or
 * a template built from allowed parts.
 */

import { describe, expect, it, vi } from "vitest";

import { CURIO_CATALOGUE, CURIO_KINDS } from "../world/curios";
import { HERO_CATALOGUE, HERO_KINDS } from "../world/heroes";
import { PROP_CATALOGUE, PROP_KINDS } from "../world/props";
import { createBuilder } from "./geometry";
import { createKit } from "./kit";
import { LOOKS } from "./looks";
import type { KitAt } from "./models/common";
import { buildCurio } from "./models/curios";
import { buildHero } from "./models/heroes";
import * as pixels from "./models/heroes/pixels";
import { buildProp } from "./models/props";

const recorded = vi.hoisted(() => [] as string[]);

vi.mock("./models/heroes/pixels", async (importOriginal) => {
  const real = await importOriginal<typeof pixels>();
  return {
    ...real,
    textRows: (text: string) => {
      recorded.push(text);
      return real.textRows(text);
    },
  };
});

/**
 * The allowlist, read from `heroText.test.ts`'s own `READABLE` set, so the
 * guard keeps one list: the double-quoted strings between `const READABLE
 * = new Set([` and its `]);`, comments stripped.
 */
function readable(): ReadonlySet<string> {
  const src = Object.values(
    import.meta.glob<string>("./heroText.test.ts", {
      query: "?raw",
      import: "default",
      eager: true,
    }),
  )[0];
  if (src === undefined) throw new Error("no heroText.test.ts");
  const start = src.indexOf("const READABLE = new Set([");
  const end = src.indexOf("]);", start);
  if (start === -1 || end === -1) throw new Error("no READABLE set");
  const body = src.slice(start, end).replace(/\/\/.*$/gm, "");
  return new Set([...body.matchAll(/"([^"]*)"/g)].map((m) => m[1] ?? ""));
}

/** A kit factory on a fresh builder, thrown away after one build. */
const fresh = (): KitAt => {
  const b = createBuilder();
  return (f) => createKit(b, f);
};

/** Builds every kind and variant of the three families in every look. */
function buildEverything(): void {
  for (const look of Object.values(LOOKS)) {
    for (const kind of HERO_KINDS)
      for (let v = 0; v < HERO_CATALOGUE[kind].variants; v++)
        buildHero(fresh(), kind, v, look);
    for (const kind of CURIO_KINDS)
      for (let v = 0; v < CURIO_CATALOGUE[kind].variants; v++)
        buildCurio(fresh(), kind, v, look);
    for (const kind of PROP_KINDS)
      for (let v = 0; v < PROP_CATALOGUE[kind].variants; v++)
        buildProp(fresh(), kind, v, { look });
  }
}

describe("readable text set while the recipes build", () => {
  it("reads a non-empty allowlist holding the station's titles", () => {
    // Mutation caught: the parse finding nothing, which would make the
    // check below vacuous.
    const list = readable();
    expect(list.has("TILEFALL")).toBe(true);
    expect(list.size).toBeGreaterThan(20);
  });

  it("sets no string but the allowed ones, in any family, kind or variant", () => {
    // Nothing is cleared first: a recipe that sets its rows once at
    // import time was recorded then, and is checked too.
    buildEverything();
    // The cabinets' titles at least go through the wrapper, so the mock
    // is live and the check below is not vacuous.
    expect(recorded).toContain("TILEFALL");
    const list = readable();
    for (const t of new Set(recorded))
      expect(list.has(t), JSON.stringify(t)).toBe(true);
  });
});
