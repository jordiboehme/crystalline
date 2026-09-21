/**
 * What a download needs and the overlay does not otherwise have: a name that
 * says which document and which drawing, and the hand-over to the browser.
 *
 * The hash is FNV-1a, 32 bits, over the diagram's source: not for security,
 * for identity. Two diagrams in one document get two names, the same diagram
 * gets the same name on every visit, and four hex characters are enough to
 * keep a folder of downloads apart without making the name unreadable.
 */

const FNV_OFFSET = 0x811c9dc5;
const FNV_PRIME = 0x01000193;

/** FNV-1a over the UTF-16 code units, as eight lowercase hex characters. */
export function fnv1a32(text: string): string {
  let hash = FNV_OFFSET;
  for (let i = 0; i < text.length; i += 1) {
    hash ^= text.charCodeAt(i);
    // `Math.imul` rather than `*`: the product of two 32-bit numbers leaves
    // the range a double holds exactly, and a plain multiply would quietly
    // round the low bits this hash is made of away.
    hash = Math.imul(hash, FNV_PRIME) >>> 0;
  }
  return hash.toString(16).padStart(8, "0");
}

/** `<slug>-diagram-<hash4>.<ext>`, or `diagram-<hash4>.<ext>` with no slug. */
export function diagramFileName(
  slug: string | undefined,
  source: string,
  extension: "mmd" | "svg",
): string {
  const base = slug === undefined ? "diagram" : `${slug}-diagram`;
  return `${base}-${fnv1a32(source).slice(0, 4)}.${extension}`;
}

/**
 * The last path segment of a written image target, fragment and query
 * stripped; "image" when there is none.
 *
 * The segment is decoded, the rule `assetPath` applies to a path: the
 * renderer hands a target over percent-encoded, and a file called
 * `map%20of%20town.png` on disk is nobody's idea of the picture's name. A
 * target that is already decoded passes through unchanged, so a caller that
 * decoded it for its own reasons may hand over either one.
 */
export function imageFileName(target: string): string {
  const bare = target.split(/[?#]/)[0] ?? "";
  const last =
    bare
      .split("/")
      .filter((part) => part !== "")
      .at(-1) ?? "";
  let name = last;
  try {
    name = decodeURIComponent(last);
  } catch {
    // A target that will not decode is handed over as written.
  }
  return name === "" ? "image" : name;
}

/**
 * A Blob handed to the browser as a download under this name: object URL,
 * hidden anchor, click, revoke.
 *
 * The anchor is never in the document. It does not need to be - a click on a
 * detached anchor still starts the download - and an element added to the
 * page would have to be taken out again on a path that may throw.
 */
export function saveBlob(blob: Blob, name: string): void {
  const href = URL.createObjectURL(blob);
  const anchor = document.createElement("a");
  anchor.href = href;
  anchor.download = name;
  anchor.click();
  URL.revokeObjectURL(href);
}
