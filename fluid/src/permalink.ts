/**
 * The permalink a path derives, spelled the way the engine spells it.
 *
 * The move dialog shows what an engram will answer to before the move is
 * sent, so it needs the engine's own derivation rather than a guess at it:
 * `crystalline_core::slugify` lowercases the path, drops a `.md` suffix, turns
 * every run of characters outside `[a-z0-9/-]` into one hyphen and trims each
 * folder segment of hyphens at its ends, dropping a segment left empty. The
 * move's receipt names the permalink the engine actually chose, so a
 * disagreement here would cost a wrong preview and never a wrong address.
 */

/** The permalink a domain-relative path answers to when nothing overrides it. */
export function pathPermalink(path: string): string {
  const stem = path.replace(/\.(md|MD)$/, "");
  const collapsed = stem.toLowerCase().replace(/[^a-z0-9/-]+/g, "-");
  return collapsed
    .split("/")
    .map((segment) => segment.replace(/^-+|-+$/g, ""))
    .filter((segment) => segment !== "")
    .join("/");
}

/** The folder part of a permalink: everything before the last `/`. */
export function permalinkFolder(permalink: string): string {
  const cut = permalink.lastIndexOf("/");
  return cut < 0 ? "" : permalink.slice(0, cut);
}

/** The destination path the move sends, with its `.md` suffix. */
export function destinationFile(destination: string): string {
  const trimmed = destination
    .trim()
    .split("/")
    .filter((segment) => segment !== "")
    .join("/");
  if (trimmed === "") {
    return "";
  }
  return /\.md$/i.test(trimmed) ? trimmed : `${trimmed}.md`;
}

/**
 * What an engram answers to after a move onto `destination`, when the move
 * sends no permalink of its own: the destination's own when the current one
 * was in step with the current path, the current one otherwise.
 */
export function defaultMovedPermalink(
  permalink: string,
  path: string | null,
  destination: string,
): string {
  const inStep = path === null || permalink === pathPermalink(path);
  return inStep ? pathPermalink(destinationFile(destination)) : permalink;
}
