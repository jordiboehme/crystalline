/**
 * The decisions behind a diagram's PNG: how sharp it can be and what is said
 * when it cannot be, and that a failure leaves no file and no live URL.
 *
 * jsdom has no canvas, no SVG image decoding and no layout, so what the
 * browser does is faked here at its edges, and only so the decisions can be
 * reached. Whether the file is any good is `e2e/diagram-png.spec.ts`'s question.
 */

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import {
  exportDiagramPng,
  measureDrawing,
  planScale,
  PNG_FAILED,
  PNG_HAS_OUTSIDE_PICTURE,
  PNG_NOT_SHARP,
  PNG_TOO_LARGE,
  serializeForImage,
} from "./diagramPng";

const SVG_NS = "http://www.w3.org/2000/svg";

function parse(markup: string): SVGSVGElement {
  const host = document.createElement("div");
  host.innerHTML = markup;
  document.body.append(host);
  return host.querySelector("svg") as SVGSVGElement;
}

const matrix = (scale: number) =>
  ({
    a: scale,
    b: 0,
    c: 0,
    d: scale,
    inverse: () => matrix(1 / scale),
    multiply: (other: { a: number }) => matrix(scale * other.a),
  }) as unknown as DOMMatrix;

/** What a layout would answer, in a test environment that has none. */
function lay(element: Element, scale: number) {
  Object.assign(element, {
    getScreenCTM: () => matrix(scale),
    getBBox: () => ({ x: 0, y: 0, width: 10, height: 10 }),
  });
}

/** A browser that allows this many pixels in all. */
const upTo =
  (pixels: number) =>
  (width: number, height: number): boolean =>
    width * height <= pixels;

