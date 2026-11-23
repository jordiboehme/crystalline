/**
 * The Administration area against a real daemon: `/admin` opens General, the
 * side navigation moves between its pages, and the addresses the admin pages
 * had before it (`/users`, `/settings/github`) lead on with their query and
 * under the base path. The server's own landing after a GitHub App install
 * sends the browser there too.
 *
 * Shares the admin account and daemon `run-smoke.sh` sets up; run it the same
 * way (`bash fluid/e2e/run-smoke.sh e2e/admin.spec.ts`), and once more with
 * `FLUID_E2E_BASE_PATH=/crystalline` for the base path.
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

test.describe("the Administration area", () => {
  test.use({ viewport: { width: 1280, height: 800 } });

  test("opens on General and moves between its pages", async ({ page }) => {
    await signIn(page);
    await page.goto("admin");

    await expect(page).toHaveURL(
      (url) => url.pathname === `${BASE_PATH}/admin/general`,
    );
    await expect(
      page.getByRole("heading", { name: "General", level: 1 }),
    ).toBeVisible();
    const nav = page.getByRole("navigation", { name: "Administration" });
    await expect(nav.getByRole("link", { name: "General" })).toHaveAttribute(
      "aria-current",
      "page",
    );

    await nav.getByRole("link", { name: "Users" }).click();
    await expect(
      page.getByRole("heading", { name: "Users", level: 1 }),
    ).toBeVisible();
    await expect(page).toHaveURL(
      (url) => url.pathname === `${BASE_PATH}/admin/users`,
    );
    await expect(nav.getByRole("link", { name: "Users" })).toHaveAttribute(
      "aria-current",
      "page",
    );

    await nav.getByRole("link", { name: "Maintenance" }).click();
    await expect(page).toHaveURL(
      (url) => url.pathname === `${BASE_PATH}/maintenance`,
    );
  });

  test("the old addresses lead on with their query", async ({ page }) => {
    await signIn(page);

    await page.goto("users?keep=1#here");
    await expect(
      page.getByRole("heading", { name: "Users", level: 1 }),
    ).toBeVisible();
    await expect(page).toHaveURL(
      (url) =>
        url.pathname === `${BASE_PATH}/admin/users` &&
        url.search === "?keep=1" &&
        url.hash === "#here",
    );

    await page.goto("settings/github?keep=2");
    await expect(
      page.getByRole("heading", { name: "GitHub connection", level: 1 }),
    ).toBeVisible();
    await expect(page).toHaveURL(
      (url) =>
        url.pathname === `${BASE_PATH}/admin/github` &&
        url.search === "?keep=2",
    );
  });

  test("the server's GitHub App landing goes to the area", async ({ page }) => {
    await signIn(page);
    const response = await page.request.get(
      `${BASE_PATH}/api/v1/github/app/setup`,
      {
        maxRedirects: 0,
      },
    );
    expect(response.status()).toBe(303);
    expect(response.headers()["location"]).toBe(
      `${BASE_PATH}/admin/github?app=installed`,
    );
  });
});

test.describe("the Administration area at drawer width", () => {
  test.use({ viewport: { width: 390, height: 844 } });

  test("a select stands in for the side navigation", async ({ page }) => {
    await signIn(page);
    await page.goto("admin/general");

    await expect(
      page.getByRole("navigation", { name: "Administration" }),
    ).toBeHidden();
    await page
      .getByRole("combobox", { name: "Administration page" })
      .selectOption({ label: "Invites" });
    await expect(
      page.getByRole("heading", { name: "Invites", level: 1 }),
    ).toBeVisible();
    await expect(page).toHaveURL(
      (url) => url.pathname === `${BASE_PATH}/admin/invites`,
    );
  });
});
