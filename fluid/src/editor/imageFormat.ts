/**
 * How an image says where it stands: a URL fragment on its own target.
 *
 * `![Shot](assets/2026/08/shot.png#right,w=50%)` is a floated half-width
 * image here and an ordinary image everywhere else, which is the whole reason
 * the convention is a fragment rather than an attribute or a wrapper. A
 * fragment is legal markdown in every renderer, every one of them ignores it
 * and still draws the picture, and the file route never sees it: the server
 * strips fragments before it resolves a reference, so `#right` is a view
 * concern from end to end. What lands in somebody's engram is prose that reads
 * as prose.
 *
 * That puts one property above the rest, and it is what the tests are built
 * on: what parses rebuilds identically. The toolbar menu writes into a
 * document a human owns, so a click that reordered directives or dropped a
 * width would churn the file for nothing. Directive order is therefore fixed
 * on the way out (placement, then width), the default is written as nothing at
 * all, and a directive this app does not honor is dropped rather than carried
 * along as a promise nobody keeps.
 *
 * `#` may not appear inside a stored asset path - the core validator refuses
 * one - so the split is unambiguous and needs no escaping rule.
 *
 * THE MIRROR. This module also decides which targets Fluid treats as
 * attachments at all, and that answer has to be the engine's answer: a
 * reference the core counts and Fluid ignores is a broken image beside a rail
 * that lists nothing, and the reverse is a rail claiming a file the sweep will
 * call orphaned. `crates/core/src/attachment.rs` (`find_asset_refs`,
 * `line_targets`) is the authority and these are its rules, transcribed:
 *
 * - both markdown forms count, `![alt](assets/x.png)` and `[text](assets/x.pdf)`;
 * - a reference is found by scanning for `](`, so brackets inside the label
 *   (`![a [b] c](assets/a.png)`) cannot hide one;
 * - a destination ends at the DEPTH-ZERO `)`, so a balanced pair inside it
 *   (`assets/a(1).png`) belongs to the target;
 * - an unclosed `](` ends the scan of that line;
 * - the destination is trimmed and cut at the first whitespace, so a title
 *   clause (`(assets/x.png "Q3")`) is dropped;
 * - a leading `./` is stripped before the `assets/` prefix is tested;
 * - the fragment is cut at the first `#`, and a target left naming no file
 *   (`assets/`, `assets/#left`) is dropped;
 * - fenced code is skipped, where a fence opens on three or more backticks or
 *   tildes at up to three spaces of indent and closes only on the same
 *   character, at that length or longer, with nothing else on the line.
 *
 * The two sides now decode alike: since 0.15.1 the core percent-decodes a
 * target the way {@link decodeTarget} does, strict hex nibbles and the raw
 * token back when the escape will not decode, so the rail, the reading page
 * and the maintenance sweep answer one path for one file. The agreement is
 * pinned rather than asserted, by the corpus both suites read -
 * `crates/core/tests/fixtures/asset_ref_corpus.json`, checked here in
 * imageFormat.test.ts and there in attachment.rs - so changing what either
 * scanner answers means changing the corpus, and changing the corpus means
 * moving the other side in the same commit.
 *
 * One divergence is left and is stated where it is made: {@link assetPath}
 * refuses a `.` or `..` segment, which the core's SCANNER counts but its
 * VALIDATOR would never let be stored.
 */

import { ASSETS_PREFIX, isImageAttachment } from "../api/files";

/** Where an image stands in the column of prose around it. */
export type ImageAlign = "center" | "full" | "left" | "right";

/** Everything the fragment can say about one image. */
export interface ImageFormat {
  align: ImageAlign;
  /**
   * The width as written: `"300"` for pixels, `"50%"` for a share of the
   * column. Absent means the image keeps its own size, capped at the column.
   */
  width?: string;
}

/** What an image with no fragment means, and what a rebuild writes nothing for. */
export const DEFAULT_ALIGN: ImageAlign = "center";

/** The placements a directive may name, as a set the parser tests against. */
const ALIGNMENTS = new Set<string>(["center", "full", "left", "right"]);

/** A width nobody can render is not a width: digits, optionally a percent. */
const WIDTH = /^w=(\d+%?)$/;

