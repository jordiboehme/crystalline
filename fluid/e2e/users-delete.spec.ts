/**
 * The Users screen, against a real daemon: add an account and delete it
 * behind the two-step confirm.
 *
 * Run in WebKit as well as Chromium on purpose. Safari does not focus a
 * button when it is clicked; the focus goes to the page's focusable `<main>`
 * on mousedown, and a confirm that read that as "focus left" closed itself
 * before the click landed. Chromium never showed it.
 *
 * Shares the admin account and daemon `run-smoke.sh` sets up; run it the same
 * way (`bash fluid/e2e/run-smoke.sh e2e/users-delete.spec.ts`).
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

test("an account is added and deleted behind the two-step confirm", async ({
  page,
}) => {
  const name = `gone-${String(Date.now())}`;

  await signIn(page);
  await page.goto("users");
  await expect(page.getByRole("heading", { name: "Users" })).toBeVisible();

  await page.getByLabel("Login name").fill(name);
  await page.getByLabel("Password", { exact: true }).fill("a-long-enough-pass");
  await page.getByRole("button", { name: "Add user" }).click();
  await expect(
    page.getByRole("button", { name: `Delete ${name}` }),
  ).toBeVisible();

  await page.getByRole("button", { name: `Delete ${name}` }).click();
  await page.getByRole("button", { name: `Confirm delete of ${name}` }).click();

  await expect(
    page.getByRole("button", { name: `Delete ${name}` }),
  ).toHaveCount(0);
});
