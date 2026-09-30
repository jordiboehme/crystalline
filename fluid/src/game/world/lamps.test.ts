import { describe, expect, it } from "vitest";

import { CANNED_BRIDGE, CANNED_HUB, CANNED_WORKSHOP } from "./canned";
import { generateRoom } from "./generate";
import { LAMP_HALF_D, LAMP_HALF_W, lampBoxes } from "./lamps";
import lampsSource from "./lamps.ts?raw";
import { isFloor } from "./layout";
import { CELL } from "./units";

const ROOMS = [
  { name: "bridge", room: generateRoom(CANNED_BRIDGE) },
  { name: "hub", room: generateRoom(CANNED_HUB) },
  { name: "workshop", room: generateRoom(CANNED_WORKSHOP) },
];

describe("lamp boxes", () => {
  it("gives one box per light zone that has floor", () => {
    for (const { name, room } of ROOMS) {
      const withFloor = room.lights.filter((z) => {
        for (let y = z.y0; y < z.y1; y++)
          for (let x = z.x0; x < z.x1; x++)
            if (isFloor(room.grid, x, y)) return true;
        return false;
      });
      expect(withFloor.length, name).toBeGreaterThan(0);
      expect(lampBoxes(room), name).toHaveLength(withFloor.length);
    }
  });

  it("makes every box a lamp panel's size, lying over floor cells", () => {
    for (const { name, room } of ROOMS)
      for (const b of lampBoxes(room)) {
        expect(b.x1 - b.x0, name).toBeCloseTo(2 * LAMP_HALF_W, 9);
        expect(b.z1 - b.z0, name).toBeCloseTo(2 * LAMP_HALF_D, 9);
        for (const x of [b.x0 + 1e-6, b.x1 - 1e-6])
          for (const z of [b.z0 + 1e-6, b.z1 - 1e-6])
            expect(
              isFloor(room.grid, Math.floor(x / CELL), Math.floor(z / CELL)),
              `${name} ${JSON.stringify(b)}`,
            ).toBe(true);
      }
  });

  it("imports only layout, types and units", () => {
    const from = [...lampsSource.matchAll(/from\s+["']([^"']+)["']/g)].map(
      (m) => m[1],
    );
    for (const f of from)
      expect(["./layout", "./types", "./units"]).toContain(f);
  });
});