/**
 * The target as the author wrote it, out of the URL a renderer hands over.
 *
 * micromark percent-encodes every link and image target on its way through - a
 * stray `%` becomes `%25`, a name written in Japanese becomes a row of UTF-8
 * escapes - so what the reading view is handed is a URL rather than the stored
 * path it was written as. The rail and the editor read the raw source, where no
 * such encoding happened. Both come through here, exactly once, so the two
 * answer the same path for the same file: a hand-written `%E8%A8%AD…` target
 * lists in the rail as the file it draws on the page, rather than as missing.
 *
 * A target that will not decode is handed back exactly as it came: it is then
 * not a path this app can resolve either way, and a thrown `URIError` inside a
 * render would take the whole document down over one malformed link.
 *
 * The cost is stated rather than hidden: a stored name holding a literal `%25`
 * is indistinguishable from one holding `%`, which is the parked finding about
 * banning `%` in asset paths. The core's scanner decodes the same way as of
 * 0.15.1 and pays the same cost, so this is no longer a divergence: it is one
 * rule kept in two languages, and the shared corpus in
 * `crates/core/tests/fixtures` is what keeps the two spellings honest.
 */
export function decodeTarget(target: string): string {
  try {
    return decodeURIComponent(target);
  } catch {
    return target;
  }
}

/**
 * The stored path a target names, or null when it names none.
 *
 * The canonical form, and the one identity every surface uses: the reading
 * view builds its URL from it, the editor asks the files route for it, and the
 * rail matches the domain's listing against it. It mirrors the core scanner's
 * own reading of a target - decode aside, see {@link decodeTarget} - so a
 * reference this app resolves is a reference the engine counts:
 *
 * - a leading `./` is stripped, so `./assets/a.png` is `assets/a.png`;
 * - the fragment is dropped at the first `#`, which a stored path can never
 *   hold;
 * - the result must sit under `assets/` with something after it, so `assets/`
 *   and `assets/#left` name no file and answer null.
 *
 * One rule is stricter than the scanner's on purpose: a `.` or `..` segment
 * answers null. The core VALIDATOR refuses such a path outright, so no file
 * can be stored under one, and a URL built from it would ask the browser for a
 * different address than the one written. Nothing is lost by refusing it here -
 * there is no file at the other end either way - and the maintenance sweep is
 * what tells an author their reference is dangling.
 */
export function assetPath(target: string): string | null {
  const decoded = decodeTarget(target);
  const relative = decoded.startsWith("./") ? decoded.slice(2) : decoded;
  const hash = relative.indexOf("#");
  const path = hash < 0 ? relative : relative.slice(0, hash);
  const rest = path.startsWith(ASSETS_PREFIX)
    ? path.slice(ASSETS_PREFIX.length)
    : null;
  if (rest === null || rest === "") {
    return null;
  }
  if (rest.split("/").some((segment) => segment === "." || segment === "..")) {
    return null;
  }
  return path;
}

/**
 * Whether this target is an attachment of the domain the document lives in.
 *
 * Relative and under the reserved prefix, which is exactly what an upload
 * writes. A leading slash, a scheme or a host make the target somebody else's
 * address, and this app rewrites none of those.
 */
export function isAssetTarget(target: string): boolean {
  return assetPath(target) !== null;
}

/**
 * A target split into the path the server knows and the format only this app
 * reads.
 *
 * Total by construction: a target with no fragment, an empty fragment or a
 * fragment made entirely of directives this app does not honor all come back
 * as the default - a centered image at its own size - which is what an upload
 * inserts and what every other renderer draws.
 */
export function parseImageFragment(target: string): {
  path: string;
  format: ImageFormat;
} {
  const hash = target.indexOf("#");
  if (hash < 0) {
    return { path: target, format: { align: DEFAULT_ALIGN } };
  }
  const path = target.slice(0, hash);
  let align: ImageAlign = DEFAULT_ALIGN;
  let width: string | null = null;
  for (const directive of target.slice(hash + 1).split(",")) {
    const token = directive.trim();
    if (ALIGNMENTS.has(token)) {
      // The last one wins rather than the first: a fragment naming two
      // placements is a mistake either way, and reading it the way the toolbar
      // writes it - append, then re-parse - keeps the two halves agreeing.
      align = token as ImageAlign;
      continue;
    }
    const measured = WIDTH.exec(token);
    if (measured?.[1] !== undefined) {
      width = measured[1];
    }
  }
  return {
    path,
    format: width === null ? { align } : { align, width },
  };
}

/**
 * The target one path and one format are written as.
 *
 * The default is silence: a centered image at its own size is a bare path,
 * which is what an upload inserts, so choosing Centered in the menu leaves the
 * document as clean as it started. A width beside a centered image states only
 * the width, for the same reason.
 */
export function buildImageTarget(path: string, format: ImageFormat): string {
  const directives: string[] = [];
  if (format.align !== DEFAULT_ALIGN) {
    directives.push(format.align);
  }
  if (format.width !== undefined && format.width !== "") {
    directives.push(`w=${format.width}`);
  }
  return directives.length === 0 ? path : `${path}#${directives.join(",")}`;
}

