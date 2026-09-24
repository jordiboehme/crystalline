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
 * A pictogram's place in the pictogram layer (`LAYER.pictogram`), as the uv
 * rectangle `kit.panel` takes: `(u0, v0)` its bottom-left corner in texture
 * space (v 0 at the bottom, as `flipRows` uploads it) and `uw` by `vh` its
 * size.
 */
export interface PictogramRect {
  u0: number;
  v0: number;
  uw: number;
  vh: number;
}

/**
 * The pictogram set, a 2 by 2 sheet of square tiles in the one pictogram
 * layer, so a square plate shows its sign undistorted: the service hatch
 * top left, the portal top right, the door bottom left, and the bottom
 * right tile left dark for a later sign. This is the single source of the
 * layout: `drawPictogramLayer` draws each sign into the tile named here, and
 * a model maps its plate to the same rectangle (`SERVICE_PICTOGRAM` in
 * `models/hatch.ts` is `PICTOGRAM.service`), so the two cannot drift.
 */
export const PICTOGRAM = {
  service: { u0: 0, v0: 0.5, uw: 0.5, vh: 0.5 },
  portal: { u0: 0.5, v0: 0.5, uw: 0.5, vh: 0.5 },
  door: { u0: 0, v0: 0, uw: 0.5, vh: 0.5 },
} as const satisfies Record<string, PictogramRect>;

/** The pictograms of the set, each drawn into a unit tile (0 to 16). */
const SIGNS: Record<
  keyof typeof PICTOGRAM,
  (ctx: CanvasRenderingContext2D) => void
> = {
  // A low hatch with an arrow coming out of it: the service way back.
  service(ctx) {
    ctx.fillRect(2.5, 8.5, 6.5, 5.5);
    ctx.beginPath();
    ctx.moveTo(9.5, 8.5);
    ctx.lineTo(12, 8.5);
    ctx.lineTo(12, 6.5);
    ctx.lineTo(14, 9.25);
    ctx.lineTo(12, 12);
    ctx.lineTo(12, 10);
    ctx.lineTo(9.5, 10);
    ctx.closePath();
    ctx.fill();
    ctx.lineWidth = 0.8;
    ctx.beginPath();
    ctx.moveTo(2.5, 5.5);
    ctx.lineTo(9, 5.5);
    ctx.stroke();
  },
  // A ring with a spiral inside: the way to another domain.
  portal(ctx) {
    ctx.lineWidth = 1.2;
    ctx.beginPath();
    ctx.arc(8, 8, 5.2, 0, Math.PI * 2);
    ctx.stroke();
    ctx.lineWidth = 0.9;
    ctx.beginPath();
    for (let t = 0; t <= Math.PI * 4; t += 0.1) {
      const r = 0.4 + t * 0.28;
      const x = 8 + Math.cos(t) * r;
      const y = 8 + Math.sin(t) * r;
      if (t === 0) ctx.moveTo(x, y);
      else ctx.lineTo(x, y);
    }
    ctx.stroke();
  },
  // A doorway with an arrow walking through it: milestone 1's sign.
  door(ctx) {
    ctx.fillRect(9, 4, 3, 8);
    ctx.beginPath();
    ctx.moveTo(3.5, 7);
    ctx.lineTo(7, 7);
    ctx.lineTo(7, 5);
    ctx.lineTo(9, 8);
    ctx.lineTo(7, 11);
    ctx.lineTo(7, 9);
    ctx.lineTo(3.5, 9);
    ctx.closePath();
    ctx.fill();
  },
};

/**
 * The pictogram layer: the `PICTOGRAM` set of Semiotic Standard style
 * signs, each a square frame around its symbol, drawn as vector paths in
 * white on black so the shader can tint them. Each tile keeps a dark margin
 * on every side, so the coarser mip levels do not bleed one sign into its
 * neighbour.
 */
export function drawPictogramLayer(size: number): Uint8Array {
  const ctx = canvas2d(size);
  ctx.fillStyle = "#000";
  ctx.fillRect(0, 0, size, size);
  for (const key of Object.keys(PICTOGRAM) as (keyof typeof PICTOGRAM)[]) {
    const rect = PICTOGRAM[key];
    const tile = rect.uw * size;
    // The canvas's top is texture v 1: a tile at v0 starts this far down.
    const left = rect.u0 * size;
    const top = (1 - rect.v0 - rect.vh) * size;
    ctx.save();
    ctx.translate(left, top);
    ctx.scale(tile / 16, (rect.vh * size) / 16);
    ctx.strokeStyle = "#fff";
    ctx.fillStyle = "#fff";
    ctx.lineWidth = 1;
    ctx.strokeRect(1.5, 1.5, 13, 13);
    SIGNS[key](ctx);
    ctx.restore();
  }
  return pixels(ctx, size);
}
