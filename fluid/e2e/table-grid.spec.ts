/**
 * The table grid, in a real browser against a real daemon.
 *
 * What jsdom cannot answer here: a table drawn as a grid of cells, each cell
 * opening a nested editor over the same document; Tab, Escape and Shift+F10 as
 * the browser delivers them; the pointer reaching a hover button on a border;
 * two browsers (and an agent behind the daemon's MCP door) changing cells of
 * one table in one room.
 *
 * Every journey makes its own engram through the REST API, so nothing else in
 * the suite sees a table it did not write, and reads the file back through the
 * REST API to say what ended up on disk. Shares the daemon, the fixture domain
 * and the accounts `run-smoke.sh` sets up; run it the same way
 * (`bash fluid/e2e/run-smoke.sh e2e/table-grid.spec.ts`).
 */

import type { APIRequestContext, Locator, Page } from "@playwright/test";
import { expect, request as playwrightRequest, test } from "@playwright/test";
import { mkdirSync } from "node:fs";
import { join } from "node:path";

const USER = process.env.FLUID_E2E_USER ?? "smoke";
const PASSWORD = process.env.FLUID_E2E_PASSWORD ?? "smoke-password";
const DOMAIN = process.env.FLUID_E2E_DOMAIN ?? "fluid-smoke";
const PEER = process.env.FLUID_E2E_PEER ?? "peer";
const PEER_PASSWORD = process.env.FLUID_E2E_PEER_PASSWORD ?? "peer-password";
const BASE_PATH = process.env.FLUID_E2E_BASE_PATH ?? "";

/** The daemon itself, where the MCP door is (the preview server only proxies /api). */
const DAEMON = process.env.CRYSTALLINE_API_TARGET ?? "http://127.0.0.1:7411";

/** Where the checkpoint screenshots go, when a run asks for them. */
const SHOTS = process.env.FLUID_E2E_SHOTS ?? "";

/** A three by three table with a line right above it for the main view. */
const CREW = [
  "## Crew",
  "| Name | Role | Tide |",
  "| --- | --- | --- |",
  "| Ada | Lead | High |",
  "| Bo | Crew | Low |",
  "| Cy | Cook | Mid |",
  "",
].join("\n");

