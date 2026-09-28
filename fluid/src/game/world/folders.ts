/**
 * Folders, folder slugs, deck numbers and sections (M3 C6, C8): the pure
 * layer decks, lifts and the bridge's lift are built on, kept apart from
 * `paths.ts` because a folder is a tree path, not an address on its own -
 * `StationAddress`'s `deck` variant carries one, but nothing here knows
 * about routes.
 *
 * A folder is the raw path a level of the tree sits at, `""` for a domain's
 * root, `"a/b"` for a nested one. `slugPath` is a small pure port of the
 * server's `slugify` (`crates/core/src/address.rs`), so a folder's slug and
 * a deck's number agree with the permalinks the engine actually gives the
 * engrams inside it, even though the game never talks to the server for
 * this: `folders.test.ts` pins the port against the Rust test vectors and
 * against Fluid's own `pathPermalink`, so the two ports cannot drift apart.
 *
 * Sections cut a deck's engrams into hubs of at most `SECTION_SIZE`, sorted
 * by permalink so the cut never depends on the order an API answered in.
 */

import { deckNumber } from "./decals";

/**
 * A path slugged exactly as the server's `slugify` slugs a file's path into
 * its permalink: a trailing `.md` or `.MD` dropped, lowercased, every run of
 * characters outside `[a-z0-9/-]` collapsed to one hyphen, each `/`-segment
 * trimmed of hyphens at both ends, and an empty segment dropped.
 *
 * The same algorithm as `pathPermalink` in `fluid/src/permalink.ts`, ported
 * rather than imported: game code may not import outside `fluid/src/game`
 * except through the whitelist in the global constraints, and this small
 * function is cheaper to keep in step by test than to add to it.
 */
export function slugPath(path: string): string {
  const stem = path.replace(/\.(md|MD)$/, "");
  const collapsed = stem.toLowerCase().replace(/[^a-z0-9/-]+/g, "-");
  return collapsed
    .split("/")
    .map((segment) => segment.replace(/^-+|-+$/g, ""))
    .filter((segment) => segment !== "")
    .join("/");
}

/** The folder part of a permalink: everything before the last `/`, `""` without one. */
export function folderOfPermalink(permalink: string): string {
  const cut = permalink.lastIndexOf("/");
  return cut < 0 ? "" : permalink.slice(0, cut);
}

/**
 * A folder's own slug: the folder part of `slugPath` run on the folder with
 * a made-up `/x` file appended, so the server slugging a real file inside it
 * would land in the same place.
 *
 * The `/x` guard matters for a folder like `x.md`: without it, slugging the
 * folder name alone would drop the `.md` (the trailing-suffix rule only
 * fires at the very end of the string), while a file inside it slugs to
 * `x-md/...` because the suffix rule never reaches the folder's own `.md`
 * once something follows it. Appending `/x` puts the folder in that same
 * position before slugging, so the two always agree.
 */
export function folderSlug(folder: string): string {
  return folder === "" ? "" : folderOfPermalink(slugPath(`${folder}/x`));
}

/**
 * A deck's number (M3 C6): the same `deckNumber` a room's stencils read,
 * hashed from the folder's own slug rather than its raw path, so a deck's
 * label and the stencils of the rooms whose permalinks follow their path
 * agree. `1` for the root and for a folder whose slug is empty (an
 * all-punctuation name slugs away to nothing).
 */
export function folderDeck(domain: string, folder: string): number {
  const slug = folderSlug(folder);
  return slug === "" ? 1 : deckNumber(domain, `${slug}/_`);
}

/** A folder's own name: its last segment, `""` for the root. */
export function folderName(folder: string): string {
  if (folder === "") {
    return "";
  }
  const cut = folder.lastIndexOf("/");
  return cut < 0 ? folder : folder.slice(cut + 1);
}

/**
 * A folder's parent: everything before its last segment, `""` for a
 * top-level folder's parent (the root), `null` for the root's own parent -
 * the root has none to walk up to.
 */
export function parentFolder(folder: string): string | null {
  if (folder === "") {
    return null;
  }
  const cut = folder.lastIndexOf("/");
  return cut < 0 ? "" : folder.slice(0, cut);
}

/** A folder nested one level under `parent`, named `name`. */
export function childFolder(parent: string, name: string): string {
  return parent === "" ? name : `${parent}/${name}`;
}

