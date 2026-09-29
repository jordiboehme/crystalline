import { describe, expect, it } from "vitest";

import { CANNED_BRIDGE, CANNED_HUB } from "../world/canned";
import { generateRoom } from "../world/generate";
import { isFloor } from "../world/layout";
import { fillLightTexels, lightGrid } from "./lightgrid";

describe("lightGrid", () => {
  for (const [name, place] of [
    ["bridge", CANNED_BRIDGE],
    ["hub", CANNED_HUB],
  ] as const) {
    const room = generateRoom(place);
    const grid = lightGrid(room);

    it(`is as large as the ${name}'s grid`, () => {
      expect(grid.width).toBe(room.width);
      expect(grid.depth).toBe(room.depth);
      expect(grid.zoneOfCell).toHaveLength(room.width * room.depth);
    });

    it(`maps every floor cell of the ${name} to the zone that contains it, void to -1`, () => {
      for (let y = 0; y < room.depth; y++) {
        for (let x = 0; x < room.width; x++) {
          const zone = grid.zoneOfCell[y * room.width + x];
          if (!isFloor(room.grid, x, y)) {
            expect(zone).toBe(-1);
            continue;
          }
          const containing = room.lights
            .map((z, i) => ({ z, i }))
            .filter(({ z }) => x >= z.x0 && x < z.x1 && y >= z.y0 && y < z.y1);
          expect(containing).toHaveLength(1);
          expect(zone).toBe(containing[0]?.i);
        }
      }
    });
  }

  it("fills each floor cell with its zone's rounded level and void with 0", () => {
    const room = generateRoom(CANNED_HUB);
    const grid = lightGrid(room);
    const levels = new Float32Array(room.lights.length).map(
      (_, i) => (i * 37.4) % 256,
    );
    const out = new Uint8Array(grid.width * grid.depth).fill(99);
    fillLightTexels(grid, levels, out);
    let voids = 0;
    for (let i = 0; i < out.length; i++) {
      const zone = grid.zoneOfCell[i] ?? -1;
      if (zone < 0) {
        voids++;
        expect(out[i]).toBe(0);
      } else {
        expect(out[i]).toBe(Math.round(levels[zone] ?? NaN));
      }
    }
    expect(voids).toBeGreaterThan(0);
  });

  it("clamps levels to 0..255 instead of wrapping", () => {
    const room = generateRoom(CANNED_BRIDGE);
    const grid = lightGrid(room);
    const out = new Uint8Array(grid.width * grid.depth);
    fillLightTexels(grid, new Float32Array(room.lights.length).fill(300), out);
    expect(Math.max(...out)).toBe(255);
    fillLightTexels(grid, new Float32Array(room.lights.length).fill(-5), out);
    expect(Math.max(...out)).toBe(0);
  });
});
