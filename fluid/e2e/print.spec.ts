/**
 * A page on paper is one full-width column, whatever the window's width.
 *
 * Safari evaluates the width media queries against the window while it
 * prints, so a wide window keeps the side column's grid track even though
 * the column itself is hidden on paper, and the text prints in what is left.
 * Emulating print media at a wide viewport is that situation exactly: the
 * text column must still fill the page. Runs in Chromium and WebKit.
 */

import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { unlinkSync, writeFileSync } from "node:fs";

/** The account and the domains `run-smoke.sh` set up. Kept in step with it. */
const USER = process.env.FLUID_E2E_USER ?? "smoke";
const PASSWORD = process.env.FLUID_E2E_PASSWORD ?? "smoke-password";
const DOMAIN = process.env.FLUID_E2E_DOMAIN ?? "fluid-smoke";
const PAGES = process.env.FLUID_E2E_REFLECTION_DOMAIN ?? "smoke-pages";

const SCREENS = [
  { name: "an engram", path: `d/${DOMAIN}/e/berth-assignment-flow` },
  {
    name: "a Reflection page",
    path: `d/${PAGES}/reflection/guides/welcome.md`,
  },
  // PACT.md draws through the same Reflection page; the fixture domain has
  // none until a journey writes it.
];

async function signIn(page: Page): Promise<void> {
  await page.goto("./");
  await page.getByLabel("Name", { exact: true }).fill(USER);
  await page.getByLabel("Password", { exact: true }).fill(PASSWORD);
  await page.getByRole("button", { name: "Log in" }).click();
  await expect(
    page.getByRole("heading", { name: "Home", level: 1 }),
  ).toBeVisible();
}

for (const width of [1440, 390]) {
  test(`prints every page type as one full-width column from a ${String(width)} px window`, async ({
    page,
  }) => {
    await page.setViewportSize({ width, height: 900 });
    await signIn(page);
    for (const screen of SCREENS) {
      await page.emulateMedia({ media: "screen" });
      await page.goto(screen.path);
      const article = page.locator("main article").first();
      await expect(
        article,
        `${screen.name} at ${String(width)} px`,
      ).toBeVisible();
      await page.emulateMedia({ media: "print" });
      const share = await article.evaluate((element) => {
        const main = element.closest("main");
        return main === null
          ? 0
          : element.getBoundingClientRect().width / main.clientWidth;
      });
      expect(share, `${screen.name} at ${String(width)} px`).toBeGreaterThan(
        0.95,
      );
    }
  });
}

/** A4 paper less the page margins, in CSS pixels: what Safari lays out in. */
const A4_COLUMN = 794;

