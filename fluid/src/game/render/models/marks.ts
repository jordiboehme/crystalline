/**
 * The originals' own text and marks the station draws (2.6d C16). Where an
 * original's letters, marks or logo make a prop more recognisable, the
 * station uses them, as on the original (the text-and-logos rule, Jordi,
 * 2026-09-26). Every string of 2.6d and 2.6f, and every picture that is a
 * logo or a badge, lives here and nowhere else: the recipes import these
 * constants and never spell a readable literal, and `heroText.test.ts`
 * allows exactly these strings. The one exception is the saucer poster's
 * photograph (`SAUCER_PICTURE`, `props/rare.ts`), which stays beside its
 * recipe since it is the original's image, not a logo or a text mark.
 *
 * The block-pixel font has capitals, lower-case letters, digits, the
 * space, the period, `&` and `<`, `=`, `>`, so a badge is set as the
 * original sets it: in capitals, or in lower case where the original's
 * wordmark is lower case, and a number in numerals. A mark that is no
 * text (the round C, the signature, the planet badge, the computers'
 * maker's logo, the hammer's runes) is a picture: rows of
 * characters, row 0 at the top, `.` dark, drawn with `pixelPanel` or
 * `pixelBoxes`. A multi-line text mark (`boardLogo`, `boxNotice`) is a
 * `readonly string[]` of lines, set through `textBlock`/`markLines`
 * (`heroes/pixels.ts`), never a picture of its own.
 *
 * The deck and bay stencils' words, numerals and letters (2.7 C19) live
 * here too, and `stencilMarks` sets a stencil from them, mark by mark; so
 * do the airlock's words (M3 C24): its hatch's stencil and its floor's
 * `CYCLE`, which `wordMarks` sets.
 *
 * Comments here name props by their shape, never an original.
 */

import { LIFT_WORDS } from "../../world/lifts";
import type { StencilText, StencilWord } from "../../world/types";
import type { Rgb } from "../looks";

/** Every approved string of 2.6d and 2.6f, by the prop that draws it. */
export const MARKS = {
  /** The saucer poster's caption, white on its dark lower band. */
  poster: "I WANT TO BELIEVE",
  /** The capsule maker's word, under its round C: capsule case, marked crate, gravity console. */
  capsuleWord: "CAPSULE CORP.",
  /** The five capsules' numbers, one each. */
  capsuleNumbers: ["1", "2", "3", "4", "5"],
  /** The ooze canisters' maker's letters. */
  canister: "TCRI",
  /** The designer tower's badge. */
  towerBadge: "HIGHSCREEN",
  /** The designer tower's clock-speed display, red. */
  towerClock: "40",
  /** The gravity console's readout. */
  gravity: "300G",
  /** Both desk computers' badge, in the original's lower-case wordmark. */
  computerBadge: "commodore 64",
  /** The reactor case's plaque line, in capitals. */
  reactorPlaque: "PROOF THAT TONY STARK HAS A HEART",

  /** The eye panel's badge: two separate fields, not one block, "HAL" on the panel's own dark trim and "9000" on a blue field. */
  panelName: ["HAL", "9000"],
  /** The photo console's small wordmark plate, below the screen. */
  deskBadge: "esper",
  /** The tube bench's two embossed labels, separate marks: the short one on the bottom rail under the tubes, the long one along the strip over the status lights. */
  benchLabels: [
    "SHIELD EYES FROM LIGHT",
    "DISCONNECT CAPACITOR DRIVE BEFORE OPENING",
  ],
  /** The recruitment cabinet's marquee, steady and lit. */
  recruitMarquee: "STARFIGHTER",
  /** The red bike's fairing stickers, four separate marks, one per sponsor. */
  bikeStickers: ["CANON", "CITIZEN", "ARAI", "SHOEI"],
  /** The hoverboard's deck wordmark, one two-line block: "HOVER" over "BOARD". */
  boardLogo: ["HOVER", "BOARD"],
  /** The police box's left door notice, nine lines set as one block. */
  boxNotice: [
    "POLICE TELEPHONE",
    "FREE",
    "FOR USE OF",
    "PUBLIC",
    "ADVICE & ASSISTANCE",
    "OBTAINABLE IMMEDIATELY",
    "OFFICER & CARS",
    "RESPOND TO ALL CALLS",
    "PULL TO OPEN",
  ],
  /** The pocket console's bezel badge, two separate marks side by side: the maker's name left of the console's own. */
  consoleBadge: ["Nintendo", "GAME BOY"],
  /** The tape drive's top badge, the maker's word alone, in its lower-case wordmark (no model name, unlike the desk computers' badge). */
  driveBadge: "commodore",
  /** The tape player's lid badge, two separate marks: the maker beside the model's own wordmark. */
  playerBadge: ["SONY", "WALKMAN"],
  /** The fuel case's three hazard labels, separate marks at three different spots on the case and its canisters. */
  caseLabels: [
    "RADIOACTIVE III",
    "CAUTION RADIOACTIVE MATERIAL",
    "PLUTONIUM HANDLE WITH CARE",
  ],

  /** The wall stencil's first word, over the deck's number (2.7 C19). */
  deckWord: "DECK",
  /** Every stencil's word before the bay's number (2.7 C19). */
  bayWord: "BAY",
  /** The stencils' numerals, each set on its own, by its value: `numerals[7]` is "7". */
  numerals: ["0", "1", "2", "3", "4", "5", "6", "7", "8", "9"],
  /** The overflow bays' letters after the bay's number, bay 1 to 4: `bayLetters[0]` is "A". */
  bayLetters: ["A", "B", "C", "D"],

  /** The airlock's outer hatch's stencil (M3 C24): the lift's own word for the place. */
  airlockWord: LIFT_WORDS.airlock,
  /** The airlock's floor stencils inside its hazard ring (M3 C24). */
  cycleWord: "CYCLE",
} as const;

