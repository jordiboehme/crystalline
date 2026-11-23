/**
 * Invite links against a real daemon, from both ends.
 *
 * The owner of a private domain searches the Members card for someone who
 * has no account, makes an invite link from "Invite someone new" and revokes
 * it again. An admin makes a local-account link in Administration > Invites,
 * and a fresh browser opens it: the token leaves the address bar at once,
 * the page names the inviter and the domain, and choosing a login and a
 * password makes the account, signs it in and links into the domain.
 *
 * The smoke daemon has single sign-on configured, so a link from the card
 * offers that sign-in, and only an admin's local-account link offers a
 * password. Run it the way `run-smoke.sh` runs every spec (`bash
 * fluid/e2e/run-smoke.sh e2e/invite-redeem.spec.ts`), and once more with
 * `FLUID_E2E_BASE_PATH=/crystalline`.
 */

import type { Page } from "@playwright/test";
import { expect, test } from "@playwright/test";

const USER = process.env.FLUID_E2E_USER ?? "smoke";
const BASE_PATH = process.env.FLUID_E2E_BASE_PATH ?? "";
const PASSWORD = process.env.FLUID_E2E_PASSWORD ?? "smoke-password";
/** A private domain `USER` owns that stays private for the whole run. */
const DOMAIN = process.env.FLUID_E2E_REFLECTION_DOMAIN ?? "smoke-pages";

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

/** The link a ShownOnce group shows, checked to be under the base path. */
async function shownLink(page: Page): Promise<string> {
  const shown = page.getByRole("group", { name: "Invite link" });
  const link = await shown
    .getByRole("textbox", { name: "Invite link" })
    .inputValue();
  const url = new URL(link);
  expect(url.pathname).toBe(`${BASE_PATH}/invite`);
  expect(url.hash).toMatch(/^#cin_/);
  expect(url.search).toBe("");
  return link;
}

test.describe("invite links", () => {
  test.use({ viewport: { width: 1280, height: 800 } });

  test("the members card makes a link for someone new", async ({
    page,
    browser,
  }) => {
    const nobody = `zz${Date.now().toString(36)}`;
    await signIn(page);
    await page.goto(`d/${DOMAIN}/settings`);
    const members = page.getByRole("region", { name: "Members" });
    await members.getByRole("combobox", { name: "Account" }).fill(nobody);
    await expect(members.getByText("No account matches.")).toBeVisible();
    await members.getByRole("button", { name: "Invite someone new" }).click();
    await members.getByLabel(`Access in ${DOMAIN}`).selectOption("editor");
    await members.getByLabel("Label", { exact: true }).fill(nobody);
    await members
      .getByRole("button", { name: "Make link", exact: true })
      .click();
    const link = await shownLink(page);

    // Opened elsewhere it offers the single sign-on, which lands on the
    // domain, and the address bar keeps no token.
    const guest = await browser.newContext();
    const visitor = await guest.newPage();
    await visitor.goto(link);
    await expect(
      visitor.getByRole("button", { name: "Continue with single sign-on" }),
    ).toBeVisible();
    await expect(
      visitor.getByText(`You come back signed in, on the page of ${DOMAIN}.`),
    ).toBeVisible();
    expect(new URL(visitor.url()).hash).toBe("");
    expect(new URL(visitor.url()).pathname).toBe(`${BASE_PATH}/invite`);
    await guest.close();

    // The card lists it as the caller's own, and revokes it.
    const pending = members.getByRole("list", { name: "Your pending links" });
    const row = pending.getByRole("listitem").filter({ hasText: nobody });
    await row
      .getByRole("button", { name: new RegExp(`^Revoke ${nobody}`) })
      .click();
    await row.getByRole("button", { name: /^Confirm revoke of link/ }).click();
    await expect(row).toHaveCount(0);
  });

  test("a local-account link makes the account and joins the domain", async ({
    page,
    browser,
  }) => {
    const login = `i${Date.now().toString(36).slice(-5)}`;
    await signIn(page);
    await page.goto("admin/invites");
    // Role none: the account sees only the domains it is invited into.
    await page.getByLabel("Role", { exact: true }).selectOption("none");
    await page.getByLabel("Domain", { exact: true }).selectOption(DOMAIN);
    await page.getByLabel(`Level in ${DOMAIN}`).selectOption("viewer");
    await page.getByLabel("Label", { exact: true }).fill(login);
    await page.getByLabel("Allow a local account with a password").check();
    await page.getByRole("button", { name: "Make link" }).click();
    const link = await shownLink(page);

    const guest = await browser.newContext();
    const visitor = await guest.newPage();
    await visitor.goto(link);
    await expect(
      visitor.getByRole("heading", { name: "You are invited", level: 1 }),
    ).toBeVisible();
    await expect(visitor.getByText(DOMAIN, { exact: true })).toBeVisible();
    expect(new URL(visitor.url()).hash).toBe("");

    await visitor.getByLabel("Choose a login name").fill(login);
    await visitor.getByLabel("Choose a password").fill("invitee-password-1");
    await visitor
      .getByRole("button", { name: "Create account and join" })
      .click();
    await expect(visitor.getByRole("status")).toHaveText(
      `You joined ${DOMAIN} as viewer.`,
    );
    await visitor.getByRole("link", { name: `Open ${DOMAIN}` }).click();
    await expect(visitor).toHaveURL((url) =>
      url.pathname.startsWith(`${BASE_PATH}/d/${DOMAIN}`),
    );

    // The same link again is not valid any more.
    const again = await browser.newContext();
    const late = await again.newPage();
    await late.goto(link);
    await expect(late.getByRole("alert")).toHaveText(
      "This invite link is not valid any more.",
    );
    await again.close();
    await guest.close();
  });
});