test("on A4 paper nothing is wider than the column and a heading keeps its next block", async ({
  page,
}) => {
  await page.setViewportSize({ width: A4_COLUMN, height: 1123 });
  await signIn(page);
  for (const screen of SCREENS) {
    await page.emulateMedia({ media: "screen" });
    await page.goto(screen.path);
    const article = page.locator("main article").first();
    await expect(article, screen.name).toBeVisible();
    await page.waitForLoadState("networkidle");
    await page.emulateMedia({ media: "print" });
    const wider = await article.evaluate((root) => {
      const box = root.getBoundingClientRect();
      return Array.from(root.querySelectorAll("*"))
        .filter((element) => {
          const rect = element.getBoundingClientRect();
          return (
            rect.width > 0 &&
            (rect.right > box.right + 1 || rect.left < box.left - 1)
          );
        })
        .map((element) => element.tagName.toLowerCase());
    });
    expect(wider, `${screen.name}: wider than the column`).toEqual([]);
    const kept = await article.evaluate((root) =>
      Array.from(root.querySelectorAll(".measured > [data-keep]")).map(
        (wrapper) => {
          const style = getComputedStyle(wrapper);
          return {
            first: wrapper.firstElementChild?.tagName.toLowerCase() ?? "",
            display: style.display,
            breakInside: style.breakInside,
            pageBreakInside: style.pageBreakInside,
          };
        },
      ),
    );
    // A heading prints at its own height, its words at its top: the flex
    // heading of a sourced Reflection section included.
    const headings = await article.evaluate((root) =>
      Array.from(root.querySelectorAll(".measured h2")).map((heading) => {
        const box = heading.getBoundingClientRect();
        const label = heading.getAttribute("aria-labelledby");
        const words =
          (label === null ? null : document.getElementById(label)) ??
          heading.firstElementChild ??
          heading;
        const line = parseFloat(getComputedStyle(heading).lineHeight);
        return {
          drop: words.getBoundingClientRect().top - box.top,
          height: box.height,
          line: Number.isNaN(line) ? 40 : line,
        };
      }),
    );
    for (const heading of headings) {
      expect(heading.drop, `${screen.name}: heading words drop`).toBeLessThan(
        6,
      );
      expect(heading.height).toBeLessThan(heading.line * 2);
    }
    // The engram's only heading is its title, which the page folds away;
    // the Reflection page's "Night deploys" keeps its paragraph.
    if (screen.name === "a Reflection page") {
      expect(kept.length, `${screen.name}: kept headings`).toBeGreaterThan(0);
    }
    for (const wrapper of kept) {
      expect(wrapper.first).toMatch(/^h[1-6]$/);
      expect(wrapper.display).toBe("block");
      expect(wrapper.breakInside).toBe("avoid");
      expect(wrapper.pageBreakInside).toBe("avoid");
    }
  }
});

/*
 * Diagrams on A4 paper, in both browsers and both themes (a dark page prints
 * its light copy). A4 is 210 x 297 mm and the print sheet's `@page` margins
 * are 17 mm at the sides and 20 mm at the top and bottom, so a page holds a
 * column 176 mm wide and 257 mm tall: 665 x 971 CSS pixels at 96 per inch,
 * which is what Chrome lays out in. A page laid out at the full sheet width
 * (`A4_COLUMN` above) and scaled down onto the paper is that width and as
 * tall as the paper's proportion makes it. WebKit's own print path lays a
 * page out narrower, at about 1.25 CSS pixels to the point (`WEBKIT_PAPERS`
 * below), so the same column holds less height there.
 *
 * Emulated print media lays the page out as one long strip, with no pages to
 * break across, and WebKit has no `page.pdf`. So the pages are made with the
 * engine's own fragmentation: the body becomes a multi-column box of one
 * page's size, each column one page. The browser breaks a column with the
 * same rules it breaks a page with (`break-inside: avoid` on a figure and on
 * a kept heading), so a figure cut at a page edge is a figure with rects in
 * two columns, and a heading left at the bottom of a page is a heading in
 * another column than its diagram. The body and not `main`: the frame around
 * `main` is part of what WebKit breaks badly (`index.css`).
 *
 * One thing this does not show: WebKit printing for real (Safari, or a web
 * view's print operation) still left a kept heading at the foot of a page
 * and moved only its diagram, where its columns kept the two together. So
 * the pair is also asserted to be one inline block, which a browser moves
 * whole to the next page when it fits on one.
 */
const PAPERS = [
  { name: "A4 paper", width: 665, height: 971 },
  {
    name: "A4 paper laid out at the sheet's width",
    width: A4_COLUMN,
    height: Math.round((A4_COLUMN * 257) / 176),
  },
];

/**
 * US Letter with the same margins: 181.9 x 239.4 mm, the shorter page the
 * print sheet sizes a tall diagram for.
 */
const LETTER = { name: "Letter paper", width: 687, height: 905 };

/**
 * A4 and Letter as WebKit's own print path on a Mac lays them out, measured
 * with a web view's print operation: a line of A4 wraps at 620 CSS pixels
 * and of Letter at 640, and a page holds 900 and 840 of height. A tall
 * diagram or picture with its heading has to fit these, the shortest pages.
 */
const WEBKIT_PAPERS = [
  { name: "A4 paper as WebKit prints it", width: 620, height: 900 },
  { name: "Letter paper as WebKit prints it", width: 640, height: 840 },
];

