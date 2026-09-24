/**
 * Text and pictograms, drawn on a 2D canvas and uploaded as texture layers.
 *
 * Only the browser has a 2D canvas (jsdom does not), so everything here but
 * `flipRows` runs in the game and is checked by eye. Canvases are created on
 * call, never at import, so a test can import the module safely.
 *
 * WebGL2 refuses `UNPACK_FLIP_Y_WEBGL` for 3D texture uploads from a byte
 * array, and a canvas's first row is its top while a texture's first row is
 * its bottom, so every layer is flipped here, once, on the CPU.
 *
 * A text layer is square but its quad is not: the text is drawn stretched
 * vertically by the quad's aspect and the quad squeezes it back. Labels
 * share a layer six to a layer, each drawn into its own row with the same
 * squeeze per row, as `layerPlan` in `layers.ts` lays them out. Terminals
 * use the look's terminal style: green phosphor on dark glass in the manner
 * of MU/TH/UR, or the C64's light blue on blue with upper-case characters.
 */

import {
  ASPECT,
  LAYER_SIZE,
  ROW_HEIGHT,
  type LayerPlan,
  type TextKind,
  type TextRequest,
} from "./layers";
import { C64_PALETTE, type Look, type Rgb } from "./looks";

/** A copy of RGBA rows in reverse order. */
export function flipRows(
  data: Uint8ClampedArray | Uint8Array,
  width: number,
  height: number,
): Uint8Array {
  const out = new Uint8Array(width * height * 4);
  const row = width * 4;
  for (let y = 0; y < height; y++) {
    out.set(data.subarray(y * row, (y + 1) * row), (height - 1 - y) * row);
  }
  return out;
}

function css([r, g, b]: Rgb): string {
  return `rgb(${Math.round(r * 255)} ${Math.round(g * 255)} ${Math.round(b * 255)})`;
}

function canvas2d(size: number) {
  const canvas = document.createElement("canvas");
  canvas.width = size;
  canvas.height = size;
  const ctx = canvas.getContext("2d");
  if (ctx === null) throw new Error("no 2D canvas");
  return ctx;
}

function pixels(ctx: CanvasRenderingContext2D, size: number): Uint8Array {
  return flipRows(ctx.getImageData(0, 0, size, size).data, size, size);
}

/** The colours of a kind of text in a look: its background and its ink. */
function colours(kind: TextKind, look: Look): { background: Rgb; ink: Rgb } {
  const petscii = look.terminal === "petscii";
  switch (kind) {
    case "screen":
      return {
        background: petscii
          ? (C64_PALETTE[6] ?? look.palette.screen)
          : look.palette.screen,
        ink: petscii
          ? (C64_PALETTE[14] ?? look.palette.screenText)
          : look.palette.screenText,
      };
    case "placard":
    case "poster":
      return { background: [0.12, 0.12, 0.12], ink: [0.95, 0.95, 0.9] };
    case "hatch":
      // Stencil amber on black, like the hazard stripes around a hatch.
      return { background: [0.05, 0.05, 0.06], ink: [1, 0.78, 0.2] };
    case "label":
      return { background: [0.05, 0.05, 0.06], ink: [1, 1, 1] };
  }
}

/**
 * Draws one request into the band `top` to `top + height` of a square
 * canvas of side `size`: a whole layer is the full height, a label its row.
 * The band is filled with the request's background and clipped, so a tall
 * glyph never spills into the next row. Inside the band the text is laid
 * out in a logical space `size` wide and `size / aspect` tall, stretched
 * vertically to fill the band, which the quad of that aspect squeezes back.
 */
