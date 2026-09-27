/**
 * Named vantage points inside the console room (2.6e C21), for
 * `dev/Gallery.tsx`'s `?hall=console&view=<name>` seam: `entry` is the
 * room's own arrival spot facing the console; `console` and `rotor` stand
 * in front of the console, the second closer in and pitched up at the
 * column and its mechanism; `wall` faces a plain roundel wall; `doors`
 * and `scanner` stand on their fitting's own centre line facing it (south);
 * `corner` looks into a corner where two roundel walls meet. Every view
 * stands on the room's floor, clear of the console's footprint
 * (`interiorFootprint`), and pitches no more than `MAX_PITCH` up or down,
 * so a still taken from it never clips through a wall or the console.
 *
 * `CONSOLE_VIEWS` is the catalogue; `consoleView` looks a name up in it,
 * answering null for an unknown name or no name at all, so the gallery
 * falls back to `CONSOLE_VIEWS.entry` itself rather than failing to load.
 */

import type { RoomSpec } from "../world/types";
import { CELL } from "../world/units";

/** The named vantage points `?view=` accepts inside the console room. */
export type ConsoleView =
  "entry" | "console" | "rotor" | "wall" | "doors" | "scanner" | "corner";

/**
 * A spawn point at `(x, z)` metres facing `yaw` (radians, `forwardOf`'s
 * convention): `RoomSpec["spawn"]`'s fields are `spawnPlayer`'s continuous
 * cell units, which add 0.5 back before the metre scale, so this is the
 * inverse of that: `x / CELL - 0.5`.
 */
function at(x: number, z: number, yaw: number): RoomSpec["spawn"] {
  return { x: x / CELL - 0.5, y: z / CELL - 0.5, yaw };
}

/**
 * Every named view, in metres and radians before `at` converts them: the
 * console stands at the room's centre `(6, 6)` (C3), the inner doors on
 * their centre line `x = 6` at the south wall `z = 12` (`EXIT_X`), and the
 * scanner on cell 4's south edge at `x = 9`.
 */
export const CONSOLE_VIEWS: Readonly<
  Record<ConsoleView, { spawn: RoomSpec["spawn"]; pitch: number }>
> = {
  // The room's own arrival spot (C3): 1.6 m in front of the inner doors,
  // facing north at the console.
  entry: { spawn: at(6, 10.4, 0), pitch: 0 },
  // In front of the console's desk, facing it, pitched down a little to
  // frame the panels.
  console: { spawn: at(6, 8.5, 0), pitch: -0.15 },
  // Closer in on the console's centre line, pitched up at the column and
  // the rotor sliding inside it.
  rotor: { spawn: at(6, 8.2, 0), pitch: 0.3 },
  // North of the console, facing the plain roundel wall on the north side.
  wall: { spawn: at(3, 3, 0), pitch: 0 },
  // On the doors' own centre line, facing south at the inner doors.
  doors: { spawn: at(6, 9, Math.PI), pitch: 0.1 },
  // On the scanner's own centre line, facing south at its housing.
  scanner: { spawn: at(9, 10, Math.PI), pitch: 0.15 },
  // Toward the room's north-west corner, where two roundel walls meet.
  corner: { spawn: at(2, 2, Math.PI / 4), pitch: 0 },
};

/**
 * The vantage point named `name`, or null for an unknown name or no name
 * at all (`?view=` absent). The gallery falls back to `CONSOLE_VIEWS.entry`
 * on null itself, so this never guesses.
 */
export function consoleView(
  name: string | null,
): { spawn: RoomSpec["spawn"]; pitch: number } | null {
  if (name === null) return null;
  const names = Object.keys(CONSOLE_VIEWS) as ConsoleView[];
  return names.includes(name as ConsoleView)
    ? CONSOLE_VIEWS[name as ConsoleView]
    : null;
}
