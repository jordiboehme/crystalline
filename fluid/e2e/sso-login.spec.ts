/**
 * The single sign-on button on the login screen, against a real daemon with a
 * provider configured.
 *
 * Only the render, deliberately. The browser half of the OIDC dance - the
 * redirect out, the provider's own form, the callback and what it does to the
 * accounts database - is driven end to end in `crates/service/tests/auth/oidc.rs`
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

const BASE_PATH = process.env.FLUID_E2E_BASE_PATH ?? "";
const SSO_NAME = process.env.FLUID_E2E_SSO_NAME ?? "Contoso";

test("the login screen offers the provider beside the local form", async ({
  page,
}) => {
  await page.goto("./");
  await expect(page).toHaveURL((url) => url.pathname === `${BASE_PATH}/login`);

  // Local accounts first: single sign-on is layered over them, never in place
  // of them, and the form is what the first admin and every seeded account
  // here uses.
  await expect(page.getByLabel("Name", { exact: true })).toBeVisible();
  await expect(page.getByRole("button", { name: "Log in" })).toBeVisible();

  const sso = page.getByRole("link", { name: `Sign in with ${SSO_NAME}` });
  await expect(sso).toBeVisible();
  // The API path, not a client route: following it leaves the app for the
  // provider and comes back through the callback.
  await expect(sso).toHaveAttribute(
    "href",
    `${BASE_PATH}/api/v1/auth/oidc/login`,
  );
});

test("the Log in button stays where it is when the provider answer arrives", async ({
  page,
}) => {
  // Hold the providers answer back, so the form is on screen without the
  // single sign-on link, as it is for a moment on every load.
  let release: () => void = () => {};
  const held = new Promise<void>((resolve) => {
    release = resolve;
  });
  await page.route("**/api/v1/auth/providers", async (route) => {
    await held;
    await route.continue();
  });
  await page.goto("./");
  const button = page.getByRole("button", { name: "Log in" });
  await expect(button).toBeVisible();
  const sso = page.getByRole("link", { name: `Sign in with ${SSO_NAME}` });
  await expect(sso).toHaveCount(0);
  const before = await button.boundingBox();

  release();
  await expect(sso).toBeVisible();
  const after = await button.boundingBox();
  expect(before).not.toBeNull();
  expect(after?.y).toBe(before?.y);
  expect(after?.x).toBe(before?.x);
});
