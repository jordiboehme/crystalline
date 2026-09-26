/**
 * The readable-text guard (C12): the hero and curio recipes draw text as
 * block-pixel geometry, and the homage rule allows only the station's own
 * titles and the three approved exceptions. Every double-quoted literal
 * in a recipe file that holds a capital letter or one of the marks
 * `<`, `=`, `>` or `?` must be on the list. Mutation caught: any new
 * readable string in a recipe (add `"FOO"` to a recipe and this fails).
 */

import { describe, expect, it } from "vitest";

import curioCommon from "./models/curios/common.ts?raw";
import gear from "./models/curios/gear.ts?raw";
import keepsakes from "./models/curios/keepsakes.ts?raw";
import retro from "./models/curios/retro.ts?raw";
import arcade from "./models/heroes/arcade.ts?raw";
import heroCommon from "./models/heroes/common.ts?raw";
import exhibits from "./models/heroes/exhibits.ts?raw";
import floaters from "./models/heroes/floaters.ts?raw";
import heroIndex from "./models/heroes/index.ts?raw";
import living from "./models/heroes/living.ts?raw";
import mechs from "./models/heroes/mechs.ts?raw";
import optics from "./models/heroes/optics.ts?raw";
import street from "./models/heroes/street.ts?raw";
import workshop from "./models/heroes/workshop.ts?raw";

/** The station's own titles, then the approved exceptions (2.6a, 2.6b, 2.6c). */
const READABLE = new Set([
  "TILEFALL",
  "ROCK RAIN",
  "MAZE HUNT",
  "VOID WING",
  "BLOK",
  "<=>",
  "W.O.P.R.",
  "POLICE",
  "PUBLIC",
  "CALL",
  "BOX",
]);

const SOURCES = {
  curioCommon,
  gear,
  keepsakes,
  retro,
  arcade,
  heroCommon,
  exhibits,
  floaters,
  heroIndex,
  living,
  mechs,
  optics,
  street,
  workshop,
};

describe("readable text in the recipes", () => {
  it("draws no string but the station's titles and the approved exceptions", () => {
    for (const [name, src] of Object.entries(SOURCES)) {
      const found = [...src.matchAll(/"([A-Z0-9 .?<=>]+)"/g)]
        .map((m) => m[1] ?? "")
        .filter((t) => /[A-Z?<=>]/.test(t));
      for (const t of found)
        expect(READABLE.has(t), `${name}: ${t}`).toBe(true);
    }
  });
});
