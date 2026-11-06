/**
 * A diagram as a picture file, made in the browser and nowhere else.
 *
 * Its own module because it is loaded on the click: nobody who does not ask
 * for a PNG pays for this code, and the full window it is reached from also
 * shows plain images.
 *
 * Everything here works from the drawing's own size, the one its viewBox
 * states, and never from its size on screen. Mermaid shrinks a wide diagram to
 * fit its column and the full window shrinks it again, so the screen says
 * nothing about how small its smallest text really is.
 *
 * The drawing on screen is only read. What goes into the image is a copy,
 * because an SVG loaded as an image is alone in the world: no page styles, no
 * CSS variables, no web fonts and no network. So what the page's styles
 * resolved to is written onto the copy, and a picture the drawing fetches from
 * elsewhere cannot come along at all.
 */

import { PNG_FAILED } from "./downloads";

export { PNG_FAILED };

/** The file is saved, but the smallest text is under the size it should have. */
export const PNG_NOT_SHARP =
  "This diagram is too large for a sharp PNG in this browser. The SVG stays sharp at any size.";

/** No file: the smallest text would come out too small to read. */
export const PNG_TOO_LARGE =
  "This diagram is too large for a PNG in this browser, so no file was saved. Use Download SVG instead.";

/** No file: the PNG would have a hole where the picture is. */
export const PNG_HAS_OUTSIDE_PICTURE =
  "This diagram shows a picture from outside. A PNG cannot hold it, so no file was saved. Use Download SVG instead.";

const SVG_NS = "http://www.w3.org/2000/svg";

/** The scale every diagram gets at least, so its lines stay sharp. */
const MIN_SCALE = 2;
/** What the smallest text comes out at, when the browser has the room. */
const SHARP_TEXT_PX = 28;
/** What the thinnest line comes out at, when the browser has the room. */
const SHARP_STROKE_PX = 1.5;
/**
 * Strokes thinner than this do not count. A diagram's markers can carry a
 * hairline such as 0.01 that nobody reads as a detail, and it would ask for a
 * scale of 150 and push the whole file into the canvas limit.
 */
const MIN_COUNTED_STROKE_PX = 0.1;
/** Under this the text cannot be read, and a file with it is no use to anyone. */
const MIN_TEXT_PX = 10;
/** A drawing without text has no size to keep readable, so it keeps its own. */
const MIN_SCALE_WITHOUT_TEXT = 1;
/** Halvings of the gap between too small and too large: closer than a pixel. */
const SEARCH_STEPS = 14;
/** The size of the rough copy that tells a drawing from an empty canvas. */
const SAMPLE_PX = 128;

/** Whether the browser can really hold a canvas of this size. */
export type CanvasFits = (width: number, height: number) => boolean;

export type ScalePlan =
  | { kind: "sharp"; scale: number; pixelWidth: number; pixelHeight: number }
  | { kind: "reduced"; scale: number; pixelWidth: number; pixelHeight: number }
  | { kind: "refused" };

/**
 * How large the file can be. The wanted scale first; if that does not fit, the
 * largest one that does, as long as the smallest text stays readable.
 *
 * Pixels round up: a fractional drawing rounded down would lose its last row,
 * and the smallest text would land just under its size.
 */
export function planScale(
  drawing: {
    width: number;
    height: number;
    smallestFontPx: number | null;
    thinnestStrokePx?: number | null;
  },
  fits: CanvasFits,
): ScalePlan {
  const { width, height, smallestFontPx, thinnestStrokePx } = drawing;
  const pixels = (scale: number) => ({
    pixelWidth: Math.ceil(width * scale),
    pixelHeight: Math.ceil(height * scale),
  });
  const fitsAt = (scale: number) => {
    const { pixelWidth, pixelHeight } = pixels(scale);
    return fits(pixelWidth, pixelHeight);
  };
  const wanted = Math.max(
    MIN_SCALE,
    smallestFontPx === null ? 0 : SHARP_TEXT_PX / smallestFontPx,
    thinnestStrokePx == null ? 0 : SHARP_STROKE_PX / thinnestStrokePx,
  );
  if (fitsAt(wanted)) {
    return { kind: "sharp", scale: wanted, ...pixels(wanted) };
  }
  const lowest =
    smallestFontPx === null
      ? MIN_SCALE_WITHOUT_TEXT
      : MIN_TEXT_PX / smallestFontPx;
  if (!fitsAt(lowest)) {
    return { kind: "refused" };
  }
  let fitting = lowest;
  let tooLarge = wanted;
  for (let step = 0; step < SEARCH_STEPS; step += 1) {
    const middle = (fitting + tooLarge) / 2;
    if (fitsAt(middle)) {
      fitting = middle;
    } else {
      tooLarge = middle;
    }
  }
  return { kind: "reduced", scale: fitting, ...pixels(fitting) };
}

