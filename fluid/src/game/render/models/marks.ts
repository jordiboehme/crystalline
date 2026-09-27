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
 * maker's logo, the hammer's runes, the trefoil) is a picture: rows of
 * characters, row 0 at the top, `.` dark, drawn with `pixelPanel` or
 * `pixelBoxes`. A multi-line text mark (`boardLogo`, `boxNotice`) is a
 * `readonly string[]` of lines, set through `textBlock`/`markLines`
 * (`heroes/pixels.ts`), never a picture of its own.
 *
 * Comments here name props by their shape, never an original.
 */

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
  /** The tube bench's two embossed labels, separate marks: one across the tube area, one by the door. */
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

/**
 * The thunder hammer's rune band (2.6f C13, C14): a schematic angular
 * band, not a literal transliteration and not the font's letters, five
 * separate marks each two columns wide with one dark column between two,
 * so it reads as a continuous carved band rather than a solid bar or a
 * word.
 */
export const HAMMER_RUNES: readonly string[] = [
  ".#.#..##.#..#.",
  "#...#..#..#.#.",
  ".#.#..##.#...#",
  "#...#.#...#.#.",
  "#...#.##.#..#.",
];

/**
 * The fuel case's trefoil (2.6f C13, C14): `#` the trefoil ink, one blade
 * straight up and two more at the lower left and right (the case's own
 * three sector angles), a filled disc at the centre and a dark ring
 * between the disc and the blades. Mirror-symmetric left to right, since
 * the case's own angles put one blade on the vertical axis and the other
 * two at equal angles either side of it.
 */
export const TREFOIL_MARK: readonly string[] = [
  "..#######..",
  "...#####...",
  "....###....",
  "...........",
  "....###....",
  "....###....",
  "....###....",
  "...........",
  "..#.....#..",
  ".##.....##.",
  "###.....###",
];
