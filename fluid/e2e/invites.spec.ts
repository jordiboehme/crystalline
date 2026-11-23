/**
 * Administration > Invites against a real daemon: make a link for the
 * private domain, see it once with a copy button and the token after `#`
 * under the base path, find it pending, and revoke it behind the two-step
 * confirm. The smoke daemon has single sign-on configured, so the local
 * account choice is offered.
 *
 * Run it the way `run-smoke.sh` runs every spec (`bash fluid/e2e/run-smoke.sh
 * e2e/invites.spec.ts`), and once more with `FLUID_E2E_BASE_PATH=/crystalline`.
 */

import type { Page } from "@playwright/test";
import { expect, test } from "@playwright/test";

const USER = process.env.FLUID_E2E_USER ?? "smoke";
const BASE_PATH = process.env.FLUID_E2E_BASE_PATH ?? "";
const PASSWORD = process.env.FLUID_E2E_PASSWORD ?? "smoke-password";
const PRIVATE_DOMAIN = process.env.FLUID_E2E_PRIVATE_DOMAIN ?? "smoke-vault";

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

test.describe("the invites page", () => {
  test.use({ viewport: { width: 1280, height: 800 } });

  test("makes a link, shows it once and revokes it", async ({ page }) => {
    const label = `smoke ${Date.now().toString(36)}`;
    await signIn(page);
    await page.goto("admin/invites");
    await expect(
      page.getByRole("heading", { name: "Invites", level: 1 }),
    ).toBeVisible();

    await page
      .getByLabel("Domain", { exact: true })
      .selectOption(PRIVATE_DOMAIN);
    await page.getByLabel(`Level in ${PRIVATE_DOMAIN}`).selectOption("editor");
    await page.getByLabel("Label", { exact: true }).fill(label);
    // Single sign-on is configured, so a local account is a choice.
    await expect(
      page.getByLabel("Allow a local account with a password"),
    ).toBeVisible();
    await page.getByRole("button", { name: "Make link" }).click();

    const shown = page.getByRole("group", { name: "Invite link" });
    const field = shown.getByRole("textbox", { name: "Invite link" });
    const link = await field.inputValue();
    const url = new URL(link);
    expect(url.pathname).toBe(`${BASE_PATH}/invite`);
    expect(url.search).toBe("");
    expect(url.hash).toMatch(/^#cin_/);
    await expect(shown).toContainText("It is not shown again.");
    await expect(
      shown.getByRole("button", { name: "Copy the invite link" }),
    ).toBeVisible();

    const pending = page.getByRole("region", { name: "Pending", exact: true });
    const row = pending.getByRole("row").filter({ hasText: label });
    await expect(row).toContainText(`editor in ${PRIVATE_DOMAIN}`);

    await shown.getByRole("button", { name: "Done" }).click();
    await expect(page.getByRole("group", { name: "Invite link" })).toHaveCount(
      0,
    );

    await row
      .getByRole("button", { name: new RegExp(`^Revoke ${label}`) })
      .click();
    await row
      .getByRole("button", { name: /^Confirm revoke of link \d+$/ })
      .click();
    await expect(page.getByRole("status").first()).toContainText(
      `Revoked ${label}.`,
    );
    await expect(
      pending.getByRole("row").filter({ hasText: label }),
    ).toHaveCount(0);

    await page.getByLabel("Show", { exact: true }).selectOption("revoked");
    await expect(
      page.getByRole("region", { name: "Revoked", exact: true }),
    ).toContainText(label);
  });
});