/** The space between two pages laid side by side, so they never touch. */
const PAGE_GAP = 100;

/** Waits until every diagram, and on a dark page every paper copy, is drawn. */
async function diagramsDrawn(page: Page, dark: boolean): Promise<number> {
  const figures = page.locator('figure[aria-label="Diagram"]');
  await expect(figures.first()).toBeVisible({ timeout: 30_000 });
  const count = await figures.count();
  // The paper copies come one at a time after the screen drawings.
  await expect
    .poll(
      () =>
        page.evaluate(
          (paper) =>
            Array.from(
              document.querySelectorAll('figure[aria-label="Diagram"]'),
            ).filter(
              (figure) =>
                figure.querySelector(
                  paper
                    ? '[data-print] svg[id^="mermaid-"]'
                    : 'svg[id^="mermaid-"]',
                ) !== null,
            ).length,
          dark,
        ),
      { timeout: 60_000 },
    )
    .toBe(count);
  return count;
}

interface Printed {
  name: string;
  /** The printed drawing's width, its viewBox width and the column's. */
  drawn: number;
  natural: number;
  column: number;
  /** The figure's height, and its heading's and its own together. */
  figure: number;
  unit: number;
  /** Whether the heading and the figure share one unbreakable wrapper. */
  kept: boolean;
  /** How that wrapper is laid out on paper. */
  keptAs: string;
  /** The pages the figure starts and ends on, and its heading's. */
  first: number;
  last: number;
  heading: number;
}

/**
 * Where each diagram prints: its size on the long strip under print media,
 * then its page once the body is cut into pages. Leaves the page cut up.
 */
async function printed(
  page: Page,
  paper: { width: number; height: number },
): Promise<Printed[]> {
  await page.emulateMedia({ media: "print" });
  return page.evaluate(
    ({ width, height, gap }) => {
      const figures = Array.from(
        document.querySelectorAll('figure[aria-label="Diagram"]'),
      );
      const headingOf = (figure: Element) =>
        figure.closest("[data-keep]")?.querySelector("h1, h2, h3, h4") ??
        figure.previousElementSibling;
      const sizes = figures.map((figure) => {
        const column =
          figure.closest(".measured")?.getBoundingClientRect().width ?? 0;
        const svg = Array.from(
          figure.querySelectorAll<SVGSVGElement>('svg[id^="mermaid-"]'),
        ).find((candidate) => candidate.getBoundingClientRect().width > 0);
        const box = figure.getBoundingClientRect();
        const top = Math.min(
          box.top,
          headingOf(figure)?.getBoundingClientRect().top ?? box.top,
        );
        return {
          name: headingOf(figure)?.textContent?.trim() ?? "",
          drawn: svg?.getBoundingClientRect().width ?? 0,
          natural: svg?.viewBox.baseVal.width ?? 0,
          column,
          figure: box.height,
          // The kept pair's own box, its heading's top margin included.
          unit:
            figure.closest("[data-keep]")?.getBoundingClientRect().height ??
            box.bottom - top,
          kept: figure.closest("[data-keep]") !== null,
          keptAs: getComputedStyle(figure.closest("[data-keep]") ?? figure)
            .display,
        };
      });
      const body = document.body;
      Object.assign(body.style, {
        columnWidth: `${String(width)}px`,
        columnGap: `${String(gap)}px`,
        columnFill: "auto",
        width: `${String(width)}px`,
        height: `${String(height)}px`,
        maxWidth: "none",
        overflow: "visible",
        margin: "0",
      });
      const left = body.getBoundingClientRect().left;
      const pageOf = (x: number) => Math.floor((x - left + 1) / (width + gap));
      const pagesOf = (element: Element) =>
        Array.from(element.getClientRects())
          .filter((rect) => rect.height > 0)
          .flatMap((rect) => [pageOf(rect.left), pageOf(rect.right - 1)]);
      return figures.map((figure, index) => {
        const own = pagesOf(figure);
        const heading = headingOf(figure);
        return {
          ...sizes[index]!,
          first: Math.min(...own),
          last: Math.max(...own),
          heading: heading === null ? -1 : Math.max(...pagesOf(heading)),
        };
      });
    },
    { width: paper.width, height: paper.height, gap: PAGE_GAP },
  );
}