/**
 * A word stencil's marks (M3 C24): one line of one mark, the word's own
 * literal from `MARKS`, so a floor stencil that reads a word hands
 * `textRows` an approved string as a deck and bay stencil does.
 */
export function wordMarks(word: StencilWord): readonly (readonly string[])[] {
  const marks = { cycle: MARKS.cycleWord } as const satisfies Record<
    StencilWord,
    string
  >;
  return [[marks[word]]];
}

/**
 * A stencil's marks (2.7 C19), line by line, each line's marks in reading
 * order: the wall's two lines (`lines` 2) are the deck word and the deck's
 * numerals over the bay word and the bay's numerals; a floor stencil's one
 * line is the bay's alone. An overflow bay's letter (`letter` 1 to 4)
 * follows the bay's numerals. Every mark is one of `MARKS`' own literals,
 * so the recipe hands `textRows` nothing but approved strings, and a
 * number is set digit by digit, never as a string made from it. The
 * recipe lays the marks with the font's own advance, a space after the
 * word. Throws on a number outside 1 to 99 or a letter outside 0 to 4,
 * which no placed stencil carries.
 */
export function stencilMarks(t: StencilText): readonly (readonly string[])[] {
  const digits = (n: number): string[] => {
    if (!Number.isInteger(n) || n < 1 || n > 99)
      throw new Error(`stencilMarks: no number ${String(n)}`);
    const out: string[] = [];
    for (let m = n; m > 0; m = Math.floor(m / 10)) {
      const mark = MARKS.numerals[m % 10];
      if (mark === undefined) throw new Error("stencilMarks: no numeral");
      out.unshift(mark);
    }
    return out;
  };
  if (!Number.isInteger(t.letter) || t.letter < 0 || t.letter > 4)
    throw new Error(`stencilMarks: no letter ${String(t.letter)}`);
  const letter = t.letter === 0 ? undefined : MARKS.bayLetters[t.letter - 1];
  const bay = [
    MARKS.bayWord,
    ...digits(t.bay),
    ...(letter === undefined ? [] : [letter]),
  ];
  return t.lines === 2 ? [[MARKS.deckWord, ...digits(t.deck)], bay] : [bay];
}

/** The capsule maker's round C: `o` the outer ring, `#` the C, open to the right. */
export const CAPSULE_LOGO: readonly string[] = [
  "...ooooo...",
  ".oo.....oo.",
  ".o..###..o.",
  "o..#......o",
  "o.#.......o",
  "o.#.......o",
  "o.#.......o",
  "o..#......o",
  ".o..###..o.",
  ".oo.....oo.",
  "...ooooo...",
];

/** The designer's signature on the tower: one unbroken squiggle, no letters. */
export const SIGNATURE: readonly string[] = [
  "...#..........#...",
  "..#.#..##..#.#.#..",
  ".#...##..##.#...#.",
  "#................#",
];

/** The capsule maker's dark blue, for the round C and the word on every prop that carries them. */
export const MARK_BLUE: Rgb = [0.1, 0.2, 0.5];

/** The space bricks' figure's chest badge: `p` the planet, `o` the orbit across it. */
export const SPACE_BADGE: readonly string[] = [
  ".....o.",
  "..ppo..",
  ".ppop..",
  "..op...",
  ".o.....",
];

/**
 * The desk computers' maker's logo, left of the word on the breadbin's
 * badge: `#` a thick C open to the right, with two short flags in its
 * mouth, the upper one along the C's top end and the lower one along its
 * bottom end, each cut on a slant towards the middle, and the mouth open
 * between them. Symmetric top to bottom.
 */
export const COMPUTER_LOGO: readonly string[] = [
  ".##.##",
  "#...#.",
  "#.....",
  "#...#.",
  ".##.##",
];

/**
 * The thunder hammer's rune band (2.6f C13, C14): `#` the rune ink, one
 * dark column between each of five separate marks each two columns wide,
 * a schematic angular band, not a literal transliteration and not the
 * font's letters, so it reads as a continuous carved band rather than a
 * solid bar or a word.
 */
export const HAMMER_RUNES: readonly string[] = [
  ".#.#..##.#..#.",
  "#...#..#..#.#.",
  ".#.#..##.#...#",
  "#...#.#...#.#.",
  "#...#.##.#..#.",
];
