/**
 * The browser smoke: the paths a unit test cannot reach.
 *
 * Everything asserted here needs something jsdom does not have. A real login
 * against a real daemon over a real cookie; a mermaid fence that has to become
 * an SVG rather than a mocked promise; a deep link typed into the address bar,
 * which is the static server's SPA fallback and the app's per-segment permalink
 * encoding meeting for the first time; a search answered by the index rather
 * than by a fixture; a graph canvas that only a browser with layout can draw;
 * and a keyboard shortcut delivered by the browser itself.
 *
 * The domain behind it is `e2e/fixtures/domain`, copied to a scratch directory
 * and registered by `e2e/run-smoke.sh`, which is also what seeds the account
 * these tests sign in with. Every title, permalink and tag asserted on below is
 * a line in one of those files.
 */

import type { Page } from "@playwright/test";
import { expect, test } from "@playwright/test";

/** The account and the domain `run-smoke.sh` set up. Kept in step with it. */
const USER = process.env.FLUID_E2E_USER ?? "smoke";
const PASSWORD = process.env.FLUID_E2E_PASSWORD ?? "smoke-password";
const DOMAIN = process.env.FLUID_E2E_DOMAIN ?? "fluid-smoke";

/** The engram three folders down, whose permalink is a path rather than a word. */
const DEEP_PERMALINK = "notes/deep/gamma";

/**
 * Sign in, and land on the home screen.
 *
 * The unauthenticated app redirects to the login screen on its own, so this
 * starts at the root: what it proves on the way past is that the gate holds.
 */
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

/**
 * The engram page's own title.
 *
 * By its id rather than by role, because an engram body conventionally opens
 * with its own title as a heading, so the page carries two: the header's and
 * the document's. The id is the one the article is labelled by, so it is the
 * page's title by contract rather than by position.
 */
function engramTitle(page: Page) {
  return page.locator("h1#engram-title");
}

/** Open one engram of the fixture domain, from the home screen. */
async function openEngram(page: Page, title: string): Promise<void> {
  await page
    .locator("main")
    .getByRole("link", { name: DOMAIN, exact: true })
    .click();
  await expect(
    page.getByRole("heading", { name: DOMAIN, level: 1 }),
  ).toBeVisible();

  await page.getByRole("link", { name: new RegExp(`^${title}, `) }).click();
  await expect(engramTitle(page)).toHaveText(title);
}

test.beforeEach(async ({ page }) => {
  await signIn(page);
});

test("the home screen lists the fixture domain", async ({ page }) => {
  const card = page
    .locator("main")
    .getByRole("link", { name: DOMAIN, exact: true });
  await expect(card).toBeVisible();

  // The sidebar reads the same listing, so a domain missing from one of the two
  // is a different bug from a domain missing from both.
  await expect(
    page.getByRole("navigation", { name: "Domains" }).getByRole("link", {
      name: new RegExp(`^${DOMAIN}`),
    }),
  ).toBeVisible();
});

test("an engram renders its mermaid fence as a diagram", async ({ page }) => {
  await openEngram(page, "Lantern Protocol");

  // Mermaid names the SVG after the element it rendered into, so this is the
  // diagram itself rather than any other picture on the page. A fence mermaid
  // refuses falls back to a <pre> of its source, which this would not match.
  await expect(page.locator('article svg[id^="mermaid-"]')).toBeVisible();
});

test("a multi-segment permalink loads from the address bar", async ({
  page,
}) => {
  await page.goto(`/d/${DOMAIN}/e/${DEEP_PERMALINK}`);

  await expect(engramTitle(page)).toHaveText("Deep Gamma Note");
  await expect(page.getByText(DEEP_PERMALINK, { exact: true })).toBeVisible();
});

test("a search finds an engram by what is in it", async ({ page }) => {
  await page
    .getByRole("searchbox", { name: "Search", exact: true })
    .fill("Lantern");
  await page.keyboard.press("Enter");

  await expect(page).toHaveURL(/\/search\?q=Lantern/);
  await expect(
    page.getByRole("link", { name: /^Lantern Protocol, / }),
  ).toBeVisible();
});

test("the neighborhood graph draws a canvas", async ({ page }) => {
  await openEngram(page, "Lantern Protocol");

  await page.getByRole("button", { name: "Show the neighborhood" }).click();
  // The drawing arrives with a lazy chunk, so the canvas is what says the
  // renderer both loaded and attached.
  await expect(page.locator("canvas").first()).toBeVisible();
  await expect(
    page.getByRole("list", { name: "Connections in this neighborhood" }),
  ).toBeVisible();

  // And the same neighborhood on its own screen, which is a different route
  // reading the same payload.
  await page.getByRole("link", { name: "Open the full view" }).click();
  await expect(page).toHaveURL(/\/graph\?anchor=/);
  await expect(page.locator("canvas").first()).toBeVisible();
});

test("the command palette opens on the keyboard and jumps", async ({
  page,
}) => {
  // ControlOrMeta is Cmd on a Mac and Ctrl elsewhere, which is exactly the pair
  // the palette listens for.
  await page.keyboard.press("ControlOrMeta+k");

  const palette = page.getByPlaceholder(
    "Jump to a domain, or find an engram by title",
  );
  await expect(palette).toBeVisible();

  await palette.fill(DOMAIN);
  await page.getByRole("option", { name: new RegExp(`^${DOMAIN}`) }).click();

  await expect(page).toHaveURL(new RegExp(`/d/${DOMAIN}$`));
  await expect(
    page.getByRole("heading", { name: DOMAIN, level: 1 }),
  ).toBeVisible();
});
