/**
 * The generator's version, folded into every seed the station derives.
 *
 * The station is regenerated from the content on every visit and nothing is
 * stored, so the only thing that can make a room look different while its
 * engram stays the same is a change to the generator itself. Bumping this
 * number is how such a change is made on purpose: every seed moves with it,
 * and the golden rooms in `world/golden/` are rewritten in the same commit.
 * A generator change that moves a golden without bumping this is a bug.
 */
export const GAME_VERSION = 2;
