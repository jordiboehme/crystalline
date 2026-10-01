/**
 * Every host surface a curio may stand on (C3), pinned to its host's own
 * mesh at turns 0 and 1: an upward face at `h` under the surface (a floor
 * spot, `h` 0, has none to find), nothing of the host inside the box
 * `clear` tall above it, and the surface inside the host's footprint.
 * Seeded hosts (a workbench's tools, a lab bench's tubes) are built with
 * three seeds.
 *
 * The surfaces are the world side's (`hostSurfaces` of a room holding the
 * one host), so this pins the tables and the transform together. A
 * fixture or a piece of decor is built where it stands, through its slot's
 * or its own frame; a floor prop or a hero is built once at the origin and
 * placed the way the GPU places an instance (`placeMesh`).
 */

import { describe, expect, it } from "vitest";

import { galleryRoom } from "../world/canned";
import {
  DECOR_SURFACES,
  FIXTURE_SURFACES,
  PROP_SURFACES,
  hostSurfaces,
} from "../world/curios";
import {
  HERO_FOOTING,
  decorFootprint,
  footprintOf,
  heroFootprint,
  propFootprint,
} from "../world/footprints";
import { HERO_CATALOGUE, HERO_KINDS } from "../world/heroes";
import { wallAnchor } from "../world/sites";
import type {
  Box,
  Decor,
  Fixture,
  Hero,
  HeroKind,
  MachineKind,
  Prop,
  RoomSpec,
  Side,
  WallSlot,
} from "../world/types";
import { CELL } from "../world/units";
import { VARIANT_COUNTS } from "../world/variants";
import { createBuilder, type MeshData } from "./geometry";
import { createKit } from "./kit";
import { LOOK } from "./looks";
import { clearAboveBox, placeMesh, upwardFaceAt } from "./modelChecks";
import { buildDecor, buildFixture, type ModelContext } from "./models";
import { buildHeroMesh } from "./models/heroes";
import { buildPropMesh } from "./models/props";

const SIDES: readonly Side[] = ["n", "e", "s", "w"];
/** The turns every host is checked at. */
const TURNS = [0, 1] as const;
/** The fixture seeds a seeded host is built with. */
const SEEDS = [1, 2, 3] as const;
/** How far every surface box is shrunk before it is sampled, in metres. */
const SHRINK = 0.001;

/** A room holding nothing but what the caller puts in. */
const BASE: RoomSpec = {
  ...galleryRoom(),
  fixtures: [],
  decor: [],
  heroes: [],
  props: [],
  curios: [],
};

/** A model context like `models.test.ts`'s stub: every text on one layer. */
const CTX: ModelContext = {
  look: LOOK,
  ceiling: 3.0,
  hall: BASE.hall,
  textLayer: () => ({ layer: 0, v0: 0, v1: 1 }),
};

/** The wall side whose wall anchor takes turn `t`. */
function sideFor(t: number): Side {
  const side = SIDES.find(
    (s) => wallAnchor({ x: 3, y: 4, side: s }).turn === t,
  );
  if (side === undefined) throw new Error(`no side for turn ${String(t)}`);
  return side;
}

const shrunk = (b: Box): Box => ({
  x0: b.x0 + SHRINK,
  x1: b.x1 - SHRINK,
  z0: b.z0 + SHRINK,
  z1: b.z1 - SHRINK,
});

const inside = (inner: Box, outer: Box, eps = 1e-6) =>
  inner.x0 >= outer.x0 - eps &&
  inner.x1 <= outer.x1 + eps &&
  inner.z0 >= outer.z0 - eps &&
  inner.z1 <= outer.z1 + eps;

/**
 * Checks every surface `room`'s one host offers against that host's
 * `mesh` and `footprint`, labelling each failure with `label`.
 */
function checkHost(
  label: string,
  room: RoomSpec,
  mesh: MeshData,
  footprint: Box,
): void {
  const surfaces = hostSurfaces(room);
  expect(surfaces.length, label).toBeGreaterThan(0);
  for (const [j, s] of surfaces.entries()) {
    const at = `${label} surface ${String(j)} (h ${String(s.h)})`;
    const box = shrunk(s.box);
    if (s.h > 0)
      for (let ix = 0; ix < 3; ix++)
        for (let iz = 0; iz < 3; iz++) {
          const x = box.x0 + ((box.x1 - box.x0) * ix) / 2;
          const z = box.z0 + ((box.z1 - box.z0) * iz) / 2;
          expect(
            upwardFaceAt(mesh, x, z, s.h),
            `${at} face at (${String(ix)},${String(iz)})`,
          ).toBe(true);
        }
    expect(clearAboveBox(mesh, box, s.h, s.clear), `${at} clear`).toBe(true);
    expect(inside(s.box, footprint), `${at} inside the footprint`).toBe(true);
  }
}

