/**
 * A diagram saved as a PNG, from the full window, as a real file.
 *
 * jsdom has no canvas and no SVG image decoding, so this is where the file is
 * proven. The file is read back and judged on its own bytes and pixels, and
 * how large it should be is worked out here from the drawing, not taken from
 * the code that made it.
 *
 * Set `FLUID_PNG_OUT` to a folder to keep every file for a look with the eye.
 */

import { expect, type Download, type Page, test } from "@playwright/test";
import { mkdirSync, readFileSync } from "node:fs";
import { join } from "node:path";

/** The account and the domain `run-smoke.sh` set up. Kept in step with it. */
const USER = process.env.FLUID_E2E_USER ?? "smoke";
const PASSWORD = process.env.FLUID_E2E_PASSWORD ?? "smoke-password";
const DOMAIN = process.env.FLUID_E2E_DOMAIN ?? "fluid-smoke";
const OUT = process.env.FLUID_PNG_OUT;

const NOT_SHARP =
  "This diagram is too large for a sharp PNG in this browser. The SVG stays sharp at any size.";
const TOO_LARGE =
  "This diagram is too large for a PNG in this browser, so no file was saved. Use Download SVG instead.";
const OUTSIDE_PICTURE =
  "This diagram shows a picture from outside. A PNG cannot hold it, so no file was saved. Use Download SVG instead.";

/** The order of the fences in the fixture, `diagram-png.md`. */
const WIDE = 0;
const SMALL_TEXT = 1;
const SEQUENCE = 2;
const SHORT = 3;
const WITH_PICTURE = 4;
const LONG_JOURNEY = 5;
const THIN_LINES = 6;

/** The user journey's place in `diagram-gallery.md`. */
const USER_JOURNEY = 8;

/** The text colour and the node fill mermaid is given per theme, `theme/mermaid.ts`. */
const INK = { light: [0x0f, 0x17, 0x2a], dark: [0xe2, 0xe8, 0xf0] };
const NODE = { light: [0xec, 0xe8, 0xf9], dark: [0x2a, 0x1f, 0x5f] };

/** One red pixel, as a PNG. */
const ONE_PIXEL =
  "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8DwHwAFBQIAX8jx0gAAAABJRU5ErkJggg==";

type Theme = "light" | "dark";

async function logIn(page: Page, theme: Theme): Promise<void> {
  await page.addInitScript((value) => {
    window.localStorage.setItem("fluid-theme", value);
  }, theme);
  await page.goto("./");
  await page.getByLabel("Name", { exact: true }).fill(USER);
  await page.getByLabel("Password", { exact: true }).fill(PASSWORD);
  await page.getByRole("button", { name: "Log in" }).click();
  await expect(
    page.getByRole("heading", { name: "Home", level: 1 }),
  ).toBeVisible();
}

/** The full window on one figure of an engram. */
async function openFigure(page: Page, index: number): Promise<void> {
  const figure = page.locator('figure[aria-label="Diagram"]').nth(index);
  await figure.scrollIntoViewIfNeeded();
  await expect(figure.locator('svg[id^="mermaid-"]').first()).toBeVisible();
  // Twice if need be: figures above this one are still drawing, and when one
  // arrives this one moves out from under the pointer and its toolbar hides.
  await expect(async () => {
    await figure.hover();
    await figure
      .getByRole("button", { name: "Open in full window" })
      .click({ timeout: 2000 });
  }).toPass();
  await expect(
    page.getByRole("dialog").locator('svg[id^="mermaid-"]'),
  ).toBeVisible();
  // The full window fits the drawing once its library has arrived.
  await expect(
    page.getByRole("button", { name: "Download PNG" }),
  ).toBeVisible();
}

async function closeFigure(page: Page): Promise<void> {
  await page.getByRole("button", { name: "Close", exact: true }).click();
  await expect(page.getByRole("dialog")).toHaveCount(0);
}

