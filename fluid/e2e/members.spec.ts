/**
 * Who can see a private domain, against a real daemon and three real accounts.
 *
 * What only this can prove is the whole loop: an invitation written by one
 * browser changes what a different browser's next listing contains. The
 * component tests cover the card's own controls and
 * `crates/service/tests/visibility.rs` covers the filter, but neither can put
 * two sessions on one instance and watch a domain appear in one of them.
 *
 * The cast `run-smoke.sh` seeds: the admin owns `smoke-vault`, the peer is
 * the account invited into it, and the outsider is an editor at exactly the
 * peer's instance role who is never invited - so the only thing that ever
 * separates the two is the membership row.
 *
 * One test rather than four, because the journey is one-way: opening the
 * domain again forgets who was invited, so a second block could not start from
 * the state the first one ended in.
 *
 * Run it the way the whole suite runs (`bash fluid/e2e/run-smoke.sh
 * e2e/members.spec.ts`); `pnpm exec playwright test` on its own has nothing to
 * talk to.
 */

import type { Page } from "@playwright/test";
import { expect, test } from "@playwright/test";

/** The admin `run-smoke.sh` creates, who owns the private domain. */
const USER = process.env.FLUID_E2E_USER ?? "smoke";
const PASSWORD = process.env.FLUID_E2E_PASSWORD ?? "smoke-password";

/** The account this journey invites. */
const PEER = process.env.FLUID_E2E_PEER ?? "peer";
const PEER_PASSWORD = process.env.FLUID_E2E_PEER_PASSWORD ?? "peer-password";

/** The account it never invites. */
const OUTSIDER = process.env.FLUID_E2E_OUTSIDER ?? "outsider";
const OUTSIDER_PASSWORD =
  process.env.FLUID_E2E_OUTSIDER_PASSWORD ?? "outsider-password";

/** The domain the same script registers and closes to `USER`. */
const DOMAIN = process.env.FLUID_E2E_PRIVATE_DOMAIN ?? "smoke-vault";

/**
 * The shared fixture domain every account on this instance can read.
 *
 * Never asserted about for its own sake: it is the anchor the absence
 * assertions below need, and nothing else.
 */
const SHARED_DOMAIN = process.env.FLUID_E2E_DOMAIN ?? "fluid-smoke";

/** Sign in and land on the home screen, as every spec in this suite does. */
async function signIn(
  page: Page,
  name: string,
  password: string,
): Promise<void> {
  await page.goto("/");
  await expect(page).toHaveURL(/\/login$/);

  await page.getByLabel("Name", { exact: true }).fill(name);
  await page.getByLabel("Password", { exact: true }).fill(password);
  await page.getByRole("button", { name: "Log in" }).click();

  await expect(
    page.getByRole("heading", { name: "Home", level: 1 }),
  ).toBeVisible();
}

/** The sidebar's link to the private domain, however many it lists. */
function sidebarLink(page: Page) {
  return page
    .getByRole("navigation", { name: "Domains" })
    .getByRole("link", { name: new RegExp(`^${DOMAIN}`) });
}

/** The home screen's card for it, by the link that carries its name. */
function homeCard(page: Page) {
  return page.locator("main").getByRole("link", { name: DOMAIN, exact: true });
}

/** That card's own article, so the badge beside its link can be asserted. */
function homeArticle(page: Page) {
  return page
    .locator("main article")
    .filter({ has: page.getByRole("link", { name: DOMAIN, exact: true }) });
}

/**
 * The private badge inside one of those, matched exactly.
 *
 * Exactly, because the badge's whole text is the word: a substring match would
 * also be satisfied by a domain whose NAME carried it, and this is the
 * assertion that has to be able to fail.
 */
function badge(within: ReturnType<typeof homeCard>) {
  return within.getByText("private", { exact: true });
}

/**
 * Wait until this browser has a domain listing on screen.
 *
 * Every "the private domain is not here" assertion below is a `toHaveCount(0)`,
 * which succeeds on its first poll - including the poll that runs while the
 * listing is still in flight, when nothing is there yet and every domain is
 * equally absent. Anchoring on the shared domain first is what makes those
 * assertions able to fail: the listing has arrived, this account can read it,
 * and the private domain is missing from an answer that was actually given.
 */