function drawRequest(
  ctx: CanvasRenderingContext2D,
  request: TextRequest,
  look: Look,
  size: number,
  top: number,
  height: number,
) {
  const aspect = ASPECT[request.kind];
  const logicalHeight = size / aspect;
  const petscii = look.terminal === "petscii";
  const lines = petscii
    ? request.lines.map((l) => l.toUpperCase())
    : request.lines;
  const { background, ink } = colours(request.kind, look);

  ctx.save();
  ctx.setTransform(1, 0, 0, 1, 0, 0);
  ctx.beginPath();
  ctx.rect(0, top, size, height);
  ctx.clip();
  ctx.fillStyle = css(background);
  ctx.fillRect(0, top, size, height);
  ctx.setTransform(1, 0, 0, height / logicalHeight, 0, top);
  ctx.fillStyle = css(ink);
  ctx.textBaseline = "top";

  if (request.kind === "label" || request.kind === "hatch") {
    const text = (lines[0] ?? "").toUpperCase();
    let px = logicalHeight * 0.7;
    ctx.font = `bold ${px}px ui-monospace, Menlo, Consolas, monospace`;
    while (ctx.measureText(text).width > size * 0.92 && px > 6) {
      px -= 1;
      ctx.font = `bold ${px}px ui-monospace, Menlo, Consolas, monospace`;
    }
    ctx.textAlign = "center";
    ctx.fillText(text, size / 2, (logicalHeight - px) / 2);
  } else {
    const rows = request.kind === "screen" ? 9 : 5;
    const px = logicalHeight / (rows + 1);
    lines.slice(0, rows).forEach((line, i) => {
      const heading = i === 0;
      ctx.font = `${heading ? "bold " : ""}${px * 0.85}px ui-monospace, Menlo, Consolas, monospace`;
      if (heading && request.kind === "screen") {
        // Headings in reverse video, as the CRT reader will draw them.
        ctx.fillRect(px * 0.3, px * 0.4, size - px * 0.6, px);
        ctx.fillStyle = css(background);
        ctx.fillText(line, px * 0.5, px * 0.5, size - px);
        ctx.fillStyle = css(ink);
      } else {
        ctx.fillText(line, px * 0.5, px * (0.5 + i * 1.05), size - px);
      }
    });
    if (request.kind === "screen") {
      // Scanlines.
      ctx.setTransform(1, 0, 0, 1, 0, 0);
      ctx.fillStyle = "rgb(0 0 0 / 0.25)";
      for (let y = top; y < top + height; y += 3) ctx.fillRect(0, y, size, 1);
    }
  }
  ctx.restore();
}

/**
 * One text layer's pixels: which layer of the texture array it fills and
 * its RGBA bytes, flipped for upload.
 */
export interface TextLayer {
  layer: number;
  pixels: Uint8Array;
}

/**
 * Every text layer of a plan drawn for a look, one canvas per layer: a
 * screen, poster or placard fills its layer alone, and the labels that
 * share a layer are drawn each into its row (`ROW_HEIGHT` texels at
 * `row * ROW_HEIGHT` from the top), the rows below the last label left
 * dark. Layers come back in ascending order, each once.
 */
export function drawTextLayers(
  plan: LayerPlan,
  look: Look,
  size: number,
): TextLayer[] {
  const byLayer = new Map<number, TextRequest[]>();
  for (const request of plan.text) {
    const layer = plan.lookup(request.key).layer;
    const list = byLayer.get(layer) ?? [];
    list.push(request);
    byLayer.set(layer, list);
  }
  return [...byLayer.entries()]
    .sort(([a], [b]) => a - b)
    .map(([layer, requests]) => {
      const ctx = canvas2d(size);
      ctx.fillStyle = "#000";
      ctx.fillRect(0, 0, size, size);
      for (const request of requests) {
        if (request.row === null) {
          drawRequest(ctx, request, look, size, 0, size);
        } else {
          const rowHeight = (ROW_HEIGHT * size) / LAYER_SIZE;
          drawRequest(
            ctx,
            request,
            look,
            size,
            request.row * rowHeight,
            rowHeight,
          );
        }
      }
      return { layer, pixels: pixels(ctx, size) };
    });
}

/**
 * The pictogram layer: a Semiotic Standard style sign (a square frame with
 * an arrow through a door), drawn as vector paths in white on black so the
 * shader can tint it.
 */
export function drawPictogramLayer(size: number): Uint8Array {
  const ctx = canvas2d(size);
  const s = size / 16;
  ctx.fillStyle = "#000";
  ctx.fillRect(0, 0, size, size);
  ctx.strokeStyle = "#fff";
  ctx.fillStyle = "#fff";
  ctx.lineWidth = s;
  ctx.strokeRect(s * 1.5, s * 1.5, size - s * 3, size - s * 3);
  ctx.fillRect(s * 9, s * 4, s * 3, s * 8);
  ctx.beginPath();
  ctx.moveTo(s * 3.5, s * 7);
  ctx.lineTo(s * 7, s * 7);
  ctx.lineTo(s * 7, s * 5);
  ctx.lineTo(s * 9, s * 8);
  ctx.lineTo(s * 7, s * 11);
  ctx.lineTo(s * 7, s * 9);
  ctx.lineTo(s * 3.5, s * 9);
  ctx.closePath();
  ctx.fill();
  return pixels(ctx, size);
}