/** The press, and the file it starts. */
async function pressAndSave(page: Page, keepAs: string): Promise<Buffer> {
  const started: Promise<Download> = page.waitForEvent("download");
  await page.getByRole("button", { name: "Download PNG" }).click();
  const download = await started;
  expect(download.suggestedFilename()).toMatch(
    /^diagram-png-diagram-[0-9a-f]{4}\.png$|^diagram-gallery-diagram-[0-9a-f]{4}\.png$/,
  );
  const path = await download.path();
  if (OUT !== undefined) {
    mkdirSync(OUT, { recursive: true });
    await download.saveAs(join(OUT, `${keepAs}.png`));
    await page.screenshot({ path: join(OUT, `${keepAs}.screen.png`) });
  }
  return readFileSync(path);
}

/** The size a PNG's own header states, and that it is one. */
function pngSize(bytes: Buffer): { width: number; height: number } {
  expect(Array.from(bytes.subarray(0, 8))).toEqual([
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a,
  ]);
  return { width: bytes.readUInt32BE(16), height: bytes.readUInt32BE(20) };
}

/** The drawing's own numbers, measured here and not by the code under test. */
function measure(page: Page) {
  return page
    .getByRole("dialog")
    .locator('svg[id^="mermaid-"]')
    .evaluate((svg: SVGSVGElement) => {
      const toDrawing = svg.getScreenCTM()?.inverse();
      const scaleOf = (element: SVGGraphicsElement) => {
        const matrix = element.getScreenCTM();
        if (matrix === null || toDrawing === undefined) {
          return 1;
        }
        const { a, b } = toDrawing.multiply(matrix);
        return Math.hypot(a, b);
      };
      let font = Infinity;
      for (const text of svg.querySelectorAll<SVGGraphicsElement>("text")) {
        if ((text.textContent ?? "").trim() === "") {
          continue;
        }
        for (const run of [text, ...text.querySelectorAll("tspan")]) {
          const size = Number.parseFloat(getComputedStyle(run).fontSize);
          font = Math.min(font, size * scaleOf(text));
        }
      }
      let stroke = Infinity;
      for (const shape of svg.querySelectorAll<SVGGraphicsElement>(
        "path, line, rect, circle, ellipse, polygon",
      )) {
        const style = getComputedStyle(shape);
        const width = Number.parseFloat(style.strokeWidth);
        if (style.stroke !== "none" && width > 0) {
          stroke = Math.min(stroke, width * scaleOf(shape));
        }
      }
      const box = (svg.getAttribute("viewBox") ?? "").trim().split(/[\s,]+/);
      return {
        font,
        stroke,
        width: Number(box[2]),
        height: Number(box[3]),
        onScreenWidth: svg.getBoundingClientRect().width,
      };
    });
}

/** What the pixels of a file are, decoded by the browser that made it. */
function readPixels(page: Page, bytes: Buffer, ink: number[], node: number[]) {
  return page.evaluate(
    async ({ base64, ink, node }) => {
      const data = Uint8Array.from(atob(base64), (c) => c.charCodeAt(0));
      const bitmap = await createImageBitmap(
        new Blob([data], { type: "image/png" }),
      );
      // A rough copy is enough to judge a whole file, and the corners and the
      // colours are read from a piece at full size.
      const side = Math.min(bitmap.width, 4096);
      const tall = Math.min(bitmap.height, 4096);
      const canvas = document.createElement("canvas");
      canvas.width = side;
      canvas.height = tall;
      const context = canvas.getContext("2d");
      if (context === null) {
        throw new Error("no canvas");
      }
      context.drawImage(bitmap, 0, 0);
      const { data: pixels } = context.getImageData(0, 0, side, tall);
      const near = (i: number, colour: number[]) =>
        Math.abs((pixels[i] ?? 0) - (colour[0] ?? 0)) <= 12 &&
        Math.abs((pixels[i + 1] ?? 0) - (colour[1] ?? 0)) <= 12 &&
        Math.abs((pixels[i + 2] ?? 0) - (colour[2] ?? 0)) <= 12;
      let notOpaque = 0;
      let inked = 0;
      let filled = 0;
      for (let i = 0; i < pixels.length; i += 4) {
        if (pixels[i + 3] !== 255) {
          notOpaque += 1;
        }
        if (near(i, ink)) {
          inked += 1;
        }
        if (near(i, node)) {
          filled += 1;
        }
      }
      const corner = Array.from(pixels.subarray(0, 4));
      return { notOpaque, inked, filled, corner };
    },
    { base64: bytes.toString("base64"), ink, node },
  );
}

