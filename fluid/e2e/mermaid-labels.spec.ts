/**
 * What a label draws as, in a browser, through the reading page's real drawing
 * path: the app's mermaid configuration (SVG labels) and `drawnLabels`.
 *
 * The db cannot answer this. On SVG labels mermaid leaves entity forms as text
 * and loses `<` and `>`, so a label can agree with the db and draw as something
 * else. The first test is a label already in somebody's diagram, the second is
 * the whole table `rename` must draw as typed: `fixtures/label-table.json` holds
 * what the op wrote for each text (`opsProperty.test.ts` pins that file against
 * the op), and the drawn rows must be the text. The arrow test does the same for
 * `edgeLabel`, on words long enough for mermaid to wrap inside an entity form.
 */

import { readFileSync } from "node:fs";

import { expect, type Page, test } from "@playwright/test";

const TABLE = JSON.parse(
  readFileSync(new URL("./fixtures/label-table.json", import.meta.url), "utf8"),
) as {
  /** What each box `N<i>` must draw, one `\\n` per row. */
  drawn: string[];
  /** The indices that are markup in a markdown label, which draw as such. */
  markup: number[];
  /** Per holder, the body after `rename` wrote each text. */
  bodies: Record<string, string>;
  /** Labels written by hand, and what the rename field shows for each. */
  handWritten: { label: string; shown: string }[];
  /** Arrows `E<i>` to `F<i>`: each body, and the text each arrow must draw. */
  arrows: { texts: string[]; body: string }[];
};

const USER = process.env.FLUID_E2E_USER ?? "smoke";
const PASSWORD = process.env.FLUID_E2E_PASSWORD ?? "smoke-password";
const DOMAIN = process.env.FLUID_E2E_DOMAIN ?? "fluid-smoke";

async function logIn(page: Page): Promise<void> {
  await page.goto("./");
  await page.getByLabel("Name", { exact: true }).fill(USER);
  await page.getByLabel("Password", { exact: true }).fill(PASSWORD);
  await page.getByRole("button", { name: "Log in" }).click();
  await expect(
    page.getByRole("heading", { name: "Home", level: 1 }),
  ).toBeVisible();
}

/** Each drawn box of the page: its id and its rows of text. */
function drawnBoxes(page: Page): Promise<Record<string, string>[]> {
  return page.evaluate(() =>
    Array.from(
      document.querySelectorAll('figure[aria-label="Diagram"]'),
      (figure) => {
        const out: Record<string, string> = {};
        const svg = Array.from(
          figure.querySelectorAll('svg[id^="mermaid-"]'),
        ).find((s) => s.closest("[data-print]") === null);
        for (const node of svg?.querySelectorAll("g.node") ?? []) {
          const id = /flowchart-(.+)-\d+$/.exec(node.id)?.[1] ?? node.id;
          out[id] = Array.from(
            node.querySelectorAll("tspan.text-outer-tspan"),
            (row) => row.textContent,
          ).join("\n");
        }
        return out;
      },
    ),
  );
}

/** Each drawn arrow label of the page, by its arrow's id: its rows of text. */
function drawnArrowLabels(page: Page): Promise<Record<string, string[]>[]> {
  return page.evaluate(() =>
    Array.from(
      document.querySelectorAll('figure[aria-label="Diagram"]'),
      (figure) => {
        const out: Record<string, string[]> = {};
        const svg = Array.from(
          figure.querySelectorAll('svg[id^="mermaid-"]'),
        ).find((s) => s.closest("[data-print]") === null);
        for (const label of svg?.querySelectorAll(
          "g.edgeLabel g.label[data-id]",
        ) ?? []) {
          out[label.getAttribute("data-id") ?? ""] = Array.from(
            label.querySelectorAll("tspan.text-outer-tspan"),
            (row) => row.textContent,
          );
        }
        return out;
      },
    ),
  );
}

/** Has the engram's one fence hold `bodies` instead, for this page only. */
async function replaceFences(page: Page, bodies: string[]): Promise<void> {
  await page.route(
    `**/api/v1/domains/*/engrams/diagram-labels`,
    async (route) => {
      const response = await route.fetch();
      const json = (await response.json()) as { content: string };
      json.content = json.content.replace(
        /```mermaid[\s\S]*```/,
        // A function: the labels hold `$`, which a replacement string would read.
        () => bodies.map((b) => `\`\`\`mermaid\n${b}\n\`\`\``).join("\n\n"),
      );
      await route.fulfill({ response, json });
    },
  );
}