/**
 * Every diagram at most its own width and the column's, no taller than a
 * page with its heading, whole on one page and on the page of its heading.
 * Soft: one run names every diagram that prints wrong, not the first.
 */
function expectPrintedWell(
  diagrams: Printed[],
  paper: { name: string; height: number },
): void {
  for (const d of diagrams) {
    const at = `${paper.name}, ${d.name}`;
    expect.soft(d.drawn, `${at}: drawn`).toBeGreaterThan(0);
    expect
      .soft(d.drawn, `${at}: wider than its own width`)
      .toBeLessThan(d.natural + 1);
    expect
      .soft(d.drawn, `${at}: wider than the column`)
      .toBeLessThan(d.column + 1);
    expect
      .soft(d.figure, `${at}: taller than a page`)
      .toBeLessThan(paper.height + 1);
    expect
      .soft(d.unit, `${at}: taller than a page with its heading`)
      .toBeLessThan(paper.height + 1);
    // Where the page edges fall depends on the paper and the browser; the
    // one unbreakable wrapper around a heading and its diagram is what keeps
    // them together anywhere.
    expect.soft(d.kept, `${at}: not kept with its heading`).toBe(true);
    expect
      .soft(d.keptAs, `${at}: kept in a box that breaks`)
      .toBe("inline-block");
    expect.soft(d.last, `${at}: cut across pages`).toBe(d.first);
    expect.soft(d.heading, `${at}: heading on another page`).toBe(d.first);
  }
}

for (const theme of ["light", "dark"] as const) {
  for (const paper of PAPERS) {
    test(`on ${paper.name}, a ${theme} gallery prints each diagram at its own size, whole and with its heading`, async ({
      page,
    }) => {
      test.setTimeout(120_000);
      await page.addInitScript((value) => {
        window.localStorage.setItem("fluid-theme", value);
      }, theme);
      await page.setViewportSize(paper);
      await signIn(page);
      await page.emulateMedia({ media: "screen" });
      await page.goto(`d/${DOMAIN}/e/diagram-gallery`);
      const count = await diagramsDrawn(page, theme === "dark");
      const diagrams = await printed(page, paper);
      expect(diagrams).toHaveLength(count);
      expectPrintedWell(diagrams, paper);
    });
  }
}

/** A top-down chain of boxes, far taller than a page at its own size. */
const TALL = [
  "flowchart TD",
  ...Array.from(
    { length: 30 },
    (_, i) => `    s${String(i)}[Step ${String(i)}] --> s${String(i + 1)}`,
  ),
].join("\n");