/** Where a foreign object's label sits in the drawing, and what it says. */
function objectBox(page: Page, index: number) {
  return page
    .getByRole("dialog")
    .locator("foreignObject")
    .nth(index)
    .evaluate((object: SVGGraphicsElement) => {
      const svg = object.ownerSVGElement;
      const toDrawing = svg?.getScreenCTM()?.inverse();
      const box = object.getBoundingClientRect();
      const at = (x: number, y: number) =>
        new DOMPoint(x, y).matrixTransform(toDrawing);
      const start = at(box.left, box.top);
      const end = at(box.right, box.bottom);
      return {
        words: object.textContent,
        x: start.x,
        y: start.y,
        width: end.x - start.x,
        height: end.y - start.y,
      };
    });
}

/** How much of the theme's text colour a piece of a file holds. */
function inkInside(
  page: Page,
  bytes: Buffer,
  ink: number[],
  piece: { x: number; y: number; width: number; height: number },
) {
  return page.evaluate(
    async ({ base64, ink, piece }) => {
      const data = Uint8Array.from(atob(base64), (c) => c.charCodeAt(0));
      const bitmap = await createImageBitmap(
        new Blob([data], { type: "image/png" }),
      );
      const canvas = document.createElement("canvas");
      canvas.width = Math.ceil(piece.width);
      canvas.height = Math.ceil(piece.height);
      const context = canvas.getContext("2d");
      if (context === null) {
        throw new Error("no canvas");
      }
      context.drawImage(bitmap, -piece.x, -piece.y);
      const { data: pixels } = context.getImageData(
        0,
        0,
        canvas.width,
        canvas.height,
      );
      let inked = 0;
      for (let i = 0; i < pixels.length; i += 4) {
        if (
          Math.abs((pixels[i] ?? 0) - (ink[0] ?? 0)) <= 40 &&
          Math.abs((pixels[i + 1] ?? 0) - (ink[1] ?? 0)) <= 40 &&
          Math.abs((pixels[i + 2] ?? 0) - (ink[2] ?? 0)) <= 40
        ) {
          inked += 1;
        }
      }
      return inked;
    },
    { base64: bytes.toString("base64"), ink, piece },
  );
}

/** The window's background as the screen has it, as red, green and blue. */
function screenBackground(page: Page): Promise<number[]> {
  return page.getByRole("dialog").evaluate((dialog) => {
    const canvas = document.createElement("canvas");
    canvas.width = 1;
    canvas.height = 1;
    const context = canvas.getContext("2d");
    if (context === null) {
      return [];
    }
    context.fillStyle = getComputedStyle(dialog).backgroundColor;
    context.fillRect(0, 0, 1, 1);
    return Array.from(context.getImageData(0, 0, 1, 1).data.subarray(0, 3));
  });
}

/** A browser whose canvases stop existing above this many pixels, as a real one's do. */
async function limitCanvases(page: Page, pixels: number): Promise<void> {
  await page.addInitScript((limit) => {
    // Called below with `apply` and the right `this`.
    // eslint-disable-next-line @typescript-eslint/unbound-method
    const real = CanvasRenderingContext2D.prototype.getImageData;
    CanvasRenderingContext2D.prototype.getImageData = function (
      this: CanvasRenderingContext2D,
      ...args: Parameters<typeof real>
    ) {
      return this.canvas.width * this.canvas.height > limit
        ? new ImageData(1, 1)
        : real.apply(this, args);
    };
  }, pixels);
}