test("a #quot; label already in a diagram reads right on the reading page", async ({
  page,
}) => {
  await logIn(page);
  await page.goto(`d/${DOMAIN}/e/diagram-labels`);
  await expect
    .poll(() => drawnBoxes(page), { timeout: 30_000 })
    .toEqual([
      {
        A: 'say "hi"',
        B: "\u2665 # sign",
        C: "a < b > c",
        D: "unknown &bogus; stays",
      },
    ]);
  await expect(page.locator("article svg text script")).toHaveCount(0);
});

test("every text in the table draws as typed", async ({ page }) => {
  test.setTimeout(240_000);
  const holders = Object.entries(TABLE.bodies);
  await logIn(page);
  await replaceFences(
    page,
    holders.map(([, b]) => b),
  );
  await page.goto(`d/${DOMAIN}/e/diagram-labels`);
  await expect
    .poll(
      async () =>
        (await drawnBoxes(page)).filter((d) => d.N0 !== undefined).length,
      { timeout: 120_000 },
    )
    .toBe(holders.length);

  const drawn = await drawnBoxes(page);
  const wrong: string[] = [];
  holders.forEach(([holder], h) => {
    TABLE.drawn.forEach((text, i) => {
      const got = drawn[h]?.[`N${String(i)}`];
      if (
        got !== text &&
        !(holder === "markdown" && TABLE.markup.includes(i))
      ) {
        wrong.push(
          `${holder} ${JSON.stringify(text)} drew ${JSON.stringify(got)}`,
        );
      }
    });
  });
  expect(wrong).toEqual([]);
  // Whatever a label says, the drawing holds no markup from it.
  await expect(
    page.locator("article svg script, article svg text img"),
  ).toHaveCount(0);
});

test("an arrow's label draws as typed, also where mermaid wraps inside a form", async ({
  page,
}) => {
  test.setTimeout(120_000);
  await logIn(page);
  await replaceFences(
    page,
    TABLE.arrows.map((a) => a.body),
  );
  await page.goto(`d/${DOMAIN}/e/diagram-labels`);
  await expect
    .poll(
      async () =>
        (await drawnArrowLabels(page)).filter((d) => d.L_E0_F0_0 !== undefined)
          .length,
      { timeout: 60_000 },
    )
    .toBe(TABLE.arrows.length);

  const drawn = await drawnArrowLabels(page);
  const wrong: string[] = [];
  let wrapped = 0;
  TABLE.arrows.forEach(({ texts }, a) => {
    texts.forEach((text, i) => {
      const rows = drawn[a]?.[`L_E${String(i)}_F${String(i)}_0`] ?? [];
      wrapped += rows.length > 1 ? 1 : 0;
      // A wrapped word stays one word: mermaid's rows, with what stood between
      // two words of a row left as written.
      if (rows.join("") !== text && rows.join(" ") !== text) {
        wrong.push(`${JSON.stringify(text)} drew ${JSON.stringify(rows)}`);
      }
    });
  });
  expect(wrong).toEqual([]);
  expect(wrapped).toBeGreaterThan(10);
  await expect(page.locator("article svg text img")).toHaveCount(0);
});

test("the rename field shows what a hand-written label draws", async ({
  page,
}) => {
  const body = [
    "flowchart TD",
    ...TABLE.handWritten.map((row, i) => `  H${String(i)}["${row.label}"]`),
  ].join("\n");
  await logIn(page);
  await replaceFences(page, [body]);
  await page.goto(`d/${DOMAIN}/e/diagram-labels`);
  await expect
    .poll(async () => (await drawnBoxes(page))[0]?.H0 !== undefined, {
      timeout: 30_000,
    })
    .toBe(true);
  const drawn = (await drawnBoxes(page))[0];
  const wrong = TABLE.handWritten.flatMap((row, i) => {
    const got = drawn?.[`H${String(i)}`];
    return got === row.shown
      ? []
      : [`${row.label} shows ${row.shown}, drew ${String(got)}`];
  });
  expect(wrong).toEqual([]);
});
