/**
 * A Reflection page's details column against a real daemon (design 4): a
 * curator opens the settings behind Edit and sets a freshness from a preset
 * and a custom value, the line under the title reads the page clock and
 * names the pacts, the page pact is created and joins that line, the page
 * is retired from the More actions menu (the banner says so and the
 * navigation hides it behind its switch), restored from the banner, and
 * deleted from the menu with the two-step confirm.
 *
 * WebKit runs it too: the confirm closes itself when focus leaves it, and
 * Safari does not focus a button on a click (see `focusLeft.ts`), so only
 * WebKit can show a confirm that shuts before its second press lands.
 *
 * Every page here is its own, made through the API and removed at the end,
 * so the journeys of `reflection.spec.ts` on the same domain never see it.
 */

import type { Page } from "@playwright/test";
import { expect, test } from "@playwright/test";

const USER = process.env.FLUID_E2E_USER ?? "smoke";
const PASSWORD = process.env.FLUID_E2E_PASSWORD ?? "smoke-password";
const DOMAIN = process.env.FLUID_E2E_REFLECTION_DOMAIN ?? "smoke-pages";
const BASE_PATH = process.env.FLUID_E2E_BASE_PATH ?? "";

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

/** The session's CSRF token, which a write through `page.request` echoes. */
async function csrf(page: Page): Promise<string> {
  const me = await page.request.get(`${BASE_PATH}/api/v1/auth/me`);
  return ((await me.json()) as { csrf: string }).csrf;
}

function api(path: string): string {
  return `${BASE_PATH}/api/v1/domains/${DOMAIN}/reflections/${path}`;
}

/** The page view the server reads for `path`. */
async function pageView(
  page: Page,
  path: string,
): Promise<{ freshness: string | null; retired: unknown }> {
  const response = await page.request.get(api(path));
  return (
    (await response.json()) as {
      page: { freshness: string | null; retired: unknown };
    }
  ).page;
}