for (const theme of ["light", "dark"] as const) {
  for (const [name, index] of [
    ["flowchart", SHORT],
    ["sequence", SEQUENCE],
  ] as const) {
    test(`a ${name} in ${theme} saves the picture on the screen`, async ({
      page,
      browserName,
    }) => {
      await logIn(page, theme);
      await page.goto(`d/${DOMAIN}/e/diagram-png`);
      await openFigure(page, index);
      const live = page.getByRole("dialog").locator('svg[id^="mermaid-"]');
      const before = await live.evaluate((svg) => svg.outerHTML);
      const drawing = await measure(page);
      const asked: string[] = [];
      page.on("request", (request) => {
        asked.push(request.url());
      });

      const bytes = await pressAndSave(page, `${browserName}-${theme}-${name}`);

      const size = pngSize(bytes);
      const scale = Math.max(2, 28 / drawing.font);
      expect(size.width).toBe(Math.ceil(drawing.width * scale));
      expect(size.height).toBe(Math.ceil(drawing.height * scale));
      const pixels = await readPixels(page, bytes, INK[theme], NODE[theme]);
      expect(pixels.notOpaque, "every pixel is solid").toBe(0);
      expect(pixels.corner.slice(0, 3), "the theme's background").toEqual(
        await screenBackground(page),
      );
      expect(
        pixels.inked,
        "the text is there, in the theme's colour",
      ).toBeGreaterThan(0);
      expect(
        pixels.filled,
        "the shapes are there, in the theme's colour",
      ).toBeGreaterThan(0);
      // Nothing was changed on the screen and nothing went to the server.
      expect(await live.evaluate((svg) => svg.outerHTML)).toBe(before);
      expect(asked.filter((url) => url.includes("/api/"))).toEqual([]);
      console.log(
        `png ${browserName} ${theme} ${name}: font ${String(drawing.font)} stroke ${String(drawing.stroke)} scale ${String(size.width / drawing.width)} size ${String(size.width)}x${String(size.height)} bytes ${String(bytes.length)}`,
      );
    });
  }

  test(`a very wide flowchart in ${theme} keeps its smallest text at 28 pixels`, async ({
    page,
    browserName,
  }) => {
    await logIn(page, theme);
    await page.goto(`d/${DOMAIN}/e/diagram-png`);
    await openFigure(page, WIDE);
    const drawing = await measure(page);
    // The point of the test: on screen it is far smaller than its own size,
    // once the full window has fitted it.
    await expect
      .poll(async () => (await measure(page)).onScreenWidth)
      .toBeLessThan(drawing.width / 2);
    const bytes = await pressAndSave(page, `${browserName}-${theme}-wide`);
    const size = pngSize(bytes);
    expect((size.width / drawing.width) * drawing.font).toBeGreaterThanOrEqual(
      28,
    );
    await expect(
      page.getByRole("dialog").getByRole("status"),
    ).not.toContainText("too large");
    const pixels = await readPixels(page, bytes, INK[theme], NODE[theme]);
    expect(pixels.notOpaque).toBe(0);
    expect(pixels.inked).toBeGreaterThan(0);
    console.log(
      `png ${browserName} ${theme} wide: viewBox ${String(drawing.width)}x${String(drawing.height)} on screen ${String(drawing.onScreenWidth)} font ${String(drawing.font)} stroke ${String(drawing.stroke)} scale ${String(size.width / drawing.width)} size ${String(size.width)}x${String(size.height)} bytes ${String(bytes.length)}`,
    );
  });
}

test("a user journey with long words keeps them in their boxes", async ({
  page,
  browserName,
}) => {
  await logIn(page, "light");
  await page.goto(`d/${DOMAIN}/e/diagram-png`);
  await openFigure(page, LONG_JOURNEY);
  const drawing = await measure(page);
  const task = await objectBox(page, 1);
  const bytes = await pressAndSave(page, `${browserName}-light-long-journey`);
  const scale = pngSize(bytes).width / drawing.width;
  expect(
    await inkInside(page, bytes, INK.light, {
      x: task.x * scale,
      y: task.y * scale,
      width: task.width * scale,
      height: task.height * scale,
    }),
    "the words are in the box",
  ).toBeGreaterThan(50);
  expect(
    await inkInside(page, bytes, INK.light, {
      x: (task.x - 40) * scale,
      y: (task.y + 4) * scale,
      width: 36 * scale,
      height: (task.height - 8) * scale,
    }),
    "and do not run out of it",
  ).toBe(0);
});