for (const theme of ["light", "dark"] as const) {
  test(`a ${theme} Reflection page prints a small diagram at its own size and a tall one, widened, on one page`, async ({
    page,
  }) => {
    // `run-smoke.sh` exports where the daemon's copy of the fixture lives.
    const root = process.env.FLUID_E2E_REFLECTION_ROOT;
    test.skip(!root, "run through run-smoke.sh, which names the domain root");
    test.setTimeout(120_000);
    const file = `${root ?? ""}/reflections/print-diagrams-${theme}.md`;
    writeFileSync(
      file,
      [
        "---",
        "title: Printed diagrams",
        "---",
        "",
        "# Printed diagrams",
        "",
        "A page with a small diagram and a tall one.",
        "",
        "## Model",
        "",
        "```mermaid",
        "erDiagram",
        "    CUSTOMER ||--o{ ORDER : places",
        "```",
        "",
        "## Steps",
        "",
        "```mermaid",
        TALL,
        "```",
        "",
        "After the diagrams.",
        "",
      ].join("\n"),
    );
    try {
      await page.addInitScript((value) => {
        window.localStorage.setItem("fluid-theme", value);
      }, theme);
      await page.setViewportSize(PAPERS[0]!);
      await signIn(page);
      await page.emulateMedia({ media: "screen" });
      await page.goto(`d/${PAGES}/reflection/print-diagrams-${theme}.md`);
      expect(await diagramsDrawn(page, theme === "dark")).toBe(2);
      // Widened on screen, the tall one keeps its own width as a floor
      // there; paper still scales it down to one page.
      // From the keyboard: the toolbar takes the pointer only on hover.
      await page
        .getByRole("button", { name: "Show at full width" })
        .last()
        .focus();
      await page.keyboard.press("Enter");
      await expect(
        page.getByRole("button", { name: "Show at reading width" }),
      ).toHaveCount(1);
      for (const paper of [PAPERS[0]!, LETTER, ...WEBKIT_PAPERS]) {
        await page.setViewportSize(paper);
        await page.emulateMedia({ media: "screen" });
        await page.evaluate(() => document.body.removeAttribute("style"));
        const diagrams = await printed(page, paper);
        expect(diagrams.map((d) => d.name)).toEqual(["Model", "Steps"]);
        expectPrintedWell(diagrams, paper);
        // The tall one is scaled down to fit, the small one is not.
        expect(diagrams[0]!.drawn).toBeGreaterThan(diagrams[0]!.natural - 1);
        expect(diagrams[1]!.drawn).toBeLessThan(diagrams[1]!.natural - 1);
      }
    } finally {
      unlinkSync(file);
    }
  });
}

/*
 * Headings and pictures on paper. A page of headings, each followed by a
 * picture (small, wide, tall, in a link, and an attached one set to the full
 * width), a paragraph, a long list, a table, code, a quote, a long paragraph,
 * a task list, a nested list and a footnote, and a picture with no heading.
 *
 * Where a heading falls depends on everything above it, so one layout would
 * show a heading at the foot of a page only by chance. Each heading is
 * therefore walked down one page: a spacer before its block grows in steps
 * smaller than a line, and at every step the body is cut into pages the way
 * the diagram tests cut it. At every step each heading must start on the page
 * of the first line of what follows it, and each picture must be whole.
 *
 * Chromium honours `break-after: avoid` on a heading in columns, so a heading
 * left at the foot of a page shows in WebKit; Safari printing for real is
 * checked by hand with the same page (`keepWithNext`).
 */

/** A picture as an SVG file of `width` x `height` CSS pixels. */
function svgPicture(width: number, height: number, label: string): string {
  const w = String(width);
  const h = String(height);
  return [
    `<svg xmlns="http://www.w3.org/2000/svg" width="${w}" height="${h}" viewBox="0 0 ${w} ${h}">`,
    `<rect width="100%" height="100%" fill="#dbeafe" stroke="#1e3a8a" stroke-width="6"/>`,
    `<line x1="0" y1="0" x2="${w}" y2="${h}" stroke="#1e3a8a" stroke-width="3"/>`,
    `<line x1="${w}" y1="0" x2="0" y2="${h}" stroke="#1e3a8a" stroke-width="3"/>`,
    `<text x="10" y="22" font-size="16" font-family="sans-serif" fill="#1e3a8a">${label} ${w}x${h}</text>`,
    `</svg>`,
  ].join("");
}

/** The pictures the page shows from outside the app, by name. */
const PICTURES: Record<string, { width: number; height: number }> = {
  small: { width: 160, height: 100 },
  wide: { width: 1800, height: 450 },
  tall: { width: 500, height: 2200 },
  linked: { width: 300, height: 180 },
  lone: { width: 700, height: 1400 },
};

const PICTURE_HOST = "https://pictures.example";

/** `count` sentences of prose. */
function prose(count: number): string {
  return Array.from(
    { length: count },
    (_, i) =>
      `Sentence ${String(i + 1)} of the harbor log wraps across the column like real prose does.`,
  ).join(" ");
}