describe("the scale of a diagram's PNG", () => {
  const anything = () => true;

  it("is 2 when the text is large enough at 2", () => {
    expect(
      planScale({ width: 600, height: 400, smallestFontPx: 16 }, anything),
    ).toEqual({
      kind: "sharp",
      scale: 2,
      pixelWidth: 1200,
      pixelHeight: 800,
    });
  });

  it("brings the smallest text to 28 pixels when 2 would leave it smaller", () => {
    const plan = planScale(
      { width: 1000, height: 100, smallestFontPx: 7 },
      anything,
    );
    expect(plan).toEqual({
      kind: "sharp",
      scale: 4,
      pixelWidth: 4000,
      pixelHeight: 400,
    });
  });

  it("brings the thinnest stroke to 1.5 pixels when it asks for more than the text", () => {
    expect(
      planScale(
        {
          width: 100,
          height: 100,
          smallestFontPx: 16,
          thinnestStrokePx: 0.5,
        },
        anything,
      ),
    ).toEqual({ kind: "sharp", scale: 3, pixelWidth: 300, pixelHeight: 300 });
  });

  it("leaves the scale to the text when the text asks for more", () => {
    expect(
      planScale(
        { width: 100, height: 100, smallestFontPx: 7, thinnestStrokePx: 0.5 },
        anything,
      ),
    ).toMatchObject({ scale: 4 });
  });

  it("lets a thin stroke drive a drawing with no text", () => {
    expect(
      planScale(
        {
          width: 100,
          height: 100,
          smallestFontPx: null,
          thinnestStrokePx: 0.25,
        },
        anything,
      ),
    ).toMatchObject({ scale: 6 });
  });

  it("holds a thin stroke to the same limit, and says it is not sharp", () => {
    const plan = planScale(
      { width: 1000, height: 1000, smallestFontPx: 16, thinnestStrokePx: 0.5 },
      upTo(2000 * 2000),
    );
    expect(plan.kind).toBe("reduced");
    if (plan.kind === "reduced") {
      expect(plan.scale).toBeLessThanOrEqual(2);
      expect(plan.scale).toBeGreaterThan(1.99);
    }
  });

  it("rounds the pixels up, so a fractional drawing is never cut", () => {
    const plan = planScale(
      { width: 100.25, height: 50.5, smallestFontPx: 16 },
      anything,
    );
    expect(plan).toMatchObject({ pixelWidth: 201, pixelHeight: 101 });
  });

  it("takes 2 for a drawing with no text in it", () => {
    expect(
      planScale({ width: 10, height: 10, smallestFontPx: null }, anything),
    ).toMatchObject({ kind: "sharp", scale: 2 });
  });

  it("steps down to the largest scale that fits and says it is not sharp", () => {
    const plan = planScale(
      { width: 1000, height: 1000, smallestFontPx: 14 },
      upTo(1_000_000 * 1.5 * 1.5),
    );
    expect(plan.kind).toBe("reduced");
    if (plan.kind !== "reduced") {
      return;
    }
    expect(plan.scale).toBeLessThanOrEqual(1.5);
    expect(plan.scale).toBeGreaterThan(1.49);
    expect(plan.pixelWidth * plan.pixelHeight).toBeLessThanOrEqual(2_250_000);
  });

  it("respects a limit on one side, not only on the area", () => {
    const plan = planScale(
      { width: 10_000, height: 10, smallestFontPx: 14 },
      (width) => width <= 15_000,
    );
    expect(plan.kind).toBe("reduced");
    if (plan.kind === "reduced") {
      expect(plan.pixelWidth).toBeLessThanOrEqual(15_000);
      expect(plan.pixelWidth).toBeGreaterThan(14_900);
    }
  });

  it("still saves when the smallest text comes out at exactly 10 pixels", () => {
    // 10 / 16 of 1000 is 625 pixels a side, and nothing more fits.
    const plan = planScale(
      { width: 1000, height: 1000, smallestFontPx: 16 },
      upTo(625 * 625),
    );
    expect(plan).toMatchObject({ kind: "reduced", scale: 0.625 });
  });

  it("refuses when the smallest text would come out below 10 pixels", () => {
    expect(
      planScale(
        { width: 1000, height: 1000, smallestFontPx: 16 },
        upTo(624 * 624),
      ),
    ).toEqual({ kind: "refused" });
  });

  it("refuses a drawing with no text when it does not fit at its own size", () => {
    expect(
      planScale({ width: 1000, height: 1000, smallestFontPx: null }, upTo(10)),
    ).toEqual({ kind: "refused" });
  });
});