test("small text sets a scale above two", async ({ page, browserName }) => {
  await logIn(page, "light");
  await page.goto(`d/${DOMAIN}/e/diagram-png`);
  await openFigure(page, SMALL_TEXT);
  const drawing = await measure(page);
  const bytes = await pressAndSave(page, `${browserName}-light-small-text`);
  const size = pngSize(bytes);
  const scale = size.width / drawing.width;
  console.log(
    `png ${browserName} light small text: font ${String(drawing.font)} scale ${String(scale)} size ${String(size.width)}x${String(size.height)}`,
  );
  expect(drawing.font * scale).toBeGreaterThanOrEqual(28);
});

test("a thin line sets a scale above two", async ({ page, browserName }) => {
  await logIn(page, "light");
  await page.goto(`d/${DOMAIN}/e/diagram-png`);
  await openFigure(page, THIN_LINES);
  const drawing = await measure(page);
  const bytes = await pressAndSave(page, `${browserName}-light-thin-lines`);
  const size = pngSize(bytes);
  const scale = size.width / drawing.width;
  console.log(
    `png ${browserName} light thin lines: font ${String(drawing.font)} stroke ${String(drawing.stroke)} scale ${String(scale)} size ${String(size.width)}x${String(size.height)}`,
  );
  // 1.5 pixels from a 0.4 line, where the 16 pixel text alone would ask for 2.
  expect(drawing.stroke).toBeCloseTo(0.4);
  expect(scale).toBeGreaterThanOrEqual(1.5 / 0.4);
  expect(scale).toBeLessThan(3.8);
});

test("past the canvas limit the file comes smaller and says so", async ({
  page,
  browserName,
}) => {
  const limit = 2_000_000;
  await limitCanvases(page, limit);
  await logIn(page, "dark");
  await page.goto(`d/${DOMAIN}/e/diagram-png`);
  await openFigure(page, WIDE);
  const drawing = await measure(page);
  const bytes = await pressAndSave(page, `${browserName}-dark-forced-limit`);
  const size = pngSize(bytes);
  const scale = size.width / drawing.width;
  await expect(page.getByRole("dialog").getByRole("status")).toHaveText(
    NOT_SHARP,
  );
  expect(size.width * size.height).toBeLessThanOrEqual(limit);
  // The largest that fits: one more row and column of pixels would not. A
  // percentage is no test here, since one row of a very wide picture is more
  // than one percent of it.
  expect((size.width + 1) * (size.height + 1)).toBeGreaterThan(limit);
  expect(drawing.font * scale).toBeLessThan(28);
  expect(drawing.font * scale).toBeGreaterThanOrEqual(10);
  console.log(
    `png ${browserName} forced limit: scale ${String(scale)} size ${String(size.width)}x${String(size.height)} smallest text ${String(drawing.font * scale)}`,
  );
});

test("where the text would be under 10 pixels no file is saved", async ({
  page,
}) => {
  await limitCanvases(page, 100_000);
  await logIn(page, "light");
  await page.goto(`d/${DOMAIN}/e/diagram-png`);
  await openFigure(page, WIDE);
  let saved = 0;
  page.on("download", () => {
    saved += 1;
  });
  await page.getByRole("button", { name: "Download PNG" }).click();
  await expect(page.getByRole("dialog").getByRole("status")).toHaveText(
    TOO_LARGE,
  );
  expect(saved).toBe(0);
});

test("the sentence fits the bar of a 390 wide phone", async ({ page }) => {
  await limitCanvases(page, 100_000);
  await page.setViewportSize({ width: 390, height: 844 });
  await logIn(page, "light");
  await page.goto(`d/${DOMAIN}/e/diagram-png`);
  await openFigure(page, WIDE);
  const png = page.getByRole("button", { name: "Download PNG" });
  await png.click();
  const said = page.getByRole("dialog").getByRole("status");
  await expect(said).toHaveText(TOO_LARGE);
  if (OUT !== undefined) {
    mkdirSync(OUT, { recursive: true });
    await page.screenshot({ path: join(OUT, "phone-sentence.png") });
  }
  const box = await said.boundingBox();
  expect(box?.x ?? -1).toBeGreaterThanOrEqual(0);
  // On a row of its own: squeezed beside the buttons it takes up half the screen.
  expect(box?.width ?? 0).toBeGreaterThan(300);
  expect(box?.height ?? 999).toBeLessThan(100);
  await expect(
    page.getByRole("button", { name: "Close", exact: true }),
  ).toBeInViewport();
  expect(
    await page.evaluate(
      () => document.documentElement.scrollWidth - window.innerWidth,
    ),
  ).toBeLessThanOrEqual(0);
});

