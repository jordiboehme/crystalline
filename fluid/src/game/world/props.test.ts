import { describe, expect, it } from "vitest";

import { FOOTPRINTS } from "./footprints";
import { EXTRAS, FILLER, PALETTES, PROP_CATALOGUE, PROP_KINDS } from "./props";
import type { Archetype, Condition, PropKind } from "./types";

const ARCHETYPES = Object.keys({
  bridge: true,
  council: true,
  engineering: true,
  archive: true,
  lab: true,
} satisfies Record<Archetype, true>) as Archetype[];

describe("the prop catalogue", () => {
  it("lists every kind once, as PROP_CATALOGUE does", () => {
    expect(new Set(PROP_KINDS).size).toBe(PROP_KINDS.length);
    expect([...PROP_KINDS].sort()).toEqual(Object.keys(PROP_CATALOGUE).sort());
    expect(PROP_KINDS).toHaveLength(40);
  });

  it("gives every kind 2 or 3 variants, the sign plate 6", () => {
    for (const k of PROP_KINDS) {
      const n = PROP_CATALOGUE[k].variants;
      if (k === "sign-plate") expect(n).toBe(6);
      else expect([2, 3]).toContain(n);
    }
  });

  it("sizes every floor kind per variant, at most 1.4 m either way", () => {
    for (const k of PROP_KINDS) {
      if (PROP_CATALOGUE[k].anchor !== "floor") continue;
      const sizes = FOOTPRINTS.prop[k as keyof typeof FOOTPRINTS.prop];
      expect(sizes).toHaveLength(PROP_CATALOGUE[k].variants);
      for (const s of sizes) {
        expect(s.width).toBeLessThanOrEqual(1.4);
        expect(s.depth).toBeLessThanOrEqual(1.4);
      }
    }
  });

  it("lets palettes pick only non-extra kinds of the right anchor", () => {
    for (const a of ARCHETYPES) {
      const p = PALETTES[a];
      for (const [k] of [...p.wall, ...FILLER]) {
        expect(PROP_CATALOGUE[k].anchor).toBe("wall");
        expect(PROP_CATALOGUE[k].run || PROP_CATALOGUE[k].extra).toBe(false);
      }
      for (const [k] of p.floor) {
        expect(PROP_CATALOGUE[k].anchor).toBe("floor");
        expect(PROP_CATALOGUE[k].extra).toBe(false);
      }
      if (p.wallRun) expect(PROP_CATALOGUE[p.wallRun].run).toBe(true);
      if (p.ceilingRun) expect(PROP_CATALOGUE[p.ceilingRun].run).toBe(true);
    }
  });

  it("uses every extra kind in exactly its condition, and none when clean", () => {
    const seen = new Map<PropKind, Condition[]>();
    for (const c of Object.keys(EXTRAS) as Condition[])
      for (const r of EXTRAS[c])
        seen.set(r.kind, [...(seen.get(r.kind) ?? []), c]);
    expect(EXTRAS.clean).toEqual([]);
    for (const k of PROP_KINDS)
      expect(seen.get(k)?.length ?? 0).toBe(PROP_CATALOGUE[k].extra ? 1 : 0);
  });

  it("marks wide only wall kinds that are not runs, and six of them", () => {
    const wide = PROP_KINDS.filter((k) => PROP_CATALOGUE[k].wide);
    expect(wide.sort()).toEqual([
      "conduit-cabinet",
      "locker-bank",
      "padded-panel",
      "pipe-riser",
      "stowage-net",
      "tool-board",
    ]);
    for (const k of wide) {
      expect(PROP_CATALOGUE[k].anchor).toBe("wall");
      expect(PROP_CATALOGUE[k].run).toBe(false);
    }
  });

  it("keeps clear every kind a person reads or uses, and every mandatory kind", () => {
    const clear = PROP_KINDS.filter((k) => PROP_CATALOGUE[k].keepClear);
    expect(clear.sort()).toEqual([
      "extinguisher",
      "first-aid",
      "intercom",
      "keycard-reader",
      "sign-plate",
      "wall-monitor",
    ]);
    // The kinds dress.ts places as mandatory (ruling 7).
    for (const k of ["keycard-reader", "sign-plate", "extinguisher"] as const)
      expect(PROP_CATALOGUE[k].keepClear).toBe(true);
  });

  it("weights the wide kinds above the small ones in every palette", () => {
    for (const a of ARCHETYPES) {
      const wall = PALETTES[a].wall;
      const wide = wall.filter(([k]) => PROP_CATALOGUE[k].wide);
      const small = wall.filter(([k]) => !PROP_CATALOGUE[k].wide);
      expect(wide.length, a).toBeGreaterThan(0);
      const minWide = Math.min(...wide.map(([, w]) => w));
      for (const [k, w] of small)
        if (k !== "sign-plate") expect(w, `${a} ${k}`).toBeLessThan(minWide);
    }
  });

  it("gives every palette cluster kinds that are plain floor kinds", () => {
    for (const a of ARCHETYPES) {
      const cluster = PALETTES[a].cluster;
      expect(cluster.length, a).toBeGreaterThan(0);
      for (const [k, w] of cluster) {
        expect(PROP_CATALOGUE[k].anchor, `${a} ${k}`).toBe("floor");
        expect(PROP_CATALOGUE[k].extra, `${a} ${k}`).toBe(false);
        expect(PROP_CATALOGUE[k].wallBacked, `${a} ${k}`).toBe(false);
        expect(w, `${a} ${k}`).toBeGreaterThan(0);
      }
    }
  });

  it("makes spans ceiling kinds that are neither runs nor extras", () => {
    const spans = PROP_KINDS.filter((k) => PROP_CATALOGUE[k].span);
    expect(spans.sort()).toEqual(["span-duct", "span-tray"]);
    for (const k of spans) {
      expect(PROP_CATALOGUE[k].anchor).toBe("ceiling");
      expect(PROP_CATALOGUE[k].run).toBe(false);
      expect(PROP_CATALOGUE[k].extra).toBe(false);
    }
  });
});
