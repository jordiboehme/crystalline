/**
 * The browser smoke: the paths a unit test cannot reach.
 *
 * Everything asserted here needs something jsdom does not have. A real login
 * against a real daemon over a real cookie; a mermaid fence that has to become
 * an SVG rather than a mocked promise; a deep link typed into the address bar,
 * which is the static server's SPA fallback and the app's per-segment permalink
 * encoding meeting for the first time; a search answered by the index rather
 * than by a fixture; a graph canvas that only a browser with layout can draw;
 * and a keyboard shortcut delivered by the browser itself; and two browsers
 * co-editing one engram over a real websocket, which needs two of everything
 * jsdom has one of.
 *
 * The domain behind it is `e2e/fixtures/domain`, copied to a scratch directory
 * and registered by `e2e/run-smoke.sh`, which is also what seeds the accounts
 * these tests sign in with. Every title, permalink and tag asserted on below is
 * a line in one of those files.
 */

import type { Page } from "@playwright/test";
import { expect, test } from "@playwright/test";

/** The account and the domain `run-smoke.sh` set up. Kept in step with it. */
const USER = process.env.FLUID_E2E_USER ?? "smoke";
const PASSWORD = process.env.FLUID_E2E_PASSWORD ?? "smoke-password";
const DOMAIN = process.env.FLUID_E2E_DOMAIN ?? "fluid-smoke";

/** The second account the same script seeds: the other half of a room. */
const PEER = process.env.FLUID_E2E_PEER ?? "peer";
const PEER_PASSWORD = process.env.FLUID_E2E_PEER_PASSWORD ?? "peer-password";

/** The engram three folders down, whose permalink is a path rather than a word. */
const DEEP_PERMALINK = "notes/deep/gamma";

/**
 * Sign in, and land on the home screen.
 *
 * The unauthenticated app redirects to the login screen on its own, so this
 * starts at the root: what it proves on the way past is that the gate holds.
 *
 * The account defaults to the one every other test signs in as, so only the
 * two-browser journey ever passes the peer's.
 */
