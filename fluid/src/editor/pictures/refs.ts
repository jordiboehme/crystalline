/**
 * The picture model: every inline image of a document as a span with its
 * parts and its kind, read from the syntax tree and the core's own scanner.
 *
 * The tree says where markdown means an image (never inside code, never in a
 * reference definition) and where the alt text sits; the scanner
 * (`imageSpansIn`, the same one the attachment rail and the core count with)
 * says where the target token sits, as written. A node the two do not agree
 * on - a reference-style `![a][r]`, an image spread over lines - is no
 * picture here and stays text.
 *
 * What a target is, is the document kind's to say: the `pictureAssets` facet
 * holds that kind's resolver, the engram one by default. Eager and small: no
 * React, nothing of a lazy chunk.
 */

import { syntaxTree } from "@codemirror/language";
import type { EditorState } from "@codemirror/state";
import { Facet } from "@codemirror/state";
import type { SyntaxNode } from "@lezer/common";

import type { AssetResolver } from "../../api/files";
import { isImageAttachment } from "../../api/files";
import type { ImageFormat } from "../imageFormat";
import { assetPath, imageSpansIn, parseImageFragment } from "../imageFormat";
import type { MathRegion } from "../mathBlocks";
import { inMathRegion, mathRegions } from "../mathBlocks";

/**
 * The engram kind's resolver: an attachment is a stored path under `assets/`,
 * linked by that path itself, and no target is refused. The engram screen
 * provides its own with the files route's URLs.
 */
export const ENGRAM_RESOLVER: AssetResolver = {
  resolve: assetPath,
  url: (path) => path,
  refused: () => null,
};

/** What a page's picture targets mean. The last one provided wins. */
export const pictureAssets = Facet.define<AssetResolver, AssetResolver>({
  combine: (values) => values[values.length - 1] ?? ENGRAM_RESOLVER,
});

/**
 * - `attachment`: an image file of this page's own store;
 * - `external`: an `http`/`https` address, drawn from where it lives;
 * - `refused`: a target the page must not fetch at all (the resolver says why);
 * - `unavailable`: everything else - a file that is no image, a path outside
 *   the store, another scheme, an empty target.
 */
export type PictureKind = "attachment" | "external" | "refused" | "unavailable";

/** The sentence an unavailable picture shows. */
export const UNAVAILABLE = "This image is not available.";

/** One inline picture, in document positions. */
export interface PictureRef {
  /** The `Image` node: the whole `![alt](target)`. */
  from: number;
  to: number;
  /** The alt text, between `![` and the `]` before `(`. */
  altFrom: number;
  altTo: number;
  /** The target token as written. */
  targetFrom: number;
  targetTo: number;
  token: string;
  /** The token before its first `#`, for an attachment; `""` otherwise. */
  written: string;
  kind: PictureKind;
  /** The format the fragment asks for, for an attachment; null otherwise. */
  format: ImageFormat | null;
  /** What the picture loads: the attachment's URL or the external address. */
  src: string | null;
  /** Why it is not drawn: the resolver's sentence, or {@link UNAVAILABLE}. */
  reason: string | null;
  /** Inside a link: the link keeps the click. */
  inLink: boolean;
  /** Inside a table: the grid's cell draws it. */
  inCell: boolean;
}

const EXTERNAL = /^https?:\/\//i;

/**
 * The innermost enclosing `Image` node at `pos`, or null.
 *
 * Both sides are tried, for the reason `tableVerbs` states: a position is a
 * gap BETWEEN two characters, and at the reference's first character only a
 * look to the right finds it while one position past its last only a look to
 * the left does.
 */
export function imageNodeAt(
  state: EditorState,
  pos: number,
): SyntaxNode | null {
  return imageNodeOn(state, pos, -1) ?? imageNodeOn(state, pos, 1);
}

/** The innermost enclosing `Image` node at `pos`, looking to one side. */
function imageNodeOn(
  state: EditorState,
  pos: number,
  side: -1 | 1,
): SyntaxNode | null {
  let node: SyntaxNode | null = syntaxTree(state).resolveInner(pos, side);
  while (node !== null) {
    if (node.name === "Image") {
      return node;
    }
    node = node.parent;
  }
  return null;
}

/** Whether `node` has an ancestor called `name`. */
function within(node: SyntaxNode, name: string): boolean {
  for (let up = node.parent; up !== null; up = up.parent) {
    if (up.name === name) return true;
  }
  return false;
}