test("a picture from another place saves no file with a hole in it", async ({
  page,
}) => {
  // The picture is there on the screen: mermaid draws nothing without it.
  await page.route("https://pictures.example.invalid/**", (route) =>
    route.fulfill({
      contentType: "image/png",
      body: Buffer.from(ONE_PIXEL, "base64"),
    }),
  );
  await logIn(page, "light");
  await page.goto(`d/${DOMAIN}/e/diagram-png`);
  await openFigure(page, WITH_PICTURE);
  await expect(page.getByRole("dialog").locator("image")).toHaveCount(1);
  let saved = 0;
  page.on("download", () => {
    saved += 1;
  });
  await page.getByRole("button", { name: "Download PNG" }).click();
  await expect(page.getByRole("dialog").getByRole("status")).toHaveText(
    OUTSIDE_PICTURE,
  );
  expect(saved).toBe(0);
});

for (const theme of ["light", "dark"] as const) {
  test(`a user journey in ${theme} keeps its section title readable`, async ({
    page,
    browserName,
  }) => {
    await logIn(page, theme);
    await page.goto(`d/${DOMAIN}/e/diagram-gallery`);
    await openFigure(page, USER_JOURNEY);
    const drawing = await measure(page);
    const title = await objectBox(page, 0);
    expect(title.words).toBe("Sign up");
    const bytes = await pressAndSave(page, `${browserName}-${theme}-journey`);
    const scale = pngSize(bytes).width / drawing.width;
    const inked = await inkInside(page, bytes, INK[theme], {
      x: title.x * scale,
      y: title.y * scale,
      width: title.width * scale,
      height: title.height * scale,
    });
    expect(inked, "the title's words are in its box").toBeGreaterThan(50);
  });

  test(`every diagram type in ${theme} gives a sound file`, async ({
    page,
    browserName,
  }) => {
    test.setTimeout(180_000);
    await logIn(page, theme);
    await page.goto(`d/${DOMAIN}/e/diagram-gallery`);
    const figures = page.locator('figure[aria-label="Diagram"]');
    await expect(figures).toHaveCount(17);
    const verdicts: string[] = [];
    for (let index = 0; index < 17; index += 1) {
      await openFigure(page, index);
      const drawing = await measure(page);
      const objects = await page
        .getByRole("dialog")
        .locator("foreignObject")
        .count();
      const started = page.waitForEvent("download", { timeout: 20_000 });
      await page.getByRole("button", { name: "Download PNG" }).click();
      const download = await started.catch(() => null);
      if (download === null) {
        const said = await page
          .getByRole("dialog")
          .getByRole("status")
          .textContent();
        verdicts.push(
          `${String(index)}: no file (${String(objects)} foreign): ${said ?? ""}`,
        );
      } else {
        if (OUT !== undefined) {
          mkdirSync(OUT, { recursive: true });
          await download.saveAs(
            join(OUT, `${browserName}-${theme}-gallery-${String(index)}.png`),
          );
          await page.screenshot({
            path: join(
              OUT,
              `${browserName}-${theme}-gallery-${String(index)}.screen.png`,
            ),
          });
        }
        const bytes = readFileSync(await download.path());
        const size = pngSize(bytes);
        const pixels = await readPixels(page, bytes, INK[theme], NODE[theme]);
        expect(pixels.notOpaque, `figure ${String(index)}`).toBe(0);
        const scale = size.width / drawing.width;
        verdicts.push(
          `${String(index)}: font ${drawing.font.toFixed(2)} stroke ${drawing.stroke.toFixed(2)} foreign ${String(objects)} scale ${scale.toFixed(2)} ${String(size.width)}x${String(size.height)}`,
        );
      }
      await closeFigure(page);
    }
    console.log(`png ${browserName} ${theme} gallery\n${verdicts.join("\n")}`);
    expect(verdicts.filter((line) => line.includes("no file"))).toEqual([]);
  });
}
