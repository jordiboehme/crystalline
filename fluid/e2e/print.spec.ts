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
 * which is what Chrome lays out in. Safari may lay the page out at the full
 * sheet width (`A4_COLUMN` above) and scale it down onto the paper; then a
 * page is that width and as tall as the paper's proportion makes it.
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
 * the pair is also asserted to be one inline block, which no browser breaks.
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
      for (const paper of [PAPERS[0]!, LETTER]) {
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
