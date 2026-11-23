/**
 * The domain sidebar in a real browser: the draggable divider and the marks
 * after a domain's name.
 *
 * What jsdom cannot answer here: that a real mouse drag with pointer capture
 * moves the column, that the stored width comes back after a reload, and that
 * the divider is gone in the drawer layout. The marks ride on the seeded
 * cast: the admin owns `smoke-pages`, a private domain with its Reflection on.
 *
 * Read-only against the daemon, so it can run in any shard
 * (`bash fluid/e2e/run-smoke.sh e2e/sidebar.spec.ts`).
 */

import type { Page } from "@playwright/test";
import { expect, test } from "@playwright/test";

const USER = process.env.FLUID_E2E_USER ?? "smoke";
const PASSWORD = process.env.FLUID_E2E_PASSWORD ?? "smoke-password";
const PAGES_DOMAIN = process.env.FLUID_E2E_REFLECTION_DOMAIN ?? "smoke-pages";
const BASE_PATH = process.env.FLUID_E2E_BASE_PATH ?? "";

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

function divider(page: Page) {
  return page.getByRole("separator", { name: "Sidebar width" });
}

async function widthOf(page: Page): Promise<number> {
  const box = await page
    .getByRole("navigation", { name: "Domains" })
    .boundingBox();
  if (box === null) {
    throw new Error("the sidebar has no box");
  }
  return Math.round(box.width);
}

/** Drag the divider by `dx` pixels with a real mouse. */
async function drag(page: Page, dx: number): Promise<void> {
  const box = await divider(page).boundingBox();
  if (box === null) {
    throw new Error("the divider has no box");
  }
  const x = box.x + box.width / 2;
  const y = box.y + Math.min(box.height / 2, 300);
  await page.mouse.move(x, y);
  await page.mouse.down();
  await page.mouse.move(x + dx / 2, y, { steps: 4 });
  await page.mouse.move(x + dx, y, { steps: 4 });
  await page.mouse.up();
}

test.beforeEach(async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 900 });
  await signIn(page);
});

test("the divider drags within its limits, remembers the width and resets", async ({
  page,
}) => {
  await expect(divider(page)).toHaveAttribute("aria-valuenow", "256");
  const start = await widthOf(page);

  await drag(page, 100);
  await expect(divider(page)).toHaveAttribute("aria-valuenow", "356");
  expect(await widthOf(page)).toBe(start + 100);

  await drag(page, 600);
  await expect(divider(page)).toHaveAttribute("aria-valuenow", "480");

  await page.reload();
  await expect(divider(page)).toHaveAttribute("aria-valuenow", "480");

  await divider(page).dblclick();
  await expect(divider(page)).toHaveAttribute("aria-valuenow", "256");
  expect(await widthOf(page)).toBe(start);
});

test("dragging under the minimum folds the sidebar to the rail", async ({
  page,
}) => {
  await drag(page, -150);
  await expect(
    page.getByRole("button", { name: "Expand the sidebar" }),
  ).toBeVisible();
  await expect(divider(page)).toHaveCount(0);

  await page.getByRole("button", { name: "Expand the sidebar" }).click();
  await expect(divider(page)).toHaveAttribute("aria-valuenow", "256");
});

test("the drawer layout has no divider", async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await page.getByRole("button", { name: "Domains", exact: true }).click();
  await expect(page.getByRole("navigation", { name: "Domains" })).toBeVisible();
  await expect(divider(page)).toHaveCount(0);
});

test("a private domain with a Reflection carries both marks after its name", async ({
  page,
}) => {
  const link = page
    .getByRole("navigation", { name: "Domains" })
    .getByRole("link", { name: new RegExp(`^${PAGES_DOMAIN}`) });
  await expect(link).toBeVisible();
  await expect(link.getByRole("img", { name: "Private domain" })).toBeVisible();
  await expect(
    link.getByRole("img", { name: "Has a Reflection" }),
  ).toBeVisible();

  await link.getByRole("img", { name: "Private domain" }).hover();
  await expect(page.getByRole("tooltip")).toHaveText("Private domain");
});
