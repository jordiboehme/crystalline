/**
 * The text style menu of the floating "Quick format" bar, in a real browser
 * against a real daemon.
 *
 * What jsdom cannot answer here: the bar's mousedown guard against a real
 * press (Safari does not focus a button on click, Chromium does), the focus
 * moving from the editor into the menu and from the menu into the Reflection
 * page's question without the bar reading it as the focus leaving, and the
 * selection the browser draws once a style is picked. It runs in Chromium and
 * in WebKit (see playwright.config.ts).
 *
 * Every journey makes its own engram or page through the REST API and reads
 * the file back through it. Shares the daemon, the fixture domains and the
 * accounts `run-smoke.sh` sets up; run it the same way
 * (`bash fluid/e2e/run-smoke.sh e2e/quick-format.spec.ts`).
 */

import type { Locator, Page } from "@playwright/test";
import { expect, test } from "@playwright/test";

const USER = process.env.FLUID_E2E_USER ?? "smoke";
const PASSWORD = process.env.FLUID_E2E_PASSWORD ?? "smoke-password";
const DOMAIN = process.env.FLUID_E2E_DOMAIN ?? "fluid-smoke";
const REFLECTION_DOMAIN =
  process.env.FLUID_E2E_REFLECTION_DOMAIN ?? "smoke-pages";
const BASE_PATH = process.env.FLUID_E2E_BASE_PATH ?? "";

const LINE = "The tide turns at noon.";
const BODY = `Harbour notes\n\n${LINE}\n`;

async function signIn(page: Page): Promise<void> {
  await page.goto("./");
  await expect(page).toHaveURL((url) => url.pathname === `${BASE_PATH}/login`);
  await page.getByLabel("Name", { exact: true }).fill(USER);
  await page.getByLabel("Password", { exact: true }).fill(PASSWORD);
  await page.getByRole("button", { name: "Log in" }).click();
  await expect(
    page.getByRole("heading", { name: "Home", level: 1 }),
  ).toBeVisible();
}

/** The session's CSRF token, which a write through `page.request` echoes. */
async function csrf(page: Page): Promise<string> {
  const me = await page.request.get(`${BASE_PATH}/api/v1/auth/me`);
  return ((await me.json()) as { csrf: string }).csrf;
}

let unique = 0;
function titled(base: string): string {
  unique += 1;
  return `${base} ${String(Date.now())}${String(unique)}`;
}

/** Makes an engram holding `BODY`, opens its editor and returns both. */
async function editNew(
  page: Page,
): Promise<{ permalink: string; source: Locator }> {
  const response = await page.request.post(
    `${BASE_PATH}/api/v1/domains/${DOMAIN}/engrams`,
    {
      headers: { "X-CSRF-Token": await csrf(page) },
      data: { title: titled("Quick Style"), content: BODY },
    },
  );
  expect(response.ok(), await response.text()).toBeTruthy();
  const { permalink } = (await response.json()) as { permalink: string };
  await page.goto(`d/${DOMAIN}/e/${permalink}`);
  await page.getByRole("link", { name: "Edit" }).click();
  const source = page.getByRole("textbox", { name: "Engram source" });
  await expect(source).toContainText(LINE);
  return { permalink, source };
}

/** The file as the daemon holds it, once the solo surface is saved. */
async function fileHas(
  page: Page,
  permalink: string,
  text: string,
): Promise<void> {
  await expect
    .poll(
      async () => {
        const save = page.getByRole("button", { name: "Save", exact: true });
        if ((await save.count()) > 0 && (await save.isEnabled())) {
          await save.click();
        }
        const response = await page.request.get(
          `${BASE_PATH}/api/v1/domains/${DOMAIN}/engrams/${permalink}`,
        );
        return ((await response.json()) as { content: string }).content;
      },
      { timeout: 20_000 },
    )
    .toContain(text);
}

/** Selects one drawn line from its end back to its start. */
async function selectLine(source: Locator, text: string): Promise<void> {
  const line = source
    .locator(":scope > .cm-line")
    .filter({ hasText: text })
    .first();
  await line.click();
  await line.page().keyboard.press("End");
  await line.page().keyboard.press("Shift+Home");
}

/** What the browser has selected now. */
async function selection(page: Page): Promise<string> {
  return page.evaluate(() => window.getSelection()?.toString() ?? "");
}

test.beforeEach(async ({ page }) => {
  await signIn(page);
});

