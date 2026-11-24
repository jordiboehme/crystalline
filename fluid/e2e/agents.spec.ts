/**
 * Administration > Agents against a real daemon. The smoke daemon runs with
 * `auth.mcp` off, so what a browser can prove here is the path from the
 * server's own setting to the page: the note says agents need it, the add
 * button is held back with that reason, and pressing it makes nothing.
 *
 * Making, issuing, revoking and removing are pinned by the unit tests and the
 * server's route tests; a journey through them needs a daemon started with
 * `auth.mcp` on.
 *
 * Run it the way `run-smoke.sh` runs every spec (`bash fluid/e2e/run-smoke.sh
 * e2e/agents.spec.ts`), and once more with `FLUID_E2E_BASE_PATH=/crystalline`.
 */

import type { Page } from "@playwright/test";
import { expect, test } from "@playwright/test";

const USER = process.env.FLUID_E2E_USER ?? "smoke";
const BASE_PATH = process.env.FLUID_E2E_BASE_PATH ?? "";
const PASSWORD = process.env.FLUID_E2E_PASSWORD ?? "smoke-password";

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

test.describe("the agents page", () => {
  test.use({ viewport: { width: 1280, height: 800 } });

  test("says agents need auth.mcp and makes none while it is off", async ({
    page,
  }) => {
    const name = `runner${Date.now().toString(36)}`;
    await signIn(page);
    await page.goto("admin/agents");
    await expect(
      page.getByRole("heading", { name: "Agents", level: 1 }),
    ).toBeVisible();

    const note = page.getByRole("note", { name: "auth.mcp is off" });
    await expect(note).toContainText("Agents work only with auth.mcp on");
    await expect(note).toContainText("restart the server");
    await expect(page.getByText("No agents yet.")).toBeVisible();

    const add = page.getByRole("button", { name: "Add agent" });
    await expect(add).toHaveAttribute("aria-disabled", "true");
    await expect(add).toHaveAccessibleDescription(
      "Agents work only with auth.mcp on, and it is off on this instance.",
    );
    await page.getByLabel("Name", { exact: true }).fill(name);
    // Held back, not removed: a press reaches it and does nothing.
    await add.click({ force: true });

    await page.reload();
    await expect(page.getByText("No agents yet.")).toBeVisible();
    await expect(page.getByText(name)).toHaveCount(0);
  });
});
