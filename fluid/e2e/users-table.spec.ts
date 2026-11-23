/**
 * The users table against a real daemon: the search and the sort live in the
 * address, a row opens the account at its own address and the back button
 * closes it, a selection is deactivated and reactivated in bulk, and the
 * table fits the Administration column at a laptop's width.
 *
 * Shares the admin account and daemon `run-smoke.sh` sets up; run it the same
 * way (`bash fluid/e2e/run-smoke.sh e2e/users-table.spec.ts`).
 */

import type { Page } from "@playwright/test";
import { expect, test } from "@playwright/test";

const USER = process.env.FLUID_E2E_USER ?? "smoke";
const BASE_PATH = process.env.FLUID_E2E_BASE_PATH ?? "";
const PASSWORD = process.env.FLUID_E2E_PASSWORD ?? "smoke-password";

/** Sign in and land on the home screen, the same way the smoke suite does. */
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

/** Add an account through the form, then come back to the list. */
async function addAccount(page: Page, name: string): Promise<void> {
  const add = page.getByRole("region", { name: "Add an account" });
  await add.getByLabel("Login name").fill(name);
  await add.getByLabel("Password", { exact: true }).fill("a-long-enough-pass");
  await add.getByRole("button", { name: "Add user" }).click();
  await expect(page).toHaveURL(
    (url) => url.pathname === `${BASE_PATH}/admin/users/${name}`,
  );
  await page.getByRole("link", { name: "Back to users" }).click();
}

/** Delete an account from its panel. */
async function deleteAccount(page: Page, name: string): Promise<void> {
  await page.goto(`admin/users/${name}`);
  await page.getByRole("button", { name: `Delete ${name}` }).click();
  await page.getByRole("button", { name: `Confirm delete of ${name}` }).click();
  await expect(page.getByText(`Deleted ${name}.`)).toBeVisible();
}

test("search, sort and the open account live in the address", async ({
  page,
}) => {
  await signIn(page);
  await page.goto("admin/users");
  const table = page.getByRole("table");
  await expect(table.getByRole("link", { name: USER })).toBeVisible();

  await page
    .getByRole("search", { name: "Find accounts" })
    .getByRole("searchbox")
    .fill(USER);
  await expect(page).toHaveURL((url) => url.searchParams.get("q") === USER);
  await table.getByRole("button", { name: "Last seen" }).click();
  await expect(page).toHaveURL(
    (url) =>
      url.searchParams.get("sort") === "last_seen" &&
      url.searchParams.get("dir") === "desc",
  );
  await expect(page.getByText(/signs in while you page/)).toBeVisible();

  // A reload keeps all of it.
  await page.reload();
  await expect(
    page.getByRole("search", { name: "Find accounts" }).getByRole("searchbox"),
  ).toHaveValue(USER);

  await table.getByRole("link", { name: USER }).click();
  await expect(page).toHaveURL(
    (url) =>
      url.pathname === `${BASE_PATH}/admin/users/${USER}` &&
      url.searchParams.get("q") === USER,
  );
  await expect(
    page.getByRole("combobox", { name: `Role for ${USER}` }),
  ).toHaveValue("admin");

  // The back button closes the account and keeps the list as it was.
  await page.goBack();
  await expect(page).toHaveURL(
    (url) =>
      url.pathname === `${BASE_PATH}/admin/users` &&
      url.searchParams.get("q") === USER,
  );
  await expect(page.getByRole("link", { name: "Back to users" })).toHaveCount(
    0,
  );
});

test("a selection is deactivated and reactivated in bulk", async ({ page }) => {
  const stem = `bulk${String(Date.now())}`;
  const names = [`${stem}-a`, `${stem}-b`];

  await signIn(page);
  await page.goto("admin/users");
  for (const name of names) {
    await addAccount(page, name);
  }
  await page.goto(`admin/users?q=${stem}`);
  const table = page.getByRole("table");
  await expect(table.getByRole("link")).toHaveCount(2);

  await page
    .getByRole("checkbox", { name: "Select every account on this page" })
    .check();
  const bar = page.getByRole("region", { name: "Selected accounts" });
  await expect(bar.getByText("2 accounts selected")).toBeVisible();
  await bar.getByRole("button", { name: "Deactivate", exact: true }).click();
  await expect(bar.getByText("Deactivate for 2 accounts?")).toBeVisible();
  await bar.getByRole("button", { name: "Deactivate 2 accounts" }).click();

  const results = page.getByRole("status", { name: "Bulk results" });
  await expect(results).toContainText("Deactivate: 2 done.");
  await expect(table.getByText("Disabled")).toHaveCount(2);

  await page
    .getByRole("checkbox", { name: "Select every account on this page" })
    .check();
  await bar.getByRole("button", { name: "Reactivate", exact: true }).click();
  await bar.getByRole("button", { name: "Reactivate 2 accounts" }).click();
  await expect(results).toContainText("Reactivate: 2 done.");
  await expect(table.getByText("Active")).toHaveCount(2);

  for (const name of names) {
    await deleteAccount(page, name);
  }
});

test("the table fits the Administration column at a laptop's width", async ({
  page,
}) => {
  await page.setViewportSize({ width: 1024, height: 768 });
  await signIn(page);
  await page.goto("admin/users");
  await expect(page.getByRole("table")).toBeVisible();

  // The page itself never scrolls sideways.
  expect(await pageOverflow(page)).toBeLessThanOrEqual(0);
  // And the table does not need its own sideways scroll either.
  const table = page.getByRole("table");
  const tableOverflow = await table.evaluate((element) => {
    const box = element.parentElement;
    return box ? box.scrollWidth - box.clientWidth : 0;
  });
  expect(tableOverflow).toBeLessThanOrEqual(0);
});

/** How far the page itself scrolls sideways; zero or less is none. */
async function pageOverflow(page: Page): Promise<number> {
  return page.evaluate(
    () =>
      document.documentElement.scrollWidth -
      document.documentElement.clientWidth,
  );
}

test("an account opens beside the table when wide, and alone when narrow", async ({
  page,
}) => {
  await page.setViewportSize({ width: 1600, height: 900 });
  await signIn(page);
  await page.goto(`admin/users/${USER}`);
  await expect(
    page.getByRole("combobox", { name: `Role for ${USER}` }),
  ).toBeVisible();
  await expect(page.getByRole("table")).toBeVisible();
  expect(await pageOverflow(page)).toBeLessThanOrEqual(0);

  await page.setViewportSize({ width: 390, height: 844 });
  await expect(
    page.getByRole("combobox", { name: `Role for ${USER}` }),
  ).toBeVisible();
  await expect(page.getByRole("table")).toBeHidden();
  expect(await pageOverflow(page)).toBeLessThanOrEqual(0);
});
