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
    expect(PROP_KINDS).toHaveLength(34);
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
});