test("a curator edits a page's settings, makes its pact, retires, restores and deletes it", async ({
  page,
}, info) => {
  await signIn(page);
  const name = `panel-${info.project.name}-${String(Date.now())}`;
  const path = `settings/${name}.md`;
  const title = `Panel ${info.project.name}`;
  const written = await page.request.put(api(path), {
    headers: { "X-CSRF-Token": await csrf(page) },
    data: { markdown: `# ${title}\n\nText.\n`, create: true },
  });
  expect(written.ok(), await written.text()).toBeTruthy();
  try {
    await page.goto(`d/${DOMAIN}/reflection/${path}`);
    const panel = page.getByRole("complementary", { name: "Page details" });
    await expect(panel).toBeVisible();
    const status = page.getByTestId("page-status");
    await expect(panel).toContainText("No page, only PACT.md");
    const edit = panel.getByRole("button", { name: "Edit page settings" });

    // A preset: the line under the title reads the clock, and the view
    // comes back with the focus on Edit.
    await edit.click();
    await panel.getByRole("combobox", { name: "Freshness" }).selectOption("6h");
    await panel.getByRole("button", { name: "Save", exact: true }).click();
    await expect(status).toContainText(/Last checked .*, due again in /);
    await expect(edit).toBeFocused();
    await expect(panel.getByRole("status")).toHaveText("Saved");
    await expect(panel).toContainText("6 hours");
    await expect
      .poll(async () => (await pageView(page, path)).freshness)
      .toBe("6h");

    // A custom value, with the note that m means months. A refusal keeps
    // the form open.
    await edit.click();
    await panel
      .getByRole("combobox", { name: "Freshness" })
      .selectOption("custom");
    const custom = panel.getByRole("textbox", { name: "Custom freshness" });
    await expect(custom).toHaveAccessibleDescription(/m means months/);
    await custom.fill("0h");
    await panel.getByRole("button", { name: "Save", exact: true }).click();
    await expect(panel.getByRole("alert")).toBeVisible();
    await custom.fill("2w");
    await panel.getByRole("button", { name: "Save", exact: true }).click();
    await expect(edit).toBeFocused();
    await expect
      .poll(async () => (await pageView(page, path)).freshness)
      .toBe("2w");

    // Cancel and Escape leave the settings as they are.
    await edit.click();
    await panel.getByRole("combobox", { name: "Freshness" }).selectOption("1d");
    await panel.getByRole("button", { name: "Cancel" }).click();
    await expect(edit).toBeFocused();
    await edit.click();
    await page.keyboard.press("Escape");
    await expect(edit).toBeFocused();
    await expect(panel).toContainText("2 weeks");

    // The page pact: made and opened in the editor, then named last in the
    // line under the title.
    await panel.getByRole("button", { name: "Create the page pact" }).click();
    await expect(page).toHaveURL(
      (url) =>
        url.pathname ===
        `${BASE_PATH}/d/${DOMAIN}/reflection-edit/settings/${name}.pact.md`,
    );
    await page.goto(`d/${DOMAIN}/reflection/${path}`);
    await expect(
      panel.getByRole("link", { name: `${name}.pact.md` }),
    ).toBeVisible();
    // The fixture domain may have no PACT.md: the line names only the pact
    // files that exist, so the page's own pact is last either way.
    await expect(status).toContainText(
      new RegExp(`Pacts: (PACT\\.md > )?${name}\\.pact\\.md`),
    );

    // Mark as not fresh from the menu: the status line says so.
    const more = page.getByRole("button", { name: "More actions" });
    await more.click();
    await page.getByRole("menuitem", { name: "Mark as not fresh" }).click();
    await expect(
      page.getByText(
        "Marked as not fresh. The next reflect run checks it again.",
      ),
    ).toBeVisible();
    await expect(status).toContainText("Not fresh: marked by");

    // Retire from the menu: the banner says so, the navigation hides the
    // page behind its switch, and the banner restores it.
    await more.click();
    await page.getByRole("menuitem", { name: "Retire" }).click();
    const banner = page.getByRole("note", { name: "Retired" });
    await expect(banner).toHaveText(/^Retired/);
    // The focus goes to the banner's Restore.
    await expect(
      banner.getByRole("button", { name: "Restore", exact: true }),
    ).toBeFocused();
    await page.goto(`d/${DOMAIN}/reflection`);
    const nav = page.getByRole("navigation", {
      name: `Reflection of ${DOMAIN}`,
    });
    await expect(nav.getByRole("link", { name: "Overview" })).toBeVisible();
    const toggle = nav.getByRole("switch", { name: "Show retired pages" });
    await expect(toggle).toHaveAttribute("aria-checked", "false");
    // Hidden, and its folder with it (nothing else lives there).
    await expect(nav.getByRole("link", { name: title })).toHaveCount(0);
    await toggle.click();
    await expect(toggle).toHaveAttribute("aria-checked", "true");
    const expand = nav.getByRole("button", { name: "Expand settings" });
    if (await expand.isVisible()) {
      await expand.click();
    }
    await nav.getByRole("link", { name: title }).click();
    await banner.getByRole("button", { name: "Restore", exact: true }).click();
    await expect(banner).toHaveCount(0);
    // The banner went with its button: More actions, where Retire lives,
    // takes the focus.
    await expect(more).toBeFocused();

    // Delete from the menu: the first press asks, Keep gives the press
    // back, and the second press means it.
    await more.click();
    await page.getByRole("menuitem", { name: "Delete" }).click();
    await page.getByRole("button", { name: "Keep" }).click();
    await expect(more).toBeFocused();
    await expect(
      page.getByRole("button", { name: "Confirm delete" }),
    ).toHaveCount(0);
    await more.click();
    await page.getByRole("menuitem", { name: "Delete" }).click();
    await page.getByRole("button", { name: "Confirm delete" }).click();
    await expect(page).toHaveURL(
      (url) => url.pathname === `${BASE_PATH}/d/${DOMAIN}/reflection`,
    );
    await expect
      .poll(async () => (await page.request.get(api(path))).status())
      .toBe(404);
  } finally {
    const token = await csrf(page);
    for (const leftover of [path, `settings/${name}.pact.md`]) {
      const response = await page.request.get(api(leftover));
      if (response.ok()) {
        await page.request.delete(api(leftover), {
          headers: { "X-CSRF-Token": token },
        });
      }
    }
  }
});
