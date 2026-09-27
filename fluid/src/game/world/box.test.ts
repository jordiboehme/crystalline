/**
 * The police box in play: its front, the focus on it, the door steps, the
 * walk-in through its open doors, and the console room's own exit pick
 * (`world/box.ts`).
 */

import { describe, expect, it } from "vitest";

import { heroHallRoom } from "./canned";
import {
  BOX_LATCH_CLEAR,
  boxEntry,
  boxFocus,
  boxFront,
  exitSeed,
  pickExitDomain,
  stepBoxDoors,
  steppedAway,
  type DomainRow,
} from "./box";
import { heroFootprint } from "./footprints";
import type { Player } from "./move";
import type { Hero, RoomSpec } from "./types";

/** One domain row, defaulting to no canonical name and no aliases. */
const row = (
  name: string,
  canonicalName: string | null = null,
  aliases: string[] = [],
): DomainRow => ({ name, canonicalName, aliases });

/**
 * The hero hall with its heroes replaced by four police boxes standing
 * free on its floor (2.6e), one at each turn, far enough apart that only
 * one is ever in reach.
 */
function fourBoxes(): RoomSpec {
  const room = heroHallRoom();
  const { x0, y0, x1, y1 } = room.hall;
  const mx = (x0 + x1) / 2;
  const my = (y0 + y1) / 2;
  const spots: [number, number][] = [
    [mx, my + 4],
    [mx - 4, my],
    [mx, my - 4],
    [mx + 4, my],
  ];
  const heroes: Hero[] = spots.map(([x, y], i) => ({
    kind: "police-box",
    variant: 0,
    x,
    y,
    turn: i,
    seed: i,
  }));
  return { ...room, heroes };
}

/** A player standing `out` metres in front of box `h`'s front, `side` metres along it, facing it. */
function facing(h: Hero, out: number, side = 0): Player {
  const f = boxFront(h);
  return {
    x: f.x + f.inward[0] * out + f.along[0] * side,
    z: f.z + f.inward[1] * out + f.along[1] * side,
    vx: 0,
    vz: 0,
    yaw: Math.atan2(f.inward[0], f.inward[1]),
    pitch: 0,
    bob: 0,
  };
}

describe("the police box's doors and front (2.6e C10, C11)", () => {
  it("offers the box's doors from in front at every turn, and not from its side or back (2.6e C10)", () => {
    // Mutation caught: the front taken on the wrong side for a turned box,
    // or the facing test dropped.
    const room = fourBoxes();
    expect(room.heroes.length).toBe(4);
    room.heroes.forEach((h, i) => {
      expect(boxFocus(room, facing(h, 1.0), new Map())).toEqual({
        index: i,
        prompt: "SPACE OPEN",
      });
      const back = facing(h, -2.0);
      expect(boxFocus(room, back, new Map())).toBeNull();
      const away = { ...facing(h, 1.0), yaw: facing(h, 1.0).yaw + Math.PI };
      expect(boxFocus(room, away, new Map())).toBeNull();
    });
  });

  it("opens on a press in eighteen ticks and closes on the next (2.6e C10)", () => {
    // Mutation caught: the step size, or a press that does not turn it round.
    const room = fourBoxes();
    let boxes = stepBoxDoors(room, new Map(), 0);
    for (let t = 1; t < 18; t++) boxes = stepBoxDoors(room, boxes, null);
    expect(boxes.get(0)).toEqual({ open: 1, target: 1 });
    expect(boxes.get(1)).toEqual({ open: 0, target: 0 });
    boxes = stepBoxDoors(room, boxes, 0);
    expect(boxes.get(0)?.target).toBe(0);
    expect(
      boxFocus(
        room,
        facing(room.heroes[0]!, 1.0),
        new Map([[0, { open: 1, target: 1 }]]),
      )?.prompt,
    ).toBe("SPACE CLOSE");
  });

  it("puts the front on the free-standing box's front face, at its middle, at every turn (2.6e)", () => {
    // Mutation caught: the front taken a whole depth out from the anchor
    // (a backed box's), or on the box's centre, so the doors would be
    // offered and walked through half a box away from where they stand.
    for (const h of fourBoxes().heroes) {
      const f = boxFront(h);
      const box = heroFootprint(h);
      const [ix, iz] = f.inward;
      const edge = ix > 0 ? box.x1 : ix < 0 ? box.x0 : iz > 0 ? box.z1 : box.z0;
      expect(ix !== 0 ? f.x : f.z).toBeCloseTo(edge, 9);
      expect(ix !== 0 ? f.z : f.x).toBeCloseTo(
        ix !== 0 ? (box.z0 + box.z1) / 2 : (box.x0 + box.x1) / 2,
        9,
      );
    }
  });

  it("counts the player stepped away from a box only 1.2 m from its front, in any direction, at every turn (2.6e C29)", () => {
    // Mutation caught: the latch distance left at the doorway's 0.6 m, or
    // measured along the front's direction only (a player beside the box,
    // or behind it, never counted away; or one 1.0 m out counted away).
    expect(BOX_LATCH_CLEAR).toBe(1.2);
    for (const h of fourBoxes().heroes) {
      expect(steppedAway(h, facing(h, 0.45))).toBe(false);
      expect(steppedAway(h, facing(h, 1.19))).toBe(false);
      expect(steppedAway(h, facing(h, 1.21))).toBe(true);
      // Along the front, beside the doorway: the same distance counts.
      expect(steppedAway(h, facing(h, 0, 1.19))).toBe(false);
      expect(steppedAway(h, facing(h, 0, 1.21))).toBe(true);
      // Behind the box: well over 1.2 m from its front.
      expect(steppedAway(h, facing(h, -1.6))).toBe(true);
    }
  });

  it("covers all four turns", () => {
    expect(
      fourBoxes()
        .heroes.map((h) => h.turn)
        .sort(),
    ).toEqual([0, 1, 2, 3]);
  });

  it("walks in only through open doors, from in front, inside the opening (2.6e C11)", () => {
    // Mutation caught: travel through shut doors, through the side of the
    // box, or from beside the opening.
    const room = fourBoxes();
    const open = new Map(
      room.heroes.map((_, i) => [i, { open: 1, target: 1 as const }]),
    );
    room.heroes.forEach((h, i) => {
      expect(boxEntry(room, facing(h, 0.36), open)).toBe(i);
      expect(boxEntry(room, facing(h, 0.36), new Map())).toBeNull();
      expect(boxEntry(room, facing(h, 0.36, 0.5), open)).toBeNull();
      expect(boxEntry(room, facing(h, 0.7), open)).toBeNull();
    });
  });
});

