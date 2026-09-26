/**
 * The readable-text guard (C12): the hero and curio recipes draw text as
 * block-pixel geometry, and the homage rule allows only the station's own
 * titles and the three approved exceptions. Every double-quoted literal
 * in a recipe file that holds a capital letter or one of the marks
 * `<`, `=`, `>` or `?` must be on the list, and so must every backtick
 * literal of the same shape in code (comments, which name constants in
 * backticks, are stripped first). Mutation caught: any new readable
 * string in a recipe (add `"FOO"` or a backtick `FOO` to a recipe and this
 * fails).
 */

import { describe, expect, it } from "vitest";

/**
 * Every hero and curio recipe file, raw, by path: gathered by glob, so a
 * batch file a later task adds is scanned without anyone listing it.
 * `pixels.ts` is left out (its glyph keys are the font, not text drawn),
 * and so are the tests.
 */
const SOURCES = Object.fromEntries(
  Object.entries(
    import.meta.glob<string>("./models/{heroes,curios}/*.ts", {
      query: "?raw",
      import: "default",
      eager: true,
    }),
  ).filter(
    ([path]) => !path.endsWith(".test.ts") && !path.endsWith("/pixels.ts"),
  ),
);

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

describe("readable text in the recipes", () => {
  it("draws no string but the station's titles and the approved exceptions", () => {
    // The folders hold the files the 2.6c tasks fill, at the least.
    for (const name of ["floaters", "exhibits", "mechs", "street", "retro"])
      expect(
        Object.keys(SOURCES).some((p) => p.endsWith(`/${name}.ts`)),
        name,
      ).toBe(true);
    for (const [name, src] of Object.entries(SOURCES)) {
      // Comments name constants in backticks (`CLUSTER`); only code draws.
      const code = src
        .replace(/\/\*[\s\S]*?\*\//g, "")
        .replace(/\/\/.*$/gm, "");
      const found = [
        ...code.matchAll(/"([A-Z0-9 .?<=>]+)"|`([A-Z0-9 .?<=>]+)`/g),
      ]
        .map((m) => m[1] ?? m[2] ?? "")
        .filter((t) => /[A-Z?<=>]/.test(t));
      for (const t of found)
        expect(READABLE.has(t), `${name}: ${t}`).toBe(true);
    }
  });
});