/** The page, with `attached` the path of a small attached picture. */
function blocksPage(attached: string): string {
  return [
    "# Printed blocks",
    "",
    prose(6),
    "",
    "## Small picture",
    "",
    `![small](${PICTURE_HOST}/small.svg)`,
    "",
    prose(3),
    "",
    "## Wide picture",
    "",
    `![wide](${PICTURE_HOST}/wide.svg)`,
    "",
    "## Tall picture",
    "",
    `![tall](${PICTURE_HOST}/tall.svg)`,
    "",
    "## Linked picture",
    "",
    `[![linked](${PICTURE_HOST}/linked.svg)](https://example.com/harbor)`,
    "",
    "## Attached picture",
    "",
    `![attached](${attached}#full)`,
    "",
    "## Paragraph",
    "",
    prose(5),
    "",
    "## Long list",
    "",
    ...Array.from(
      { length: 40 },
      (_, i) =>
        `- Item ${String(i + 1)} of the long list, with enough words to read like a line.`,
    ),
    "",
    "## Table",
    "",
    "| Berth | Vessel | Notes |",
    "| --- | --- | --- |",
    ...Array.from(
      { length: 8 },
      (_, i) =>
        `| B${String(i + 1)} | Ship ${String(i + 1)} | Arrives at dawn and leaves at dusk |`,
    ),
    "",
    "## Code",
    "",
    "```ts",
    ...Array.from(
      { length: 14 },
      (_, i) =>
        `const line${String(i)} = assign(berth${String(i)}, vessel${String(i)});`,
    ),
    "```",
    "",
    "## Quote",
    "",
    `> ${prose(3)}`,
    "",
    "## Long paragraph",
    "",
    prose(60),
    "",
    "## Tasks",
    "",
    "- [x] Moor the ship",
    "- [ ] Check the lines",
    "- [ ] Log the tide",
    "",
    "## Nested list",
    "",
    "- Harbor",
    "  - North pier",
    "  - South pier",
    "    - Berth 7",
    "- Channel",
    "",
    "## Footnote",
    "",
    "A sentence with a note.[^1]",
    "",
    "A picture of its own, with no heading before it:",
    "",
    `![lone](${PICTURE_HOST}/lone.svg)`,
    "",
    prose(4),
    "",
    "[^1]: The note under the page.",
    "",
  ].join("\n");
}

/** The session's CSRF token, which a write through `page.request` echoes. */
async function csrf(page: Page): Promise<string> {
  const me = await page.request.get("api/v1/auth/me");
  return ((await me.json()) as { csrf: string }).csrf;
}

interface PictureOnPaper {
  name: string;
  natural: { width: number; height: number };
  drawn: { width: number; height: number };
  column: number;
}

interface BlocksOnPaper {
  pictures: PictureOnPaper[];
  /** Each kept wrapper's height, by its heading. */
  units: { name: string; height: number }[];
  /** Every heading left on another page than what follows it, and where. */
  apart: string[];
  /** Every picture that crosses a page edge, and where. */
  cut: string[];
}

/**
 * The page's pictures and kept units on the long strip under print media,
 * then each heading walked down one page in `step` pixels with the body cut
 * into pages at every step. Leaves the page cut up.
 */
