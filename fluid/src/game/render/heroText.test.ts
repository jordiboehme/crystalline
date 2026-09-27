/**
 * The readable-text guard (C12, 2.6d C16, 2.6f C13-C15): the hero, curio
 * and prop recipes draw text as block-pixel geometry, and the homage
 * rule allows only the station's own titles and the approved exceptions
 * (2.6a to 2.6c), and the 2.6d and 2.6f marks of `marks.ts`. Every
 * double-quoted literal of letters, digits, spaces, periods and the
 * marks `<`, `=`, `>`, `&` and `?` in a recipe file or in `marks.ts` that
 * holds a digit or one of those marks, or capitals and no lower case,
 * must be on the list, and so must every backtick literal of the same
 * shape in code (comments, which name constants in backticks, are
 * stripped first). The 2.6d and 2.6f strings may be spelled in
 * `marks.ts` alone, and no recipe hands `textRows` a literal with lower
 * case in it, so the font's lower case is set from `marks.ts` alone. No
 * recipe reads `PIXEL_FONT` itself either, so every glyph goes through
 * `textRows`, whose calls `textCalls.test.ts` records while every recipe
 * builds and holds to this same list. A picture (rows of `.` and `#` or
 * lower-case colour keys, drawn with `pixelPanel`, `pixelBoxes`,
 * `runsOf`, `pixelRuns` or `blinkPicture`) never reaches `textRows`, so
 * neither this scan nor that one sees it; `APPROVED_PICTURES` below
 * names every top-level one instead, so a new picture is a conscious
 * change and reviewed by eye rather than caught by a letter. Mutation caught: any new readable string in a recipe (add
 * `"FOO"`, `"42"` or a backtick `FOO` to a recipe and this fails).
 */

import { describe, expect, it } from "vitest";

/**
 * Every hero, curio and prop recipe file and `marks.ts`, raw, by path:
 * gathered by glob, so a batch file a later task adds is scanned without
 * anyone listing it. `pixels.ts` is left out (its glyph keys are the font,
 * not text drawn), and so are the tests.
 */
const SOURCES = Object.fromEntries(
  Object.entries(
    import.meta.glob<string>(
      ["./models/{heroes,curios,props}/*.ts", "./models/marks.ts"],
      {
        query: "?raw",
        import: "default",
        eager: true,
      },
    ),
  ).filter(
    ([path]) => !path.endsWith(".test.ts") && !path.endsWith("/pixels.ts"),
  ),
);

/**
 * The station's own titles, then the approved exceptions (2.6a, 2.6b,
 * 2.6c), then the 2.6d and 2.6f marks of `marks.ts` (C16).
 */
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
  // The saucer poster's caption.
  "I WANT TO BELIEVE",
  // The capsule maker's word: capsule case, marked crate, gravity console.
  "CAPSULE CORP.",
  // The five capsules' numbers.
  "1",
  "2",
  "3",
  "4",
  "5",
  // The ooze canisters' letters.
  "TCRI",
  // The designer tower's badge and its clock-speed display.
  "HIGHSCREEN",
  "40",
  // The gravity console's readout.
  "300G",
  // Both desk computers' badge.
  "commodore 64",
  // The reactor case's plaque.
  "PROOF THAT TONY STARK HAS A HEART",
  // The eye panel's badge.
  "HAL",
  "9000",
  // The photo console's wordmark plate.
  "esper",
  // The tube bench's two embossed labels.
  "SHIELD EYES FROM LIGHT",
  "DISCONNECT CAPACITOR DRIVE BEFORE OPENING",
  // The recruitment cabinet's marquee.
  "STARFIGHTER",
  // The red bike's fairing stickers.
  "CANON",
  "CITIZEN",
  "ARAI",
  "SHOEI",
  // The hoverboard's deck wordmark.
  "HOVER",
  "BOARD",
  // The police box's door notice, nine lines ("PUBLIC" is already above,
  // shared with the roof sign band).
  "POLICE TELEPHONE",
  "FREE",
  "FOR USE OF",
  "ADVICE & ASSISTANCE",
  "OBTAINABLE IMMEDIATELY",
  "OFFICER & CARS",
  "RESPOND TO ALL CALLS",
  "PULL TO OPEN",
  // The pocket console's bezel badge.
  "Nintendo",
  "GAME BOY",
  // The tape drive's badge.
  "commodore",
  // The tape player's lid badge.
  "SONY",
  "WALKMAN",
  // The fuel case's three hazard labels.
  "RADIOACTIVE III",
  "CAUTION RADIOACTIVE MATERIAL",
  "PLUTONIUM HANDLE WITH CARE",
]);

/**
 * The 2.6d and 2.6f strings, which only `marks.ts` may spell (C16).
 * "PUBLIC" is left out on purpose: the police box's own roof sign
 * (`street.ts`) already spells it outside `marks.ts`, and the door
 * notice's own "PUBLIC" line is the same string.
 */
const MARKS_ONLY = [
  "I WANT TO BELIEVE",
  "CAPSULE CORP.",
  "1",
  "2",
  "3",
  "4",
  "5",
  "TCRI",
  "HIGHSCREEN",
  "40",
  "300G",
  "commodore 64",
  "PROOF THAT TONY STARK HAS A HEART",
  "HAL",
  "9000",
  "esper",
  "SHIELD EYES FROM LIGHT",
  "DISCONNECT CAPACITOR DRIVE BEFORE OPENING",
  "STARFIGHTER",
  "CANON",
  "CITIZEN",
  "ARAI",
  "SHOEI",
  "HOVER",
  "BOARD",
  "POLICE TELEPHONE",
  "FREE",
  "FOR USE OF",
  "ADVICE & ASSISTANCE",
  "OBTAINABLE IMMEDIATELY",
  "OFFICER & CARS",
  "RESPOND TO ALL CALLS",
  "PULL TO OPEN",
  "Nintendo",
  "GAME BOY",
  "commodore",
  "SONY",
  "WALKMAN",
  "RADIOACTIVE III",
  "CAUTION RADIOACTIVE MATERIAL",
  "PLUTONIUM HANDLE WITH CARE",
];