describe("measuring a drawing", () => {
  afterEach(() => {
    document.body.innerHTML = "";
  });

  it("takes transforms into account and ignores how large it is on screen", () => {
    const svg = parse(
      `<svg xmlns="${SVG_NS}" viewBox="0 0 100 100">
        <text id="big" style="font-size: 16px">Big</text>
        <g><text id="small" style="font-size: 16px">Small</text></g>
      </svg>`,
    );
    // Shown at a quarter of its size, with one label scaled down by half more.
    lay(svg, 0.25);
    lay(svg.querySelector("#big") as Element, 0.25);
    lay(svg.querySelector("#small") as Element, 0.125);
    expect(measureDrawing(svg).smallestFontPx).toBeCloseTo(8);
  });

  it("does not count text nobody can see", () => {
    const svg = parse(
      `<svg xmlns="${SVG_NS}" viewBox="0 0 100 100">
        <text id="shown" style="font-size: 16px">Shown</text>
        <text id="none" style="font-size: 2px; display: none">Gone</text>
        <text id="hidden" style="font-size: 2px; visibility: hidden">Gone</text>
        <text id="clear" style="font-size: 2px; opacity: 0">Gone</text>
        <text id="empty" style="font-size: 2px">   </text>
        <defs><text id="defined" style="font-size: 2px">Gone</text></defs>
        <switch>
          <foreignObject id="object" width="10" height="10"></foreignObject>
          <text id="fallback" style="font-size: 2px">Gone</text>
        </switch>
      </svg>`,
    );
    lay(svg, 1);
    for (const element of svg.querySelectorAll("*")) {
      lay(element, 1);
    }
    expect(measureDrawing(svg).smallestFontPx).toBeCloseTo(16);
  });

  it("counts a run with its own size, and text with no box as not drawn", () => {
    const svg = parse(
      `<svg xmlns="${SVG_NS}" viewBox="0 0 100 100">
        <text id="runs" style="font-size: 16px"><tspan style="font-size: 9px">Run</tspan></text>
        <text id="boxless" style="font-size: 1px">Gone</text>
      </svg>`,
    );
    lay(svg, 1);
    lay(svg.querySelector("#runs") as Element, 1);
    lay(svg.querySelector("#boxless") as Element, 1);
    Object.assign(svg.querySelector("#boxless") as Element, {
      getBBox: () => ({ x: 0, y: 0, width: 0, height: 0 }),
    });
    expect(measureDrawing(svg).smallestFontPx).toBeCloseTo(9);
  });

  it("counts the text inside a foreign object through the object's transform", () => {
    const svg = parse(
      `<svg xmlns="${SVG_NS}" viewBox="0 0 100 100">
        <foreignObject id="object" width="10" height="10">
          <div xmlns="http://www.w3.org/1999/xhtml" style="font-size: 12px">Label</div>
        </foreignObject>
      </svg>`,
    );
    lay(svg, 1);
    lay(svg.querySelector("foreignObject") as Element, 0.5);
    expect(measureDrawing(svg).smallestFontPx).toBeCloseTo(6);
  });

  it("finds the thinnest stroke that is really drawn", () => {
    const svg = parse(
      `<svg xmlns="${SVG_NS}" viewBox="0 0 100 100">
        <path id="line" d="M0 0L9 9" style="stroke: #000; stroke-width: 1.5px"></path>
        <path id="scaled" d="M0 0L9 9" style="stroke: #000; stroke-width: 2px"></path>
        <path id="none" d="M0 0L9 9" style="stroke: none; stroke-width: 0.1px"></path>
        <path id="zero" d="M0 0L9 9" style="stroke: #000; stroke-width: 0"></path>
      </svg>`,
    );
    lay(svg, 1);
    for (const path of svg.querySelectorAll("path")) {
      lay(path, path.id === "scaled" ? 0.5 : 1);
    }
    expect(measureDrawing(svg).thinnestStrokePx).toBeCloseTo(1);
  });

  it("ignores a hairline, which would otherwise blow up the scale", () => {
    const svg = parse(
      `<svg xmlns="${SVG_NS}" viewBox="0 0 100 100">
        <path id="hairline" d="M0 0L9 9" style="stroke: #000; stroke-width: 0.01px"></path>
        <path id="scaled" d="M0 0L9 9" style="stroke: #000; stroke-width: 1px"></path>
        <path id="line" d="M0 0L9 9" style="stroke: #000; stroke-width: 0.4px"></path>
      </svg>`,
    );
    lay(svg, 1);
    for (const path of svg.querySelectorAll("path")) {
      // A full stroke scaled down to a hairline counts as a hairline too.
      lay(path, path.id === "scaled" ? 0.05 : 1);
    }
    expect(measureDrawing(svg).thinnestStrokePx).toBeCloseTo(0.4);
  });

  it("says nothing when every stroke is a hairline", () => {
    const svg = parse(
      `<svg xmlns="${SVG_NS}" viewBox="0 0 100 100"><path d="M0 0L9 9" style="stroke: #000; stroke-width: 0.01px"></path></svg>`,
    );
    lay(svg, 1);
    lay(svg.querySelector("path") as Element, 1);
    expect(measureDrawing(svg).thinnestStrokePx).toBeNull();
  });

  it("says nothing about what the drawing does not have", () => {
    const svg = parse(`<svg xmlns="${SVG_NS}" viewBox="0 0 100 100"></svg>`);
    lay(svg, 1);
    expect(measureDrawing(svg)).toEqual({
      smallestFontPx: null,
      thinnestStrokePx: null,
    });
  });
});