/**
 * The folder a raw tree path sits in: no slugging, since a tree folder is a
 * raw path, not a permalink (`slugPath` is only ever applied to it later,
 * by `folderSlug`).
 */
export function folderOfPath(path: string): string {
  return folderOfPermalink(path);
}

/**
 * Whether a permalink is a domain's root MANIFEST (M3 C3, C22): the same
 * case-insensitive, root-only compare `DomainNav`'s `isPinnedManifest` pins
 * a listing row with, so the two agree on which row a domain's tree leaves
 * out of its root deck.
 */
export function isManifestPermalink(permalink: string): boolean {
  return permalink.toUpperCase() === "MANIFEST";
}

/** How many engrams one section of a deck holds at most (M3 C8). */
export const SECTION_SIZE = 24;

/**
 * A deck's engrams cut into sections of at most `SECTION_SIZE` (M3 C8):
 * sorted by permalink in code-unit order first, so the cut never depends on
 * the order an API answered in, then sliced every `SECTION_SIZE` rows. The
 * input is never mutated. Every section holds at least one row - slicing
 * never runs past the sorted array's length, so there is never a trailing
 * empty section.
 */
export function sectionsOf<T extends { permalink: string }>(
  rows: readonly T[],
): T[][] {
  const sorted = [...rows].sort((a, b) =>
    a.permalink < b.permalink ? -1 : a.permalink > b.permalink ? 1 : 0,
  );
  const sections: T[][] = [];
  for (let i = 0; i < sorted.length; i += SECTION_SIZE) {
    sections.push(sorted.slice(i, i + SECTION_SIZE));
  }
  return sections;
}

/** The upper-cased first character of a permalink's last `/`-segment, `""` for an empty one. */
function firstLetterOf(permalink: string): string {
  const cut = permalink.lastIndexOf("/");
  const segment = cut < 0 ? permalink : permalink.slice(cut + 1);
  const letter = segment.charAt(0);
  return letter === "" ? "" : letter.toUpperCase();
}

/**
 * A deck's section labels (M3 C8): per section, the upper-cased first
 * character of the last `/`-segment of its first and last engram, `A-F`
 * when they differ or one letter (`C`) when they agree; then, for every
 * label that more than one section of the deck would carry, ` 1`, ` 2` and
 * so on appended in section order, so two sections never read the same on
 * the deck's lift or screen.
 */
export function sectionLabels(
  sections: readonly (readonly { permalink: string }[])[],
): string[] {
  const base = sections.map((section) => {
    const first = section[0];
    const last = section[section.length - 1];
    if (first === undefined || last === undefined) {
      return "";
    }
    const from = firstLetterOf(first.permalink);
    const to = firstLetterOf(last.permalink);
    return from === to ? from : `${from}-${to}`;
  });
  const counts = new Map<string, number>();
  for (const label of base) {
    counts.set(label, (counts.get(label) ?? 0) + 1);
  }
  const seen = new Map<string, number>();
  return base.map((label) => {
    const total = counts.get(label) ?? 0;
    if (total <= 1) {
      return label;
    }
    const ordinal = (seen.get(label) ?? 0) + 1;
    seen.set(label, ordinal);
    return `${label} ${String(ordinal)}`;
  });
}

/**
 * Which section of a deck holds a permalink, `-1` when none of them do (the
 * engram is not a member of this level, or the deck was built from a
 * truncated page that left it out).
 */
export function sectionOfPermalink(
  sections: readonly (readonly { permalink: string }[])[],
  permalink: string,
): number {
  for (let i = 0; i < sections.length; i++) {
    const section = sections[i];
    if (
      section !== undefined &&
      section.some((row) => row.permalink === permalink)
    ) {
      return i;
    }
  }
  return -1;
}

/**
 * A section index clamped to a deck's actual section count: `null` (M3 C1's
 * unresolved deck) reads as the first section, and an index past the last
 * one clamps to the last, so a stale link or a section number typed past
 * the end never reads as an address with no room behind it. `0` for a deck
 * with no sections at all (an empty folder).
 */
export function clampSection(section: number | null, count: number): number {
  if (count <= 0) {
    return 0;
  }
  if (section === null || section < 0) {
    return 0;
  }
  return section > count - 1 ? count - 1 : section;
}