describe("the console room's exit pick (2.6e C13)", () => {
  it("picks uniformly among the other domains (2.6e C13)", () => {
    // Mutation caught: the room left counted in, or a biased index (a floor
    // over the whole list, a modulo of the seed).
    const rows = ["alpha", "beta", "gamma", "delta", "eng"].map((n) => row(n));
    const counts = new Map<string, number>();
    for (let t = 0; t < 4000; t++) {
      const d = pickExitDomain(rows, "eng", exitSeed("eng", t));
      counts.set(d, (counts.get(d) ?? 0) + 1);
    }
    expect([...counts.keys()].sort()).toEqual([
      "alpha",
      "beta",
      "delta",
      "gamma",
    ]);
    for (const n of counts.values()) {
      expect(n).toBeGreaterThan(850);
      expect(n).toBeLessThan(1150);
    }
  });

  it("never picks the domain left, whichever of its names the room was entered by (Review Focus 1)", () => {
    // Mutation caught: excluding by the local name only.
    const rows = [row("moonbase", "moon", ["luna"]), row("alpha"), row("beta")];
    for (const from of ["moonbase", "moon", "luna"])
      for (let t = 0; t < 1000; t++)
        expect(pickExitDomain(rows, from, exitSeed(from, t))).not.toBe(
          "moonbase",
        );
  });

  it("gives the room left's own domain when no other exists or the listing is empty", () => {
    // Mutation caught: returning the URL spelling instead of the row's local
    // name, or throwing on an empty listing instead of falling back.
    expect(pickExitDomain([row("eng")], "eng", 1)).toBe("eng");
    expect(pickExitDomain([], "eng", 1)).toBe("eng");
    expect(pickExitDomain([row("moonbase", "moon")], "moon", 1)).toBe(
      "moonbase",
    );
  });

  it("does not depend on the listing's order", () => {
    // Mutation caught: drawing the index over the listing's own order instead
    // of a name-sorted one.
    const a = ["alpha", "beta", "gamma"].map((n) => row(n));
    for (let t = 0; t < 50; t++)
      expect(pickExitDomain(a, "eng", t)).toBe(
        pickExitDomain([...a].reverse(), "eng", t),
      );
  });

  it("never lets a shadowed canonical name exclude the domain that really holds it (fix round 1)", () => {
    // Mutation caught: matching row.canonicalName === from independently of
    // the other rows, which would also exclude "moonbase" here since its
    // canonical name collides with the real domain "moon"'s local name.
    const rows = [row("moon"), row("moonbase", "moon", ["luna"]), row("alpha")];
    const picks = new Set<string>();
    for (let t = 0; t < 300; t++)
      picks.add(pickExitDomain(rows, "moon", exitSeed("moon", t)));
    expect([...picks].sort()).toEqual(["alpha", "moonbase"]);
  });

  it("leaves nothing excluded when the entry spelling is a contested canonical name (fix round 1)", () => {
    // Mutation caught: matching row.canonicalName === from independently of
    // the other rows, which would wrongly exclude both "moonbase" and
    // "station" here, leaving only "alpha" ever pickable.
    const rows = [
      row("moonbase", "luna"),
      row("station", "luna"),
      row("alpha"),
    ];
    const picks = new Set<string>();
    for (let t = 0; t < 300; t++)
      picks.add(pickExitDomain(rows, "luna", exitSeed("luna", t)));
    expect([...picks].sort()).toEqual(["alpha", "moonbase", "station"]);
  });

  it("leaves an alias's own domain untouched when a local name shadows it (fix round 1)", () => {
    // Mutation caught: matching row.aliases.includes(from) independently of
    // the other rows, which would also exclude "archive" here since it
    // lists "eng" as an alias, even though "eng" is a real domain's own
    // local name and the alias is shadowed away from "archive".
    const rows = [row("eng"), row("archive", null, ["eng"])];
    for (let t = 0; t < 200; t++)
      expect(pickExitDomain(rows, "eng", exitSeed("eng", t))).toBe("archive");
  });
});
