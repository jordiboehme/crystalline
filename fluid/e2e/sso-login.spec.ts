/**
 * The single sign-on button on the login screen, against a real daemon with a
 * provider configured.
 *
 * Only the render, deliberately. The browser half of the OIDC dance - the
 * redirect out, the provider's own form, the callback and what it does to the
 * accounts database - is driven end to end in `crates/service/tests/oidc.rs`
 * against a fake provider that process runs itself, which can rotate a signing
 * key or lie about an issuer on demand in a way no browser test could. What
 * only a browser can answer is that the button exists, wears the provider's
 * own name, and is a link that navigates the whole page rather than a control
 * that would fetch the redirect in the background and land nowhere.
 *
 * Shares the daemon `run-smoke.sh` sets up, which configures the provider
 * through the environment; run this spec the same way
 * (`bash fluid/e2e/run-smoke.sh e2e/sso-login.spec.ts`).
 */

import { expect, test } from "@playwright/test";

const SSO_NAME = process.env.FLUID_E2E_SSO_NAME ?? "Contoso";

test("the login screen offers the provider beside the local form", async ({
  page,
}) => {
  await page.goto("/");
  await expect(page).toHaveURL(/\/login$/);

  // Local accounts first: single sign-on is layered over them, never in place
  // of them, and the form is what the first admin and every seeded account
  // here uses.
  await expect(page.getByLabel("Name", { exact: true })).toBeVisible();
  await expect(page.getByRole("button", { name: "Log in" })).toBeVisible();

  const sso = page.getByRole("link", { name: `Sign in with ${SSO_NAME}` });
  await expect(sso).toBeVisible();
  // The API path, not a client route: following it leaves the app for the
  // provider and comes back through the callback.
  await expect(sso).toHaveAttribute("href", "/api/v1/auth/oidc/login");
});