async function blocksOnPaper(
  page: Page,
  paper: { width: number; height: number },
  step: number,
): Promise<BlocksOnPaper> {
  await page.emulateMedia({ media: "print" });
  return page.evaluate(
    ({ width, height, gap, step }) => {
      const measured = document.querySelector(".measured")!;
      const nameOf = (element: Element) =>
        element.textContent?.trim().replace(/\s+/g, " ").slice(0, 40) ?? "";
      const pictures = Array.from(
        measured.querySelectorAll<HTMLImageElement>("img"),
      ).map((image) => {
        const box = image.getBoundingClientRect();
        return {
          name: image.alt,
          natural: { width: image.naturalWidth, height: image.naturalHeight },
          drawn: { width: box.width, height: box.height },
          column: measured.getBoundingClientRect().width,
        };
      });
      const units = Array.from(
        measured.querySelectorAll(":scope > [data-keep]"),
      ).map((unit) => ({
        name: nameOf(unit.querySelector("h1, h2, h3, h4, h5, h6") ?? unit),
        height: unit.getBoundingClientRect().height,
      }));
      const headings = Array.from(
        measured.querySelectorAll("h1, h2, h3, h4, h5, h6"),
      ).filter((heading) => heading.getBoundingClientRect().height > 0);
      // What follows a heading: the next block in its wrapper, or after it.
      const follower = (heading: Element): Element | null => {
        let at: Element | null = heading;
        while (at !== null && at !== measured) {
          if (at.nextElementSibling !== null) {
            return at.nextElementSibling;
          }
          at = at.parentElement;
        }
        return null;
      };
      // The first line of a block: its first character, or its first
      // picture when that comes first.
      const firstLine = (block: Element): DOMRect | null => {
        const walker = document.createTreeWalker(
          block,
          NodeFilter.SHOW_TEXT | NodeFilter.SHOW_ELEMENT,
        );
        for (let node = walker.nextNode(); node; node = walker.nextNode()) {
          if (node instanceof HTMLImageElement) {
            return node.getBoundingClientRect();
          }
          const text = node.nodeType === Node.TEXT_NODE ? node.nodeValue : "";
          const at = text?.search(/\S/) ?? -1;
          if (at >= 0) {
            const range = document.createRange();
            range.setStart(node, at);
            range.setEnd(node, at + 1);
            const rect = Array.from(range.getClientRects()).find(
              (r) => r.height > 0,
            );
            if (rect !== undefined) {
              return rect;
            }
          }
        }
        return null;
      };
      const body = document.body;
      Object.assign(body.style, {
        columnWidth: `${String(width)}px`,
        columnGap: `${String(gap)}px`,
        columnFill: "auto",
        width: `${String(width)}px`,
        height: `${String(height)}px`,
        maxWidth: "none",
        overflow: "visible",
        margin: "0",
      });
      const spacer = document.createElement("div");
      const apart = new Map<string, string>();
      const cut = new Map<string, string>();
      for (const moved of headings) {
        let block: Element = moved;
        while (block.parentElement !== measured) {
          block = block.parentElement!;
        }
        measured.insertBefore(spacer, block);
        for (let offset = 0; offset < height; offset += step) {
          spacer.style.height = `${String(offset)}px`;
          const origin = body.getBoundingClientRect();
          const pageOf = (rect: DOMRect) =>
            Math.floor((rect.left - origin.left + 1) / (width + gap));
          const at = `${nameOf(moved)} moved ${String(offset)}px`;
          for (const heading of headings) {
            const next = follower(heading);
            const line = next === null ? null : firstLine(next);
            const own = heading.getBoundingClientRect();
            if (line !== null && pageOf(line) !== pageOf(own)) {
              const key = nameOf(heading);
              if (!apart.has(key)) {
                apart.set(key, `${key} (${at})`);
              }
            }
          }
          for (const image of Array.from(measured.querySelectorAll("img"))) {
            const box = image.getBoundingClientRect();
            if (box.bottom - origin.top > height + 1 || box.top < origin.top) {
              const key = image.alt;
              if (!cut.has(key)) {
                cut.set(key, `${key} (${at})`);
              }
            }
          }
        }
        spacer.remove();
      }
      return {
        pictures,
        units,
        apart: Array.from(apart.values()),
        cut: Array.from(cut.values()),
      };
    },
    { width: paper.width, height: paper.height, gap: PAGE_GAP, step },
  );
}

