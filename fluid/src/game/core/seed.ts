/**
 * Seeds for a world that regenerates identically.
 *
 * Every thing in the station (a room, a machine, a light) gets a seed hashed
 * from a path of names, the way No Man's Sky seeds a planet from its
 * coordinates. Nothing is stored: the same names always give the same seed,
 * and adding a new thing never reshuffles the ones that already exist,
 * because no seed depends on how many siblings came before it.
 *
 * `cyrb53` hashes the path and `sfc32` turns a seed into a stream of numbers.
 * Both are the small public-domain functions from bryc's collection; they are
 * fast, have no dependency and are good enough for scenery, which is all they
 * are ever asked for.
 */

/**
 * A 53-bit string hash (bryc's cyrb53). The result is a non-negative safe
 * integer, so it survives JSON and arithmetic without losing bits.
 */
export function cyrb53(text: string, seed = 0): number {
  let h1 = 0xdeadbeef ^ seed;
  let h2 = 0x41c6ce57 ^ seed;
  for (let i = 0; i < text.length; i++) {
    const ch = text.charCodeAt(i);
    h1 = Math.imul(h1 ^ ch, 2654435761);
    h2 = Math.imul(h2 ^ ch, 1597334677);
  }
  h1 = Math.imul(h1 ^ (h1 >>> 16), 2246822507);
  h1 ^= Math.imul(h2 ^ (h2 >>> 13), 3266489909);
  h2 = Math.imul(h2 ^ (h2 >>> 16), 2246822507);
  h2 ^= Math.imul(h1 ^ (h1 >>> 13), 3266489909);
  return 4294967296 * (2097151 & h2) + (h1 >>> 0);
}

/**
 * The separator between the parts of a seed path. A NUL can appear in
 * neither a domain name nor a permalink nor a tag, so `("a/b", "c")` and
 * `("a", "b/c")` hash different strings.
 */
const SEPARATOR = "\u0000";

/**
 * The seed of the thing a path of names identifies, for example
 * `seedFor(GAME_VERSION, domain, permalink)` for a room and
 * `seedFor(roomSeed, "tag", name)` for one machine in it.
 */
export function seedFor(...parts: readonly (string | number)[]): number {
  return cyrb53(parts.map(String).join(SEPARATOR));
}

/** A stream of numbers from one seed. */
export interface Rng {
  /** A number in [0, 1). */
  next(): number;
  /** A number in [min, max). */
  range(min: number, max: number): number;
  /** An integer in [min, maxInclusive]. */
  int(min: number, maxInclusive: number): number;
  /** One item of a non-empty list. */
  pick<T>(items: readonly T[]): T;
  /** True with probability `p`. */
  chance(p: number): boolean;
}

/**
 * An `sfc32` generator seeded from a 53-bit seed. The seed is split into its
 * low and high words, mixed into the four words of state, and the first
 * dozen outputs are thrown away so neighbouring seeds do not start alike.
 */
export function createRng(seed: number): Rng {
  let a = seed >>> 0;
  let b = Math.floor(seed / 4294967296) >>> 0;
  let c = (a ^ 0x9e3779b9) >>> 0;
  let d = (b ^ 0x85ebca6b) >>> 0;
  const next = (): number => {
    a |= 0;
    b |= 0;
    c |= 0;
    d |= 0;
    const t = (((a + b) | 0) + d) | 0;
    d = (d + 1) | 0;
    a = b ^ (b >>> 9);
    b = (c + (c << 3)) | 0;
    c = (c << 21) | (c >>> 11);
    c = (c + t) | 0;
    return (t >>> 0) / 4294967296;
  };
  for (let i = 0; i < 12; i++) next();
  return {
    next,
    range: (min, max) => min + (max - min) * next(),
    int: (min, maxInclusive) =>
      min + Math.floor(next() * (maxInclusive - min + 1)),
    pick<T>(items: readonly T[]): T {
      if (items.length === 0) throw new Error("pick from an empty list");
      return items[Math.floor(next() * items.length)] as T;
    },
    chance: (p) => next() < p,
  };
}
