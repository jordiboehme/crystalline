/**
 * Pictures in the editor, in a real browser against a real daemon.
 *
 * What jsdom cannot answer here: that a real press on a picture selects
 * exactly its reference and the browser's own selection does not move it
 * afterwards (the press keeps its default, so a drag can start), and that a
 * real drag of a picture shows the drop line and moves the picture below
 * the block it is dropped on, once, with nothing left behind.
 *
 * Every journey makes its own engram through the REST API and reads the file
 * back through it. The picture points at a file the domain does not hold, so
 * it is drawn as its alt text and a sentence: the wrapper a press and a drag
 * act on is the same either way. Shares the daemon, the fixture domain and
 * the accounts `run-smoke.sh` sets up; run it the same way (`bash
 * fluid/e2e/run-smoke.sh e2e/pictures.spec.ts`).
 */

import { readFileSync } from "node:fs";

import type { BrowserContext, Locator, Page } from "@playwright/test";
import { expect, test } from "@playwright/test";

const USER = process.env.FLUID_E2E_USER ?? "smoke";
const PASSWORD = process.env.FLUID_E2E_PASSWORD ?? "smoke-password";
const DOMAIN = process.env.FLUID_E2E_DOMAIN ?? "fluid-smoke";
const BASE_PATH = process.env.FLUID_E2E_BASE_PATH ?? "";

const SOURCE = "Engram source";
const SETTING = "Show Markdown marks on the line I edit";
const PICTURE = "![harbor](assets/e2e/harbor-missing.png)";
const BODY = `# Picture moves\n\nIntro ${PICTURE} words.\n\nSecond paragraph.\n\nThird paragraph.\n\nThe end.\n`;

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

/** The engrams this file made, deleted after each test. */
const made: string[] = [];

let unique = 0;

/** Makes an engram holding `body` and returns its permalink. */
async function createEngram(page: Page, body: string): Promise<string> {
  unique += 1;
  const response = await page.request.post(
    `${BASE_PATH}/api/v1/domains/${DOMAIN}/engrams`,
    {
      headers: { "X-CSRF-Token": await csrf(page) },
      data: {
        title: `Picture moves ${String(Date.now())}${String(unique)}`,
        content: body,
        tags: ["pictures"],
      },
    },
  );
  expect(response.ok(), await response.text()).toBeTruthy();
  const { permalink } = (await response.json()) as { permalink: string };
  made.push(permalink);
  return permalink;
}