describe("the copy of the drawing an image can read", () => {
  afterEach(() => {
    document.body.innerHTML = "";
    vi.restoreAllMocks();
  });

  it("carries its output size and leaves the live drawing alone", () => {
    const svg = parse(
      `<svg xmlns="${SVG_NS}" viewBox="0 0 100 50" width="100%" style="max-width: 100px;"><text>&amp; label</text></svg>`,
    );
    const before = svg.outerHTML;
    const markup = serializeForImage(svg, 400, 200);
    const copy = new DOMParser().parseFromString(markup, "image/svg+xml");
    expect(copy.querySelector("parsererror")).toBeNull();
    const root = copy.documentElement;
    expect(root.getAttribute("width")).toBe("400");
    expect(root.getAttribute("height")).toBe("200");
    expect(root.getAttribute("style") ?? "").not.toContain("max-width");
    expect(markup).toContain(`xmlns="${SVG_NS}"`);
    expect(svg.outerHTML).toBe(before);
  });

  it("draws a foreign label as the lines on screen, on the copy alone", () => {
    const svg = parse(
      `<svg xmlns="${SVG_NS}" viewBox="0 0 100 50"><switch><foreignObject x="10" y="20" width="100" height="50"><div xmlns="http://www.w3.org/1999/xhtml"><div style="color: rgb(1, 2, 3); font-size: 11px">Two words wrap</div></div></foreignObject><text style="fill: rgb(9, 9, 9)"><tspan>Two words wrap</tspan></text></switch></svg>`,
    );
    const object = svg.querySelector("foreignObject") as Element;
    // Shown at twice its size: "Two words" on one line, "wrap" under it.
    object.getBoundingClientRect = () =>
      ({ left: 0, top: 0, width: 200, height: 100 }) as DOMRect;
    const boxes = [
      { left: 20, right: 60, top: 10, bottom: 30, height: 20 },
      { left: 70, right: 180, top: 10, bottom: 30, height: 20 },
      { left: 80, right: 120, top: 40, bottom: 60, height: 20 },
    ];
    let word = 0;
    Object.defineProperty(Range.prototype, "getClientRects", {
      configurable: true,
      value: () => [boxes[word++]],
    });

    const copy = new DOMParser().parseFromString(
      serializeForImage(svg, 100, 50),
      "image/svg+xml",
    );

    expect(copy.querySelector("foreignObject")).toBeNull();
    expect(svg.querySelector("foreignObject")).not.toBeNull();
    const label = copy.querySelector("switch > svg") as Element;
    expect(label.getAttribute("x")).toBe("10");
    expect(label.getAttribute("width")).toBe("100");
    const lines = Array.from(label.querySelectorAll("text"));
    expect(lines.map((line) => line.textContent)).toEqual([
      "Two words",
      "wrap",
    ]);
    expect(lines[0]?.getAttribute("x")).toBe("50");
    expect(lines[0]?.getAttribute("y")).toBe("10");
    expect(lines[1]?.getAttribute("x")).toBe("50");
    expect(lines[1]?.getAttribute("y")).toBe("25");
    expect(lines[0]?.getAttribute("style")).toContain("fill: rgb(1, 2, 3)");
    expect(lines[0]?.getAttribute("style")).toContain("font-size: 11px");
    // The fallback is gone: it would be drawn as well or instead.
    expect(copy.querySelectorAll("switch > text")).toHaveLength(0);
  });

  it("writes what the page's styles resolved to onto the copy alone", () => {
    const svg = parse(
      `<svg xmlns="${SVG_NS}" viewBox="0 0 100 50"><text id="label">Label</text></svg>`,
    );
    const before = svg.outerHTML;
    const real = window.getComputedStyle.bind(window);
    vi.spyOn(window, "getComputedStyle").mockImplementation((element) => {
      const style = real(element);
      if (element.id !== "label") {
        return style;
      }
      return {
        getPropertyValue: (name: string) =>
          name === "fill" ? "rgb(1, 2, 3)" : style.getPropertyValue(name),
      } as CSSStyleDeclaration;
    });
    const markup = serializeForImage(svg, 100, 50);
    const label = new DOMParser()
      .parseFromString(markup, "image/svg+xml")
      .querySelector("#label");
    expect(label?.getAttribute("style")).toContain("fill: rgb(1, 2, 3)");
    expect(svg.outerHTML).toBe(before);
  });
});