/**
 * The css one format means, as declarations both surfaces can apply.
 *
 * One function rather than a rule per surface: the reading page and the
 * editor's preview widget promise the same picture, and a float that only the
 * page honored would make the preview a lie about what a reader will see. The
 * page hands this to React's `style`, the widget assigns it onto an element's
 * own - the keys are camel case, which both take.
 *
 * `maxWidth` is on every branch and is the one rule a directive cannot
 * override: an image wider than the column would push the prose sideways,
 * whatever the author asked for. `height: auto` rides along with it so a width
 * changes the picture's size rather than its proportions.
 */
export function imageStyle(format: ImageFormat): Record<string, string> {
  const base: Record<string, string> = { maxWidth: "100%", height: "auto" };
  if (format.align === "left" || format.align === "right") {
    // Text wraps around a floated image, so the gap belongs on the side the
    // prose is on and under it, where the next line would otherwise touch.
    base.float = format.align;
    base[format.align === "left" ? "marginRight" : "marginLeft"] = "1rem";
    base.marginBottom = "0.5rem";
  } else {
    // Centered and full are both blocks of their own; centering is the auto
    // margins, which a full-width image simply has no room to use.
    base.display = "block";
    base.marginLeft = "auto";
    base.marginRight = "auto";
    if (format.align === "full") {
      base.width = "100%";
    }
  }
  if (format.width !== undefined && format.width !== "") {
    // A bare number is pixels, the way markdown authors write one; a percent
    // is a share of the column and travels as written.
    base.width = format.width.endsWith("%")
      ? format.width
      : `${format.width}px`;
  }
  return base;
}

/** One image reference to an attachment, and where its target sits in the text. */
export interface ImageRef {
  /** The whole `![alt](target)` span. */
  from: number;
  to: number;
  /** The target token alone, which is what a format change rewrites. */
  targetFrom: number;
  targetTo: number;
  /**
   * The path as the author wrote it, fragment stripped and nothing else: a
   * leading `./` is theirs and a rewrite puts it back rather than normalizing
   * somebody's prose on the way past.
   */
  written: string;
  /** The stored path: what the files route is asked for. */
  path: string;
  format: ImageFormat;
}

/**
 * One `](…)` destination on a line, as the core's `line_targets` reads one.
 */
interface RawRef {
  /** Whether the label that opened it carried the image `!`. */
  image: boolean;
  /** The target token's own span, so a title clause survives a rewrite. */
  targetFrom: number;
  targetTo: number;
  /** The whole `[label](…)` span, from the opening bracket to the `)`. */
  from: number;
  to: number;
  /** The token as written. */
  token: string;
}

/**
 * A fence marker, as `crate::parse::fence_marker` reads one: at most three
 * spaces of indent, then three or more backticks or tildes. Returns the
 * character, its run length and the offset the run starts at.
 */
function fenceMarker(line: string): { char: string; count: number } | null {
  const indent = line.length - line.replace(/^ +/, "").length;
  if (indent > 3) {
    return null;
  }
  const rest = line.slice(indent);
  const first = rest[0];
  if (first !== "`" && first !== "~") {
    return null;
  }
  let count = 0;
  while (rest[count] === first) {
    count += 1;
  }
  return count < 3 ? null : { char: first, count };
}

/**
 * Every `](…)` destination on one line, in order, duplicates included.
 *
 * A transcription of `line_targets` in `crates/core/src/attachment.rs`, which
 * is the authority, and the rules are its rules rather than a regex's
 * approximation of them:
 *
 * - a reference is found by scanning for `](` rather than by matching a label,
 *   so brackets INSIDE the alt text (`![a [b] c](assets/a.png)`) cannot hide
 *   one;
 * - the destination ends at the DEPTH-ZERO `)`, so a balanced pair inside it
 *   (`assets/a(1).png`) is part of the target rather than the end of it;
 * - a line whose `](` never closes stops the scan of that line, exactly where
 *   the core's `break` does;
 * - the destination is trimmed and cut at the first whitespace, so a title
 *   clause (`(assets/a.png "Q3")`) is dropped rather than swallowed.
 *
 * Whether the label was an image is not a question the core asks - it counts
 * both forms - but this side draws pictures for one and links for the other,
 * so the opening bracket is found by walking back through balanced brackets
 * and the `!` in front of it is what decides.
 */