async function listingLoaded(page: Page): Promise<void> {
  await expect(
    page
      .getByRole("navigation", { name: "Domains" })
      .getByRole("link", { name: new RegExp(`^${SHARED_DOMAIN}`) }),
  ).toBeVisible();
}

/**
 * Load the home screen fresh.
 *
 * A real navigation rather than a re-render: the listing is cached for the
 * session that already read it, so a browser that was signed in before the
 * invitation landed would go on showing the answer it was given then.
 */
async function reloadHome(page: Page): Promise<void> {
  await page.goto("/");
  await expect(
    page.getByRole("heading", { name: "Home", level: 1 }),
  ).toBeVisible();
}

test("an invitation is what makes a private domain visible", async ({
  browser,
}) => {
  const ownerContext = await browser.newContext();
  const memberContext = await browser.newContext();
  const strangerContext = await browser.newContext();
  const owner = await ownerContext.newPage();
  const member = await memberContext.newPage();
  const stranger = await strangerContext.newPage();

  await signIn(owner, USER, PASSWORD);
  await signIn(member, PEER, PEER_PASSWORD);
  await signIn(stranger, OUTSIDER, OUTSIDER_PASSWORD);

  // The owner sees the domain badged as private in both places that name it -
  // the field the listing carries now, rather than a read per domain.
  await expect(homeCard(owner)).toBeVisible();
  await expect(badge(homeArticle(owner))).toBeVisible();
  await expect(badge(sidebarLink(owner))).toBeVisible();

  // Nobody else sees it at all yet. Not a refusal and not an empty card: the
  // name is the whole of what a private domain keeps, so the listing simply
  // does not contain it.
  for (const page of [member, stranger]) {
    await listingLoaded(page);
    await expect(homeCard(page)).toHaveCount(0);
    await expect(sidebarLink(page)).toHaveCount(0);
  }

  // And the address is a dead end for them, in the words a domain nobody
  // registered gets.
  await member.goto(`/d/${DOMAIN}`);
  await expect(
    member.getByRole("heading", { name: "Domain not found" }),
  ).toBeVisible();

  // The invitation, made in the browser on the card the owner administers it
  // from.
  await owner.goto(`/d/${DOMAIN}`);
  await expect(
    owner.getByRole("heading", { name: DOMAIN, level: 1 }),
  ).toBeVisible();
  const members = owner.getByRole("region", { name: "Members" });
  await expect(members).toBeVisible();
  await members.getByLabel("Account").fill(PEER);
  await members.getByLabel("Level").selectOption("viewer");
  await members.getByRole("button", { name: "Invite" }).click();
  await expect(members.getByText(PEER, { exact: true })).toBeVisible();

  // The member's next listing has it, badged the same way the owner's is.
  await reloadHome(member);
  await expect(homeCard(member)).toBeVisible();
  await expect(badge(homeArticle(member))).toBeVisible();
  await expect(badge(sidebarLink(member))).toBeVisible();
  await member.goto(`/d/${DOMAIN}`);
  await expect(
    member.getByRole("heading", { name: DOMAIN, level: 1 }),
  ).toBeVisible();

  // The account nobody invited is exactly where it was.
  await reloadHome(stranger);
  await listingLoaded(stranger);
  await expect(homeCard(stranger)).toHaveCount(0);
  await expect(sidebarLink(stranger)).toHaveCount(0);

  // Opened again, from the same card. This is the one-way step: the domain
  // forgets who was invited, and every account on the instance can read it.
  await members.getByRole("button", { name: "Share with everyone" }).click();
  await members
    .getByRole("button", { name: "Confirm share with everyone" })
    .click();
  // Exactly, so the card's own caption is what is read rather than the notice
  // under it, which says the same thing in a sentence.
  await expect(
    members.getByText("Shared with everyone", { exact: true }),
  ).toBeVisible();

  // Everyone sees it now, and nobody sees a badge on it.
  for (const page of [member, stranger, owner]) {
    await reloadHome(page);
    await expect(homeCard(page)).toBeVisible();
    await expect(sidebarLink(page)).toBeVisible();
    // Visible first, then unbadged: a badge assertion on a domain that is not
    // there at all would pass for the wrong reason.
    await expect(badge(homeArticle(page))).toHaveCount(0);
    await expect(badge(sidebarLink(page))).toHaveCount(0);
  }
});