test("a press opens the text style menu with the bar up, and Heading 1 heads the line", async ({
  page,
}) => {
  const { permalink, source } = await editNew(page);
  await selectLine(source, LINE);
  const bar = page.getByRole("toolbar", { name: "Quick format" });
  await expect(bar).toBeVisible();

  await bar.getByRole("button", { name: "Text style" }).click();
  const menu = page.getByRole("menu");
  await expect(menu).toBeVisible();
  // The press took neither the bar nor the selection away.
  await expect(bar).toBeAttached();
  expect(await selection(page)).toContain(LINE);
  await expect(
    menu.getByRole("menuitemradio", { name: "Body" }),
  ).toHaveAttribute("aria-checked", "true");

  await menu.getByRole("menuitemradio", { name: "Heading 1" }).click();
  await expect(menu).toHaveCount(0);
  await expect(source).toBeFocused();
  expect(await selection(page)).toContain(LINE);
  await fileHas(page, permalink, `## ${LINE}`);

  // The line is a heading now, and the menu says so.
  await expect(bar).toBeVisible();
  await bar.getByRole("button", { name: "Text style" }).click();
  await expect(
    menu.getByRole("menuitemradio", { name: "Heading 1" }),
  ).toHaveAttribute("aria-checked", "true");
  await page.keyboard.press("Escape");
  await expect(menu).toHaveCount(0);
  await expect(source).toBeFocused();

  // One undo takes the heading off again.
  await page.keyboard.press("ControlOrMeta+z");
  await fileHas(page, permalink, `\n${LINE}`);
});

test("keyboard only: Alt+F10, Right to Aa, Enter, Down, Enter picks Heading 1", async ({
  page,
}) => {
  const { permalink, source } = await editNew(page);
  await selectLine(source, LINE);
  const bar = page.getByRole("toolbar", { name: "Quick format" });
  await expect(bar).toBeVisible();

  // Into the bar on Bold, then right along it to "Aa", after Inline code.
  await page.keyboard.press("Alt+F10");
  await expect(bar.getByRole("button", { name: "Bold" })).toBeFocused();
  for (let i = 0; i < 4; i += 1) {
    await page.keyboard.press("ArrowRight");
  }
  const trigger = bar.getByRole("button", { name: "Text style" });
  await expect(trigger).toBeFocused();
  await page.keyboard.press("Enter");
  const menu = page.getByRole("menu");
  await expect(
    menu.getByRole("menuitemradio", { name: "Title" }),
  ).toBeFocused();
  await expect(bar).toBeAttached();
  await page.keyboard.press("ArrowDown");
  await expect(
    menu.getByRole("menuitemradio", { name: "Heading 1" }),
  ).toBeFocused();
  await page.keyboard.press("Enter");

  await expect(menu).toHaveCount(0);
  await expect(source).toBeFocused();
  expect(await selection(page)).toContain(LINE);
  await fileHas(page, permalink, `## ${LINE}`);
});

test("on a Reflection page a section heading asks first, and the bar waits behind the question", async ({
  page,
}) => {
  const path = `quick/style-${String(Date.now())}${String(unique++)}.md`;
  const api = `${BASE_PATH}/api/v1/domains/${REFLECTION_DOMAIN}/reflections/${path}`;
  const written = await page.request.put(api, {
    headers: { "X-CSRF-Token": await csrf(page) },
    data: {
      markdown: "# Tides\n\n## Morning\n\nCalm.\n\n## Evening\n\nRough.\n",
      create: true,
    },
  });
  expect(written.ok(), await written.text()).toBeTruthy();
  const pageSource = async () => {
    const response = await page.request.get(api);
    return ((await response.json()) as { markdown: string }).markdown;
  };
  try {
    await page.goto(`d/${REFLECTION_DOMAIN}/reflection-edit/${path}`);
    const source = page.getByRole("textbox", { name: "Page source" });
    await expect(source).toContainText("Rough.");
    await selectLine(source, "Evening");
    const bar = page.getByRole("toolbar", { name: "Quick format" });
    await expect(bar).toBeVisible();

    await bar.getByRole("button", { name: "Text style" }).click();
    await page
      .getByRole("menu")
      .getByRole("menuitemradio", { name: "Heading 2" })
      .click();
    const dialog = page.getByRole("alertdialog", { name: "Join sections?" });
    await expect(dialog).toBeVisible();
    await expect(dialog.getByRole("button", { name: "Cancel" })).toBeFocused();
    // Behind the modal question, the bar is still there.
    await expect(
      page.locator('[role="toolbar"][aria-label="Quick format"]'),
    ).toBeAttached();

    await dialog.getByRole("button", { name: "Join sections" }).click();
    await expect(dialog).toHaveCount(0);
    await expect(source).toBeFocused();
    await expect
      .poll(pageSource, { timeout: 20_000 })
      .toMatch(/\n### Evening\n\nRough\.\n$/);
    // The joined section's id line went with it; the one above keeps its own.
    expect((await pageSource()).match(/<!-- r:[0-9a-f]{8} -->/g)).toHaveLength(
      1,
    );
  } finally {
    await page.goto(`d/${REFLECTION_DOMAIN}/reflection`);
    const removed = await page.request.delete(api, {
      headers: { "X-CSRF-Token": await csrf(page) },
    });
    expect(removed.ok(), await removed.text()).toBeTruthy();
  }
});
