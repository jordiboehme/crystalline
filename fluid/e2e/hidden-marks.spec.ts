/**
 * Hidden Markdown marks, in a real browser against a real daemon.
 *
 * What jsdom cannot answer here: where a real click puts the caret beside a
 * hidden `**`, what the browser's own input path does when typing goes on
 * after a bold word, popovers opened by a real press and closed by a real
 * Enter, the task box under a real Space and a Mac Ctrl+click, the clipboard
 * the browser fills on a copy, the worker that numbers a long document's
 * footnotes, and two browsers typing in one room.
 *
 * Every journey makes its own engram through the REST API from the body of
 * the fixture engram `buoy-marks`, so nothing else in the suite sees what it
 * wrote, and reads the file back through the REST API to say what ended up on
 * disk. The Reflection journey writes a page of its own and deletes it again.
 * Shares the daemon, the fixture domains and the accounts `run-smoke.sh` sets
 * up; run it the same way (`bash fluid/e2e/run-smoke.sh
 * e2e/hidden-marks.spec.ts`).
 *
 * Two optional parts run only when asked: `FLUID_E2E_SHOTS=<folder>` takes
 * the checkpoint screenshots, and `FLUID_E2E_MEASURE=<file>` runs the
 * timing measurements and appends their numbers to that file as JSON lines.
 * Neither asserts a timing: load would make that flaky.
 */

import type {
  Browser,
  BrowserContext,
  CDPSession,
  Locator,
  Page,
} from "@playwright/test";
import { devices, expect, test } from "@playwright/test";
import { appendFileSync, mkdirSync, readFileSync } from "node:fs";
import { join } from "node:path";

const USER = process.env.FLUID_E2E_USER ?? "smoke";
const PASSWORD = process.env.FLUID_E2E_PASSWORD ?? "smoke-password";
const DOMAIN = process.env.FLUID_E2E_DOMAIN ?? "fluid-smoke";
const PEER = process.env.FLUID_E2E_PEER ?? "peer";
const PEER_PASSWORD = process.env.FLUID_E2E_PEER_PASSWORD ?? "peer-password";
const REFLECTION_DOMAIN =
  process.env.FLUID_E2E_REFLECTION_DOMAIN ?? "smoke-pages";
const BASE_PATH = process.env.FLUID_E2E_BASE_PATH ?? "";

/** Where the checkpoint screenshots go, when a run asks for them. */
const SHOTS = process.env.FLUID_E2E_SHOTS ?? "";
/** Where the measurements are appended, when a run asks for them. */
const MEASURE = process.env.FLUID_E2E_MEASURE ?? "";

const SOURCE = "Engram source";
const PAGE_SOURCE = "Page source";
const SETTING = "Show Markdown marks on the line I edit";

/** The fixture's body: everything after its frontmatter. */
function bodyOf(file: string): string {
  const match = /^---\n[\s\S]*?\n---\n\n?/.exec(file);
  return match ? file.slice(match[0].length) : file;
}

const BUOY = bodyOf(
  readFileSync(
    new URL("./fixtures/domain/buoy-marks.md", import.meta.url),
    "utf8",
  ),
);

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
  body: string = BUOY,
): Promise<string> {
  const response = await page.request.post(
    `${BASE_PATH}/api/v1/domains/${DOMAIN}/engrams`,
    {
      headers: { "X-CSRF-Token": await csrf(page) },
      // A tag, so the engram carries no finding and the editor no banner.
      data: { title, content: body, tags: ["marks"] },
    },
  );
  expect(response.ok(), await response.text()).toBeTruthy();
  const { permalink } = (await response.json()) as { permalink: string };
  made.push(permalink);
  return permalink;
}

/**
 * The engrams this file made, deleted after each test: the specs after this
 * one open fixture engrams from the domain's list, which shows the newest
 * first, one page at a time.
 */
const made: string[] = [];

test.afterEach(async ({ page }) => {
  if (made.length === 0) return;
  // Off any editor first, so no room of this page holds an engram open.
  if (page.url().startsWith("http")) await page.goto("./");
  else await signIn(page);
  const token = await csrf(page);
  for (const permalink of made.splice(0)) {
    const at = `${BASE_PATH}/api/v1/domains/${DOMAIN}/engrams/${permalink}`;
    // The delete names the version it removes (If-Match, the detail's ETag),
    // read again on each try: a room may still be saving its last change.
    await expect
      .poll(
        async () => {
          const detail = await page.request.get(at);
          const etag = detail.headers()["etag"] ?? "";
          const removed = await page.request.delete(at, {
            headers: { "X-CSRF-Token": token, "If-Match": etag },
          });
          return removed.status();
        },
        { timeout: 15_000 },
      )
      .toBeLessThan(300);
  }
});

/** The whole file as the daemon holds it now. */
async function fileOf(page: Page, permalink: string): Promise<string> {
  const response = await page.request.get(
    `${BASE_PATH}/api/v1/domains/${DOMAIN}/engrams/${permalink}`,
  );
  return ((await response.json()) as { content: string }).content;
}

/** Saves the solo surface when it has a Save button; a room saves itself. */
async function settle(page: Page): Promise<void> {
  const save = page.getByRole("button", { name: "Save", exact: true });
  if ((await save.count()) > 0 && (await save.isEnabled())) await save.click();
}