test.afterEach(async ({ page }) => {
  if (made.length === 0) return;
  if (page.url().startsWith("http")) await page.goto("./");
  else await signIn(page);
  const token = await csrf(page);
  for (const permalink of made.splice(0)) {
    const at = `${BASE_PATH}/api/v1/domains/${DOMAIN}/engrams/${permalink}`;
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

/** The engram's body as the daemon holds it now: the file after its frontmatter. */
async function bodyOnDisk(page: Page, permalink: string): Promise<string> {
  const response = await page.request.get(
    `${BASE_PATH}/api/v1/domains/${DOMAIN}/engrams/${permalink}`,
  );
  const file = ((await response.json()) as { content: string }).content;
  const match = /^---\n[\s\S]*?\n---\n\n?/.exec(file);
  return match ? file.slice(match[0].length) : file;
}

/** Saves the solo surface when it has a Save button; a room saves itself. */
async function settle(page: Page): Promise<void> {
  const save = page.getByRole("button", { name: "Save", exact: true });
  if ((await save.count()) > 0 && (await save.isEnabled())) await save.click();
}

/**
 * The CodeMirror view behind a content element, through the field
 * `EditorView.findFromDOM` reads (as `hidden-marks.spec.ts` explains), as
 * `window.fluidView(label)`.
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

interface Held {
  doc: string;
  from: number;
  to: number;
}

/** The editor's text and its main selection. Read only; `caretAt` writes. */
function held(page: Page): Promise<Held> {
  return page.evaluate((name) => {
    type View = {
      state: {
        doc: { toString(): string };
        selection: { main: { from: number; to: number } };
      };
    };
    const view = (
      window as unknown as { fluidView?: (label: string) => View | null }
    ).fluidView?.(name);
    if (!view) throw new Error(`no editor view for ${name}`);
    return {
      doc: view.state.doc.toString(),
      from: view.state.selection.main.from,
      to: view.state.selection.main.to,
    };
  }, SOURCE);
}

/** Opens the editor on the engram and answers its source and its picture. */
async function edit(
  page: Page,
  permalink: string,
): Promise<{ source: Locator; picture: Locator }> {
  await page.goto(`d/${DOMAIN}/e/${permalink}`);
  await page.getByRole("link", { name: "Edit" }).click();
  const source = page.getByRole("textbox", { name: SOURCE });
  await expect(source).toBeVisible();
  await expect(source).toContainText("Third paragraph.");
  const picture = source.locator(".cm-picture").first();
  await expect(picture).toBeVisible();
  return { source, picture };
}

/** The middle of an element, in page coordinates. */
async function middleOf(target: Locator): Promise<{ x: number; y: number }> {
  const box = await target.boundingBox();
  if (!box) throw new Error("not drawn");
  return { x: box.x + box.width / 2, y: box.y + box.height / 2 };
}

test.describe("pictures", () => {
  test.beforeEach(async ({ page, context }) => {
    await installViewHandle(context);
    await signIn(page);
  });

  test("a press on a picture selects exactly its reference, and the selection stays", async ({
    page,
  }) => {
    const permalink = await createEngram(page, BODY);
    const { picture } = await edit(page, permalink);
    const at = await middleOf(picture);
    await page.mouse.click(at.x, at.y);
    const { doc } = await held(page);
    const from = doc.indexOf(PICTURE);
    expect(from).toBeGreaterThan(0);
    await expect
      .poll(async () => {
        const now = await held(page);
        return [now.from, now.to];
      })
      .toEqual([from, from + PICTURE.length]);
    await expect(picture).toHaveClass(/cm-picture-selected/);
    // The browser's own selection settles after the press: the selection
    // CodeMirror then reads from it must still be the picture.
    await steady(page, [from, from + PICTURE.length]);
    await expect(picture).toHaveClass(/cm-picture-selected/);
  });

  test("a picture dragged onto a later paragraph moves below it, once, and shows the drop line", async ({
    page,
  }) => {
    const permalink = await createEngram(page, BODY);
    const { source, picture } = await edit(page, permalink);
    // One gesture from a picture nobody selected.
    await dragOnto(page, picture, paragraph(source, "Third paragraph."));
    await expectMoved(page, permalink);
  });

  test("a picture clicked and then dragged moves the same way", async ({
    page,
  }) => {
    const permalink = await createEngram(page, BODY);
    const { source, picture } = await edit(page, permalink);
    const at = await middleOf(picture);
    await page.mouse.click(at.x, at.y);
    await expect(picture).toHaveClass(/cm-picture-selected/);
    // From a press inside the selection the browser drags the selection.
    await dragOnto(page, picture, paragraph(source, "Third paragraph."));
    await expectMoved(page, permalink);
  });

  test("a picture dragged onto a drawn table moves below the table (P38)", async ({
    page,
  }) => {
    const table = "| Berth | Ship |\n| --- | --- |\n| 1 | Lantern |";
    const body = `# Picture moves\n\nIntro ${PICTURE} words.\n\n${table}\n\nThird paragraph.\n`;
    const permalink = await createEngram(page, body);
    const { source, picture } = await edit(page, permalink);
    const cell = source.locator(".cm-grid td").filter({ hasText: "Lantern" });
    await expect(cell).toBeVisible();
    await dragOnto(page, picture, cell);
    const moved = `# Picture moves\n\nIntro  words.\n\n${table}\n\n${PICTURE}\n\n\nThird paragraph.\n`;
    await expect
      .poll(async () => (await held(page)).doc.endsWith(moved))
      .toBe(true);
    expect((await held(page)).doc.split(PICTURE)).toHaveLength(2);
    await expect
      .poll(
        async () => {
          await settle(page);
          return bodyOnDisk(page, permalink);
        },
        { timeout: 20_000 },
      )
      .toBe(moved);
  });

  test("dragging a corner handle writes the width once, in percent, on release", async ({
    page,
  }) => {
    const path = `assets/e2e/handle-${String(Date.now())}.png`;
    const token = await csrf(page);
    const bytes = readFileSync(
      new URL(
        "./fixtures/reflection-domain/assets/old-harbor.png",
        import.meta.url,
      ),
    );
    const files = `${BASE_PATH}/api/v1/domains/${DOMAIN}/files/${path}`;
    const put = await page.request.put(files, {
      headers: { "X-CSRF-Token": token, "Content-Type": "image/png" },
      data: bytes,
    });
    expect(put.ok(), await put.text()).toBeTruthy();
    try {
      const reference = `![harbor](${path})`;
      const permalink = await createEngram(
        page,
        `# Picture moves\n\nIntro ${reference} words.\n\nSecond paragraph.\n\nThird paragraph.\n\nThe end.\n`,
      );
      const { picture } = await edit(page, permalink);
      await expect(picture.locator("img")).toBeVisible();
      const at = await middleOf(picture);
      await page.mouse.click(at.x, at.y);
      await expect(picture).toHaveClass(/cm-picture-selected/);
      const handle = picture.locator(".cm-picture-handle[data-corner=se]");
      await expect(handle).toBeVisible();
      const grip = await middleOf(handle);
      const before = await held(page);
      await page.mouse.move(grip.x, grip.y);
      await page.mouse.down();
      await page.mouse.move(grip.x + 100, grip.y + 10, { steps: 10 });
      await page.mouse.move(grip.x + 200, grip.y + 20, { steps: 10 });
      // Held: the image grew, nothing was written.
      const grown = await picture.locator("img").boundingBox();
      expect(grown?.width ?? 0).toBeGreaterThan(150);
      expect((await held(page)).doc).toBe(before.doc);
      await page.mouse.up();
      await expect
        .poll(async () => (await held(page)).doc)
        .toMatch(/!\[harbor\]\([^)]*#[^)]*w=\d+%\)/);
      const { doc } = await held(page);
      const written = Number(/w=(\d+)%/.exec(doc)?.[1]);
      expect(written).toBeGreaterThanOrEqual(10);
      expect(written).toBeLessThanOrEqual(100);
      expect(doc.split("w=")).toHaveLength(2);
      // The picture stays selected and the page keeps one reference.
      await expect(picture).toHaveClass(/cm-picture-selected/);
      await expect
        .poll(
          async () => {
            await settle(page);
            return bodyOnDisk(page, permalink);
          },
          { timeout: 20_000 },
        )
        .toMatch(/w=\d+%/);
    } finally {
      await page.request.delete(files, { headers: { "X-CSRF-Token": token } });
    }
  });

  test("with the marks shown, a held press on the block picture under the edited line leaves it in place (P25)", async ({
    page,
  }) => {
    const alone = `# Picture moves\n\nIntro words.\n\n${PICTURE}\n\nThird paragraph.\n\nThe end.\n`;
    const permalink = await createEngram(page, alone);
    expect(await flipSetting(page)).toBe(true);
    try {
      await edit(page, permalink);
      // The caret on the picture's line: the line shows its source and the
      // picture is drawn as a block below it.
      const { doc } = await held(page);
      const from = doc.indexOf(PICTURE);
      await caretAt(page, from + PICTURE.length);
      const block = page.locator(".cm-picture-block").first();
      await expect(block).toBeVisible();
      await page.evaluate(() => {
        (window as unknown as { pressed: Element | null }).pressed =
          document.querySelector(".cm-picture-block");
      });
      const at = await middleOf(block);
      await page.mouse.move(at.x, at.y);
      await page.mouse.down();
      // Nothing moves under the pointer while the press is held: the
      // caret stays where it was and the block is the same element.
      await steady(page, [from + PICTURE.length, from + PICTURE.length]);
      expect(
        await page.evaluate(
          () =>
            (window as unknown as { pressed: Element | null }).pressed ===
            document.querySelector(".cm-picture-block"),
        ),
      ).toBe(true);
      await page.mouse.up();
      await steady(page, [from, from + PICTURE.length]);
      // Selected, it is still the block under its edited line.
      await expect(block).toHaveClass(/cm-picture-selected/);
      expect(
        await page.evaluate(
          () =>
            (window as unknown as { pressed: Element | null }).pressed ===
            document.querySelector(".cm-picture-block"),
        ),
      ).toBe(true);
    } finally {
      expect(await flipSetting(page)).toBe(false);
    }
  });
});

/**
 * Waits until the editor's main selection reads `[from, to]` on several
 * reads in a row, each after the browser drew two more frames: a selection
 * that moves after a press settles fails here, however late it moves within
 * the poll.
 */
async function steady(page: Page, expected: [number, number]): Promise<void> {
  const reads: string[] = [];
  const want = JSON.stringify(expected);
  await expect
    .poll(async () => {
      await page.evaluate(
        () =>
          new Promise<void>((resolve) => {
            requestAnimationFrame(() => {
              requestAnimationFrame(() => {
                resolve();
              });
            });
          }),
      );
      const now = await held(page);
      reads.push(JSON.stringify([now.from, now.to]));
      return reads.length >= 6 && reads.slice(-6).every((r) => r === want);
    })
    .toBe(true);
}

/** Puts the editor's caret at `pos`, as a click there would. */
async function caretAt(page: Page, pos: number): Promise<void> {
  await page.evaluate(
    ([name, at]) => {
      type View = {
        dispatch(spec: unknown): void;
        focus(): void;
      };
      const view = (
        window as unknown as { fluidView?: (label: string) => View | null }
      ).fluidView?.(name);
      if (!view) throw new Error(`no editor view for ${name}`);
      view.dispatch({ selection: { anchor: at }, userEvent: "select.pointer" });
      view.focus();
    },
    [SOURCE, pos] as const,
  );
}

/** A drawn line of the editor, by the text it shows. */
function paragraph(source: Locator, text: string): Locator {
  return source.locator(":scope > .cm-line").filter({ hasText: text }).first();
}

/**
 * Drags `from` with the mouse onto the middle of `onto`, checking that the
 * drop line is drawn while the pointer is there and gone after the drop.
 */
async function dragOnto(page: Page, from: Locator, onto: Locator) {
  const start = await middleOf(from);
  await page.mouse.move(start.x, start.y);
  await page.mouse.down();
  await page.mouse.move(start.x + 8, start.y + 8, { steps: 4 });
  const target = await middleOf(onto);
  await page.mouse.move(target.x, target.y, { steps: 12 });
  await expect(page.locator(".cm-drop-line")).toHaveCount(1);
  await page.mouse.up();
  await expect(page.locator(".cm-drop-line")).toHaveCount(0);
}

/** The page after the picture moved below the third paragraph. */
const MOVED = `# Picture moves\n\nIntro  words.\n\nSecond paragraph.\n\nThird paragraph.\n\n${PICTURE}\n\n\nThe end.\n`;

/** The editor and then the file hold the moved page, the picture once and selected. */
async function expectMoved(page: Page, permalink: string): Promise<void> {
  await expect
    .poll(async () => (await held(page)).doc.endsWith(MOVED))
    .toBe(true);
  const { doc, from, to } = await held(page);
  expect(doc.split(PICTURE)).toHaveLength(2);
  const at = doc.indexOf(PICTURE);
  expect([from, to]).toEqual([at, at + PICTURE.length]);
  await expect
    .poll(
      async () => {
        await settle(page);
        return bodyOnDisk(page, permalink);
      },
      { timeout: 20_000 },
    )
    .toBe(MOVED);
}

/** Flips the "show marks on the line I edit" setting; answers whether it is on. */
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