/**
 * Whether a canvas of this size really exists. A browser past its limit does
 * not say so: it hands back a canvas that stays empty. So one pixel is painted
 * in the far corner and read back, which finds a limit on the sides and a limit
 * on the area alike, without knowing either number.
 */
export function canvasFits(width: number, height: number): boolean {
  const canvas = document.createElement("canvas");
  canvas.width = width;
  canvas.height = height;
  try {
    const context = canvas.getContext("2d");
    if (context === null) {
      return false;
    }
    context.fillStyle = "#000";
    context.fillRect(width - 1, height - 1, 1, 1);
    const corner = context.getImageData(width - 1, height - 1, 1, 1).data;
    return corner[3] === 255;
  } catch {
    return false;
  } finally {
    // WebKit counts the memory of every canvas still alive against one budget.
    canvas.width = 0;
    canvas.height = 0;
  }
}

/** Whether this element, or anything around it, keeps it from being seen. */
function unseen(element: Element, root: Element): boolean {
  const own = window.getComputedStyle(element);
  if (own.display === "none" || own.visibility === "hidden") {
    return true;
  }
  for (
    let at: Element | null = element;
    at !== null;
    at = at === root ? null : at.parentElement
  ) {
    const style = window.getComputedStyle(at);
    if (style.display === "none" || style.opacity === "0") {
      return true;
    }
  }
  return false;
}

/** What only ever draws when something refers to it, so it is not measured. */
const NOT_DRAWN_ITSELF = "defs, marker, symbol, clipPath, mask, pattern";

/**
 * How much this element is scaled against the viewBox.
 *
 * Between the two matrices, everything that has to do with the screen cancels
 * out: the viewBox mapping, mermaid's scale-to-fit and the pan and zoom. The
 * thinner axis, so a label squeezed in one direction counts as small.
 */
function scaleInDrawing(
  element: SVGGraphicsElement,
  fromScreen: DOMMatrix,
): number | null {
  const toScreen = element.getScreenCTM();
  if (toScreen === null) {
    return null;
  }
  const { a, b, c, d } = fromScreen.multiply(toScreen);
  return Math.min(Math.hypot(a, b), Math.hypot(c, d));
}

function hasOwnText(element: Element): boolean {
  return Array.from(element.childNodes).some(
    (node) =>
      node.nodeType === Node.TEXT_NODE &&
      (node.textContent ?? "").trim() !== "",
  );
}

/** The smallest of a list, or null for an empty one. */
function smallest(values: number[]): number | null {
  return values.length === 0 ? null : Math.min(...values);
}

/**
 * The smallest text and the thinnest stroke as they are drawn, in the units of
 * the viewBox: what one pixel of the file at scale 1 would show.
 *
 * Text that is not on screen is skipped carefully, because a single hidden
 * label would otherwise drive the scale of the whole file: the fallback of a
 * `switch`, a label mermaid made invisible, a measure it left in its defs.
 */
