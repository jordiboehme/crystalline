/**
 * The sound board's list (`SoundBoard.tsx`): every patch the station plays
 * by a readable name, and the bus each plays on. Kept apart from the board
 * so its component module exports components only.
 */

import { AMBIENCES, dronePatch } from "../audio/ambience";
import type { DoorSound } from "../audio/cues";
import {
  doorPatch,
  faultPatch,
  hatchPatch,
  portalPatch,
  ridePatch,
  stepPatch,
  terminalPatch,
} from "../audio/effects";
import type { Bus } from "../audio/mixer";
import type { Patch } from "../audio/patch";

/** Every door sound, checked complete against the type. */
const DOOR_SOUNDS = Object.keys({
  sliding: true,
  bulkhead: true,
  blast: true,
  exit: true,
  box: true,
} satisfies Record<DoorSound, true>) as DoorSound[];

/** Every patch the station plays, by a readable name. */
export const SOUNDS: Record<string, () => Patch> = {
  "step walk": () => stepPatch(0, false, 0),
  "step walk foot 1": () => stepPatch(1, false, 1),
  "step run": () => stepPatch(2, true, 0),
  "step run foot 1": () => stepPatch(3, true, 1),
  ...Object.fromEntries(
    DOOR_SOUNDS.flatMap((sound) => [
      [`door ${sound} open`, () => doorPatch(sound, true)],
      [`door ${sound} close`, () => doorPatch(sound, false)],
    ]),
  ),
  hatch: hatchPatch,
  portal: portalPatch,
  terminal: terminalPatch,
  "fault door": () => faultPatch("door", 1),
  "fault door run 2": () => faultPatch("door", 2),
  "fault hatch": () => faultPatch("hatch", 1),
  "fault portal": () => faultPatch("portal", 1),
  "ride depart": () => ridePatch("depart"),
  "ride arrive": () => ridePatch("arrive"),
  ...Object.fromEntries(
    AMBIENCES.map((kind) => [`drone ${kind}`, () => dronePatch(kind, 1)]),
  ),
};

/** The bus a patch plays on: a drone on the ambience, the rest on effects. */
export function busOf(patch: Patch): Bus {
  return patch.name.startsWith("drone") ? "ambience" : "effects";
}
