/**
 * Editing a sequence diagram by its participants and messages, from the first
 * click to the file.
 *
 * Every journey that writes holds the file to its exact bytes through the REST
 * API: what the page shows is not the question, what was saved is. The diagram
 * has a comment, an undeclared participant, a block and odd spacing, so a write
 * that tidied anything shows.
 *
 * Shares the daemon, the fixture domain and the accounts `run-smoke.sh` sets up;
 * run it the same way (`bash fluid/e2e/run-smoke.sh e2e/diagram-sequence.spec.ts`).
 */

import type { Page } from "@playwright/test";
import { expect, test } from "@playwright/test";

const USER = process.env.FLUID_E2E_USER ?? "smoke";
const PASSWORD = process.env.FLUID_E2E_PASSWORD ?? "smoke-password";
const DOMAIN = process.env.FLUID_E2E_DOMAIN ?? "fluid-smoke";
const BASE_PATH = process.env.FLUID_E2E_BASE_PATH ?? "";

const BODY = [
  "Text above the sequence.",
  "```mermaid",
  "sequenceDiagram",
  "  %% keep this comment",
  "  participant caller as Caller",
  "  participant service   as Service",
  "  caller->>service: Request",
  "  loop every minute",
  "    service-->>store: Read",
  "  end",
  "  service-->>caller:   Reply",
  "```",
  "",
  "Text at the end.",
  "",
].join("\n");

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

async function csrf(page: Page): Promise<string> {
  const me = await page.request.get(`${BASE_PATH}/api/v1/auth/me`);
  return ((await me.json()) as { csrf: string }).csrf;
}

/** A new engram holding `content`; its permalink. */
async function scratch(page: Page, content: string): Promise<string> {
  const token = await csrf(page);
  unique += 1;
  const response = await page.request.post(
    `${BASE_PATH}/api/v1/domains/${DOMAIN}/engrams`,
    {
      headers: { "X-CSRF-Token": token },
      data: {
        title: `Sequence journey ${String(Date.now())}${String(unique)}`,
        tags: ["smoke"],
        content,
      },
    },
  );
  expect(response.ok(), await response.text()).toBeTruthy();
  const { permalink } = (await response.json()) as { permalink: string };
  made.push(permalink);
  return permalink;
}

async function edit(page: Page, permalink: string): Promise<void> {
  await page.goto(`d/${DOMAIN}/e/${permalink}`);
  await page.getByRole("link", { name: "Edit" }).click();
  await expect(page.locator(".cm-diagram svg").first()).toBeVisible();
}

/** The file as the server holds it. */
async function saved(page: Page, permalink: string): Promise<string> {
  const response = await page.request.get(
    `${BASE_PATH}/api/v1/domains/${DOMAIN}/engrams/${permalink}`,
  );
  return ((await response.json()) as { content: string }).content;
}

/** The file reaches exactly `expected`; the room saves on its own debounce. */
async function savedIs(
  page: Page,
  permalink: string,
  expected: string,
): Promise<void> {
  await expect
    .poll(() => saved(page, permalink), { timeout: 20_000 })
    .toBe(expected);
}

/** `text` with `from` swapped once. A `from` that is not there once is a test bug. */
function swap(text: string, from: string, to: string): string {
  expect(text.split(from).length - 1, `one "${from}"`).toBe(1);
  return text.replace(from, () => to);
}

test.beforeEach(async ({ page }) => {
  await signIn(page);
});

test.afterEach(async ({ page }) => {
  // Out of the editor first, so its room can end.
  await page.goto("./");
  const token = await csrf(page);
  for (const permalink of made.splice(0)) {
    const at = `${BASE_PATH}/api/v1/domains/${DOMAIN}/engrams/${permalink}`;
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

const field = (page: Page) => page.locator(".cm-diagram-label-field");
const participant = (page: Page, name: string) =>
  page.locator(`.cm-diagram [data-et="participant"][data-id="${name}"]`);
const focusedLabel = (page: Page) =>
  page.evaluate(() => document.activeElement?.getAttribute("aria-label") ?? "");
const focusedText = (page: Page) =>
  page.evaluate(() => document.activeElement?.textContent ?? "");

/** The text of the line the caret stands on. */
const caretLine = (page: Page) =>
  page.evaluate(() => {
    const content = document.querySelector(".cm-content") as unknown as {
      cmTile: {
        view: {
          state: {
            doc: { lineAt(at: number): { text: string } };
            selection: { main: { head: number } };
          };
        };
      };
    };
    const { state } = content.cmTile.view;
    return state.doc.lineAt(state.selection.main.head).text;
  });

test("the drawing is one group named Sequence diagram, and the text stays hidden", async ({
  page,
}) => {
  const permalink = await scratch(page, BODY);
  await edit(page, permalink);
  await expect(
    page.getByRole("group", { name: "Sequence diagram" }),
  ).toBeVisible();
  await expect(page.locator(".cm-content")).not.toContainText(
    "sequenceDiagram",
  );
});

test("a click on a participant, a name and Enter change only its label", async ({
  page,
}) => {
  const permalink = await scratch(page, BODY);
  await edit(page, permalink);
  const before = await saved(page, permalink);
  expect(before.startsWith("---\n")).toBe(true);
  await participant(page, "caller").click();
  await expect(field(page)).toHaveValue("Caller");
  await expect(field(page)).toHaveAccessibleName("Participant name");
  await page.keyboard.type("Client");
  await page.keyboard.press("Enter");
  await expect(field(page)).toHaveCount(0);
  await savedIs(
    page,
    permalink,
    swap(
      before,
      "participant caller as Caller",
      "participant caller as Client",
    ),
  );
});

test("by keys alone: Add message to..., the next participant, Enter, a text and Enter append one message", async ({
  page,
}) => {
  const permalink = await scratch(page, BODY);
  await edit(page, permalink);
  const before = await saved(page, permalink);
  await page.locator(".cm-content").focus();
  for (let i = 0; i < 40; i += 1) {
    if ((await caretLine(page)) === "Text above the sequence.") {
      break;
    }
    await page.keyboard.press("ArrowDown");
  }
  expect(await caretLine(page)).toBe("Text above the sequence.");
  await page.keyboard.press("ArrowDown");
  await page.keyboard.press("ArrowRight");
  expect(await focusedLabel(page)).toBe("Participant: Caller");
  await page.keyboard.press("Shift+F10");
  await expect(
    page.getByRole("menu", { name: "Participant actions" }),
  ).toBeVisible();
  for (let i = 0; i < 6; i += 1) {
    if ((await focusedText(page)) === "Add message to...") {
      break;
    }
    await page.keyboard.press("ArrowDown");
  }
  expect(await focusedText(page)).toBe("Add message to...");
  await page.keyboard.press("Enter");
  await expect(
    page
      .getByRole("group", { name: "Sequence diagram" })
      .getByRole("status")
      .filter({
        hasText:
          "Choose the participant to send the message to. Press Escape to cancel.",
      }),
  ).toBeVisible();
  await page.keyboard.press("ArrowRight");
  expect(await focusedLabel(page)).toBe("Participant: Service");
  await page.keyboard.press("Enter");
  await expect(field(page)).toHaveValue("New message");
  await expect(field(page)).toHaveAccessibleName("Message text");
  await page.keyboard.type("Ping");
  await page.keyboard.press("Enter");
  await expect(field(page)).toHaveCount(0);
  await savedIs(
    page,
    permalink,
    swap(
      before,
      "  service-->>caller:   Reply\n```",
      "  service-->>caller:   Reply\n  caller->>service: Ping\n```",
    ),
  );
});