async function signIn(
  page: Page,
  name: string = USER,
  password: string = PASSWORD,
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

/**
 * The engram page's own title.
 *
 * By its id: the header's h1 is the page's single rendering of the title
 * (a body H1 that repeats it is folded by the reader), and the id is the
 * one the article is labelled by, so it is the title by contract.
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

/**
 * The row of presence chips beside a session buffer.
 *
 * Scoped by the list's own accessible name rather than reached for by the name
 * on a chip, because a display name is not a unique string on this screen: the
 * account is called `smoke` and so are the domain and one of the fixture's
 * tags, and the remote caret carries its owner's name in a hover label that is
 * in the DOM from the moment the caret is. The list is where the room's roster
 * lives, so it is the only place worth asserting a roster against.
 */
function presence(page: Page) {
  return page.getByRole("list", { name: /^In this session:/ });
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
  // The trail is where the address the URL carried is echoed now, one crumb
  // per folder rather than the permalink as one string. Scoped to the screen:
  // the body of this fixture quotes its own permalink in a code span, so an
  // unscoped match would be a race between the two.
  const trail = page
    .locator("main")
    .getByRole("navigation", { name: "Breadcrumb" });
  await expect(trail).toContainText("deep");
  await expect(trail.getByText("Deep Gamma Note")).toBeVisible();
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

test("an engram is created, edited, saved and retired", async ({ page }) => {
  await page
    .locator("main")
    .getByRole("link", { name: DOMAIN, exact: true })
    .click();
  await page.getByRole("button", { name: "New engram" }).click();
  await page.getByLabel("Title").fill("Smoke Journey");
  await page.getByRole("button", { name: "Create" }).click();

  // Straight into the editor; the buffer opens on the minimal document.
  const source = page.getByLabel("Engram source");
  await expect(source).toBeVisible();
  await source.click();
  await page.keyboard.press("ControlOrMeta+End");
  await page.keyboard.type("A smoke-written line.");

  // A tag is a hard requirement (E004), and a freshly created engram carries
  // none: the frontmatter form's own field is what clears it, the same way
  // an author would.
  const addTag = page.getByLabel("Add tag");
  await addTag.fill("smoke");
  await addTag.press("Enter");

  // Save waits for the validation gate to clear, then lands.
  const saveButton = page.getByRole("button", { name: "Save" });
  await expect(saveButton).toBeEnabled();
  await saveButton.click();
  await expect(page.getByText("Saved")).toBeVisible();

  // The read view shows what was written.
  await page.getByRole("link", { name: "Done" }).click();
  await expect(engramTitle(page)).toHaveText("Smoke Journey");
  await expect(page.locator("article")).toContainText("A smoke-written line.");

  // And the guided retirement fades it, from the header's overflow menu:
  // editing is the one control the header carries, everything else is a row
  // in there.
  await page.getByRole("button", { name: "More actions" }).click();
  await page.getByRole("menuitem", { name: "Retire" }).click();
  await page.getByRole("radio", { name: "archived" }).click();
  await page.getByRole("button", { name: "Retire engram" }).click();
  await expect(page.getByRole("note")).toContainText(/archived/i);
});

test("two browsers co-edit one engram and the save lands once", async ({
  browser,
}) => {
  // Two isolated sessions: different accounts, different cookie jars.
  const contextA = await browser.newContext();
  const contextB = await browser.newContext();
  const pageA = await contextA.newPage();
  const pageB = await contextB.newPage();
  await signIn(pageA);
  await signIn(pageB, PEER, PEER_PASSWORD);

  // "Tide Tables" is a fixture engram (permalink tide-tables) nothing else in
  // the suite mutates, so this test owns its content. The copy under edit is
  // the scratch one `run-smoke.sh` made; the checked-in fixture is untouched.
  await openEngram(pageA, "Tide Tables");
  await pageA.getByRole("link", { name: "Edit" }).click();
  await openEngram(pageB, "Tide Tables");
  await pageB.getByRole("link", { name: "Edit" }).click();

  const editorA = pageA.getByRole("textbox", { name: "Engram source" });
  const editorB = pageB.getByRole("textbox", { name: "Engram source" });
  await expect(editorA).toBeVisible();
  await expect(editorB).toBeVisible();

  // Presence: each side lists the other by display name, and BOTH remote
  // cursors are visible - the spec's own wording. A cursor renders on the
  // OTHER browser once its owner focuses the buffer.
  await expect(presence(pageA).getByText(PEER, { exact: true })).toBeVisible();
  await expect(presence(pageB).getByText(USER, { exact: true })).toBeVisible();
  await editorA.click();
  await expect(pageB.locator(".cm-ySelectionCaret")).toBeVisible();
  await editorB.click();
  await expect(pageA.locator(".cm-ySelectionCaret")).toBeVisible();

  // The version on disk before anyone types, read now rather than after the
  // edit: the server saves a room on its own debounce, so a "before" taken
  // once the text had already moved would be racing that timer for which
  // checksum it caught.
  const detail = () =>
    pageA.request
      .get(`/api/v1/domains/${DOMAIN}/engrams/tide-tables`)
      .then(
        (response) =>
          response.json() as Promise<{ checksum: string; content: string }>,
      );
  const before = await detail();

  // An edit from A appears at B without B doing anything. Typed at the
  // document END - text ahead of the frontmatter would trip the save gate -
  // placed deterministically on every platform: select-all, then ArrowRight
  // collapses the selection to the end (macOS has no reliable Ctrl+End).
  await editorA.click();
  await pageA.keyboard.press("ControlOrMeta+A");
  await pageA.keyboard.press("ArrowRight");
  await editorA.pressSequentially("smoke-collab line");
  await expect(editorB).toContainText("smoke-collab line");

  // The save lands once. Do NOT assert on the "Saved" text: saveState starts
  // "ok", so "Saved" renders before any save ever runs. The served detail is
  // the truth: the checksum moves off the pre-edit value, the file carries the
  // typed line exactly once - two copies would be the shared document applied
  // twice - and then everything holds steady while the room idles past its
  // debounce.
  await pageA.getByRole("button", { name: "Save" }).click();
  await expect
    .poll(async () => (await detail()).content)
    .toContain("smoke-collab line");
  const first = await detail();
  expect(first.checksum).not.toBe(before.checksum);
  expect(first.content.split("smoke-collab line").length - 1).toBe(1);
  await pageA.waitForTimeout(3000); // longer than the server debounce
  const second = await detail();
  expect(second.checksum).toBe(first.checksum);
  expect(second.content).toBe(first.content);

  await contextA.close();
  await contextB.close();
});