async function signIn(
  page: Page,
  name: string = USER,
  password: string = PASSWORD,
): Promise<void> {
  await page.goto("./");
  await expect(page).toHaveURL((url) => url.pathname === `${BASE_PATH}/login`);
  await page.getByLabel("Name", { exact: true }).fill(name);
  await page.getByLabel("Password", { exact: true }).fill(password);
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

/** Makes an engram holding `body` and returns its permalink. */
async function createEngram(
  page: Page,
  title: string,
  body: string,
): Promise<string> {
  const response = await page.request.post(
    `${BASE_PATH}/api/v1/domains/${DOMAIN}/engrams`,
    {
      headers: { "X-CSRF-Token": await csrf(page) },
      data: { title, content: body },
    },
  );
  expect(response.ok(), await response.text()).toBeTruthy();
  const made = (await response.json()) as { permalink: string };
  return made.permalink;
}

/** The whole file as the daemon holds it now. */
async function fileOf(page: Page, permalink: string): Promise<string> {
  const response = await page.request.get(
    `${BASE_PATH}/api/v1/domains/${DOMAIN}/engrams/${permalink}`,
  );
  return ((await response.json()) as { content: string }).content;
}

/** The lines of the file that are table rows, trimmed of the line break. */
function tableLines(file: string): string[] {
  return file.split("\n").filter((line) => line.includes("|"));
}

/** Opens the editor on an engram of the fixture domain. */
async function edit(page: Page, permalink: string): Promise<void> {
  await page.goto(`d/${DOMAIN}/e/${permalink}`);
  await page.getByRole("link", { name: "Edit" }).click();
  await expect(page.locator(".cm-grid [role=grid]")).toBeVisible();
}

/** One cell of the drawn table, 1-based like the grid's own aria indexes. */
function cell(page: Page, row: number, column: number): Locator {
  return page.locator(
    `.cm-grid [aria-rowindex="${String(row)}"][aria-colindex="${String(column)}"]`,
  );
}

/** The editor of the cell that is open, if any. */
function openCell(page: Page): Locator {
  return page.locator(".cm-grid .cm-content[contenteditable=true]");
}

/** Saves the solo surface when it has a Save button; a room saves itself. */
async function settle(page: Page): Promise<void> {
  const save = page.getByRole("button", { name: "Save", exact: true });
  if ((await save.count()) > 0 && (await save.isEnabled())) await save.click();
}

/** Polls the file until `check` holds, saving the solo surface on the way. */
async function fileSatisfies(
  page: Page,
  permalink: string,
  check: (file: string) => boolean,
): Promise<string> {
  let last = "";
  try {
    await expect
      .poll(
        async () => {
          await settle(page);
          last = await fileOf(page, permalink);
          return check(last);
        },
        { timeout: 20_000 },
      )
      .toBe(true);
  } catch (error) {
    throw new Error(`the file reads:\n${last}`, { cause: error });
  }
  return last;
}

async function shot(page: Page, name: string): Promise<void> {
  if (!SHOTS) return;
  mkdirSync(SHOTS, { recursive: true });
  await page.screenshot({ path: join(SHOTS, name) });
}

let unique = 0;
function titled(base: string): string {
  unique += 1;
  return `${base} ${String(Date.now())}${String(unique)}`;
}

test.beforeEach(async ({ page }) => {
  await signIn(page);
});

test("typing in cells, Tab to the end and Tab again adds one row", async ({
  page,
}) => {
  const permalink = await createEngram(page, titled("Grid Tab"), CREW);
  await edit(page, permalink);
  await shot(page, "01-grid.png");

  await cell(page, 4, 3).click();
  await expect(openCell(page)).toBeFocused();
  await page.keyboard.press("End");
  await page.keyboard.type(" tide");
  // Tab on the last cell of the last row adds a row and opens its first cell.
  await page.keyboard.press("Tab");
  await expect(cell(page, 5, 1)).toBeVisible();
  await page.keyboard.type("Dee");

  const file = await fileSatisfies(page, permalink, (f) => f.includes("Dee"));
  const rows = tableLines(file);
  // The header, the delimiter, three data rows and the new one.
  expect(rows).toHaveLength(6);
  expect(rows[4]).toMatch(/\| Cy \| Cook \| Mid tide \|/);
  expect(rows[5]).toMatch(/^\|\s*Dee\s*\|\s*\|\s*\|$/);
  expect(rows.slice(0, 4).join("\n")).toBe(
    tableLines(CREW).slice(0, 4).join("\n"),
  );
});

test("keyboard only: into the header, to a cell, edit and align center", async ({
  page,
}) => {
  const permalink = await createEngram(page, titled("Grid Keys"), CREW);
  await edit(page, permalink);

  // The line above the table: click it, ArrowDown enters the header.
  await page
    .locator(".cm-content > .cm-line")
    .filter({ hasText: "Crew" })
    .click();
  await page.keyboard.press("End");
  await page.keyboard.press("ArrowDown");
  await expect(openCell(page)).toBeFocused();
  await expect(cell(page, 1, 1).locator(".cm-content")).toBeVisible();

  // Escape steps out to grid navigation; arrows walk to the Role of Bo.
  await page.keyboard.press("Escape");
  await expect(cell(page, 1, 1)).toBeFocused();
  await page.keyboard.press("ArrowRight");
  await page.keyboard.press("ArrowDown");
  await page.keyboard.press("ArrowDown");
  await expect(cell(page, 3, 2)).toBeFocused();
  await page.keyboard.press("Enter");
  await expect(openCell(page)).toBeFocused();
  await page.keyboard.type("!");

  await page.keyboard.press("Shift+F10");
  const menu = page.getByRole("menu", { name: "Cell actions" });
  await expect(menu).toBeVisible();
  await shot(page, "03-cell-actions-menu.png");
  await menu.getByRole("menuitemradio", { name: "Align center" }).click();

  const file = await fileSatisfies(page, permalink, (f) => f.includes(":-"));
  const rows = tableLines(file);
  expect(rows[1]).toMatch(/^\|\s*-+\s*\|\s*:-+:\s*\|\s*-+\s*\|$/);
  // Nothing else moved: the typed bang and the same other rows.
  expect(rows[0]).toBe(tableLines(CREW)[0]);
  expect(rows[2]).toBe(tableLines(CREW)[2]);
  expect(rows[3]).toMatch(/\| Bo \| Crew! \| Low \|/);
  expect(rows[4]).toBe(tableLines(CREW)[4]);
});

test("the hover plus inserts a column and the new header takes typing", async ({
  page,
}) => {
  const permalink = await createEngram(page, titled("Grid Plus"), CREW);
  await edit(page, permalink);

  const header = cell(page, 1, 1);
  const plus = page.getByRole("button", { name: "Insert column here" });
  // The pointer goes in from the middle of the cell to its right border, so
  // the grid sees a few moves. The box is read again on each try: the page
  // may still shift (the Details rail filling in) between the read and the
  // move, and a single move to a stale box can miss the 8 px border zone.
  await expect(async () => {
    const box = await header.boundingBox();
    if (!box) throw new Error("the header cell has no box");
    await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
    await page.mouse.move(box.x + box.width - 3, box.y + box.height / 2, {
      steps: 4,
    });
    await expect(plus).toBeVisible({ timeout: 1_000 });
  }).toPass({ timeout: 15_000 });
  await shot(page, "04-hover-plus.png");
  await plus.click();

  await expect(openCell(page)).toBeFocused();
  // A new column starts with a placeholder name, selected: typing replaces it.
  await page.keyboard.type("Boat");
  const file = await fileSatisfies(page, permalink, (f) => f.includes("Boat"));
  const rows = tableLines(file);
  expect(rows[0]).toMatch(
    /^\|\s*Name\s*\|\s*Boat\s*\|\s*Role\s*\|\s*Tide\s*\|$/,
  );
  // Every other row gained one empty cell and kept its text.
  expect(rows[2]).toMatch(/^\|\s*Ada\s*\|\s*\|\s*Lead\s*\|\s*High\s*\|$/);
});

test("the quick format bar makes a selected word bold", async ({ page }) => {
  const permalink = await createEngram(page, titled("Grid Bold"), CREW);
  await edit(page, permalink);

  await cell(page, 4, 1).click();
  await expect(openCell(page)).toBeFocused();
  await page.keyboard.press("Home");
  await page.keyboard.press("Shift+End");
  const bar = page.getByRole("toolbar", { name: "Quick format" });
  await expect(bar).toBeVisible();
  await shot(page, "02-quick-format.png");
  await page.keyboard.press("Alt+F10");
  await bar.getByRole("button", { name: "Bold" }).click();

  const file = await fileSatisfies(page, permalink, (f) => f.includes("**"));
  expect(file).toMatch(/\| \*\*Cy\*\* \| Cook \| Mid \|/);

  // Out of the cell, the grid draws it bold.
  await page.keyboard.press("Escape");
  await page.keyboard.press("Escape");
  await expect(cell(page, 4, 1).locator("strong")).toHaveText("Cy");
});

test("Raw with a cell open shows the source and the typed text", async ({
  page,
}) => {
  const permalink = await createEngram(page, titled("Grid Raw"), CREW);
  await edit(page, permalink);

  await cell(page, 2, 1).click();
  await expect(openCell(page)).toBeFocused();
  await page.keyboard.press("End");
  await page.keyboard.type(" Rawwed");

  await page.getByRole("button", { name: "Raw", exact: true }).click();
  await expect(page.locator(".cm-grid")).toHaveCount(0);
  const source = page.getByRole("textbox", { name: "Engram source" });
  await expect(source).toContainText("| Ada Rawwed | Lead | High |");
  await expect(source).toContainText("| --- | --- | --- |");
  // The sticky bar's table buttons are back in Raw.
  await shot(page, "07-raw.png");
});

test("two browsers type in different cells and see each other", async ({
  browser,
}) => {
  const contextA = await browser.newContext();
  const contextB = await browser.newContext();
  const pageA = await contextA.newPage();
  const pageB = await contextB.newPage();
  await signIn(pageA);
  await signIn(pageB, PEER, PEER_PASSWORD);
  const permalink = await createEngram(pageA, titled("Grid Room"), CREW);
  await edit(pageA, permalink);
  await edit(pageB, permalink);

  await cell(pageA, 2, 1).click();
  await expect(openCell(pageA)).toBeFocused();
  await cell(pageB, 4, 3).click();
  await expect(openCell(pageB)).toBeFocused();
  await pageA.keyboard.press("End");
  await pageB.keyboard.press("End");

  await Promise.all([
    pageA.keyboard.type(" alpha"),
    pageB.keyboard.type(" bravo"),
  ]);

  // Identical text in both, and each sees the other's caret marker.
  for (const page of [pageA, pageB]) {
    await expect(cell(page, 2, 1)).toContainText("Ada alpha");
    await expect(cell(page, 4, 3)).toContainText("Mid bravo");
  }
  await expect(pageA.locator(".cm-grid .cm-ySelectionCaret")).not.toHaveCount(
    0,
  );
  await expect(pageB.locator(".cm-grid .cm-ySelectionCaret")).not.toHaveCount(
    0,
  );
  await shot(pageA, "05-two-browsers-a.png");
  await shot(pageB, "05-two-browsers-b.png");

  const file = await fileSatisfies(
    pageA,
    permalink,
    (f) => f.includes("Ada alpha") && f.includes("Mid bravo"),
  );
  expect(tableLines(file)).toHaveLength(5);

  await contextA.close();
  await contextB.close();
});

/** The text of an MCP answer, whether it came as JSON or as one SSE event. */
function mcpJson(text: string): {
  result?: { isError?: boolean; content?: { text: string }[] };
  error?: unknown;
} {
  const data = text
    .split("\n")
    .filter((line) => line.startsWith("data:"))
    .map((line) => line.slice(5).trim())
    .filter((line) => line.length > 0)
    .pop();
  return JSON.parse(data ?? text) as ReturnType<typeof mcpJson>;
}

/** Opens a legacy MCP session on the daemon with a bearer token. */
async function mcpSession(token: string): Promise<{
  call: (name: string, args: object) => Promise<string>;
  close: () => Promise<void>;
}> {
  const api: APIRequestContext = await playwrightRequest.newContext({
    baseURL: DAEMON,
    extraHTTPHeaders: {
      Authorization: `Bearer ${token}`,
      "Content-Type": "application/json",
      Accept: "application/json, text/event-stream",
    },
  });
  const init = await api.post("/", {
    data: {
      jsonrpc: "2.0",
      id: 1,
      method: "initialize",
      params: {
        protocolVersion: "2025-06-18",
        capabilities: {},
        clientInfo: { name: "table-grid-smoke", version: "0.0.0" },
      },
    },
  });
  expect(init.ok(), await init.text()).toBeTruthy();
  const session = init.headers()["mcp-session-id"];
  const headers = session ? { "Mcp-Session-Id": session } : {};
  await api.post("/", {
    headers,
    data: { jsonrpc: "2.0", method: "notifications/initialized" },
  });
  let id = 1;
  return {
    call: async (name, args) => {
      id += 1;
      const answer = await api.post("/", {
        headers,
        data: {
          jsonrpc: "2.0",
          id,
          method: "tools/call",
          params: { name, arguments: args },
        },
      });
      const body = mcpJson(await answer.text());
      const text = body.result?.content?.map((c) => c.text).join("\n") ?? "";
      expect(body.result?.isError ?? false, text).toBe(false);
      return text;
    },
    close: () => api.dispose(),
  };
}

test("an agent changes one cell while a browser types in another of the row", async ({
  page,
}) => {
  const permalink = await createEngram(page, titled("Grid Agent"), CREW);
  const issued = await page.request.post(`${BASE_PATH}/api/v1/me/mcp-tokens`, {
    headers: { "X-CSRF-Token": await csrf(page) },
    data: { label: `table-grid-${String(Date.now())}` },
  });
  expect(issued.ok(), await issued.text()).toBeTruthy();
  const { token } = (await issued.json()) as { token: string };
  const agent = await mcpSession(token);

  await edit(page, permalink);
  // The browser's cell: Role of Ada. The agent takes the Name of the same row.
  await cell(page, 2, 2).click();
  await expect(openCell(page)).toBeFocused();
  await page.keyboard.press("End");
  await page.keyboard.type(" one");

  await agent.call("edit_engram", {
    identifier: permalink,
    domain: DOMAIN,
    operation: "find_replace",
    find_text: "| Ada |",
    content: "| Agent |",
    expected_replacements: 1,
  });
  await page.keyboard.type(" two");

  // The agent's text arrives in the browser, the cell is still open with its
  // caret, and what is typed after the arrival still lands.
  await expect(cell(page, 2, 1)).toContainText("Agent");
  await expect(openCell(page)).toBeFocused();
  await page.keyboard.type(" three");
  await shot(page, "06-agent-and-browser.png");

  const file = await fileSatisfies(
    page,
    permalink,
    (f) => f.includes("Lead one two three") && f.includes("Agent"),
  );
  expect(tableLines(file)).toContain(
    tableLines(file).find((l) => /Agent.*Lead one two three/.test(l)),
  );
  await agent.close();
});

test("a table without outer pipes is drawn, edits and keeps its second column", async ({
  page,
}) => {
  const permalink = await createEngram(
    page,
    titled("Grid Bare"),
    "## Bare\na | b\n--|--\n1 | 2\n",
  );
  await edit(page, permalink);
  await expect(cell(page, 1, 1)).toContainText("a");
  await expect(cell(page, 2, 2)).toContainText("2");

  // A cell edit works, and a mouse click opens even a one-character cell:
  // the small "Cell actions" button is drawn only on the focused or open
  // cell, on its corner, so it never takes a click meant for a cell.
  await cell(page, 1, 1).hover();
  await expect(page.getByRole("button", { name: "Cell actions" })).toBeHidden();
  await cell(page, 1, 1).click();
  await expect(openCell(page)).toBeFocused();
  await expect(cell(page, 1, 1).locator(".cm-content")).toBeVisible();
  await page.keyboard.press("End");
  await page.keyboard.type("x");
  await fileSatisfies(page, permalink, (f) => /^ax \| b$/m.test(f));

  // Deleting the text of the first cell leaves it empty and keeps the 2.
  await page.keyboard.press("Escape");
  await page.keyboard.press("ArrowDown");
  await expect(cell(page, 2, 1)).toBeFocused();
  await page.keyboard.press("Enter");
  await expect(openCell(page)).toBeFocused();
  await page.keyboard.press("ControlOrMeta+A");
  await page.keyboard.press("Backspace");
  await expect(cell(page, 2, 2)).toContainText("2");
  const file = await fileSatisfies(page, permalink, (f) =>
    /^\|\s*\|\s*2$/m.test(f),
  );
  const rows = tableLines(file);
  expect(rows).toHaveLength(3);
  expect(rows[2]).toBe("|  | 2");
  await expect(cell(page, 2, 1)).toHaveText("");
});

test("the sticky bar's Bold acts on the open cell, its line verbs are off", async ({
  page,
}) => {
  const permalink = await createEngram(page, titled("Grid Sticky"), CREW);
  await edit(page, permalink);

  // Main's caret on the heading first, then a cell opened by mouse.
  await page
    .locator(".cm-content > .cm-line")
    .filter({ hasText: "Crew" })
    .first()
    .click();
  await cell(page, 2, 1).click();
  await expect(openCell(page)).toBeFocused();
  // The heading's marks stay folded: main's caret line is not being edited.
  await expect(
    page.locator(".cm-content > .cm-line").filter({ hasText: "Crew" }).first(),
  ).toHaveText("Crew");
  const small = page.getByRole("button", { name: "Cell actions" });
  await expect(small).toBeVisible();
  await expect(
    page.getByRole("toolbar", { name: "Quick format" }),
  ).toBeVisible();
  await shot(page, "09-cell-actions-button.png");

  await page.keyboard.press("Home");
  await page.keyboard.press("Shift+End");
  const sticky = page.getByRole("toolbar", { name: "Formatting" });
  await expect(
    sticky.getByRole("button", { name: "Bulleted list" }),
  ).toBeDisabled();
  await expect(
    sticky.getByRole("button", { name: "Insert", exact: true }),
  ).toBeDisabled();
  await sticky.getByRole("button", { name: "Bold" }).click();

  const file = await fileSatisfies(page, permalink, (f) => f.includes("**"));
  expect(file).toMatch(/\| \*\*Ada\*\* \| Lead \| High \|/);
  expect(file).toMatch(/^## Crew$/m);
  await expect(openCell(page)).toBeFocused();

  // A mouse click on the next cell opens that cell, not the button's menu.
  await cell(page, 2, 2).click();
  await expect(page.getByRole("menu", { name: "Cell actions" })).toHaveCount(0);
  await expect(cell(page, 2, 2).locator(".cm-content")).toBeVisible();
});

test("the grid in the dark theme", async ({ page }) => {
  await page.emulateMedia({ colorScheme: "dark" });
  const permalink = await createEngram(page, titled("Grid Dark"), CREW);
  await edit(page, permalink);
  await cell(page, 3, 2).click();
  await expect(openCell(page)).toBeFocused();
  await expect(page.locator("html")).toHaveAttribute("data-theme", "dark");
  await shot(page, "08-dark.png");
});