/** The fixture hosts: a terminal and the machines with surfaces. */
function fixtureHost(
  host: "terminal" | MachineKind,
  slot: WallSlot,
  seed: number,
  variant: number,
): Fixture {
  if (host === "terminal")
    return {
      kind: "terminal",
      slot,
      heading: "x",
      lines: [],
      section: 0,
      seed,
      variant,
    };
  return {
    kind: "machine",
    slot,
    machine: host,
    tag: "t",
    hue: 0,
    seed,
    variant,
  };
}

/** How many variants a fixture host draws (2.7 Task 1). */
function variantCountFor(host: "terminal" | MachineKind): number {
  return host === "terminal"
    ? VARIANT_COUNTS.terminal
    : VARIANT_COUNTS.machine[host];
}

/** A hero at turn `t`, placed as `heroModels.test.ts` places it. */
function heroAt(kind: HeroKind, t: number): Hero {
  if (HERO_FOOTING[kind] === "free")
    return { kind, variant: 0, x: 4.5, y: 3, turn: t, seed: 0 };
  const a = wallAnchor({ x: 3, y: 4, side: sideFor(t) });
  return { kind, variant: 0, x: a.x, y: a.y, turn: t, seed: 0 };
}

/** The fixture hosts: the terminal and every machine with a surface table. */
const FIXTURE_HOSTS: readonly ("terminal" | MachineKind)[] = [
  "terminal",
  ...(Object.keys(FIXTURE_SURFACES.machine) as MachineKind[]),
];

describe("host surfaces on their hosts' meshes", () => {
  it("covers the terminal and every machine with a surface table", () => {
    expect(FIXTURE_HOSTS).toEqual([
      "terminal",
      "workbench",
      "lab-bench",
      "hydroponics",
    ]);
  });

  for (const host of FIXTURE_HOSTS)
    for (const t of TURNS)
      for (const seed of SEEDS)
        for (let variant = 0; variant < variantCountFor(host); variant++)
          it(`${host} at turn ${String(t)}, seed ${String(seed)}, variant ${String(variant)}`, () => {
            const fx = fixtureHost(
              host,
              { x: 3, y: 4, side: sideFor(t) },
              seed,
              variant,
            );
            const b = createBuilder();
            buildFixture((f) => createKit(b, f), fx, 0, CTX);
            const footprint = footprintOf(fx);
            if (footprint === null) throw new Error("footprint");
            checkHost(
              `${host} t${String(t)} s${String(seed)} v${String(variant)}`,
              { ...BASE, fixtures: [fx] },
              b.build(),
              footprint,
            );
          });

  for (const kind of Object.keys(
    DECOR_SURFACES,
  ) as (keyof typeof DECOR_SURFACES)[])
    for (const t of TURNS)
      for (let variant = 0; variant < VARIANT_COUNTS.decor[kind]; variant++)
        it(`${kind} at turn ${String(t)}, variant ${String(variant)}`, () => {
          const d: Decor = { kind, x: 8.5, y: 6, turn: t, seed: 1, variant };
          const b = createBuilder();
          buildDecor((f) => createKit(b, f), d, CTX);
          const footprint = decorFootprint(d);
          if (footprint === null) throw new Error("footprint");
          checkHost(
            `${kind} t${String(t)} v${String(variant)}`,
            { ...BASE, decor: [d] },
            b.build(),
            footprint,
          );
        });

  for (const kind of Object.keys(
    PROP_SURFACES,
  ) as (keyof typeof PROP_SURFACES)[])
    for (let v = 0; v < PROP_SURFACES[kind].length; v++)
      for (const t of TURNS)
        it(`${kind} variant ${String(v)} at turn ${String(t)}`, () => {
          const p: Prop = {
            kind,
            variant: v,
            anchor: "floor",
            x: 8.5,
            y: 6.5,
            turn: t,
            seed: 1,
          };
          const mesh = placeMesh(buildPropMesh(kind, v, LOOK), t, [
            p.x * CELL,
            0,
            p.y * CELL,
          ]);
          const footprint = propFootprint(p);
          if (footprint === null) throw new Error("footprint");
          checkHost(
            `${kind} v${String(v)} t${String(t)}`,
            { ...BASE, props: [p] },
            mesh,
            footprint,
          );
        });

  const surfaced = HERO_KINDS.filter(
    (k) =>
      HERO_CATALOGUE[k].surfaces.length + HERO_CATALOGUE[k].under.length > 0,
  );

  it("covers the four hero kinds with surfaces", () => {
    expect(surfaced).toHaveLength(4);
  });

  for (const kind of surfaced)
    for (const t of TURNS)
      it(`${kind} at turn ${String(t)}`, () => {
        const h = heroAt(kind, t);
        const mesh = placeMesh(buildHeroMesh(kind, 0, LOOK), t, [
          h.x * CELL,
          0,
          h.y * CELL,
        ]);
        checkHost(
          `${kind} t${String(t)}`,
          { ...BASE, heroes: [h] },
          mesh,
          heroFootprint(h),
        );
      });
});
