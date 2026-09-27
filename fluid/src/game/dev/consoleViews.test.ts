/**
 * `CONSOLE_VIEWS` puts every named vantage point on the console room's
 * floor, clear of the console's own footprint, facing what it names;
 * `consoleView` answers null for anything it does not recognise.
 */

import { describe, expect, it } from "vitest";

import { atConsoleExit, consoleRoom, EXIT_X } from "../world/consoleRoom";
import { blockersFor, MAX_PITCH, PLAYER_RADIUS } from "../world/move";
import { CELL } from "../world/units";
import { circleOverlapsBox } from "./spots";
import { CONSOLE_VIEWS, consoleView, type ConsoleView } from "./consoleViews";

describe("CONSOLE_VIEWS", () => {
  it("puts every view on the floor, clear of the console, facing what it names", () => {
    // Mutation caught: a view inside the console's box, or off the grid.
    const room = consoleRoom();
    const blockers = blockersFor(room);
    expect(blockers.length).toBeGreaterThan(0);
    const names = Object.keys(CONSOLE_VIEWS) as ConsoleView[];
    expect(names.length).toBe(7);
    for (const n of names) {
      const v = CONSOLE_VIEWS[n];
      const x = (v.spawn.x + 0.5) * CELL;
      const z = (v.spawn.y + 0.5) * CELL;
      expect(x > PLAYER_RADIUS && x < 12 - PLAYER_RADIUS).toBe(true);
      expect(z > PLAYER_RADIUS && z < 12 - PLAYER_RADIUS).toBe(true);
      expect(blockers.some((b) => circleOverlapsBox(x, z, b))).toBe(false);
      expect(Math.abs(v.pitch)).toBeLessThanOrEqual(MAX_PITCH);
    }
  });

  it("answers null for an unknown view", () => {
    // Mutation caught: consoleView answering some view for a bad name
    // instead of null (the gallery would then never fall back to entry).
    expect(consoleView("nowhere")).toBeNull();
    expect(consoleView(null)).toBeNull();
  });

  it("has the rotor view look at the console's centre within 5 degrees, pitched up", () => {
    // Mutation caught: the rotor view aimed away from the console, or
    // pitched flat or down instead of up at the mechanism.
    const view = CONSOLE_VIEWS.rotor;
    const x = (view.spawn.x + 0.5) * CELL;
    const z = (view.spawn.y + 0.5) * CELL;
    const consoleX = 6;
    const consoleZ = 6;
    const idealYaw = Math.atan2(x - consoleX, z - consoleZ);
    const diff = Math.abs(
      Math.atan2(
        Math.sin(view.spawn.yaw - idealYaw),
        Math.cos(view.spawn.yaw - idealYaw),
      ),
    );
    expect(diff).toBeLessThanOrEqual((5 * Math.PI) / 180);
    expect(view.pitch).toBeGreaterThan(0);
  });

  it("has the doors view face south, on the doors' own centre line", () => {
    // Mutation caught: the doors view not yawed south, or off EXIT_X.
    const view = CONSOLE_VIEWS.doors;
    const x = (view.spawn.x + 0.5) * CELL;
    expect(x).toBeCloseTo(EXIT_X, 6);
    expect(view.spawn.yaw).toBeCloseTo(Math.PI, 6);
    // South of the doors' own zone, not inside it (C11).
    const z = (view.spawn.y + 0.5) * CELL;
    expect(atConsoleExit({ x, z })).toBe(false);
  });

  it("has the scanner view face cell 4's south wall, on its own centre line", () => {
    // Mutation caught: the scanner view aimed elsewhere, or off the
    // scanner's own anchor point.
    const room = consoleRoom();
    const scanner = (room.interior ?? []).find((p) => p.kind === "scanner");
    expect(scanner).toBeDefined();
    const scannerX = (scanner?.x ?? 0) * CELL;
    const view = CONSOLE_VIEWS.scanner;
    const x = (view.spawn.x + 0.5) * CELL;
    expect(x).toBeCloseTo(scannerX, 6);
    expect(view.spawn.yaw).toBeCloseTo(Math.PI, 6);
  });
});