export function measureDrawing(drawing: SVGSVGElement): {
  smallestFontPx: number | null;
  thinnestStrokePx: number | null;
} {
  const fromScreen = drawing.getScreenCTM()?.inverse();
  if (fromScreen === undefined) {
    return { smallestFontPx: null, thinnestStrokePx: null };
  }
  const fonts: number[] = [];
  for (const text of drawing.querySelectorAll<SVGGraphicsElement>("text")) {
    if (
      text.closest(NOT_DRAWN_ITSELF) !== null ||
      (text.parentElement?.tagName === "switch" &&
        text.parentElement.querySelector("foreignObject") !== null) ||
      unseen(text, drawing)
    ) {
      continue;
    }
    const scale = scaleInDrawing(text, fromScreen);
    const box = text.getBBox();
    if (scale === null || box.width <= 0 || box.height <= 0) {
      continue;
    }
    for (const run of [text, ...text.querySelectorAll("tspan")]) {
      if (hasOwnText(run) && !unseen(run, drawing)) {
        fonts.push(
          Number.parseFloat(window.getComputedStyle(run).fontSize) * scale,
        );
      }
    }
  }
  for (const object of drawing.querySelectorAll<SVGGraphicsElement>(
    "foreignObject",
  )) {
    if (object.closest(NOT_DRAWN_ITSELF) !== null || unseen(object, drawing)) {
      continue;
    }
    const scale = scaleInDrawing(object, fromScreen);
    if (scale === null) {
      continue;
    }
    for (const element of object.querySelectorAll("*")) {
      if (hasOwnText(element) && !unseen(element, drawing)) {
        fonts.push(
          Number.parseFloat(window.getComputedStyle(element).fontSize) * scale,
        );
      }
    }
  }
  const strokes: number[] = [];
  for (const shape of drawing.querySelectorAll<SVGGraphicsElement>(
    "path, line, rect, circle, ellipse, polygon, polyline",
  )) {
    if (shape.closest(NOT_DRAWN_ITSELF) !== null || unseen(shape, drawing)) {
      continue;
    }
    const style = window.getComputedStyle(shape);
    const stroke = style.getPropertyValue("stroke");
    const strokeWidth = Number.parseFloat(
      style.getPropertyValue("stroke-width"),
    );
    if (
      stroke === "" ||
      stroke === "none" ||
      stroke === "transparent" ||
      stroke === "rgba(0, 0, 0, 0)" ||
      style.getPropertyValue("stroke-opacity") === "0" ||
      !(strokeWidth > 0)
    ) {
      continue;
    }
    const scale = scaleInDrawing(shape, fromScreen);
    if (scale !== null && strokeWidth * scale >= MIN_COUNTED_STROKE_PX) {
      strokes.push(strokeWidth * scale);
    }
  }
  return {
    smallestFontPx: smallest(fonts.filter((size) => size > 0)),
    thinnestStrokePx: smallest(strokes),
  };
}

/**
 * What the page may have decided about a drawing's looks, where the drawing's
 * own style sheet does not. Written out as the computed value, which has every
 * CSS variable resolved.
 */
const CARRIED_STYLES = [
  "color",
  "fill",
  "fill-opacity",
  "stroke",
  "stroke-width",
  "stroke-opacity",
  "stroke-dasharray",
  "stroke-linecap",
  "stroke-linejoin",
  "opacity",
  "font-family",
  "font-size",
  "font-style",
  "font-weight",
  "letter-spacing",
  "line-height",
  "text-anchor",
  "dominant-baseline",
];

/**
 * The words of a label as the browser laid them out: one entry per line, with
 * where that line sits on screen.
 */
function linesOf(
  words: Element,
): { text: string; left: number; right: number; middle: number }[] {
  const lines: {
    text: string;
    left: number;
    right: number;
    middle: number;
  }[] = [];
  for (const node of words.childNodes) {
    if (node.nodeType !== Node.TEXT_NODE) {
      continue;
    }
    for (const word of (node.textContent ?? "").matchAll(/\S+/g)) {
      const range = document.createRange();
      range.setStart(node, word.index);
      range.setEnd(node, word.index + word[0].length);
      const box = range.getClientRects()[0];
      if (box === undefined) {
        continue;
      }
      const middle = (box.top + box.bottom) / 2;
      const line = lines.at(-1);
      if (
        line !== undefined &&
        Math.abs(line.middle - middle) < box.height / 2
      ) {
        line.text += ` ${word[0]}`;
        line.right = box.right;
      } else {
        lines.push({ text: word[0], left: box.left, right: box.right, middle });
      }
    }
  }
  return lines;
}