function lineRefs(line: string): RawRef[] {
  const refs: RawRef[] = [];
  let index = 0;
  for (;;) {
    const hit = line.indexOf("](", index);
    if (hit < 0) {
      return refs;
    }
    const open = hit + 2;
    let depth = 1;
    let end = -1;
    for (let at = open; at < line.length; at += 1) {
      if (line[at] === "(") {
        depth += 1;
      } else if (line[at] === ")") {
        depth -= 1;
        if (depth === 0) {
          end = at;
          break;
        }
      }
    }
    if (end < 0) {
      // An unclosed destination ends the line's scan, as it does in the core:
      // what follows is not a reference this side can read either.
      return refs;
    }
    index = end + 1;
    const inside = line.slice(open, end);
    const leading = inside.length - inside.trimStart().length;
    const token = inside.trim().split(/\s/)[0] ?? "";
    // The label's own opening bracket, through any brackets nested in it.
    let bracket = -1;
    let nesting = 1;
    for (let at = hit - 1; at >= 0; at -= 1) {
      if (line[at] === "]") {
        nesting += 1;
      } else if (line[at] === "[") {
        nesting -= 1;
        if (nesting === 0) {
          bracket = at;
          break;
        }
      }
    }
    if (bracket < 0) {
      continue;
    }
    const image = bracket > 0 && line[bracket - 1] === "!";
    refs.push({
      image,
      targetFrom: open + leading,
      targetTo: open + leading + token.length,
      from: image ? bracket - 1 : bracket,
      to: end + 1,
      token,
    });
  }
}

/**
 * Walk a document line by line, skipping fenced code, and hand each live line
 * to `visit` with its own offset into the text.
 *
 * The fence rule is the core's, restated because a plainer one is wrong in two
 * directions: a fence closes only on the character it opened with, only at
 * that length or longer, and only when the rest of that line is EMPTY - so a
 * closing marker carrying an info string (` ```js `) does not close anything,
 * and a shorter or foreign marker inside a fence is content.
 */
function eachLiveLine(
  text: string,
  visit: (line: string, offset: number) => void,
): void {
  let fence: { char: string; count: number } | null = null;
  let offset = 0;
  for (const raw of text.split("\n")) {
    const line = raw.replace(/\r$/, "");
    const marker = fenceMarker(line);
    if (fence === null) {
      if (marker !== null) {
        fence = marker;
        offset += raw.length + 1;
        continue;
      }
    } else {
      if (
        marker !== null &&
        marker.char === fence.char &&
        marker.count >= fence.count &&
        line.trimStart().slice(marker.count).trim() === ""
      ) {
        fence = null;
      }
      offset += raw.length + 1;
      continue;
    }
    visit(line, offset);
    offset += raw.length + 1;
  }
}

/**
 * Every attachment this text references, fragment stripped, deduplicated, in
 * the order written.
 *
 * The reading side's mirror of `find_asset_refs`, sharing its scanner
 * ({@link lineRefs}) and its fence rule ({@link eachLiveLine}): both markdown
 * forms count, code is skipped, a leading `./` is stripped, a title clause is
 * dropped, a percent escape is decoded ({@link decodeTarget}, which the core
 * has done since 0.15.1) and a fragment never reaches the path. The core
 * remains the authority - it is what the maintenance sweep and the resource
 * links are built on - and the agreement is pinned by the corpus both suites
 * read, `crates/core/tests/fixtures/asset_ref_corpus.json`: a change to what
 * this function answers is a change to that fixture and to the Rust scanner in
 * the same commit.
 *
 * One divergence is left, stated where it is made: {@link assetPath} refuses a
 * `.` or `..` segment the core's scanner would count and its validator would
 * refuse.
 */
export function assetRefsIn(text: string): string[] {
  const paths = new Set<string>();
  eachLiveLine(text, (line) => {
    for (const ref of lineRefs(line)) {
      const path = assetPath(ref.token);
      if (path !== null) {
        paths.add(path);
      }
    }
  });
  return [...paths];
}

/**
 * Every attachment image referenced in this text, in the order written.
 *
 * Attachment images only: an external URL is somebody else's address and a
 * non-image attachment is a link rather than a picture, so neither earns a
 * preview or a format menu. Offsets are relative to the text handed in, so a
 * caller working line by line adds the line's own start and a caller scanning
 * a whole document adds nothing.
 */
export function imageRefsIn(text: string): ImageRef[] {
  const refs: ImageRef[] = [];
  eachLiveLine(text, (line, offset) => {
    for (const ref of lineRefs(line)) {
      const path = assetPath(ref.token);
      if (!ref.image || path === null || !isImageAttachment(path)) {
        continue;
      }
      // The written path keeps whatever the author wrote in front of the
      // fragment, `./` included; only the fragment is cut.
      const hash = ref.token.indexOf("#");
      refs.push({
        from: offset + ref.from,
        to: offset + ref.to,
        targetFrom: offset + ref.targetFrom,
        targetTo: offset + ref.targetTo,
        written: hash < 0 ? ref.token : ref.token.slice(0, hash),
        path,
        format: parseImageFragment(ref.token).format,
      });
    }
  });
  return refs;
}
