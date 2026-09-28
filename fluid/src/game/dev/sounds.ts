/**
 * The sound board's list (`SoundBoard.tsx`): every patch the station plays
 * by a readable name (the effects, the drones, the signature sounds and
 * the modem's), and the bus each plays on. Kept apart from the board
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
import { handshakePatch, hangupPatch, reconnectPatch } from "../audio/modem";
import type { Patch } from "../audio/patch";
import { landingPatch, takeoffPatch, tonesPatch } from "../audio/signature";

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
  "wheeze takeoff": takeoffPatch,
  "wheeze landing": landingPatch,
  "five tones": tonesPatch,
  "modem handshake": () => handshakePatch("555-0142"),
  "modem reconnect": reconnectPatch,
  "modem hang-up": hangupPatch,
};

/** The signature sounds' patch names (M4 C24). */
const SIGNATURES: ReadonlySet<string> = new Set(
  [takeoffPatch(), landingPatch(), tonesPatch()].map((p) => p.name),
);

/** The modem's patch names (M4 C26, C27). */
const MODEMS: ReadonlySet<string> = new Set(
  [handshakePatch(""), reconnectPatch(), hangupPatch()].map((p) => p.name),
);

/**
 * The bus a patch plays on: a drone on the ambience, the wheeze and the
 * tones on the signature bus, the modem's on the modem bus, the rest on
 * effects.
 */
export function busOf(patch: Patch): Bus {
  if (patch.name.startsWith("drone")) return "ambience";
  if (MODEMS.has(patch.name)) return "modem";
  return SIGNATURES.has(patch.name) ? "signature" : "effects";
}
