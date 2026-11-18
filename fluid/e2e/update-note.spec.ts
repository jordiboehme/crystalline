/**
 * The update note in the avatar menu (spec S28-S31), in a real browser.
 *
 * The smoke daemon never checks for updates, so the probe answer is the real
 * one with `update_available` added on the way to the page. What only a
 * browser can show is the chip in both themes and in the bottom sheet of a
 * phone, and the popover opening and closing by keyboard with the focus back
 * on the avatar and the page left clickable.
 */

import type { Page } from "@playwright/test";
import { expect, test } from "@playwright/test";

const USER = process.env.FLUID_E2E_USER ?? "smoke";
const PASSWORD = process.env.FLUID_E2E_PASSWORD ?? "smoke-password";
const BASE_PATH = process.env.FLUID_E2E_BASE_PATH ?? "";

/** Sign in as the smoke admin and land on the home screen. */
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

/** Answer every probe with the real answer plus a newer version. */
async function announceUpdate(page: Page): Promise<void> {
  await page.route(
    (url) => url.pathname.endsWith("/api/v1/auth/me"),
    async (route) => {
      const response = await route.fetch();
      const body = (await response.json()) as Record<string, unknown>;
      await route.fulfill({
        response,
        json: { ...body, update_available: "0.35.0" },
      });
    },
  );
}

test("the update chip opens the note and the keyboard closes it", async ({
  page,
}) => {
  await announceUpdate(page);
  await signIn(page);
  const avatar = page.locator("header button.avatar");
  const chip = page.getByRole("menuitem", {
    name: "Crystalline 0.35.0 available",
  });

  const note = page.getByRole("dialog", {
    name: "Crystalline 0.35.0 available",
  });

  // The chip and the note in both themes.
  for (const scheme of ["light", "dark"] as const) {
    await page.emulateMedia({ colorScheme: scheme });
    await avatar.click();
    await expect(chip).toBeVisible();
    await expect(chip).toContainText("0.35.0 available");
    await chip.click();
    await expect(note).toBeVisible();
    await expect(note).toContainText("is available.");
    await page.keyboard.press("Escape");
    await expect(note).toBeHidden();
    await expect(avatar).toBeFocused();
  }

  // By keyboard: open the menu, choose the chip, close the note.
  await avatar.focus();
  await page.keyboard.press("Enter");
  await expect(chip).toBeFocused();
  await page.keyboard.press("Enter");
  await expect(note).toBeVisible();
  await expect(note).toContainText("is available.");
  await expect(
    note.getByRole("link", { name: /^Release notes/ }),
  ).toHaveAttribute(
    "href",
    "https://github.com/jordiboehme/crystalline/releases/tag/v0.35.0",
  );
  await expect(note).toContainText("crystalline update now");
  await expect(note).toContainText("The update check runs on the server.");
  await page.keyboard.press("Escape");
  await expect(note).toBeHidden();
  await expect(avatar).toBeFocused();
  await expect(page.locator("body")).not.toHaveCSS("pointer-events", "none");

  // On a phone the menu is a bottom sheet; the chip is in it and the note
  // fits the window.
  await page.setViewportSize({ width: 390, height: 780 });
  await avatar.click();
  await expect(chip).toBeVisible();
  await chip.click();
  await expect(note).toBeVisible();
  const box = await note.boundingBox();
  expect(box).not.toBeNull();
  expect((box?.x ?? -1) >= 0).toBe(true);
  expect((box?.x ?? 0) + (box?.width ?? 0) <= 390).toBe(true);
  await page.keyboard.press("Escape");
  await expect(note).toBeHidden();
  await expect(avatar).toBeFocused();
});

test("no update in the probe draws no chip", async ({ page }) => {
  await signIn(page);
  await page.locator("header button.avatar").click();
  await expect(
    page.getByRole("menuitem", { name: "Maintenance" }),
  ).toBeVisible();
  await expect(page.getByRole("menuitem", { name: /available$/ })).toHaveCount(
    0,
  );
});
