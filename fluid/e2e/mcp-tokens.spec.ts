/**
 * The Agent access card, against a real daemon: issue a token, see the
 * secret exactly once, find it listed with no secret afterward, and revoke
 * it behind the two-step confirm.
 *
 * What only a real browser proves here is the round trip with the actual
 * REST surface (`crates/service/tests/rest_mcp_tokens.rs` covers the routes
 * themselves): the token the server hands back really does match the shape
 * an agent's `Authorization: Bearer` header expects, and reloading the page
 * - a real navigation, not a re-render - never shows the secret again.
 *
 * Shares the fixture account and daemon `run-smoke.sh` sets up; run this
 * spec the same way (`bash fluid/e2e/run-smoke.sh e2e/mcp-tokens.spec.ts`),
 * since `pnpm exec playwright test` on its own has nothing to talk to.
 */

import type { Page } from "@playwright/test";
import { expect, test } from "@playwright/test";

const USER = process.env.FLUID_E2E_USER ?? "smoke";
const PASSWORD = process.env.FLUID_E2E_PASSWORD ?? "smoke-password";

/** Sign in and land on the home screen, the same way the smoke suite does. */
async function signIn(page: Page): Promise<void> {
  await page.goto("/");
  await expect(page).toHaveURL(/\/login$/);

  await page.getByLabel("Name", { exact: true }).fill(USER);
  await page.getByLabel("Password", { exact: true }).fill(PASSWORD);
  await page.getByRole("button", { name: "Log in" }).click();

  await expect(
    page.getByRole("heading", { name: "Home", level: 1 }),
  ).toBeVisible();
}

test.beforeEach(async ({ page }) => {
  await signIn(page);
  await page.goto("/profile");
  await expect(
    page.getByRole("heading", { name: "Agent access" }),
  ).toBeVisible();
});

test("a token is issued, revealed once, listed and revoked", async ({
  page,
}) => {
  const label = `smoke-token-${Date.now()}`;

  await page.getByLabel("Label").fill(label);
  await page.getByRole("button", { name: "Issue token" }).click();

  // Revealed, and shaped the way an MCP token is: `cmt_` plus 64 hex digits.
  const dialog = page.getByRole("dialog", { name: label });
  await expect(dialog).toBeVisible();
  const token = await dialog.locator("code").first().innerText();
  expect(token).toMatch(/^cmt_[0-9a-f]{64}$/);

  // The exact harness-config teaching line, with the real secret in it - the
  // sentence an agent's own MCP registration is meant to be edited from.
  await expect(
    dialog.getByText(
      `Add this as header Authorization: Bearer ${token} to the crystalline entry in your agent's MCP registration.`,
    ),
  ).toBeVisible();

  await dialog.getByRole("button", { name: "Done" }).click();
  await expect(dialog).toBeHidden();
  // Gone from the DOM the moment the dialog is dismissed, not merely hidden.
  await expect(page.getByText(token)).toHaveCount(0);

  // Listed now, by its label - never by the secret, which the list route
  // never carries in the first place.
  await expect(page.getByText(label, { exact: true })).toBeVisible();

  // A real reload, not a re-render: the secret is nowhere this page could
  // read it back from, so it never reappears.
  await page.reload();
  await expect(
    page.getByRole("heading", { name: "Agent access" }),
  ).toBeVisible();
  await expect(page.getByText(token)).toHaveCount(0);
  await expect(page.getByText(label, { exact: true })).toBeVisible();

  // Revoked behind the two-step confirm every destructive control on this
  // screen uses.
  await page.getByRole("button", { name: `Revoke ${label}` }).click();
  await page.getByRole("button", { name: `Confirm revoke ${label}` }).click();
  await expect(page.getByText(label, { exact: true })).toHaveCount(0);
});