/** The picture one `Image` node is, or null when it stays text. */
function classify(
  state: EditorState,
  node: SyntaxNode,
  resolver: AssetResolver,
  math: readonly MathRegion[],
): PictureRef | null {
  // `$$` math the page draws is formula source, as Part 2's scan reads it.
  if (math.length > 0 && inMathRegion(math, node.from)) {
    return null;
  }
  const text = state.doc.sliceString(node.from, node.to);
  const spans = imageSpansIn(text);
  const span = spans[0];
  // The scanner must read the node as exactly one inline image: a
  // reference-style image has no `](`, and one spread over lines is read
  // line by line, so neither comes back whole.
  if (
    spans.length !== 1 ||
    span === undefined ||
    span.from !== 0 ||
    span.to !== text.length
  ) {
    return null;
  }
  // The alt ends at the `]` right before the `(`, not at a `]` inside it.
  const marks = node.getChildren("LinkMark");
  const open = marks.findIndex(
    (mark) => state.doc.sliceString(mark.from, mark.to) === "(",
  );
  const first = marks[0];
  const close = marks[open - 1];
  if (open < 1 || first === undefined || close === undefined) {
    return null;
  }
  const { token } = span;
  return {
    from: node.from,
    to: node.to,
    altFrom: first.to,
    altTo: close.from,
    targetFrom: node.from + span.targetFrom,
    targetTo: node.from + span.targetTo,
    token,
    ...pictureTarget(token, resolver),
    inLink: within(node, "Link"),
    inCell: within(node, "Table"),
  };
}

/** What a target token is to a page: the parts of a {@link PictureRef} it decides. */
export type PictureTarget = Pick<
  PictureRef,
  "written" | "kind" | "format" | "src" | "reason"
>;

/**
 * What one target token is, as `resolver` reads it: the one place a token
 * becomes a kind, an address and a reason, for the main view's pictures and
 * a table cell's alike.
 */
export function pictureTarget(
  token: string,
  resolver: AssetResolver,
): PictureTarget {
  const refused = resolver.refused(token);
  const path = refused === null ? resolver.resolve(token) : null;
  const attachment = path !== null && isImageAttachment(path);
  const kind: PictureKind =
    refused !== null
      ? "refused"
      : attachment
        ? "attachment"
        : EXTERNAL.test(token)
          ? "external"
          : "unavailable";
  const hash = token.indexOf("#");
  return {
    written:
      kind === "attachment" ? (hash < 0 ? token : token.slice(0, hash)) : "",
    kind,
    format: kind === "attachment" ? parseImageFragment(token).format : null,
    src:
      kind === "attachment" && path !== null
        ? resolver.url(path)
        : kind === "external"
          ? token
          : null,
    reason:
      kind === "refused"
        ? refused
        : kind === "unavailable"
          ? UNAVAILABLE
          : null,
  };
}

/** The picture one `Image` node is under the page's resolver, or null when it stays text. */
export function pictureOfNode(
  state: EditorState,
  node: SyntaxNode,
): PictureRef | null {
  return classify(state, node, state.facet(pictureAssets), mathRegions(state));
}

/**
 * Every inline picture the tree holds between `from` and `to` (the whole
 * document by default), in document order.
 */
export function picturesIn(
  state: EditorState,
  from = 0,
  to = state.doc.length,
): PictureRef[] {
  const resolver = state.facet(pictureAssets);
  const math = mathRegions(state);
  const refs: PictureRef[] = [];
  syntaxTree(state).iterate({
    from,
    to,
    enter: (node) => {
      if (node.name !== "Image") {
        return;
      }
      const ref = classify(state, node.node, resolver, math);
      if (ref !== null) {
        refs.push(ref);
      }
      return false;
    },
  });
  return refs;
}

/** The picture whose span holds `pos`, either end included, or null. */
export function pictureAt(state: EditorState, pos: number): PictureRef | null {
  const resolver = state.facet(pictureAssets);
  const math = mathRegions(state);
  // At a junction of two images, the left one may stay text (reference
  // style, math): then the right one is the picture.
  for (const side of [-1, 1] as const) {
    const node = imageNodeOn(state, pos, side);
    const ref = node === null ? null : classify(state, node, resolver, math);
    if (ref !== null) return ref;
  }
  return null;
}

/** The picture the main selection covers exactly, or null. */
export function selectedPicture(state: EditorState): PictureRef | null {
  const { main } = state.selection;
  if (main.empty) {
    return null;
  }
  // One in from the start, so a picture that ends where this one starts
  // is not the one found.
  const ref = pictureAt(state, main.from + 1);
  return ref !== null && ref.from === main.from && ref.to === main.to
    ? ref
    : null;
}