/**
 * A foreign object taints the canvas, and no file comes out of that. The user
 * journey draws its labels as one, inside a `switch` with a plain text
 * fallback, so neither can simply stay.
 *
 * The fallback is no stand-in: it is one line that runs out of its box, and a
 * section title's has the fill of the box behind it, which nobody notices as
 * long as it is not drawn. So the copy gets the lines the screen shows, at
 * their places and in their looks, inside a nested `svg` that clips them at the
 * edge of the box, as the foreign object does.
 *
 * Places are shares of the object's box on screen rather than a matrix, which
 * keeps the pan and zoom out of it in every browser.
 */
function drawLabelsAsText(drawing: SVGSVGElement, copy: SVGSVGElement): void {
  const selector = "switch > foreignObject";
  const shown = Array.from(drawing.querySelectorAll(selector));
  Array.from(copy.querySelectorAll(selector)).forEach((object, index) => {
    const live = shown[index];
    const place = object.parentElement;
    if (live === undefined || place === null) {
      return;
    }
    const box = live.getBoundingClientRect();
    const width = Number(object.getAttribute("width"));
    const height = Number(object.getAttribute("height"));
    const label = object.ownerDocument.createElementNS(SVG_NS, "svg");
    for (const name of ["x", "y", "width", "height"]) {
      label.setAttribute(name, object.getAttribute(name) ?? "0");
    }
    label.style.setProperty("overflow", "hidden");
    const words = Array.from(live.querySelectorAll("*")).find(hasOwnText);
    if (words !== undefined) {
      const style = window.getComputedStyle(words);
      for (const line of linesOf(words)) {
        const text = object.ownerDocument.createElementNS(SVG_NS, "text");
        const across = ((line.left + line.right) / 2 - box.left) / box.width;
        const down = (line.middle - box.top) / box.height;
        text.setAttribute("x", String(across * width));
        text.setAttribute("y", String(down * height));
        text.style.setProperty("fill", style.getPropertyValue("color"));
        text.style.setProperty("text-anchor", "middle");
        text.style.setProperty("dominant-baseline", "central");
        for (const name of ["font-family", "font-size", "font-weight"]) {
          text.style.setProperty(name, style.getPropertyValue(name));
        }
        text.textContent = line.text;
        label.append(text);
      }
    }
    place.replaceChildren(label);
  });
}

/**
 * The drawing as markup an image can read on its own, drawn at the file's own
 * size: the width and height say so and the viewBox does the scaling. Drawing
 * a smaller image onto a larger canvas instead would let WebKit stretch it.
 */
export function serializeForImage(
  drawing: SVGSVGElement,
  pixelWidth: number,
  pixelHeight: number,
): string {
  const copy = drawing.cloneNode(true) as SVGSVGElement;
  const live = [drawing, ...drawing.querySelectorAll("*")];
  const copied = [copy, ...copy.querySelectorAll("*")];
  live.forEach((element, index) => {
    const target = copied[index] as SVGElement | HTMLElement | undefined;
    if (target === undefined || element.tagName.toLowerCase() === "style") {
      return;
    }
    const style = window.getComputedStyle(element);
    for (const name of CARRIED_STYLES) {
      const value = style.getPropertyValue(name);
      if (value !== "") {
        target.style.setProperty(name, value);
      }
    }
  });
  drawLabelsAsText(drawing, copy);
  copy.setAttribute("width", String(pixelWidth));
  copy.setAttribute("height", String(pixelHeight));
  copy.style.removeProperty("max-width");
  return new XMLSerializer().serializeToString(copy);
}

/** Whether the drawing shows a picture it does not carry inside itself. */
function hasOutsidePicture(drawing: SVGSVGElement): boolean {
  return Array.from(drawing.querySelectorAll("image, img")).some((picture) => {
    const address =
      picture.getAttribute("href") ??
      picture.getAttribute("xlink:href") ??
      picture.getAttribute("src") ??
      "";
    return !address.startsWith("data:");
  });
}

