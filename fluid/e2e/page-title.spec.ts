/**
 * Tabs, the history and a saved PDF name the page they show: `<page> - Fluid`
 * on screen and the bare page title while printing, which is what the browser
 * suggests as the file name.
 */

import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";

/** The account and the domain `run-smoke.sh` set up. Kept in step with it. */
const USER = process.env.FLUID_E2E_USER ?? "smoke";
const PASSWORD = process.env.FLUID_E2E_PASSWORD ?? "smoke-password";
const DOMAIN = process.env.FLUID_E2E_DOMAIN ?? "fluid-smoke";

async function signIn(page: Page): Promise<void> {
  await page.goto("./");
  await page.getByLabel("Name", { exact: true }).fill(USER);
  await page.getByLabel("Password", { exact: true }).fill(PASSWORD);
  await page.getByRole("button", { name: "Log in" }).click();
  await expect(
    page.getByRole("heading", { name: "Home", level: 1 }),
  ).toBeVisible();
}

/** Fire the event a browser sends around printing, Cmd+P included. */
async function print(page: Page, event: "beforeprint" | "afterprint") {
  await page.evaluate((name) => {
    window.dispatchEvent(new Event(name));
  }, event);
}

test("an engram names the tab, and only its bare title while printing", async ({
  page,
}) => {
  await signIn(page);
  await expect.poll(() => page.title()).toBe("Home - Fluid");

  await page.goto(`d/${DOMAIN}/e/berth-assignment-flow`);
  await expect(
    page.getByRole("heading", { name: "Berth Assignment Flow", level: 1 }),
  ).toBeVisible();
  expect(await page.title()).toBe("Berth Assignment Flow - Fluid");

  await print(page, "beforeprint");
  expect(await page.title()).toBe("Berth Assignment Flow");
  await print(page, "afterprint");
  expect(await page.title()).toBe("Berth Assignment Flow - Fluid");
});

test("a title with a slash and quotes survives as the text", async ({
  page,
}) => {
  const title = `Quay A/B "night" plan`;
  // Only the title the server sends is changed, so nothing in the domain is.
  await page.route(
    `**/domains/${DOMAIN}/engrams/berth-assignment-flow`,
    async (route) => {
      const response = await route.fetch();
      const body = (await response.json()) as Record<string, unknown>;
      await route.fulfill({ response, json: { ...body, title } });
    },
  );
  await signIn(page);
  await page.goto(`d/${DOMAIN}/e/berth-assignment-flow`);
  await expect(
    page.getByRole("heading", { name: title, level: 1 }),
  ).toBeVisible();
  expect(await page.title()).toBe(`${title} - Fluid`);

  await print(page, "beforeprint");
  expect(await page.title()).toBe(title);
  await print(page, "afterprint");
  expect(await page.title()).toBe(`${title} - Fluid`);
});
