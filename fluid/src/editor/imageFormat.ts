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
 * Whether this target is an attachment of the domain the document lives in.
 *
 * Relative and under the reserved prefix, which is exactly what an upload
 * writes. A leading slash, a scheme or a host make the target somebody else's
 * address, and this app rewrites none of those.
 */
export function isAssetTarget(target: string): boolean {
  return target.startsWith(ASSETS_PREFIX);
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
  /** The target alone, which is what a format change rewrites. */
  targetFrom: number;
  targetTo: number;
  /** The stored path, fragment stripped: what the files route is asked for. */
  path: string;
  format: ImageFormat;
}

/**
 * An image reference, as markdown writes one.
 *
 * The target admits no whitespace and no parenthesis, which is not a
 * simplification: the core validator refuses a stored asset path carrying
 * either, so a reference that needed the angle-bracket form cannot be pointing
 * at an attachment of ours.
 */
const IMAGE_REF = /!\[[^\]\n]*\]\(([^()\s]+)\)/g;

/** A reference of either kind - an embedded image or a plain link. */
const ANY_REF = /!?\[[^\]\n]*\]\(([^()\s]+)\)/g;

/** A fence line, either spelling, which opens or closes a block of code. */
const FENCE = /^\s{0,3}(`{3,}|~{3,})/;

/**
 * Every attachment this text references, fragment stripped, deduplicated, in
 * the order written.
 *
 * The reading side's mirror of the scanner the core runs, and deliberately the
 * same shape: code is skipped, because a path inside a fence is an example
 * rather than a reference, and a fragment is stripped, because
 * `assets/pic.png#right` and `assets/pic.png` are one file. Where the two ever
 * disagree the core is the authority - it is what the maintenance sweep and
 * the resource links are built on - and this one only decides what a panel
 * lists.
 */
export function assetRefsIn(text: string): string[] {
  const paths = new Set<string>();
  let fence: string | null = null;
  for (const line of text.split("\n")) {
    const marker = FENCE.exec(line)?.[1];
    if (marker !== undefined) {
      // A fence closes only on its own character, and only at the length it
      // was opened with or more - the rule the core's scanner follows.
      if (fence === null) {
        fence = marker;
      } else if (marker[0] === fence[0] && marker.length >= fence.length) {
        fence = null;
      }
      continue;
    }
    if (fence !== null) {
      continue;
    }
    for (const match of line.matchAll(ANY_REF)) {
      const target = match[1];
      if (target !== undefined && isAssetTarget(target)) {
        paths.add(parseImageFragment(target).path);
      }
    }
  }
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
  for (const match of text.matchAll(IMAGE_REF)) {
    const target = match[1];
    if (target === undefined || !isAssetTarget(target)) {
      continue;
    }
    const { path, format } = parseImageFragment(target);
    if (!isImageAttachment(path)) {
      continue;
    }
    const from = match.index;
    const to = from + match[0].length;
    refs.push({
      from,
      to,
      // The target is the last thing in the match and ends one character
      // before it does, at the closing parenthesis.
      targetFrom: to - 1 - target.length,
      targetTo: to - 1,
      path,
      format,
    });
  }
  return refs;
}
