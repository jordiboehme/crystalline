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
 *
 * The console room's fittings set no text at all (2.6e C20): every
 * interior kind and variant is built in every look with the records
 * cleared first, with the console room's moving parts (the rotor), and
 * neither `textRows` nor a pixel-mark helper may be called. Their recipes take
 * no model context, so `label` and
 * `textPanel`, which need one for a text layer, are out of their reach.
 */

import { describe, expect, it, vi } from "vitest";

import { CURIO_CATALOGUE, CURIO_KINDS } from "../world/curios";
import { HERO_CATALOGUE, HERO_KINDS } from "../world/heroes";
import { PROP_CATALOGUE, PROP_KINDS } from "../world/props";
import { createBuilder } from "./geometry";
import { createKit } from "./kit";
import { LOOK } from "./looks";
import type { KitAt } from "./models/common";
import { buildCurio } from "./models/curios";
import { buildHero } from "./models/heroes";
import { INTERIOR_CATALOGUE, consoleRoom } from "../world/consoleRoom";
import type { InteriorKind } from "../world/types";
import { buildInterior, buildInteriorMovers } from "./models/interior";
import * as pixels from "./models/heroes/pixels";
import { buildProp } from "./models/props";
import { buildDecals } from "./models/decals";
import { MARKS } from "./models/marks";
import { galleryRoom } from "../world/canned";
import type { Decal } from "../world/types";

const recorded = vi.hoisted(() => [] as string[]);

/**
 * The names of the pixel-mark helpers a recipe called (`textBlock`,
 * `markLines`, `pixelPanel`): a mark laid by any of them is text or a
 * picture on a model, even when its rows never pass `textRows` through
 * this wrapper (the helpers call it inside their own module).
 */
const marked = vi.hoisted(() => [] as string[]);

vi.mock("./models/heroes/pixels", async (importOriginal) => {
  const real = await importOriginal<typeof pixels>();
  return {
    ...real,
    textRows: (text: string) => {
      recorded.push(text);
      return real.textRows(text);
    },
    textBlock: (...args: Parameters<typeof real.textBlock>) => {
      marked.push("textBlock");
      return real.textBlock(...args);
    },
    markLines: (...args: Parameters<typeof real.markLines>) => {
      marked.push("markLines");
      return real.markLines(...args);
    },
    pixelPanel: (...args: Parameters<typeof real.pixelPanel>) => {
      marked.push("pixelPanel");
      real.pixelPanel(...args);
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
  const look = LOOK;
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

describe("no text on the fittings but the airlock's word (2.6e C20, M3 C24)", () => {
  it("records the pixel-mark helpers, so the check below is not vacuous", () => {
    // Mutation caught: the wrappers not live, so a mark on a fitting
    // would pass unseen.
    marked.length = 0;
    recorded.length = 0;
    buildEverything();
    expect(marked.length).toBeGreaterThan(0);
    expect(recorded.length).toBeGreaterThan(0);
  });

  it("sets no text and lays no mark on any console room kind or variant, in any look", () => {
    // Mutation caught: a label or a pixel mark on a piece (a caption on
    // the scanner, a number on the inner doors, a figure on a console
    // panel's dial or on the rotor).
    const kinds: InteriorKind[] = [
      "roundel-wall",
      "inner-doors",
      "scanner",
      "console",
    ];
    const pieces = consoleRoom().interior ?? [];
    expect(pieces.some((p) => p.kind === "console")).toBe(true);
    recorded.length = 0;
    marked.length = 0;
    const look = LOOK;
    for (const kind of kinds)
      for (let v = 0; v < INTERIOR_CATALOGUE[kind].variants; v++)
        buildInterior(fresh(), kind, v, look);
    pieces.forEach((p, i) => buildInteriorMovers(p, i, look));
    expect(recorded).toEqual([]);
    expect(marked).toEqual([]);
  });

  it("sets the airlock's word on its outer hatch alone, and nothing on its other fittings (M3 C24)", () => {
    // Mutation caught: a word of the hatch's own spelled past `MARKS`, a
    // second mark on the hatch, or text on a beacon, the iris light or a
    // suit locker.
    const kinds = Object.keys(INTERIOR_CATALOGUE) as InteriorKind[];
    expect(kinds.length).toBe(8);
    for (const kind of kinds.slice(4))
      for (let v = 0; v < INTERIOR_CATALOGUE[kind].variants; v++) {
        recorded.length = 0;
        marked.length = 0;
        buildInterior(fresh(), kind, v, LOOK);
        expect(recorded, kind).toEqual(
          kind === "outer-hatch" ? [MARKS.airlockWord] : [],
        );
        expect(marked, kind).toEqual(
          kind === "outer-hatch" ? ["pixelPanel"] : [],
        );
      }
  });
});

describe("readable text set by the deck and bay stencils (2.7 C19)", () => {
  it("sets no string but the allowed ones for any deck, bay or letter, on a wall or the floor", () => {
    // Mutation caught: a stencil set from a template string ("DECK 23") or
    // a number set whole ("23"), which is on no list.
    const decals: Decal[] = [];
    for (let n = 1; n <= 99; n++)
      for (const letter of [0, 1, 2, 3, 4])
        for (const lines of [1, 2] as const)
          decals.push({
            kind: "stencil",
            on: lines === 2 ? "wall" : "floor",
            x: 2.5,
            y: lines === 2 ? 3 : 2.5,
            turn: 0,
            along: 0,
            h: lines === 2 ? 1.5 : 0,
            width: lines === 2 ? 0.9 : 1.0,
            length: 0.3,
            variant: 0,
            seed: 0,
            stencil: { deck: n, bay: n, letter, lines },
          });
    // The airlock's floor stencil reads a word (M3 C24).
    decals.push({
      kind: "stencil",
      on: "floor",
      x: 2.5,
      y: 2.5,
      turn: 0,
      along: 0,
      h: 0,
      width: 2.0,
      length: 0.5,
      variant: 0,
      seed: 0,
      word: "cycle",
    });
    recorded.length = 0;
    const b = createBuilder();
    buildDecals((f) => createKit(b, f), b, { ...galleryRoom(), decals });
    expect(recorded).toContain(MARKS.cycleWord);
    expect(recorded.length).toBeGreaterThan(0);
    const list = readable();
    for (const t of new Set(recorded))
      expect(list.has(t), JSON.stringify(t)).toBe(true);
  });
});