/**
 * Whether the finished canvas holds a drawing. Solid in its four corners, the
 * background having reached them, and not one colour all over in a rough copy,
 * which is how an image that decoded to nothing comes out.
 */
function holdsDrawing(
  canvas: HTMLCanvasElement,
  context: CanvasRenderingContext2D,
): boolean {
  const right = canvas.width - 1;
  const bottom = canvas.height - 1;
  for (const [x, y] of [
    [0, 0],
    [right, 0],
    [0, bottom],
    [right, bottom],
  ] as const) {
    if (context.getImageData(x, y, 1, 1).data[3] !== 255) {
      return false;
    }
  }
  const sample = document.createElement("canvas");
  sample.width = SAMPLE_PX;
  sample.height = SAMPLE_PX;
  const rough = sample.getContext("2d");
  if (rough === null) {
    return false;
  }
  rough.drawImage(canvas, 0, 0, SAMPLE_PX, SAMPLE_PX);
  const { data } = rough.getImageData(0, 0, SAMPLE_PX, SAMPLE_PX);
  sample.width = 0;
  sample.height = 0;
  for (let i = 0; i < data.length; i += 4) {
    if (
      data[i] !== data[0] ||
      data[i + 1] !== data[1] ||
      data[i + 2] !== data[2]
    ) {
      return true;
    }
  }
  return false;
}

/** The markup as a PNG on this background. Throws whatever the browser throws. */
async function rasterize(
  markup: string,
  background: string,
  pixelWidth: number,
  pixelHeight: number,
): Promise<Blob | null> {
  const address = URL.createObjectURL(
    new Blob([markup], { type: "image/svg+xml" }),
  );
  const canvas = document.createElement("canvas");
  try {
    const image = new Image();
    image.src = address;
    await image.decode();
    canvas.width = pixelWidth;
    canvas.height = pixelHeight;
    const context = canvas.getContext("2d");
    if (context === null) {
      return null;
    }
    context.fillStyle = background;
    context.fillRect(0, 0, pixelWidth, pixelHeight);
    context.drawImage(image, 0, 0, pixelWidth, pixelHeight);
    // A tainted canvas throws on the first read, here or in `toBlob` below.
    if (!holdsDrawing(canvas, context)) {
      return null;
    }
    return await new Promise<Blob | null>((resolve) => {
      canvas.toBlob(resolve, "image/png");
    });
  } finally {
    URL.revokeObjectURL(address);
    canvas.width = 0;
    canvas.height = 0;
  }
}

export interface PngRequest {
  drawing: SVGSVGElement;
  /** The size the drawing's viewBox states. */
  natural: { width: number; height: number };
  /** The colour behind the drawing on screen, so the file is never transparent. */
  background: string;
}

export interface PngResult {
  /** The file, or null when none should be written. */
  blob: Blob | null;
  /** What the person has to be told, if anything. */
  note: string | null;
}

/** The drawing as a PNG, or the sentence that says why there is none. */
export async function exportDiagramPng(
  { drawing, natural, background }: PngRequest,
  fits: CanvasFits = canvasFits,
): Promise<PngResult> {
  try {
    if (hasOutsidePicture(drawing)) {
      return { blob: null, note: PNG_HAS_OUTSIDE_PICTURE };
    }
    const plan = planScale({ ...natural, ...measureDrawing(drawing) }, fits);
    if (plan.kind === "refused") {
      return { blob: null, note: PNG_TOO_LARGE };
    }
    const blob = await rasterize(
      serializeForImage(drawing, plan.pixelWidth, plan.pixelHeight),
      background,
      plan.pixelWidth,
      plan.pixelHeight,
    );
    if (blob === null) {
      return { blob: null, note: PNG_FAILED };
    }
    return { blob, note: plan.kind === "reduced" ? PNG_NOT_SHARP : null };
  } catch {
    return { blob: null, note: PNG_FAILED };
  }
}