/**
 * The browser's edges, faked. `decode` decides how the image comes out, `blob`
 * what the encoder hands back and `pixels` what the finished canvas reads as.
 */
function fakeBrowser(
  options: {
    decode?: () => Promise<void>;
    blob?: () => Blob | null;
    toBlobThrows?: boolean;
    pixels?: Uint8ClampedArray;
    corner?: number;
  } = {},
) {
  const made: string[] = [];
  const revoked: string[] = [];
  const painted: { fill: unknown; image: unknown }[] = [];
  Object.defineProperty(URL, "createObjectURL", {
    configurable: true,
    value: vi.fn(() => {
      const url = `blob:fake/${String(made.length)}`;
      made.push(url);
      return url;
    }),
  });
  Object.defineProperty(URL, "revokeObjectURL", {
    configurable: true,
    value: vi.fn((url: string) => {
      revoked.push(url);
    }),
  });
  // jsdom's images have no `decode` at all.
  Object.defineProperty(window.HTMLImageElement.prototype, "decode", {
    configurable: true,
    value: options.decode ?? (() => Promise.resolve()),
  });
  // A checker of background and ink, the shape of a real drawing.
  const defaultPixels = new Uint8ClampedArray(128 * 128 * 4);
  for (let i = 0; i < defaultPixels.length; i += 4) {
    const ink = i % 8 === 0 ? 0 : 255;
    defaultPixels.set([ink, ink, ink, 255], i);
  }
  const context = {
    fillStyle: "",
    fillRect: vi.fn(function (this: { fillStyle: string }) {
      painted.push({ fill: this.fillStyle, image: null });
    }),
    drawImage: vi.fn((image: unknown) => {
      painted.push({ fill: null, image });
    }),
    getImageData: vi.fn((_x: number, _y: number, w: number) => ({
      data:
        w === 1
          ? new Uint8ClampedArray([0, 0, 0, options.corner ?? 255])
          : (options.pixels ?? defaultPixels),
    })),
  };
  vi.spyOn(HTMLCanvasElement.prototype, "getContext").mockReturnValue(
    context as never,
  );
  vi.spyOn(HTMLCanvasElement.prototype, "toBlob").mockImplementation(
    function (callback) {
      if (options.toBlobThrows === true) {
        throw new DOMException("tainted", "SecurityError");
      }
      callback(
        options.blob === undefined
          ? new Blob(["png"], { type: "image/png" })
          : options.blob(),
      );
    },
  );
  return { made, revoked, painted };
}