/** Polls `read` until `check` holds, saving the solo surface on the way. */
async function until(
  page: Page,
  read: () => Promise<string>,
  check: (file: string) => boolean,
): Promise<string> {
  let last = "";
  try {
    await expect
      .poll(
        async () => {
          await settle(page);
          last = await read();
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

/** Polls the engram's body (the file after its frontmatter) until `check` holds. */
function bodySatisfies(
  page: Page,
  permalink: string,
  check: (body: string) => boolean,
): Promise<string> {
  return until(page, async () => bodyOf(await fileOf(page, permalink)), check);
}

/** Opens the editor on an engram of the fixture domain. */
async function edit(page: Page, permalink: string): Promise<Locator> {
  await page.goto(`d/${DOMAIN}/e/${permalink}`);
  await page.getByRole("link", { name: "Edit" }).click();
  const source = page.getByRole("textbox", { name: SOURCE });
  await expect(source).toBeVisible();
  // The body is drawn: the first heading of the fixture is on screen.
  await expect(source).toContainText("Buoy Marks");
  return source;
}

/** One drawn line of an editor, by text it shows. */
function line(source: Locator, text: string | RegExp): Locator {
  return source.locator(":scope > .cm-line").filter({ hasText: text }).first();
}

/**
 * Clicks right before or right after the `nth` showing of `word` in the
 * drawn line, at the middle of its first or last character.
 */
async function clickBeside(
  target: Locator,
  word: string,
  side: "before" | "after",
  nth = 0,
): Promise<void> {
  const point = await target.evaluate(
    (el, [w, s, n]) => {
      const walker = document.createTreeWalker(el, NodeFilter.SHOW_TEXT);
      let seen = 0;
      for (let node = walker.nextNode(); node; node = walker.nextNode()) {
        const text = node.textContent ?? "";
        let at = text.indexOf(w);
        while (at >= 0) {
          if (seen === n) {
            const range = document.createRange();
            const from = s === "after" ? at + w.length - 1 : at;
            range.setStart(node, from);
            range.setEnd(node, from + 1);
            const box = range.getBoundingClientRect();
            return {
              x: s === "after" ? box.right - 1 : box.left + 1,
              y: box.top + box.height / 2,
            };
          }
          seen += 1;
          at = text.indexOf(w, at + 1);
        }
      }
      return null;
    },
    [word, side, nth] as const,
  );
  if (!point) throw new Error(`"${word}" is not drawn in the line`);
  await target.page().mouse.click(point.x, point.y);
}

/** What the editor holds now: its text and its main selection. */
interface Held {
  doc: string;
  anchor: number;
  head: number;
}

/**
 * The one place this spec reaches a CodeMirror view from the page.
 * `EditorView.findFromDOM` is the public way, but the page does not expose the
 * `EditorView` class, so this reads the field `findFromDOM` itself reads: the
 * content element's `cmTile`, whose `root.view` is the view. Checked against
 * @codemirror/view 6.43.15; a bump that renames it fails every `held` call at
 * once with "no editor view". Installed on every context this file uses,
 * as `window.fluidView(label)`.
 */
async function installViewHandle(context: BrowserContext): Promise<void> {
  await context.addInitScript(() => {
    (window as unknown as { fluidView: (label: string) => unknown }).fluidView =
      (label: string) => {
        const el = document.querySelector(`.cm-content[aria-label="${label}"]`);
        return (
          (el as unknown as { cmTile?: { root?: { view?: unknown } } } | null)
            ?.cmTile?.root?.view ?? null
        );
      };
  });
}

/** A new context with the view handle, for tests that need more than one. */
async function newContext(
  browser: Browser,
  options: Parameters<Browser["newContext"]>[0] = {},
): Promise<BrowserContext> {
  const context = await browser.newContext(options);
  await installViewHandle(context);
  return context;
}

test.beforeEach(async ({ context }) => {
  await installViewHandle(context);
});

/** Reads the editor's text and main selection. Read only: nothing writes. */
function held(page: Page, label: string = SOURCE): Promise<Held> {
  return page.evaluate((name) => {
    type View = {
      state: {
        doc: { toString(): string };
        selection: { main: { anchor: number; head: number } };
      };
    };
    const view = (
      window as unknown as { fluidView?: (label: string) => View | null }
    ).fluidView?.(name);
    if (!view) throw new Error(`no editor view for ${name}`);
    return {
      doc: view.state.doc.toString(),
      anchor: view.state.selection.main.anchor,
      head: view.state.selection.main.head,
    };
  }, label);
}

/** The font weight the text `word` is drawn with, inside `target`. */
function weightOf(target: Locator, word: string): Promise<number> {
  return target.evaluate((el, w) => {
    const walker = document.createTreeWalker(el, NodeFilter.SHOW_TEXT);
    for (let node = walker.nextNode(); node; node = walker.nextNode()) {
      if ((node.textContent ?? "").includes(w) && node.parentElement) {
        return Number(getComputedStyle(node.parentElement).fontWeight);
      }
    }
    return 0;
  }, word);
}

/**
 * How many elements around the text `word` (up to `target`) draw their own
 * underline: one is a single line, two is a doubled one.
 */
function underlines(target: Locator, word: string): Promise<number> {
  return target.evaluate((root, w) => {
    const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT);
    for (let node = walker.nextNode(); node; node = walker.nextNode()) {
      if (!(node.textContent ?? "").includes(w)) continue;
      let count = 0;
      for (
        let el = node.parentElement;
        el && root.contains(el);
        el = el.parentElement
      ) {
        if (getComputedStyle(el).textDecorationLine.includes("underline")) {
          count += 1;
        }
      }
      return count;
    }
    return -1;
  }, word);
}

/** Flips the "show marks on the line I edit" setting in the avatar menu. */
async function flipSetting(page: Page): Promise<boolean> {
  await page.locator("header button.avatar").click();
  const item = page.getByRole("menuitemcheckbox", { name: SETTING });
  await expect(item).toBeVisible();
  await item.click();
  const on = (await item.getAttribute("aria-checked")) === "true";
  await page.keyboard.press("Escape");
  await expect(item).toHaveCount(0);
  return on;
}

/** Whether the setting reads as on in the avatar menu (the menu is closed after). */
async function settingIsOn(page: Page): Promise<boolean> {
  await page.locator("header button.avatar").click();
  const item = page.getByRole("menuitemcheckbox", { name: SETTING });
  await expect(item).toBeVisible();
  const on = (await item.getAttribute("aria-checked")) === "true";
  await page.keyboard.press("Escape");
  await expect(item).toHaveCount(0);
  return on;
}

async function shot(page: Page, name: string): Promise<void> {
  if (!SHOTS) return;
  mkdirSync(SHOTS, { recursive: true });
  await page.screenshot({ path: join(SHOTS, `marks-${name}`) });
}

// The runner's own trace snapshots the page after every action, which is
// main-thread work a measuring run's numbers must not carry.
if (MEASURE) test.use({ trace: "off" });

let unique = 0;
function titled(base: string): string {
  unique += 1;
  return `${base} ${String(Date.now())}${String(unique)}`;
}

test.describe("hidden marks", () => {
  test.beforeEach(async ({ page }) => {
    await signIn(page);
  });

  test("typing goes on in bold after a bold word, Cmd+B then typing, and plain typing before bold", async ({
    page,
  }) => {
    const permalink = await createEngram(page, titled("Marks Continue"));
    const source = await edit(page, permalink);
    const boldLine = line(source, "word and a chart");
    await expect(boldLine).toHaveText("A bold word and a chart to follow.");

    // Journey 1: after a bold word, typing continues bold, and a space and
    // the next word start a new pair.
    await clickBeside(boldLine, "bold", "after");
    await page.keyboard.type("er");
    await page.keyboard.type(" next");
    await expect(boldLine).toHaveText(
      "A bolder next word and a chart to follow.",
    );
    expect(await boldLine.textContent()).not.toContain("*");
    expect(await weightOf(boldLine, "bolder")).toBeGreaterThanOrEqual(600);
    expect(await weightOf(boldLine, "next")).toBeGreaterThanOrEqual(600);
    expect(await weightOf(boldLine, "word and")).toBeLessThan(600);
    await bodySatisfies(page, permalink, (b) =>
      b.includes("A **bolder** **next** word and"),
    );

    // Cmd+B with no selection, then typing: one bold word, no `****` left
    // behind.
    const watch = line(source, "Watch list");
    await clickBeside(watch, "two", "after");
    await page.keyboard.type(" ");
    await page.keyboard.press("ControlOrMeta+b");
    await page.keyboard.type("loud");
    await expect(watch).toHaveText("Watch list: one two loud");
    expect(await weightOf(watch, "loud")).toBeGreaterThanOrEqual(600);

    // At the start of a bold word in the middle of a line the formatting
    // comes from the left: plain.
    const red = line(source, "Second buoy is");
    await clickBeside(red, "red", "before");
    await page.keyboard.type("dark ");
    await expect(red).toHaveText(/^•?Second buoy is dark red$/);
    expect(await weightOf(red, "dark")).toBeLessThan(600);

    const body = await bodySatisfies(
      page,
      permalink,
      (b) =>
        b.includes("Watch list: one two **loud**") &&
        b.includes("- Second buoy is dark **red**"),
    );
    expect(body).not.toContain("****");
    expect(body).toBe(
      BUOY.replace("A **bold** word", "A **bolder** **next** word")
        .replace("Watch list: one two", "Watch list: one two **loud**")
        .replace("Second buoy is **red**", "Second buoy is dark **red**"),
    );
  });

  test("keyboard only: Shift+Arrow, Mod-b, Mod-k, an address and Enter make a bold link", async ({
    page,
  }) => {
    const permalink = await createEngram(page, titled("Marks Keyboard"));
    const source = await edit(page, permalink);
    const first = line(source, "First buoy");
    await clickBeside(first, "buoy", "after");
    for (let i = 0; i < 4; i += 1) {
      await page.keyboard.press("Shift+ArrowLeft");
    }
    await page.keyboard.press("ControlOrMeta+b");
    await bodySatisfies(page, permalink, (b) =>
      b.includes("- First **buoy**\n"),
    );
    // Bold kept the selection on the visible word.
    const state = await held(page);
    expect(
      state.doc.slice(
        Math.min(state.anchor, state.head),
        Math.max(state.anchor, state.head),
      ),
    ).toBe("buoy");

    await page.keyboard.press("ControlOrMeta+k");
    const dialog = page.getByRole("dialog", { name: "Link" });
    await expect(dialog).toBeVisible();
    const address = dialog.getByLabel("Address");
    await expect(address).toBeFocused();
    await page.keyboard.type("https://example.com/buoy");
    await page.keyboard.press("Enter");
    await expect(dialog).toHaveCount(0);
    await expect(source).toBeFocused();

    const body = await bodySatisfies(page, permalink, (b) =>
      b.includes("example.com/buoy"),
    );
    expect(body).toBe(
      BUOY.replace(
        "- First buoy",
        "- First **[buoy](https://example.com/buoy)**",
      ),
    );
  });

  test("a link press opens its popover, leaves the caret outside and Enter writes only the address", async ({
    page,
  }) => {
    const permalink = await createEngram(page, titled("Marks Link"));
    const source = await edit(page, permalink);
    const boldLine = line(source, "A bold word and a chart");
    const link = "[chart](https://example.com/chart)";

    // The press lands in the middle of the link's words. (Within about a
    // pixel of its right edge the browser hands the press to the line, which
    // puts the caret after the link, outside it, and opens nothing.)
    const words = boldLine.locator(".cm-link-text");
    await words.click();
    const dialog = page.getByRole("dialog", { name: "Link" });
    await expect(dialog).toBeVisible();
    // No caret inside the link in hidden mode.
    const doc = (await held(page)).doc;
    const from = doc.indexOf(link);
    const to = from + link.length;
    const inside = (head: number) => head > from && head < to;
    expect(inside((await held(page)).head)).toBe(false);
    await shot(page, "03-link-popover.png");

    const address = dialog.getByLabel("Address");
    await expect(address).toHaveValue("https://example.com/chart");
    await address.fill("https://example.com/map");
    await address.press("Enter");
    await expect(dialog).toHaveCount(0);
    // The commit leaves the caret after the link, not inside it.
    const committed = await held(page);
    const moved = "[chart](https://example.com/map)";
    const start = committed.doc.indexOf(moved);
    expect(committed.head).toBe(start + moved.length);

    const body = await bodySatisfies(page, permalink, (b) =>
      b.includes("example.com/map"),
    );
    expect(body).toBe(
      BUOY.replace("(https://example.com/chart)", "(https://example.com/map)"),
    );

    // A second press opens it again; Escape writes nothing and the caret is
    // still not inside the link.
    const within = (head: number) =>
      head > start && head < start + moved.length;
    const afterCommit = (await held(page)).doc;
    await words.click();
    await expect(dialog).toBeVisible();
    expect(within((await held(page)).head)).toBe(false);
    await page.keyboard.press("Escape");
    await expect(dialog).toHaveCount(0);
    expect(within((await held(page)).head)).toBe(false);

    // With the marks shown on the line being edited, a press on the link
    // opens the popover too, and it stays open.
    expect(await flipSetting(page)).toBe(true);
    await clickBeside(line(source, "Watch list"), "two", "after");
    await words.click();
    await expect(dialog).toBeVisible();
    await page.waitForTimeout(800);
    await expect(dialog).toBeVisible();
    await page.keyboard.press("Escape");
    await expect(dialog).toHaveCount(0);
    // Neither Escape nor the setting wrote anything.
    expect((await held(page)).doc).toBe(afterCommit);
  });

  test("a status pill's colour picked with Enter changes only the colour word", async ({
    page,
  }) => {
    const permalink = await createEngram(page, titled("Marks Status"));
    const source = await edit(page, permalink);
    await source.locator(".status-label").first().click();
    const dialog = page.getByRole("dialog", { name: "Status label" });
    await expect(dialog).toBeVisible();
    await expect(dialog.getByLabel("Label")).toHaveValue("Open");
    await shot(page, "04-status-popover.png");

    // A click on a colour writes nothing yet (the editor's text is as it
    // was); Enter on the radio saves.
    const before = (await held(page)).doc;
    const blue = dialog.getByRole("radio", { name: "blue" });
    await blue.click();
    await expect(blue).toBeFocused();
    await expect(blue).toBeChecked();
    expect((await held(page)).doc).toBe(before);
    await page.keyboard.press("Enter");
    await expect(dialog).toHaveCount(0);

    const body = await bodySatisfies(page, permalink, (b) =>
      b.includes("{blue}"),
    );
    expect(body).toBe(
      BUOY.replace(":status[Open]{green}", ":status[Open]{blue}"),
    );
  });

  test("a task box: a click, the menu's Not applicable, and one Space is one write", async ({
    page,
  }) => {
    const permalink = await createEngram(page, titled("Marks Task"));
    const source = await edit(page, permalink);
    const boxes = source.getByRole("checkbox", { name: "Mark task done" });
    await expect(boxes).toHaveCount(2);

    // Journey 5: a click marks it done, the menu marks it not applicable.
    await boxes.first().click();
    let body = await bodySatisfies(page, permalink, (b) =>
      b.includes("- [x] Paint"),
    );
    expect(body).toBe(BUOY.replace("- [ ] Paint", "- [x] Paint"));
    await source
      .getByRole("checkbox", { name: "Mark task open" })
      .click({ button: "right" });
    const menu = page.getByRole("menu", { name: "Task state" });
    await expect(menu).toBeVisible();
    await shot(page, "06-task-menu.png");
    await menu.getByRole("menuitemradio", { name: "Not applicable" }).click();
    await expect(menu).toHaveCount(0);
    body = await bodySatisfies(page, permalink, (b) =>
      b.includes("- [~] Paint"),
    );
    expect(body).toBe(BUOY.replace("- [ ] Paint", "- [~] Paint"));

    // Space on the other box writes once: done, and still done a moment
    // later (not flipped back by the click the browser sends on keyup).
    const bell = line(source, "Check the bell").getByRole("checkbox", {
      name: "Mark task done",
    });
    await bell.focus();
    await page.keyboard.press("Space");
    // The editor's own text, read at once and again a second later, so a
    // flip back from the click the browser sends on keyup would show.
    const doneBell = (doc: string) =>
      doc.includes("- [x] Check the bell") &&
      !doc.includes("- [ ] Check the bell");
    await expect.poll(async () => doneBell((await held(page)).doc)).toBe(true);
    await page.waitForTimeout(1000);
    expect(doneBell((await held(page)).doc)).toBe(true);
    body = await bodySatisfies(page, permalink, (b) =>
      b.includes("- [x] Check the bell"),
    );
    expect(body).toBe(
      BUOY.replace("- [ ] Paint", "- [~] Paint").replace(
        "- [ ] Check the bell",
        "- [x] Check the bell",
      ),
    );
    // And one undo takes it back to open: it was one change.
    await clickBeside(line(source, "Watch list"), "two", "after");
    await page.keyboard.press("ControlOrMeta+z");
    body = await bodySatisfies(page, permalink, (b) =>
      b.includes("- [ ] Check the bell"),
    );
    expect(body).toBe(BUOY.replace("- [ ] Paint", "- [~] Paint"));
  });

  test("Text style Heading 1 writes a level two heading", async ({ page }) => {
    const permalink = await createEngram(page, titled("Marks Heading"));
    const source = await edit(page, permalink);
    await clickBeside(line(source, "Tasks for the week"), "week", "after");
    await page.getByRole("button", { name: "Text style" }).click();
    await page.getByRole("menuitemradio", { name: "Heading 1" }).click();
    const body = await bodySatisfies(page, permalink, (b) =>
      b.includes("## Tasks for the week:"),
    );
    expect(body).toBe(
      BUOY.replace("\nTasks for the week:", "\n## Tasks for the week:"),
    );
    await expect(line(source, "Tasks for the week")).toHaveText(
      "Tasks for the week:",
    );
  });

  test("the setting shows the marks on the line being edited only, and stays on after a reload", async ({
    page,
  }) => {
    const permalink = await createEngram(page, titled("Marks Setting"));
    const source = await edit(page, permalink);
    const opened = (await held(page)).doc;
    await shot(page, "01-hidden.png");
    expect(await settingIsOn(page)).toBe(false);
    expect(await flipSetting(page)).toBe(true);

    const boldLine = line(source, /A (\*\*)?bold/);
    await clickBeside(boldLine, "bold", "after");
    await expect(boldLine).toHaveText(
      "A **bold** word and a [chart](https://example.com/chart) to follow.",
    );
    // The other lines keep their marks hidden.
    await expect(line(source, "Second buoy")).toHaveText(
      /^•?Second buoy is red$/,
    );
    await expect(line(source, "Watch list")).toHaveText("Watch list: one two");
    await shot(page, "02-setting-on-bold-line.png");

    await page.reload();
    await expect(source).toContainText("Buoy Marks");
    expect(await settingIsOn(page)).toBe(true);
    await clickBeside(line(source, /A (\*\*)?bold/), "bold", "after");
    await expect(line(source, /A (\*\*)?bold/)).toContainText("**bold**");
    await expect(line(source, "Second buoy")).toHaveText(
      /^•?Second buoy is red$/,
    );

    // Turned off again, the line hides its marks under the caret too.
    expect(await flipSetting(page)).toBe(false);
    await expect(line(source, /A (\*\*)?bold/)).toHaveText(
      "A bold word and a chart to follow.",
    );
    // Showing and hiding marks wrote nothing.
    expect((await held(page)).doc).toBe(opened);
  });

  test("Raw shows the source with every mark", async ({ page }) => {
    const permalink = await createEngram(page, titled("Marks Raw"));
    const source = await edit(page, permalink);
    await expect(source).not.toContainText("**bold**");
    await page.getByRole("button", { name: "Raw", exact: true }).click();
    for (const mark of [
      "A **bold** word and a [chart](https://example.com/chart) to follow.",
      "- Second buoy is **red**",
      "- [ ] Paint the red buoy",
      ":status[Open]{green}",
      "$`d^2`$",
      "> [!NOTE]+ Pilot notes",
      "```sh",
      "A buoy needs a light[^light].",
      "[^light]: Every buoy carries one.",
    ]) {
      await expect(source).toContainText(mark);
    }
  });

  test("a Ctrl+click on a task box on a Mac opens the menu and does not flip the box", async ({
    page,
  }) => {
    // Only Chromium's Mac build turns a Ctrl+click into a context menu
    // press, so this step runs on a Mac host only. The right click below
    // covers the menu path on every host.
    test.skip(
      process.platform !== "darwin",
      "a Ctrl+click is a context menu press only on a Mac",
    );
    const permalink = await createEngram(page, titled("Marks Mac"));
    const source = await edit(page, permalink);
    const before = (await held(page)).doc;
    await source
      .getByRole("checkbox", { name: "Mark task done" })
      .first()
      .click({ modifiers: ["Control"] });
    const menu = page.getByRole("menu", { name: "Task state" });
    await expect(menu).toBeVisible();
    await page.keyboard.press("Escape");
    await expect(menu).toHaveCount(0);
    expect((await held(page)).doc).toBe(before);
    await expect(
      source.getByRole("checkbox", { name: "Mark task done" }).first(),
    ).not.toBeChecked();
  });

  test("a right click on a task box opens the menu, a callout head press places no caret", async ({
    page,
  }) => {
    const permalink = await createEngram(page, titled("Marks Menu"));
    const source = await edit(page, permalink);

    // A right click opens the menu and flips nothing.
    const opened = (await held(page)).doc;
    await source
      .getByRole("checkbox", { name: "Mark task done" })
      .first()
      .click({ button: "right" });
    const menu = page.getByRole("menu", { name: "Task state" });
    await expect(menu).toBeVisible();
    await page.keyboard.press("Escape");
    await expect(menu).toHaveCount(0);
    expect((await held(page)).doc).toBe(opened);

    // A press on the callout's kind button opens its menu and leaves the
    // caret where it was.
    await clickBeside(line(source, "Watch list"), "two", "after");
    const before = await held(page);
    await page.getByRole("button", { name: /^Callout kind: / }).click();
    const kinds = page.getByRole("menu", { name: "Callout kind" });
    await expect(kinds).toBeVisible();
    expect((await held(page)).head).toBe(before.head);
    await page.keyboard.press("Escape");
    await expect(kinds).toHaveCount(0);
    expect((await held(page)).head).toBe(before.head);

    // The fold chevron flips the callout and places no caret either.
    await page.getByRole("button", { name: "Callout fold" }).click();
    await bodySatisfies(page, permalink, (b) =>
      b.includes("> [!NOTE]- Pilot notes"),
    );
    expect((await held(page)).head).toBe(before.head);
  });

  test("a long document's footnotes are numbered in the worker and an edit renumbers them", async ({
    page,
  }) => {
    const filler = Array.from(
      { length: 70 },
      (_, i) =>
        `Paragraph ${String(i + 1)} of the long log, written so the page is long enough for the worker to read its notes.`,
    ).join("\n\n");
    const body = [
      "# Long log",
      "",
      "Start here.",
      "",
      "The first claim[^a] and the second claim[^b].",
      "",
      filler,
      "",
      "[^a]: The note for the first claim.",
      "[^b]: The note for the second claim.",
      "",
    ].join("\n");
    expect(body.length).toBeGreaterThan(6000);
    const workers: string[] = [];
    page.on("worker", (worker) => workers.push(worker.url()));
    const permalink = await createEngram(page, titled("Marks Notes"), body);
    await page.goto(`d/${DOMAIN}/e/${permalink}`);
    await page.getByRole("link", { name: "Edit" }).click();
    const source = page.getByRole("textbox", { name: SOURCE });
    await expect(source).toContainText("Long log");

    const claims = line(source, "The first claim");
    const numbers = claims.locator(".cm-footnote-button");
    await expect(numbers).toHaveText(["1", "2"]);
    expect(workers.some((url) => /footnoteWorker/.test(url))).toBe(true);

    // A reference to the second note before the first one makes it note 1.
    await clickBeside(line(source, "Start here"), "here.", "after");
    await page.keyboard.type(" See[^b");
    await page.keyboard.type("]");
    const saved = await bodySatisfies(page, permalink, (b) =>
      b.includes("Start here. See[^b]"),
    );
    expect(saved).toBe(body.replace("Start here.", "Start here. See[^b]"));
    await expect(numbers).toHaveText(["2", "1"]);
    await expect(
      line(source, "Start here").locator(".cm-footnote-button"),
    ).toHaveText(["1"]);
  });

  test("copy and cut put plain text and HTML on the clipboard, and a paste gives the same bytes", async ({
    page,
    context,
  }) => {
    await context.grantPermissions(["clipboard-read", "clipboard-write"]);
    const permalink = await createEngram(page, titled("Marks Clipboard"));
    const source = await edit(page, permalink);
    const boldLine = line(source, "A bold word and a chart");
    const sourceLine =
      "A **bold** word and a [chart](https://example.com/chart) to follow.";

    const clipboard = () =>
      page.evaluate(async () => {
        const items = await navigator.clipboard.read();
        const out: Record<string, string> = {};
        for (const item of items) {
          for (const type of item.types) {
            out[type] = await (await item.getType(type)).text();
          }
        }
        return out;
      });
    const selectLine = async () => {
      await clickBeside(boldLine, "follow.", "after");
      await page.keyboard.press("Shift+Home");
      const state = await held(page);
      expect(
        state.doc.slice(
          Math.min(state.anchor, state.head),
          Math.max(state.anchor, state.head),
        ),
      ).toBe(sourceLine);
    };

    // Copy, then paste over the same selection: the bytes stay.
    await selectLine();
    await page.keyboard.press("ControlOrMeta+c");
    let copied = await clipboard();
    expect(copied["text/plain"]).toBe(sourceLine);
    expect(copied["text/html"]).toContain('data-crystalline-copy="1"');
    expect(copied["text/html"]).toContain("<strong>bold</strong>");
    expect(copied["text/html"]).toContain('href="https://example.com/chart"');
    // The paste over the selection collapses it to the end of the pasted
    // line, and the text is byte for byte what it was.
    const beforePaste = await held(page);
    const lineEnd = Math.max(beforePaste.anchor, beforePaste.head);
    await page.keyboard.press("ControlOrMeta+v");
    await expect
      .poll(async () => {
        const now = await held(page);
        return now.anchor === now.head && now.head === lineEnd;
      })
      .toBe(true);
    expect((await held(page)).doc).toBe(beforePaste.doc);

    // Cut, then paste back: the same bytes again.
    await selectLine();
    const beforeCut = (await held(page)).doc;
    await page.keyboard.press("ControlOrMeta+x");
    await expect(boldLine).toHaveCount(0);
    copied = await clipboard();
    expect(copied["text/plain"]).toBe(sourceLine);
    expect(copied["text/html"]).toContain('data-crystalline-copy="1"');
    expect((await held(page)).doc).not.toContain(sourceLine);
    // The cut is saved first (the room saves after a pause), so the file
    // check after the paste reads a save the paste made.
    await bodySatisfies(page, permalink, (b) => !b.includes(sourceLine));
    await page.keyboard.press("ControlOrMeta+v");
    await expect(line(source, "A bold word and a chart")).toHaveText(
      "A bold word and a chart to follow.",
    );
    expect((await held(page)).doc).toBe(beforeCut);
    // And the file: the save of the paste brings the same bytes back.
    const body = await bodySatisfies(page, permalink, (b) => b === BUOY);
    expect(body).toBe(BUOY);
  });

  test("two browsers: one presses Mod-b and types while the other types beside it", async ({
    browser,
  }) => {
    const contextA = await newContext(browser);
    const contextB = await newContext(browser);
    try {
      const pageA = await contextA.newPage();
      const pageB = await contextB.newPage();
      await signIn(pageA);
      await signIn(pageB, PEER, PEER_PASSWORD);
      const permalink = await createEngram(pageA, titled("Marks Room"));
      const sourceA = await edit(pageA, permalink);
      const sourceB = await edit(pageB, permalink);

      // A's caret right after "one", B's at the end of the same line.
      await clickBeside(line(sourceA, "Watch list"), "one", "after");
      await clickBeside(line(sourceB, "Watch list"), "two", "after");
      await expect(pageA.locator(".cm-ySelectionCaret")).not.toHaveCount(0);
      await expect(pageB.locator(".cm-ySelectionCaret")).not.toHaveCount(0);

      // A presses Mod-b with no selection (a pending bold), B types, and only
      // then A types: B's change arriving must not drop A's pending bold.
      // Each side's text, read from its editor (the drawn line also carries
      // the other person's caret label).
      const watchOf = async (page: Page) =>
        (await held(page)).doc.split("\n").find((l) => l.startsWith("Watch"));
      await pageA.keyboard.type(" ");
      await expect.poll(() => watchOf(pageB)).toBe("Watch list: one  two");
      await pageA.keyboard.press("ControlOrMeta+b");
      await pageB.keyboard.type(" bravo");
      await expect
        .poll(() => watchOf(pageA))
        .toBe("Watch list: one  two bravo");
      await pageA.keyboard.type("alpha");

      for (const page of [pageA, pageB]) {
        await expect
          .poll(() => watchOf(page))
          .toBe("Watch list: one **alpha** two bravo");
      }
      expect(await weightOf(line(sourceB, "Watch list"), "alpha")).toBe(
        await weightOf(line(sourceA, "Watch list"), "alpha"),
      );
      expect(
        await weightOf(line(sourceA, "Watch list"), "alpha"),
      ).toBeGreaterThanOrEqual(600);
      expect((await held(pageA)).doc).toBe((await held(pageB)).doc);

      const body = await bodySatisfies(
        pageA,
        permalink,
        (b) => b.includes("bravo") && b.includes("alpha"),
      );
      expect(body).toBe(
        BUOY.replace(
          "Watch list: one two",
          "Watch list: one **alpha** two bravo",
        ),
      );
    } finally {
      await contextA.close();
      await contextB.close();
    }
  });

  test("an open table cell draws its bold word bold with the marks hidden", async ({
    page,
  }) => {
    const table = await createEngram(
      page,
      titled("Marks Cell"),
      [
        "# Buoy Marks",
        "",
        "| Buoy | Colour |",
        "| --- | --- |",
        "| **North** | green |",
        "| South | *red* |",
        "",
      ].join("\n"),
    );
    await edit(page, table);
    await page
      .locator('.cm-grid [aria-rowindex="2"][aria-colindex="1"]')
      .click();
    const open = page.locator(".cm-grid .cm-content[contenteditable=true]");
    await expect(open).toBeFocused();
    await expect(open).toHaveText("North");
    expect(await weightOf(open, "North")).toBeGreaterThanOrEqual(600);
    await shot(page, "08-table-cell-bold.png");
    // Typing at its end goes on in bold, inside the marks.
    await page.keyboard.press("End");
    await page.keyboard.type("ern");
    await bodySatisfies(page, table, (b) => b.includes("| **Northern** |"));
  });

  test("a link in an open table cell is underlined as often as one in prose", async ({
    page,
  }) => {
    const table = await createEngram(
      page,
      titled("Marks Cell Link"),
      [
        "# Buoy Marks",
        "",
        "See the [Chart](https://example.com/c) here.",
        "",
        "| Buoy | Colour |",
        "| --- | --- |",
        "| [Chart](https://example.com/c) | blue |",
        "",
      ].join("\n"),
    );
    const source = await edit(page, table);
    await page
      .locator('.cm-grid [aria-rowindex="2"][aria-colindex="1"]')
      .click();
    const open = page.locator(".cm-grid .cm-content[contenteditable=true]");
    await expect(open).toBeFocused();
    await expect(open).toHaveText("Chart");
    await shot(page, "10-table-cell-link.png");
    const inCell = await underlines(open, "Chart");
    const inProse = await underlines(line(source, "See the"), "Chart");
    expect(inProse).toBeGreaterThanOrEqual(1);
    expect(inCell).toBe(inProse);
  });

  test("in a Reflection page the bold and link journeys work and no comment or id shows", async ({
    page,
  }) => {
    const path = "marks/buoy-watch.md";
    const api = `${BASE_PATH}/api/v1/domains/${REFLECTION_DOMAIN}/reflections/${path}`;
    const written = await page.request.put(api, {
      headers: { "X-CSRF-Token": await csrf(page) },
      data: {
        markdown: [
          "# Buoy watch",
          "",
          "## Watch",
          "",
          "A **bold** word on the watch.",
          "",
          "<!-- a note for the agent -->",
          "",
          "Watch the flags",
          "",
        ].join("\n"),
        create: true,
      },
    });
    expect(written.ok(), await written.text()).toBeTruthy();
    const pageSource = async () => {
      const response = await page.request.get(api);
      return ((await response.json()) as { markdown: string }).markdown;
    };
    try {
      const original = await pageSource();
      expect(original).toMatch(/<!-- r:[0-9a-f]{8} -->/);

      await page.goto(`d/${REFLECTION_DOMAIN}/reflection-edit/${path}`);
      const source = page.getByRole("textbox", { name: PAGE_SOURCE });
      await expect(source).toContainText("A bold word on the watch.");
      const nothingHidden = async () => {
        const text = (await source.innerText()).replace(/\s+/g, " ");
        expect(text).not.toMatch(/r:[0-9a-f]{8}/);
        expect(text).not.toContain("<!--");
        expect(text).not.toContain("a note for the agent");
      };
      await nothingHidden();

      const boldLine = line(source, "word on the watch");
      await clickBeside(boldLine, "bold", "after");
      await page.keyboard.type("er");
      await page.keyboard.type(" next");
      await expect(boldLine).toHaveText("A bolder next word on the watch.");

      const flags = line(source, "Watch the flags");
      await clickBeside(flags, "flags", "after");
      for (let i = 0; i < 5; i += 1) {
        await page.keyboard.press("Shift+ArrowLeft");
      }
      await page.keyboard.press("ControlOrMeta+b");
      await page.keyboard.press("ControlOrMeta+k");
      const dialog = page.getByRole("dialog", { name: "Link" });
      await expect(dialog.getByLabel("Address")).toBeFocused();
      await page.keyboard.type("https://example.com/flags");
      await page.keyboard.press("Enter");
      await expect(dialog).toHaveCount(0);
      await expect(flags).toHaveText("Watch the flags");

      const saved = await until(page, pageSource, (m) =>
        m.includes("example.com/flags"),
      );
      // The page's own frontmatter notes when each section changed; the
      // text after it is what was typed.
      expect(bodyOf(saved)).toBe(
        bodyOf(original)
          .replace("A **bold** word", "A **bolder** **next** word")
          .replace(
            "Watch the flags",
            "Watch the **[flags](https://example.com/flags)**",
          ),
      );
      await nothingHidden();

      // With the setting on, the line being edited shows its marks, and still
      // no comment and no id shows, not even beside the heading.
      const typed = (await held(page, PAGE_SOURCE)).doc;
      expect(await flipSetting(page)).toBe(true);
      await clickBeside(line(source, /^#* ?Watch$/), "Watch", "after");
      await expect(line(source, /^#* ?Watch$/)).toHaveText("## Watch");
      await nothingHidden();
      // The caret walks down from the heading over the id line, and up from
      // "Watch the flags" over the comment: wherever it stops, the line
      // under it shows no id and no comment.
      for (let i = 0; i < 3; i += 1) {
        await page.keyboard.press("ArrowDown");
        await nothingHidden();
      }
      await clickBeside(line(source, "Watch the"), "the", "after");
      for (let i = 0; i < 3; i += 1) {
        await page.keyboard.press("ArrowUp");
        await nothingHidden();
      }
      await clickBeside(line(source, "word on the watch"), "watch.", "after");
      await expect(line(source, "word on the watch")).toContainText(
        "**bolder**",
      );
      await nothingHidden();
      // The walk and the setting wrote nothing.
      expect((await held(page, PAGE_SOURCE)).doc).toBe(typed);
    } finally {
      await page.goto(`d/${REFLECTION_DOMAIN}/reflection`);
      const removed = await page.request.delete(api, {
        headers: { "X-CSRF-Token": await csrf(page) },
      });
      expect(removed.ok(), await removed.text()).toBeTruthy();
    }
  });

  test("Tab to the toolbar, Enter on Bold, then typing: bold, and the caret stays (D38)", async ({
    page,
  }) => {
    const permalink = await createEngram(page, titled("Marks Toolbar Keys"));
    const source = await edit(page, permalink);
    const watch = line(source, "Watch list");
    await clickBeside(watch, "two", "after");
    await page.keyboard.type(" ");
    const before = await held(page);
    // Out of the editor with the keyboard, to the sticky bar's Bold button.
    const bold = page
      .getByRole("button", { name: "Bold", exact: true })
      .first();
    for (let i = 0; i < 40; i += 1) {
      await page.keyboard.press("Tab");
      if (await bold.evaluate((el) => el === document.activeElement)) break;
    }
    await expect(bold).toBeFocused();
    await page.keyboard.press("Enter");
    await expect(source).toBeFocused();
    expect((await held(page)).head).toBe(before.head);
    await page.keyboard.type("calm");
    await expect(watch).toHaveText("Watch list: one two calm");
    expect(await weightOf(watch, "calm")).toBeGreaterThanOrEqual(600);
    await bodySatisfies(page, permalink, (b) =>
      b.includes("Watch list: one two **calm**"),
    );
  });

  test("on a phone keyboard a composition is judged when it ends: Bold, a word after bold, a shrink beside bold", async ({
    browser,
  }) => {
    const context = await newContext(browser, { ...devices["Pixel 7"] });
    try {
      const page = await context.newPage();
      await signIn(page);
      const permalink = await createEngram(
        page,
        titled("Marks Phone"),
        // Near the top, so a phone's short view draws it.
        BUOY.replace("- First buoy", "Tide **a.** b\n\n- First buoy"),
      );
      const source = await edit(page, permalink);
      const cdp: CDPSession = await context.newCDPSession(page);
      const lineOf = async (text: string) =>
        (await held(page)).doc.split("\n").find((l) => l.includes(text)) ?? "";
      /** A word composed letter by letter, then committed. */
      const compose = async (word: string) => {
        for (let i = 1; i <= word.length; i += 1) {
          await cdp.send("Input.imeSetComposition", {
            text: word.slice(0, i),
            selectionStart: i,
            selectionEnd: i,
          });
        }
        await cdp.send("Input.insertText", { text: word });
        // The check at a composition's end waits 60 ms for its last change.
        await page.waitForTimeout(300);
      };

      // Bold with no selection, then a word composed: the word is bold.
      const watch = line(source, "Watch list");
      await clickBeside(watch, "one", "after");
      await expect(source).toBeFocused();
      await page.waitForTimeout(300);
      // The emulated phone keeps the host's platform, so Mod is Cmd on a
      // Mac host (where Ctrl+B moves the caret left, as in Emacs).
      await page.keyboard.press("ControlOrMeta+b");
      await compose("sea");
      expect(await lineOf("Watch list")).toBe("Watch list: one**sea** two");
      expect(await weightOf(watch, "sea")).toBeGreaterThanOrEqual(600);

      // The space committed on its own, then the next word composed, right
      // after a bold word: bold, as on a desktop (D34).
      await clickBeside(line(source, "Second buoy is"), "red", "after");
      await expect(source).toBeFocused();
      await page.waitForTimeout(300);
      await cdp.send("Input.insertText", { text: " " });
      await compose("sea");
      expect(await lineOf("Second buoy")).toBe(
        "- Second buoy is **red** **sea**",
      );

      // The Task 14 sequence beside `**a.** b`: a word composed, shrunk twice
      // inside the composition, committed. Whatever the composition did, the
      // pair is kept when it ends: no mark shows and `a.` stays bold.
      const tide = line(source, "Tide");
      await clickBeside(tide, "a.", "after");
      await expect(source).toBeFocused();
      await page.waitForTimeout(300);
      const typed = " harbour";
      for (let i = 1; i <= typed.length; i += 1) {
        await cdp.send("Input.imeSetComposition", {
          text: typed.slice(0, i),
          selectionStart: i,
          selectionEnd: i,
        });
      }
      for (const n of [typed.length - 1, typed.length - 2]) {
        await cdp.send("Input.imeSetComposition", {
          text: typed.slice(0, n),
          selectionStart: n,
          selectionEnd: n,
        });
      }
      await cdp.send("Input.insertText", { text: typed.slice(0, -2) });
      await page.waitForTimeout(300);
      expect(await tide.textContent()).not.toContain("*");
      expect(await lineOf("Tide")).toMatch(/^Tide \*\*a\.[^*]*\*\*/);
      expect(await weightOf(tide, "a.")).toBeGreaterThanOrEqual(600);
      await bodySatisfies(page, permalink, (b) =>
        b.includes("- Second buoy is **red** **sea**"),
      );
    } finally {
      await context.close();
    }
  });
});

/** The checkpoint screenshots Jordi looks at; only when a run asks for them. */
test.describe("hidden marks checkpoint", () => {
  test.skip(!SHOTS, "set FLUID_E2E_SHOTS to take the checkpoint screenshots");

  test("the checkpoint screenshots", async ({ page }) => {
    await signIn(page);
    const permalink = await createEngram(page, titled("Marks Checkpoint"));
    const source = await edit(page, permalink);

    await source.locator(".cm-math-inline").first().click();
    const formula = page.getByRole("dialog", { name: "Formula" });
    await expect(formula).toBeVisible();
    await shot(page, "05-math-popover.png");
    await page.keyboard.press("Escape");
    await expect(formula).toHaveCount(0);

    await page.getByRole("button", { name: /^Code language: / }).click();
    const languages = page.getByRole("menu", { name: "Code language" });
    await expect(languages).toBeVisible();
    await shot(page, "07-code-language-menu.png");
    await page.keyboard.press("Escape");
    await expect(languages).toHaveCount(0);

    await page.emulateMedia({ colorScheme: "dark" });
    await edit(page, permalink);
    await expect(page.locator("html")).toHaveAttribute("data-theme", "dark");
    await clickBeside(line(source, "Watch list"), "two", "after");
    await expect(
      source.locator(".cm-math-inline .katex").first(),
    ).toBeVisible();
    await shot(page, "09-dark.png");
  });
});

/** Appends one measurement to the file the run named. */
function record(name: string, data: object): void {
  const entry = JSON.stringify({ name, ...data });
  console.log(entry);
  appendFileSync(MEASURE, `${entry}\n`);
}

interface TraceEvent {
  name: string;
  ph: string;
  ts: number;
  dur?: number;
  pid: number;
  tid: number;
  args?: { name?: string };
}

/** The renderer main threads' top-level tasks and user timing marks of a trace. */
function readTrace(buffer: Buffer): {
  tasks: { ts: number; dur: number; key: string }[];
  marks: { name: string; ts: number }[];
} {
  const events = (
    JSON.parse(buffer.toString("utf8")) as {
      traceEvents: TraceEvent[];
    }
  ).traceEvents;
  const mains = new Set(
    events
      .filter(
        (e) => e.name === "thread_name" && e.args?.name === "CrRendererMain",
      )
      .map((e) => `${String(e.pid)}:${String(e.tid)}`),
  );
  const tasks = events
    .filter(
      (e) =>
        e.name === "RunTask" &&
        e.ph === "X" &&
        mains.has(`${String(e.pid)}:${String(e.tid)}`),
    )
    .map((e) => ({
      ts: e.ts,
      dur: e.dur ?? 0,
      key: `${String(e.pid)}:${String(e.tid)}`,
    }));
  const marks = events
    .filter((e) => e.name.startsWith("fluid-key-") && e.ph !== "M")
    .map((e) => ({ name: e.name, ts: e.ts }));
  return { tasks, marks };
}

const CATEGORIES = [
  "devtools.timeline",
  "disabled-by-default-devtools.timeline",
  "blink.user_timing",
];

/**
 * Types `count` letters at the caret, each followed by an undo, with a pause
 * between keys, and answers what each letter and each undo cost:
 *
 * - `input`: the main-thread task that ran the keydown (the key's handlers,
 *   CodeMirror's read of the DOM change, its transaction and its view
 *   update), from a Chromium trace;
 * - `frame`: every main-thread task that started in the 50 ms after the
 *   keydown (the input task, the measure and paint frame after it, and any
 *   room traffic), from the same trace;
 * - `dispatch`: the time inside `view.dispatch` alone.
 *
 * Nothing is asked of the page between keys, so the trace holds only the
 * page's own work. Keys whose next paint came 16 ms or more after the press
 * are counted from the Event Timing API.
 */
async function typingCost(
  page: Page,
  browser: Browser,
  count: number,
): Promise<object> {
  await page.evaluate((label) => {
    type View = { dispatch: (...args: unknown[]) => void };
    const view = (
      window as unknown as { fluidView: (l: string) => View }
    ).fluidView(label);
    const w = window as unknown as {
      fluidDispatch: number[][];
      fluidSlow: { name: string; duration: number; processing: number }[];
    };
    w.fluidDispatch = [];
    w.fluidSlow = [];
    let key = -1;
    const dispatch = view.dispatch.bind(view);
    view.dispatch = (...args: unknown[]) => {
      const start = performance.now();
      dispatch(...args);
      if (key >= 0) {
        (w.fluidDispatch[key] ??= []).push(performance.now() - start);
      }
    };
    window.addEventListener(
      "keydown",
      (event) => {
        if (event.key === "Meta" || event.key === "Control") return;
        key += 1;
        performance.mark(`fluid-key-${String(key)}`);
      },
      true,
    );
    new PerformanceObserver((list) => {
      for (const entry of list.getEntries()) {
        const timing = entry as PerformanceEventTiming;
        w.fluidSlow.push({
          name: timing.name,
          duration: timing.duration,
          processing: timing.processingEnd - timing.processingStart,
        });
      }
    }).observe({
      type: "event",
      durationThreshold: 16,
    } as PerformanceObserverInit);
  }, SOURCE);
  await browser.startTracing(page, { categories: CATEGORIES });
  const letters = "abcdefghijklmnopqrstuvwxyz";
  for (let i = 0; i < count; i += 1) {
    await page.keyboard.press(letters[i % letters.length] ?? "a");
    await page.waitForTimeout(200);
    await page.keyboard.press("ControlOrMeta+z");
    await page.waitForTimeout(200);
  }
  const trace = readTrace(await browser.stopTracing());
  const page_ = await page.evaluate(() => {
    const w = window as unknown as {
      fluidDispatch: number[][];
      fluidSlow: { name: string; duration: number; processing: number }[];
    };
    return { dispatch: w.fluidDispatch, slow: w.fluidSlow };
  });
  const marks = trace.marks.sort((a, b) => a.ts - b.ts);
  // The page's main thread: the one the keydown marks were made on.
  const input: number[] = [];
  const frame: number[] = [];
  for (const mark of marks) {
    let inputTask = 0;
    const busy = new Map<string, number>();
    for (const task of trace.tasks) {
      if (task.ts <= mark.ts && task.ts + task.dur >= mark.ts) {
        inputTask = Math.max(inputTask, task.dur);
      }
      if (task.ts + task.dur >= mark.ts && task.ts < mark.ts + 50_000) {
        busy.set(task.key, (busy.get(task.key) ?? 0) + task.dur);
      }
    }
    input.push(inputTask / 1000);
    frame.push(Math.max(0, ...busy.values()) / 1000);
  }
  const dispatch = marks.map((_, i) =>
    (page_.dispatch[i] ?? []).reduce((a, b) => a + b, 0),
  );
  const even = <T>(values: T[]) => values.filter((_, i) => i % 2 === 0);
  const odd = <T>(values: T[]) => values.filter((_, i) => i % 2 === 1);
  const stats = (values: number[]) => {
    const sorted = [...values].sort((a, b) => a - b);
    const mean = values.reduce((a, b) => a + b, 0) / (values.length || 1);
    const at = (q: number) =>
      Number((sorted[Math.floor((sorted.length - 1) * q)] ?? 0).toFixed(2));
    return {
      mean: Number(mean.toFixed(2)),
      median: at(0.5),
      p90: at(0.9),
      max: at(1),
    };
  };
  const pair = (values: number[]) =>
    even(values).map((v, i) => v + (odd(values)[i] ?? 0));
  return {
    keys: marks.length,
    letter: {
      input: stats(even(input)),
      frame: stats(even(frame)),
      dispatch: stats(even(dispatch)),
    },
    undo: {
      input: stats(odd(input)),
      frame: stats(odd(frame)),
      dispatch: stats(odd(dispatch)),
    },
    letterPlusUndo: {
      input: stats(pair(input)),
      frame: stats(pair(frame)),
      dispatch: stats(pair(dispatch)),
    },
    eventsOver16ms: page_.slow.filter((e) => e.name.startsWith("key")).length,
    slowEvents: page_.slow,
  };
}

/**
 * Where the main thread spends a letter and its undo: a CPU profile over
 * `count` of each, as self time per function, the heaviest first.
 */
async function profileTyping(page: Page, count: number): Promise<object[]> {
  const cdp = await page.context().newCDPSession(page);
  await cdp.send("Profiler.enable");
  await cdp.send("Profiler.setSamplingInterval", { interval: 100 });
  await cdp.send("Profiler.start");
  for (let i = 0; i < count; i += 1) {
    await page.keyboard.press("x");
    await page.waitForTimeout(100);
    await page.keyboard.press("ControlOrMeta+z");
    await page.waitForTimeout(100);
  }
  const { profile } = (await cdp.send("Profiler.stop")) as {
    profile: {
      nodes: {
        id: number;
        callFrame: { functionName: string; url: string; lineNumber: number };
      }[];
      samples: number[];
      timeDeltas: number[];
    };
  };
  await cdp.detach();
  const byNode = new Map(profile.nodes.map((n) => [n.id, n]));
  const self = new Map<string, number>();
  profile.samples.forEach((id, i) => {
    const node = byNode.get(id);
    if (!node) return;
    const { functionName, url, lineNumber } = node.callFrame;
    if (functionName === "(idle)" || functionName === "(program)") return;
    const file = url.split("/").pop() ?? "";
    const key = `${functionName || "(anonymous)"} ${file}:${String(lineNumber)}`;
    self.set(key, (self.get(key) ?? 0) + (profile.timeDeltas[i] ?? 0));
  });
  return [...self.entries()]
    .sort((a, b) => b[1] - a[1])
    .slice(0, 25)
    .map(([fn, us]) => ({
      fn,
      msPerPair: Number((us / 1000 / count).toFixed(2)),
    }));
}

test.describe("hidden marks measurements", () => {
  test.skip(!MEASURE, "set FLUID_E2E_MEASURE to run the measurements");
  test.setTimeout(240_000);

  const repo = new URL("../../", import.meta.url);
  const dialect = readFileSync(
    new URL("fluid/src/editor/fixtures/dialect/everything.md", repo),
    "utf8",
  );
  const deployment = readFileSync(new URL("docs/deployment.md", repo), "utf8");

  test("typing cost per letter, undo included", async ({ page, browser }) => {
    await signIn(page);
    for (const [name, text, word] of [
      ["dialect everything.md", bodyOf(dialect), "Two words:"],
      ["docs/deployment.md", deployment, "Crystalline"],
    ] as const) {
      const permalink = await createEngram(page, titled("Marks Cost"), text);
      await page.goto(`d/${DOMAIN}/e/${permalink}`);
      await page.getByRole("link", { name: "Edit" }).click();
      const source = page.getByRole("textbox", { name: SOURCE });
      await expect(source).toBeVisible();
      const target = line(source, word);
      await expect(target).toBeVisible();
      await page.waitForTimeout(1500);
      // Inside a bold word where there is one, so the typing rules run.
      if (name.startsWith("dialect")) {
        await clickBeside(target, "one", "after");
      } else {
        await clickBeside(target, word, "after");
      }
      const result = await typingCost(page, browser, 20);
      record("typing", { file: name, bytes: text.length, ...result });
      record("typing profile", {
        file: name,
        top: await profileTyping(page, 20),
      });
    }
  });

  test("the cell parse when a cell deep in the longest page opens", async ({
    page,
    browser,
  }) => {
    await signIn(page);
    const permalink = await createEngram(
      page,
      titled("Marks Deep Cell"),
      deployment,
    );
    await page.goto(`d/${DOMAIN}/e/${permalink}`);
    await page.getByRole("link", { name: "Edit" }).click();
    const source = page.getByRole("textbox", { name: SOURCE });
    await expect(source).toBeVisible();
    // The row's place in the editor's text (which holds the frontmatter too).
    const at = await page.evaluate(
      ([label, needle]) => {
        type View = {
          state: { doc: { toString(): string } };
          dispatch: (spec: object) => void;
        };
        const view = (
          window as unknown as { fluidView: (l: string) => View }
        ).fluidView(label);
        const pos = view.state.doc.toString().indexOf(needle);
        view.dispatch({ selection: { anchor: pos }, scrollIntoView: true });
        return pos;
      },
      [SOURCE, "| `Remote-Name` |"] as const,
    );
    expect(at).toBeGreaterThan(0);
    const grid = page
      .locator(".cm-grid")
      .filter({ hasText: "Remote-Groups" })
      .first();
    await expect(grid).toBeVisible();
    await page.waitForTimeout(2000);
    const results: object[] = [];
    for (const row of [2, 3, 4]) {
      await page.keyboard.press("Escape");
      await page.keyboard.press("Escape");
      await page.waitForTimeout(500);
      const target = grid.locator(
        `[aria-rowindex="${String(row)}"][aria-colindex="2"]`,
      );
      await target.scrollIntoViewIfNeeded();
      await page.waitForTimeout(500);
      await browser.startTracing(page, { categories: CATEGORIES });
      await target.click();
      await expect(
        grid.locator(".cm-content[contenteditable=true]"),
      ).toBeFocused();
      await page.waitForTimeout(500);
      const trace = readTrace(await browser.stopTracing());
      const durations = trace.tasks.map((t) => t.dur / 1000);
      results.push({
        row,
        longestTaskMs: Number(Math.max(0, ...durations).toFixed(2)),
        busyMs: Number(durations.reduce((a, b) => a + b, 0).toFixed(2)),
      });
    }
    record("deep cell open", {
      file: "docs/deployment.md",
      bytes: deployment.length,
      offset: at,
      opens: results,
    });
  });

  test("how often the Android composition guard turns off the delete check", async ({
    browser,
  }) => {
    const context = await newContext(browser, { ...devices["Pixel 7"] });
    try {
      const page = await context.newPage();
      await signIn(page);
      const permalink = await createEngram(page, titled("Marks Android"));
      const source = await edit(page, permalink);
      // Every dispatch that shortens the text is a deletion. CodeMirror hands
      // its DOM reads to `dispatch` as specs (with `userEvent`) or as
      // transactions; both are read here, and whether the view was composing
      // when it came.
      await page.evaluate((label) => {
        type View = {
          composing: boolean;
          state: { doc: { length: number } };
          dispatch: (...args: unknown[]) => void;
        };
        const view = (
          window as unknown as { fluidView: (l: string) => View }
        ).fluidView(label);
        const tally = {
          deletes: 0,
          whileComposing: 0,
          composeEvents: 0,
          checkedEvents: 0,
          events: [] as string[],
        };
        (window as unknown as { fluidTally: typeof tally }).fluidTally = tally;
        const known = ["input.type.compose", "delete.backward", "delete"];
        const eventOf = (arg: unknown): string => {
          const spec = arg as {
            userEvent?: string;
            isUserEvent?: (e: string) => boolean;
          };
          if (typeof spec.userEvent === "string") return spec.userEvent;
          if (typeof spec.isUserEvent === "function") {
            return known.find((e) => spec.isUserEvent?.(e)) ?? "";
          }
          return "";
        };
        const dispatch = view.dispatch.bind(view);
        view.dispatch = (...args: unknown[]) => {
          const before = view.state.doc.length;
          const composing = view.composing;
          dispatch(...args);
          if (view.state.doc.length >= before) return;
          const event = eventOf(args[0]);
          tally.deletes += 1;
          tally.events.push(
            `${event || "?"}${composing ? " (composing)" : ""}`,
          );
          if (event === "input.type.compose") tally.composeEvents += 1;
          if (composing || event === "input.type.compose") {
            tally.whileComposing += 1;
          } else if (event.startsWith("delete")) {
            tally.checkedEvents += 1;
          }
        };
      }, SOURCE);
      const cdp: CDPSession = await context.newCDPSession(page);
      await clickBeside(line(source, "Watch list"), "two", "after");

      // At two places (the end of a plain line, and right after a bold word):
      // a word composed letter by letter, two letters taken back inside the
      // composition (as a phone keyboard does), then committed; then two
      // Backspaces outside any composition.
      const places: [string, string][] = [
        ["Watch list", "two"],
        ["word and a chart", "bold"],
      ];
      for (const [text, word] of places) {
        await clickBeside(line(source, text), word, "after");
        await expect(source).toBeFocused();
        await page.waitForTimeout(300);
        const typed = " harbour";
        for (let i = 1; i <= typed.length; i += 1) {
          await cdp.send("Input.imeSetComposition", {
            text: typed.slice(0, i),
            selectionStart: i,
            selectionEnd: i,
          });
        }
        for (const n of [typed.length - 1, typed.length - 2]) {
          await cdp.send("Input.imeSetComposition", {
            text: typed.slice(0, n),
            selectionStart: n,
            selectionEnd: n,
          });
        }
        await cdp.send("Input.insertText", { text: typed.slice(0, -2) });
        await page.waitForTimeout(300);
        await page.keyboard.press("Backspace");
        await page.keyboard.press("Backspace");
        await page.waitForTimeout(300);
      }
      // As a phone keyboard usually types after a word: the space committed on
      // its own, then the next word composed and committed, right after the
      // bold "red".
      await clickBeside(line(source, "Second buoy is"), "red", "after");
      await expect(source).toBeFocused();
      await page.waitForTimeout(300);
      await cdp.send("Input.insertText", { text: " " });
      for (const part of ["s", "se", "sea"]) {
        await cdp.send("Input.imeSetComposition", {
          text: part,
          selectionStart: part.length,
          selectionEnd: part.length,
        });
      }
      await cdp.send("Input.insertText", { text: "sea" });
      await page.waitForTimeout(300);
      const spaceThenCompose =
        (await held(page)).doc
          .split("\n")
          .find((l) => l.includes("Second buoy")) ?? "";
      const tally = await page.evaluate(
        () =>
          (
            window as unknown as {
              fluidTally: {
                deletes: number;
                whileComposing: number;
                composeEvents: number;
                checkedEvents: number;
                events: string[];
              };
            }
          ).fluidTally,
      );
      const doc = (await held(page)).doc;
      record("android composition", {
        ...tally,
        lines: doc
          .split("\n")
          .filter(
            (l) => l.startsWith("Watch list") || l.includes("word and a"),
          ),
        spaceThenCompose,
      });
    } finally {
      await context.close();
    }
  });
});