for (const theme of ["light", "dark"] as const) {
  for (const paper of [...PAPERS, LETTER, ...WEBKIT_PAPERS]) {
    test(`on ${paper.name}, a ${theme} page keeps each heading with what follows and prints each picture whole at its own size`, async ({
      page,
    }) => {
      test.setTimeout(240_000);
      await page.addInitScript((value) => {
        window.localStorage.setItem("fluid-theme", value);
      }, theme);
      await page.route(`${PICTURE_HOST}/**`, (route) => {
        const name = new URL(route.request().url()).pathname
          .slice(1)
          .replace(/\.svg$/, "");
        const size = PICTURES[name] ?? { width: 10, height: 10 };
        return route.fulfill({
          contentType: "image/svg+xml",
          body: svgPicture(size.width, size.height, name),
        });
      });
      await page.setViewportSize(paper);
      await signIn(page);
      const token = await csrf(page);
      const stamp = `${String(Date.now())}-${theme}`;
      const attached = `assets/e2e/print-${stamp}.svg`;
      const files = `api/v1/domains/${DOMAIN}/files/${attached}`;
      const put = await page.request.put(files, {
        headers: { "X-CSRF-Token": token, "Content-Type": "image/svg+xml" },
        data: svgPicture(160, 100, "attached"),
      });
      expect(put.ok(), await put.text()).toBeTruthy();
      const made = await page.request.post(`api/v1/domains/${DOMAIN}/engrams`, {
        headers: { "X-CSRF-Token": token },
        data: {
          title: `Printed blocks ${stamp}`,
          content: blocksPage(attached),
          tags: ["print"],
        },
      });
      expect(made.ok(), await made.text()).toBeTruthy();
      const { permalink } = (await made.json()) as { permalink: string };
      const engram = `api/v1/domains/${DOMAIN}/engrams/${permalink}`;
      try {
        await page.emulateMedia({ media: "screen" });
        await page.goto(`d/${DOMAIN}/e/${permalink}`);
        const images = page.locator(".measured img");
        await expect(images).toHaveCount(6);
        // The pictures load lazily: each is brought into view once.
        for (const image of await images.all()) {
          await image.scrollIntoViewIfNeeded();
        }
        await expect
          .poll(() =>
            images.evaluateAll((all) =>
              all.every(
                (image) =>
                  (image as HTMLImageElement).complete &&
                  (image as HTMLImageElement).naturalWidth > 0,
              ),
            ),
          )
          .toBe(true);
        await page.evaluate(() => window.scrollTo(0, 0));
        const printed = await blocksOnPaper(page, paper, 10);
        expect(printed.pictures).toHaveLength(6);
        for (const p of printed.pictures) {
          const at = `${paper.name}, picture ${p.name}`;
          expect.soft(p.drawn.width, `${at}: drawn`).toBeGreaterThan(0);
          expect
            .soft(p.drawn.width, `${at}: wider than its own width`)
            .toBeLessThan(p.natural.width + 1);
          expect
            .soft(p.drawn.width, `${at}: wider than the column`)
            .toBeLessThan(p.column + 1);
          expect
            .soft(p.drawn.height, `${at}: taller than a page`)
            .toBeLessThan(paper.height + 1);
          // Its box keeps the picture's own proportion: not squeezed, and
          // not a page-tall box with a small drawing inside.
          expect
            .soft(
              Math.abs(
                p.drawn.width / p.drawn.height -
                  p.natural.width / p.natural.height,
              ) /
                (p.natural.width / p.natural.height),
              `${at}: out of proportion`,
            )
            .toBeLessThan(0.01);
        }
        for (const unit of printed.units) {
          expect
            .soft(
              unit.height,
              `${paper.name}, ${unit.name}: kept unit taller than a page`,
            )
            .toBeLessThan(paper.height + 1);
        }
        expect
          .soft(printed.apart, `${paper.name}: heading apart from what follows`)
          .toEqual([]);
        expect
          .soft(printed.cut, `${paper.name}: picture cut at a page edge`)
          .toEqual([]);
      } finally {
        const etag = (await page.request.get(engram)).headers()["etag"] ?? "";
        await page.request.delete(engram, {
          headers: { "X-CSRF-Token": token, "If-Match": etag },
        });
        await page.request.delete(files, {
          headers: { "X-CSRF-Token": token },
        });
      }
    });
  }
}
