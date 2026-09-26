/**
 * The originals' own text and marks the station draws (2.6d C16). Where an
 * original's letters, marks or logo make a prop more recognisable, the
 * station uses them, as on the original (the text-and-logos rule, Jordi,
 * 2026-09-26). Every such string and picture of 2.6d lives here and
 * nowhere else: the recipes import these constants and never spell a
 * readable literal, and `heroText.test.ts` allows exactly these strings.
 *
 * The block-pixel font has capitals, lower-case letters, digits, the
 * space, the period and `<`, `=`, `>`, so a badge is set as the original
 * sets it: in capitals, or in lower case where the original's wordmark is
 * lower case, and a number in numerals. A mark that is no text (the round
 * C, the signature, the planet badge, the computers' maker's logo) is a
 * picture: rows of characters,
 * row 0 at the top, `.` dark, drawn with `pixelPanel` or `pixelBoxes`.
 *
 * Comments here name props by their shape, never an original.
 */

import type { Rgb } from "../looks";

/** Every approved string of 2.6d, by the prop that draws it. */
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
} as const;

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