/** A source with its comments stripped: comments name constants in backticks (`CLUSTER`); only code draws. */
const codeOf = (src: string): string =>
  src.replace(/\/\*[\s\S]*?\*\//g, "").replace(/\/\/.*$/gm, "");

/**
 * Every top-level picture, by grep of `export const NAME: readonly
 * string[] =` across the hero, curio and prop recipes and `marks.ts`
 * (2.6d C16 fix): `RECRUIT_DEMO`, `QUESTION_MARK`, `CONSOLE_TITLE` and
 * `CONSOLE_PLAY` from before 2.6d, 2.6d's own `SAUCER_PICTURE`,
 * `CAPSULE_LOGO`, `SIGNATURE`, `SPACE_BADGE` and `COMPUTER_LOGO`, and
 * 2.6f's own `HAMMER_RUNES` and `TREFOIL_MARK`. `PLAQUE_LINES`
 * (`finds.ts`) is left out on purpose: it is `readonly (readonly
 * string[])[]` built from `textRows`, so its digits and letters are
 * caught by the guards above already.
 */
const APPROVED_PICTURES = new Set([
  "RECRUIT_DEMO",
  "QUESTION_MARK",
  "CONSOLE_TITLE",
  "CONSOLE_PLAY",
  "SAUCER_PICTURE",
  "CAPSULE_LOGO",
  "SIGNATURE",
  "SPACE_BADGE",
  "COMPUTER_LOGO",
  "HAMMER_RUNES",
  "TREFOIL_MARK",
]);

/** Every top-level `readonly string[]` constant's name, across `SOURCES`. */
function picturesFound(): Set<string> {
  const found = new Set<string>();
  for (const src of Object.values(SOURCES))
    for (const m of codeOf(src).matchAll(
      /^(?:export\s+)?const\s+([A-Z][A-Za-z0-9_]*)\s*:\s*readonly string\[\]\s*=/gm,
    ))
      found.add(m[1] ?? "");
  return found;
}

describe("readable text in the recipes", () => {
  it("draws no string but the station's titles and the approved exceptions", () => {
    // The folders hold the files the 2.6c and 2.6d tasks fill, at the least.
    for (const name of [
      "floaters",
      "exhibits",
      "mechs",
      "street",
      "retro",
      "finds",
      "desktop",
      "critters",
      "marked",
      "rare",
      "walker",
      "marks",
    ])
      expect(
        Object.keys(SOURCES).some((p) => p.endsWith(`/${name}.ts`)),
        name,
      ).toBe(true);
    for (const [name, src] of Object.entries(SOURCES)) {
      const code = codeOf(src);
      const found = [
        ...code.matchAll(/"([A-Za-z0-9 .?<=>&]+)"|`([A-Za-z0-9 .?<=>&]+)`/g),
      ]
        .map((m) => m[1] ?? m[2] ?? "")
        .filter(
          (t) => /[0-9?<=>&]/.test(t) || (/[A-Z]/.test(t) && !/[a-z]/.test(t)),
        );
      for (const t of found)
        expect(READABLE.has(t), `${name}: ${t}`).toBe(true);
    }
  });

  it("keeps the 2.6d strings in marks.ts alone (2.6d C16)", () => {
    // Mutation caught: a recipe spelling "TCRI" itself instead of reading
    // MARKS, which the allowlist alone would let through.
    for (const [name, src] of Object.entries(SOURCES)) {
      if (name.endsWith("/marks.ts")) continue;
      const code = codeOf(src);
      for (const t of MARKS_ONLY)
        expect(
          code.includes(`"${t}"`) || code.includes(`\`${t}\``),
          `${name}: ${t}`,
        ).toBe(false);
    }
  });

  it("reaches the font through textRows alone (2.6d C16)", () => {
    // `textCalls.test.ts` records every string `textRows` sets while the
    // recipes build; a recipe reading `PIXEL_FONT` itself would set glyphs
    // around that record. Mutation caught: `PIXEL_FONT[c]` or an import of
    // `PIXEL_FONT` in any recipe.
    for (const [name, src] of Object.entries(SOURCES))
      expect(/\bPIXEL_FONT\b/.test(codeOf(src)), name).toBe(false);
  });

  it("hands textRows no literal of a recipe's own (2.6d C16)", () => {
    // The font sets lower case too, which the filter above does not see
    // (it passes lower-case and mixed-case literals, which name kinds,
    // banks and parameters), so a lower-case word set straight into
    // `textRows` would pass it. Mutation caught: `textRows("word")` or
    // `textRows(`Word`)` in any recipe.
    for (const [name, src] of Object.entries(SOURCES))
      expect(/textRows\(\s*["'`][^"'`]*[a-z]/.test(codeOf(src)), name).toBe(
        false,
      );
  });

  it("names every top-level picture on the approved list (2.6d C16 fix)", () => {
    // A picture's `.`/`#` cells never pass through textRows, so the two
    // guards above never see one, and a letter it spells would pass both.
    // Mutation caught: an unlisted `export const FOO: readonly string[] =
    // [...]` added to a recipe or to marks.ts.
    expect([...picturesFound()].sort()).toEqual([...APPROVED_PICTURES].sort());
  });
});