describe("making the PNG", () => {
  const drawing = (
    markup = '<text id="label" style="font-size: 16px">Label</text>',
  ) => {
    const svg = parse(
      `<svg xmlns="${SVG_NS}" viewBox="0 0 100 50">${markup}</svg>`,
    );
    lay(svg, 1);
    for (const element of svg.querySelectorAll("*")) {
      lay(element, 1);
    }
    return svg;
  };
  const request = () => ({
    drawing: drawing(),
    natural: { width: 100, height: 50 },
    background: "rgb(255, 255, 255)",
  });

  beforeEach(() => {
    vi.spyOn(console, "error").mockImplementation(() => undefined);
  });

  afterEach(() => {
    document.body.innerHTML = "";
    vi.restoreAllMocks();
  });

  it("paints the background first, then the drawing, and hands over a PNG", async () => {
    const browser = fakeBrowser();
    const made = await exportDiagramPng(request(), () => true);
    expect(made.blob?.type).toBe("image/png");
    expect(made.note).toBeNull();
    expect(browser.painted[0]?.fill).toBe("rgb(255, 255, 255)");
    expect(browser.painted.some((step) => step.image !== null)).toBe(true);
    expect(browser.revoked).toEqual(browser.made);
  });

  it("says so when the file is not as sharp as it should be", async () => {
    fakeBrowser();
    const made = await exportDiagramPng({ ...request() }, upTo(100 * 50));
    expect(made.blob).not.toBeNull();
    expect(made.note).toBe(PNG_NOT_SHARP);
  });

  it("writes no file and points to the SVG below 10 pixel text", async () => {
    const browser = fakeBrowser();
    const made = await exportDiagramPng({ ...request() }, upTo(1));
    expect(made.blob).toBeNull();
    expect(made.note).toBe(PNG_TOO_LARGE);
    expect(made.note).toContain("Download SVG");
    expect(browser.made).toEqual([]);
  });

  it("makes the file large enough for the thinnest stroke", async () => {
    const browser = fakeBrowser();
    const sizes: number[] = [];
    const svg = drawing(
      '<text style="font-size: 16px">Label</text><path d="M0 0L9 9" style="stroke: #000; stroke-width: 0.5px"></path>',
    );
    const made = await exportDiagramPng(
      { ...request(), drawing: svg },
      (width) => {
        sizes.push(width);
        return true;
      },
    );
    expect(made.blob).not.toBeNull();
    expect(sizes).toEqual([300]);
    expect(browser.revoked).toEqual(browser.made);
  });

  it("writes no file when the image will not decode, and revokes its URL", async () => {
    const browser = fakeBrowser({
      decode: () => Promise.reject(new Error("no")),
    });
    const made = await exportDiagramPng(request());
    expect(made).toMatchObject({ blob: null, note: PNG_FAILED });
    expect(browser.made).toHaveLength(1);
    expect(browser.revoked).toEqual(browser.made);
  });

  it("writes no file when the canvas is tainted, and revokes its URL", async () => {
    const browser = fakeBrowser({ toBlobThrows: true });
    const made = await exportDiagramPng(request());
    expect(made).toMatchObject({ blob: null, note: PNG_FAILED });
    expect(browser.revoked).toEqual(browser.made);
  });

  it("writes no file when the encoder hands back nothing", async () => {
    const browser = fakeBrowser({ blob: () => null });
    const made = await exportDiagramPng(request());
    expect(made).toMatchObject({ blob: null, note: PNG_FAILED });
    expect(browser.revoked).toEqual(browser.made);
  });

  it("writes no file when the canvas came out all one colour", async () => {
    const browser = fakeBrowser({
      pixels: new Uint8ClampedArray(128 * 128 * 4).fill(255),
    });
    const made = await exportDiagramPng(request());
    expect(made).toMatchObject({ blob: null, note: PNG_FAILED });
    expect(browser.revoked).toEqual(browser.made);
  });

  it("writes no file when the background did not reach the corners", async () => {
    fakeBrowser({ corner: 0 });
    // The limit is out of the way here: this is the finished canvas's own read.
    const made = await exportDiagramPng(request(), () => true);
    expect(made).toMatchObject({ blob: null, note: PNG_FAILED });
  });

  it("writes no file for a picture an image cannot reach, and says why", async () => {
    const browser = fakeBrowser();
    const svg = drawing('<image href="https://example.invalid/a.png"></image>');
    const made = await exportDiagramPng({ ...request(), drawing: svg });
    expect(made).toMatchObject({ blob: null, note: PNG_HAS_OUTSIDE_PICTURE });
    expect(browser.made).toEqual([]);
  });

  it("takes a picture that travels inside the drawing", async () => {
    fakeBrowser();
    const svg = drawing('<image href="data:image/png;base64,AAAA"></image>');
    const made = await exportDiagramPng({ ...request(), drawing: svg });
    expect(made.blob).not.toBeNull();
  });
});
