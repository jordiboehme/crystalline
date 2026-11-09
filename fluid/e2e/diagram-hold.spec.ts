/**
 * A mermaid fence the caret is in stays text (R52) and, once the pointer
 * moves the caret away, is drawn as the diagram box. What jsdom cannot answer:
 * the draw changes the page height, so it must not happen between the press
 * and the release of a click, or under the second press of a double click.
 *
 * Shares the daemon, the fixture domain and the accounts `run-smoke.sh` sets
 * up; run it the same way (`bash fluid/e2e/run-smoke.sh e2e/diagram-hold.spec.ts`).
 */

import type { Locator, Page } from "@playwright/test";
import { expect, test } from "@playwright/test";

const USER = process.env.FLUID_E2E_USER ?? "smoke";
const PASSWORD = process.env.FLUID_E2E_PASSWORD ?? "smoke-password";
const DOMAIN = process.env.FLUID_E2E_DOMAIN ?? "fluid-smoke";
const BASE_PATH = process.env.FLUID_E2E_BASE_PATH ?? "";

/** A fence mermaid refuses (the last arrow has no end): it stays text until fixed. */
const FENCE = "```mermaid\nflowchart TD\n  A --> B\n  B --> C\n  C -->\n```";
const FILLER = Array.from(
  { length: 40 },
  (_, i) => `Filler line ${String(i + 1)}`,
).join("\n\n");

let unique = 0;
const made: string[] = [];

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

async function openEditor(page: Page): Promise<void> {
  const me = await page.request.get(`${BASE_PATH}/api/v1/auth/me`);
  const csrf = ((await me.json()) as { csrf: string }).csrf;
  unique += 1;
  const response = await page.request.post(
    `${BASE_PATH}/api/v1/domains/${DOMAIN}/engrams`,
    {
      headers: { "X-CSRF-Token": csrf },
      data: {
        title: `Diagram hold ${String(Date.now())}${String(unique)}`,
        content: `${FENCE}\n\n${FILLER}\n`,
      },
    },
  );
  expect(response.ok(), await response.text()).toBeTruthy();
  const { permalink } = (await response.json()) as { permalink: string };
  made.push(permalink);
  await page.goto(`d/${DOMAIN}/e/${permalink}`);
  await page.getByRole("link", { name: "Edit" }).click();
  await expect(line(page, "Filler line 2")).toBeVisible();
}

function line(page: Page, text: string): Locator {
  return page.locator(".cm-content > .cm-line", {
    hasText: new RegExp(`^${text}$`),
  });
}

async function top(locator: Locator): Promise<number> {
  const box = await locator.boundingBox();
  if (box === null) throw new Error("line is not on the page");
  return box.y;
}

/** What the browser has selected, and the line it starts in. */
async function selected(page: Page): Promise<{ text: string; line: string }> {
  return page.evaluate(() => {
    const selection = window.getSelection();
    const node = selection?.anchorNode;
    const element = node instanceof Element ? node : node?.parentElement;
    return {
      text: selection?.toString() ?? "",
      line: element?.closest(".cm-line")?.textContent ?? "",
    };
  });
}

test.beforeEach(async ({ page }) => {
  await signIn(page);
  await openEditor(page);
  // Fix the fence by typing at the end of its last line: the verdict turns ok
  // with the caret in the fence, which stays text however long the check takes.
  await expect(page.locator(".cm-diagram")).toHaveCount(0);
  await page.locator(".cm-content > .cm-line", { hasText: "C -->" }).click();
  await page.keyboard.press("End");
  await page.keyboard.type(" D");
  await page.waitForTimeout(2500);
  await expect(page.locator(".cm-diagram")).toHaveCount(0);
  await expect(page.locator(".cm-content")).toContainText("C --> D");
});

test.afterEach(async ({ page }) => {
  const me = await page.request.get(`${BASE_PATH}/api/v1/auth/me`);
  const token = ((await me.json()) as { csrf: string }).csrf;
  for (const permalink of made.splice(0)) {
    const at = `${BASE_PATH}/api/v1/domains/${DOMAIN}/engrams/${permalink}`;
    // Named by version (If-Match); a room may still be saving its last change.
    await expect
      .poll(
        async () => {
          const detail = await page.request.get(at);
          const removed = await page.request.delete(at, {
            headers: {
              "X-CSRF-Token": token,
              "If-Match": detail.headers()["etag"] ?? "",
            },
          });
          return removed.status();
        },
        { timeout: 15_000 },
      )
      .toBeLessThan(300);
  }
});

test("a press below a held fence does not move the page under the pointer", async ({
  page,
}) => {
  const target = line(page, "Filler line 2");
  // On screen first, so the press lands on it and scrolling is not the shift.
  await target.scrollIntoViewIfNeeded();
  const before = await top(target);
  const box = await target.boundingBox();
  if (box === null) throw new Error("no box");
  const x = box.x + 12;
  const y = box.y + box.height / 2;
  await page.mouse.move(x, y);
  await page.mouse.down();
  expect(await top(target)).toBe(before);
  await page.mouse.move(x + 2, y);
  expect(await top(target)).toBe(before);
  await page.mouse.up();
  // The caret is a click's, on the pressed line: typed text lands there and
  // replaces nothing (a selection of nine lines would have swallowed it).
  await page.keyboard.type("Z");
  const typed = page.locator(".cm-content > .cm-line", { hasText: "Z" });
  await expect(typed).toHaveCount(1);
  expect((await typed.textContent())?.replace("Z", "")).toBe("Filler line 2");
  await expect(line(page, "Filler line 3")).toHaveCount(1);
  // Then the fence is drawn.
  await expect(page.locator(".cm-diagram")).toHaveCount(1);
});

test("a double click below a held fence selects the word pressed", async ({
  page,
}) => {
  const target = line(page, "Filler line 3");
  await target.dblclick({ position: { x: 12, y: 8 } });
  const picked = await selected(page);
  expect(picked.line).toBe("Filler line 3");
  expect(picked.text.trim()).toBe("Filler");
  await expect(page.locator(".cm-diagram")).toHaveCount(1);
});
