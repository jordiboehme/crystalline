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

import { mkdirSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { deflateSync } from "node:zlib";

import type { BrowserContext, Locator, Page } from "@playwright/test";
import { expect, test } from "@playwright/test";

const USER = process.env.FLUID_E2E_USER ?? "smoke";
const PASSWORD = process.env.FLUID_E2E_PASSWORD ?? "smoke-password";
const DOMAIN = process.env.FLUID_E2E_DOMAIN ?? "fluid-smoke";
const BASE_PATH = process.env.FLUID_E2E_BASE_PATH ?? "";
const PEER = process.env.FLUID_E2E_PEER ?? "peer";
const PEER_PASSWORD = process.env.FLUID_E2E_PEER_PASSWORD ?? "peer-password";
const REFLECTION_DOMAIN =
  process.env.FLUID_E2E_REFLECTION_DOMAIN ?? "smoke-pages";
/** Where the checkpoint screenshots go, when a run asks for them. */
const SHOTS = process.env.FLUID_E2E_SHOTS ?? "";

const SOURCE = "Engram source";
const SETTING = "Show Markdown marks on the line I edit";
const PICTURE = "![harbor](assets/e2e/harbor-missing.png)";

/** The page every journey starts from, with `reference` in its first paragraph. */
function pageOf(reference: string): string {
  return `# Picture moves\n\nIntro ${reference} words.\n\nSecond paragraph.\n\nThird paragraph.\n\nThe end.\n`;
}

const BODY = pageOf(PICTURE);

/**
 * A picture the daemon can serve: a gradient with a chequered corner, drawn
 * here so the smoke needs no binary fixture and the screenshots show
 * something larger than a few pixels.
 */
function makePng(width: number, height: number): Buffer {
  const table = Array.from({ length: 256 }, (_, n) => {
    let c = n;
    for (let k = 0; k < 8; k += 1) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
    return c >>> 0;
  });
  const crc = (bytes: Buffer): number => {
    let c = 0xffffffff;
    for (const byte of bytes) c = (table[(c ^ byte) & 0xff] ?? 0) ^ (c >>> 8);
    return (c ^ 0xffffffff) >>> 0;
  };
  const chunk = (type: string, data: Buffer): Buffer => {
    const length = Buffer.alloc(4);
    length.writeUInt32BE(data.length);
    const body = Buffer.concat([Buffer.from(type, "ascii"), data]);
    const sum = Buffer.alloc(4);
    sum.writeUInt32BE(crc(body));
    return Buffer.concat([length, body, sum]);
  };
  const header = Buffer.alloc(13);
  header.writeUInt32BE(width, 0);
  header.writeUInt32BE(height, 4);
  header[8] = 8;
  header[9] = 2;
  const rows: number[] = [];
  for (let y = 0; y < height; y += 1) {
    rows.push(0);
    for (let x = 0; x < width; x += 1) {
      const check = ((x >> 4) + (y >> 4)) % 2 === 0 && x < 64 && y < 64;
      rows.push(
        check ? 250 : Math.round((x * 200) / width) + 30,
        check ? 250 : Math.round((y * 160) / height) + 60,
        check ? 250 : 190,
      );
    }
  }
  return Buffer.concat([
    Buffer.from([137, 80, 78, 71, 13, 10, 26, 10]),
    chunk("IHDR", header),
    chunk("IDAT", deflateSync(Buffer.from(rows))),
    chunk("IEND", Buffer.alloc(0)),
  ]);
}

const PNG = makePng(400, 200);

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

/** The files this file stored in the domain, deleted after each test. */
const madeFiles: string[] = [];

/** The reflection pages this file made, and their files, deleted after each test. */
const madePages: { page: string; assets: string[] }[] = [];

/** Stores a picture in the domain and answers its path and a reference to it. */
async function attached(
  page: Page,
  alt = "harbor",
): Promise<{ path: string; reference: string }> {
  unique += 1;
  const path = `assets/e2e/pic-${String(Date.now())}-${String(unique)}.png`;
  const put = await page.request.put(
    `${BASE_PATH}/api/v1/domains/${DOMAIN}/files/${path}`,
    {
      headers: {
        "X-CSRF-Token": await csrf(page),
        "Content-Type": "image/png",
      },
      data: PNG,
    },
  );
  expect(put.ok(), await put.text()).toBeTruthy();
  madeFiles.push(path);
  return { path, reference: `![${alt}](${path})` };
}

test.afterEach(async ({ page }) => {
  if (made.length === 0 && madeFiles.length === 0 && madePages.length === 0) {
    return;
  }
  if (page.url().startsWith("http")) await page.goto("./");
  else await signIn(page);
  const token = await csrf(page);
  for (const entry of madePages.splice(0)) {
    const root = `${BASE_PATH}/api/v1/domains/${REFLECTION_DOMAIN}`;
    await page.request.delete(`${root}/reflections/${entry.page}`, {
      headers: { "X-CSRF-Token": token },
    });
    for (const asset of entry.assets) {
      await page.request.delete(`${root}/reflection/assets/${asset}`, {
        headers: { "X-CSRF-Token": token },
      });
    }
  }
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
  for (const path of madeFiles.splice(0)) {
    await page.request.delete(
      `${BASE_PATH}/api/v1/domains/${DOMAIN}/files/${path}`,
      { headers: { "X-CSRF-Token": token } },
    );
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

/** The page text in the editor: the document after its frontmatter. */
async function text(page: Page): Promise<string> {
  const { doc } = await held(page);
  const match = /^---\n[\s\S]*?\n---\n\n?/.exec(doc);
  return match ? doc.slice(match[0].length) : doc;
}

/** Opens the editor on the engram and answers its source and its picture. */
async function edit(
  page: Page,
  permalink: string,
  drawn = true,
  ready = "Third paragraph.",
): Promise<{ source: Locator; picture: Locator }> {
  await page.goto(`d/${DOMAIN}/e/${permalink}`);
  await page.getByRole("link", { name: "Edit" }).click();
  const source = page.getByRole("textbox", { name: SOURCE });
  await expect(source).toBeVisible();
  await expect(source).toContainText(ready);
  const picture = source.locator(".cm-picture").first();
  if (drawn) await expect(picture).toBeVisible();
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

  test("a file dropped on the second paragraph shows the drop line and lands under it", async ({
    page,
  }) => {
    const plain =
      "# Picture moves\n\nFirst paragraph.\n\nSecond paragraph.\n\nThird paragraph.\n";
    const permalink = await createEngram(page, plain);
    const { source } = await edit(page, permalink, false);
    const second = paragraph(source, "Second paragraph.");
    const third = paragraph(source, "Third paragraph.");
    const name = `shot-${String(Date.now())}.png`;
    const transfer = await fileTransfer(page, name);
    const where = await middleOf(second);
    const init = { dataTransfer: transfer, clientX: where.x, clientY: where.y };
    await second.dispatchEvent("dragenter", init);
    await second.dispatchEvent("dragover", init);
    // The line is under the second paragraph: between its middle and the
    // third paragraph's middle.
    const line = page.locator(".cm-drop-line");
    await expect(line).toHaveCount(1);
    const lineBox = await line.boundingBox();
    expect(lineBox?.y ?? 0).toBeGreaterThan(where.y);
    expect(lineBox?.y ?? 0).toBeLessThan((await middleOf(third)).y);
    await second.dispatchEvent("drop", init);
    await expect(line).toHaveCount(0);
    const expected = new RegExp(
      `^# Picture moves\\n\\nFirst paragraph\\.\\n\\nSecond paragraph\\.\\n\\n(!\\[${name.replace(".", "\\.")}\\]\\((assets/\\d{4}/\\d{2}/[^)]*\\.png)\\))\\n+Third paragraph\\.\\n$`,
    );
    await expect
      .poll(
        async () => {
          await settle(page);
          return expected.test(await bodyOnDisk(page, permalink));
        },
        { timeout: 20_000 },
      )
      .toBe(true);
    const stored = expected.exec(await bodyOnDisk(page, permalink))?.[2];
    if (stored !== undefined) madeFiles.push(stored);
    // Drawn as the picture, with no reference in sight.
    await expect(source.locator(".cm-picture img")).toBeVisible();
    await expect(source).not.toContainText("![");
  });

  test("a picture is selected with its frame and four handles, then set right and small from the bar", async ({
    page,
  }) => {
    const { path, reference } = await attached(page);
    const permalink = await createEngram(page, pageOf(reference));
    const { picture } = await edit(page, permalink);
    await expect(picture.locator("img")).toBeVisible();
    const at = await middleOf(picture);
    await page.mouse.click(at.x, at.y);
    await expect(picture).toHaveClass(/cm-picture-selected/);
    await expect(picture.locator(".cm-picture-handle")).toHaveCount(4);
    const bar = page.getByRole("toolbar", { name: "Picture options" });
    await expect(bar).toBeVisible();
    await bar.getByRole("button", { name: "Right with text wrap" }).click();
    await bar.getByRole("button", { name: "Small", exact: true }).click();
    const formatted = pageOf(`![harbor](${path}#right,w=25%)`);
    await expect.poll(async () => await text(page)).toBe(formatted);
    await expect
      .poll(
        async () => {
          await settle(page);
          return bodyOnDisk(page, permalink);
        },
        { timeout: 20_000 },
      )
      .toBe(formatted);
  });

  test("with the keyboard alone the alt text changes, and Escape changes nothing", async ({
    page,
  }) => {
    const { path, reference } = await attached(page);
    const permalink = await createEngram(page, pageOf(reference));
    const { source, picture } = await edit(page, permalink);
    await expect(picture.locator("img")).toBeVisible();
    const bar = page.getByRole("toolbar", { name: "Picture options" });
    const alt = bar.getByRole("textbox", { name: "Alt text" });

    /** Caret before the picture, ArrowRight, Alt+F10, arrows to the field. */
    const toAltField = async (): Promise<void> => {
      const { doc } = await held(page);
      const from = doc.indexOf("![");
      const end = doc.indexOf(")", from) + 1;
      await caretAt(page, from);
      await page.keyboard.press("ArrowRight");
      await steady(page, [from, end]);
      await page.keyboard.press("Alt+F10");
      for (let tries = 0; tries < 14; tries += 1) {
        if (await alt.evaluate((el) => el === document.activeElement)) break;
        await page.keyboard.press("ArrowRight");
      }
      await expect(alt).toBeFocused();
    };

    await toAltField();
    await page.keyboard.press("End");
    await page.keyboard.type(" at dusk");
    await page.keyboard.press("Enter");
    const renamed = pageOf(`![harbor at dusk](${path})`);
    await expect.poll(async () => await text(page)).toBe(renamed);
    await expect(source).toBeFocused();

    await toAltField();
    await page.keyboard.type(" abandoned");
    await page.keyboard.press("Escape");
    await expect(source).toBeFocused();
    // Where the selection rests after Escape is not asserted: it reads the
    // picture in most runs and the start of the page in some (see the report).
    // The abandoned text never reaches the page, however late a blur is.
    for (let frames = 0; frames < 6; frames += 1) {
      await settled(page);
      expect(await text(page)).toBe(renamed);
    }
    await expect
      .poll(
        async () => {
          await settle(page);
          return bodyOnDisk(page, permalink);
        },
        { timeout: 20_000 },
      )
      .toBe(renamed);
  });

  for (const selectFirst of [false, true]) {
    test(`a real drag moves ${selectFirst ? "a selected" : "an unselected"} picture once, and Mod-z puts it back byte for byte`, async ({
      page,
    }) => {
      const { reference } = await attached(page);
      const permalink = await createEngram(page, pageOf(reference));
      const { source, picture } = await edit(page, permalink);
      await expect(picture.locator("img")).toBeVisible();
      if (selectFirst) {
        const at = await middleOf(picture);
        await page.mouse.click(at.x, at.y);
        await expect(picture).toHaveClass(/cm-picture-selected/);
      }
      await dragOnto(page, picture, paragraph(source, "Third paragraph."));
      await expectMoved(page, permalink, reference);
      await page.keyboard.press("ControlOrMeta+z");
      await expect.poll(async () => await text(page)).toBe(pageOf(reference));
      await expect
        .poll(
          async () => {
            await settle(page);
            return bodyOnDisk(page, permalink);
          },
          { timeout: 20_000 },
        )
        .toBe(pageOf(reference));
    });
  }

  test("Backspace on a selected picture removes it and its line, and the file stays attached", async ({
    page,
  }) => {
    const { path, reference } = await attached(page);
    const alone = `# Picture moves\n\nIntro words.\n\n${reference}\n\nSecond paragraph.\n\nThird paragraph.\n\nThe end.\n`;
    const permalink = await createEngram(page, alone);
    const { source, picture } = await edit(page, permalink);
    await expect(picture.locator("img")).toBeVisible();
    const at = await middleOf(picture);
    await page.mouse.click(at.x, at.y);
    await expect(picture).toHaveClass(/cm-picture-selected/);
    await page.keyboard.press("Backspace");
    await expect(source.locator(".cm-picture")).toHaveCount(0);
    const without =
      "# Picture moves\n\nIntro words.\n\nSecond paragraph.\n\nThird paragraph.\n\nThe end.\n";
    await expect.poll(async () => await text(page)).toBe(without);
    await expect
      .poll(
        async () => {
          await settle(page);
          return bodyOnDisk(page, permalink);
        },
        { timeout: 20_000 },
      )
      .toBe(without);
    // The stored file is untouched: the domain's attachment listing still
    // holds it. (The page's own rail lists only files the page names.)
    const listing = await page.request.get(
      `${BASE_PATH}/api/v1/domains/${DOMAIN}/attachments`,
    );
    const rows = ((await listing.json()) as { attachments: { path: string }[] })
      .attachments;
    expect(rows.map((row) => row.path)).toContain(path);
  });

  test("an external picture shows one line in place of the size controls, and can be moved and removed", async ({
    page,
  }) => {
    await routeExternal(page);
    const external = `![lighthouse](${EXTERNAL})`;
    const permalink = await createEngram(page, pageOf(external));
    const { source, picture } = await edit(page, permalink);
    await expect(picture.locator("img")).toBeVisible();
    const at = await middleOf(picture);
    await page.mouse.click(at.x, at.y);
    await expect(picture).toHaveClass(/cm-picture-selected/);
    const bar = page.getByRole("toolbar", { name: "Picture options" });
    await expect(bar).toContainText(
      "Size and place can be set only for attached pictures.",
    );
    for (const name of ["Small", "Medium", "Full width", "Centered"]) {
      await expect(bar.getByRole("button", { name })).toHaveCount(0);
    }
    await dragOnto(page, picture, paragraph(source, "Third paragraph."));
    await expectMoved(page, permalink, external);
    await expect(bar).toBeVisible();
    await bar.getByRole("button", { name: "Remove picture" }).click();
    await expect(source.locator(".cm-picture")).toHaveCount(0);
    await expect.poll(async () => await text(page)).not.toContain(EXTERNAL);
    expect(await text(page)).toContain("The end.");
  });

  test("a corner handle dragged to three quarters of the column writes about w=75% as one change", async ({
    page,
  }) => {
    const { path, reference } = await attached(page);
    const permalink = await createEngram(page, pageOf(reference));
    const { picture } = await edit(page, permalink);
    const image = picture.locator("img");
    await expect(image).toBeVisible();
    const at = await middleOf(picture);
    await page.mouse.click(at.x, at.y);
    await expect(picture).toHaveClass(/cm-picture-selected/);
    const handle = picture.locator(".cm-picture-handle[data-corner=se]");
    const grip = await middleOf(handle);
    // The share is of the line the picture stands in.
    const column = await picture.evaluate((el) => {
      const line = el.closest(".cm-line");
      if (!line) return 0;
      const style = getComputedStyle(line);
      return (
        line.clientWidth -
        Number.parseFloat(style.paddingLeft) -
        Number.parseFloat(style.paddingRight)
      );
    });
    expect(column).toBeGreaterThan(0);
    await page.mouse.move(grip.x, grip.y);
    await page.mouse.down();
    let x = grip.x;
    for (let tries = 0; tries < 8; tries += 1) {
      const width = (await image.boundingBox())?.width ?? 0;
      const gap = column * 0.75 - width;
      if (Math.abs(gap) < column * 0.02) break;
      // A centered picture grows on both sides: half the gap per pixel moved.
      x += gap / 2;
      await page.mouse.move(x, grip.y, { steps: 6 });
    }
    await page.mouse.up();
    await expect
      .poll(async () => await text(page))
      .toMatch(new RegExp(`\\(${path}#w=(7\\d|80)%\\)`));
    const written = await text(page);
    expect(written.split("w=")).toHaveLength(2);
    // One change: one undo puts the page back.
    await page.keyboard.press("ControlOrMeta+z");
    await expect.poll(async () => await text(page)).toBe(pageOf(reference));
  });

  test("Raw with a picture selected shows the source, no picture and no bar", async ({
    page,
  }) => {
    const { path, reference } = await attached(page);
    const permalink = await createEngram(page, pageOf(reference));
    const { source, picture } = await edit(page, permalink);
    await expect(picture.locator("img")).toBeVisible();
    const at = await middleOf(picture);
    await page.mouse.click(at.x, at.y);
    await expect(picture).toHaveClass(/cm-picture-selected/);
    await expect(
      page.getByRole("toolbar", { name: "Picture options" }),
    ).toBeVisible();
    await page.getByRole("button", { name: "Raw", exact: true }).click();
    await expect(source).toContainText(`![harbor](${path})`);
    await expect(source.locator(".cm-picture")).toHaveCount(0);
    await expect(
      page.getByRole("toolbar", { name: "Picture options" }),
    ).toHaveCount(0);
  });

  test("in a Reflection a file dropped on a section heading lands below it and the section ids stay", async ({
    page,
  }) => {
    const pagePath = `e2e-pictures-${String(Date.now())}.md`;
    const root = `${BASE_PATH}/api/v1/domains/${REFLECTION_DOMAIN}`;
    const entry = { page: pagePath, assets: [] as string[] };
    madePages.push(entry);
    const put = await page.request.put(`${root}/reflections/${pagePath}`, {
      headers: { "X-CSRF-Token": await csrf(page) },
      data: {
        markdown:
          "# Pictures page\n\nIntro.\n\n## Night deploys\n\nDeploys run at night.\n\n## Day deploys\n\nBy day.\n",
        create: true,
      },
    });
    expect(put.ok(), await put.text()).toBeTruthy();
    const sourceOf = async (): Promise<string> =>
      (
        (await (
          await page.request.get(`${root}/reflections/${pagePath}`)
        ).json()) as {
          markdown: string;
        }
      ).markdown;
    const ids = (markdown: string): string[] =>
      Array.from(markdown.matchAll(/<!-- r:([^ ]+) -->/g), (m) => m[1] ?? "");
    const before = await sourceOf();
    expect(ids(before)).toHaveLength(2);

    await page.goto(`d/${REFLECTION_DOMAIN}/reflection-edit/${pagePath}`);
    const editor = page.getByRole("textbox", { name: "Page source" });
    await expect(editor).toContainText("Deploys run at night.");
    const heading = editor
      .locator(":scope > .cm-line")
      .filter({ hasText: "Night deploys" })
      .first();
    const name = `section-${String(Date.now())}.png`;
    const transfer = await fileTransfer(page, name);
    const where = await middleOf(heading);
    const init = { dataTransfer: transfer, clientX: where.x, clientY: where.y };
    await heading.dispatchEvent("dragenter", init);
    await heading.dispatchEvent("dragover", init);
    await expect(page.locator(".cm-drop-line")).toHaveCount(1);
    await heading.dispatchEvent("drop", init);
    await expect(page.locator(".cm-drop-line")).toHaveCount(0);

    await expect
      .poll(sourceOf, { timeout: 20_000 })
      .toMatch(/## Night deploys\n(<!-- r:[^ ]+ -->\n)?\n?!\[section-/);
    const after = await sourceOf();
    expect(ids(after)).toEqual(ids(before));
    expect(after.indexOf("![section-")).toBeLessThan(
      after.indexOf("Deploys run at night."),
    );
    const stored = /\(\.\.\/assets\/([^)]+)\)/.exec(after)?.[1];
    if (stored !== undefined) entry.assets.push(stored);
  });

  test("two browsers: one moves a picture while the other types where it lands", async ({
    browser,
  }) => {
    const contextA = await browser.newContext();
    const contextB = await browser.newContext();
    try {
      await installViewHandle(contextA);
      await installViewHandle(contextB);
      const pageA = await contextA.newPage();
      const pageB = await contextB.newPage();
      await signIn(pageA);
      await signIn(pageB, PEER, PEER_PASSWORD);
      const { reference } = await attached(pageA);
      const permalink = await createEngram(pageA, pageOf(reference));
      const a = await edit(pageA, permalink);
      const b = await edit(pageB, permalink);
      await expect(b.picture.locator("img")).toBeVisible();
      await expect(pageA.locator(".cm-ySelectionCaret")).toHaveCount(0);

      // B puts the caret at the end of the third paragraph.
      const thirdB = paragraph(b.source, "Third paragraph.");
      await thirdB.click();
      await pageB.keyboard.press("End");
      await expect(pageA.locator(".cm-ySelectionCaret")).not.toHaveCount(0);

      // A holds the picture over the third paragraph while B types.
      const start = await middleOf(a.picture);
      await pageA.mouse.move(start.x, start.y);
      await pageA.mouse.down();
      await pageA.mouse.move(start.x + 8, start.y + 8, { steps: 4 });
      const thirdA = paragraph(a.source, "Third paragraph.");
      const target = await middleOf(thirdA);
      await pageA.mouse.move(target.x, target.y, { steps: 12 });
      await expect(pageA.locator(".cm-drop-line")).toHaveCount(1);
      await pageB.keyboard.type(" bravo");
      await expect
        .poll(async () => await text(pageA))
        .toContain("Third paragraph. bravo");
      const again = await middleOf(paragraph(a.source, "Third paragraph."));
      await pageA.mouse.move(again.x + 2, again.y, { steps: 2 });
      await expect(pageA.locator(".cm-drop-line")).toHaveCount(1);
      await pageA.mouse.up();

      const expected = `# Picture moves\n\nIntro  words.\n\nSecond paragraph.\n\nThird paragraph. bravo\n\n${reference}\n\n\nThe end.\n`;
      for (const page of [pageA, pageB]) {
        await expect.poll(async () => await text(page)).toBe(expected);
      }
      await expect
        .poll(() => bodyOnDisk(pageA, permalink), { timeout: 20_000 })
        .toBe(expected);
    } finally {
      await contextA.close();
      await contextB.close();
    }
  });
});

const EXTERNAL = "https://pictures.example/lighthouse.png";

/** Answers the external picture's address with the test picture. */
async function routeExternal(page: Page): Promise<void> {
  await page.route("https://pictures.example/**", (route) =>
    route.fulfill({ contentType: "image/png", body: PNG }),
  );
}

/**
 * A drag payload carrying one PNG. Playwright cannot drag an operating
 * system file, so the page builds the transfer and the journey dispatches
 * the drag events with it.
 */
function fileTransfer(page: Page, name: string) {
  return page.evaluateHandle(
    ({ bytes, fileName }) => {
      const transfer = new DataTransfer();
      transfer.items.add(
        new File([new Uint8Array(bytes)], fileName, { type: "image/png" }),
      );
      return transfer;
    },
    { bytes: Array.from(PNG), fileName: name },
  );
}

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
function movedOf(reference: string): string {
  return `# Picture moves\n\nIntro  words.\n\nSecond paragraph.\n\nThird paragraph.\n\n${reference}\n\n\nThe end.\n`;
}

/** The editor and then the file hold the moved page, the picture once and selected. */
async function expectMoved(
  page: Page,
  permalink: string,
  reference: string = PICTURE,
): Promise<void> {
  const moved = movedOf(reference);
  await expect.poll(async () => (await text(page)).endsWith(moved)).toBe(true);
  const { doc, from, to } = await held(page);
  expect(doc.split(reference)).toHaveLength(2);
  const at = doc.indexOf(reference);
  expect([from, to]).toEqual([at, at + reference.length]);
  await expect
    .poll(
      async () => {
        await settle(page);
        return bodyOnDisk(page, permalink);
      },
      { timeout: 20_000 },
    )
    .toBe(moved);
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

async function shot(page: Page, name: string): Promise<void> {
  if (!SHOTS) return;
  mkdirSync(SHOTS, { recursive: true });
  await page.screenshot({ path: join(SHOTS, name) });
}

/** Two drawn frames after the last change, so a screenshot shows the settled page. */
function settled(page: Page): Promise<void> {
  return page.evaluate(
    () =>
      new Promise<void>((resolve) => {
        requestAnimationFrame(() => {
          requestAnimationFrame(() => {
            resolve();
          });
        });
      }),
  );
}

const LONG =
  "The harbor master keeps the tide tables on the wall of the office and reads them aloud at dawn. Boats leave on the ebb and come back on the flood, and nobody argues with the water. A long paragraph is what shows a float best, because the lines beside the picture wrap around it and the lines below it run back to the full width of the column. ";

/** The checkpoint screenshots Jordi looks at; only when a run asks for them. */
test.describe("pictures checkpoint", () => {
  test.skip(!SHOTS, "set FLUID_E2E_SHOTS to take the checkpoint screenshots");

  test.beforeEach(async ({ page, context }) => {
    await installViewHandle(context);
    await signIn(page);
  });

  test("the checkpoint screenshots", async ({ page }) => {
    await routeExternal(page);
    const centered = await attached(page, "Harbor at noon");
    const floated = await attached(page, "Lighthouse");
    const floating = `![Lighthouse](${floated.path}#right,w=40%)`;
    const external = `![Pier](${EXTERNAL})`;
    const body = [
      "# Harbor notes",
      "",
      `Morning ${centered.reference} then coffee.`,
      "",
      "A short paragraph under the first picture.",
      "",
      external,
      "",
      `${floating} ${LONG.repeat(6)}`,
      "",
      "Closing paragraph, back at the full width.",
      "",
    ].join("\n");
    const permalink = await createEngram(page, body);
    const { source, picture } = await edit(
      page,
      permalink,
      true,
      "Harbor notes",
    );
    await expect(source.locator(".cm-picture img").first()).toBeVisible();
    await settled(page);
    await expect(source).not.toContainText("![");
    await shot(page, "01-pictures-in-place.png");

    // A file over the text: the drop line under the block it would land after.
    const short = paragraph(source, "A short paragraph under");
    const transfer = await fileTransfer(page, "sketch.png");
    const where = await middleOf(short);
    const init = { dataTransfer: transfer, clientX: where.x, clientY: where.y };
    await short.dispatchEvent("dragenter", init);
    await short.dispatchEvent("dragover", init);
    await expect(page.locator(".cm-drop-line")).toHaveCount(1);
    await shot(page, "02-drop-line.png");
    await short.dispatchEvent("dragend", init);
    await expect(page.locator(".cm-drop-line")).toHaveCount(0);

    // A selected picture: frame, four handles and the floating bar.
    const at = await middleOf(picture);
    await page.mouse.click(at.x, at.y);
    await expect(picture).toHaveClass(/cm-picture-selected/);
    await expect(
      page.getByRole("toolbar", { name: "Picture options" }),
    ).toBeVisible();
    await settled(page);
    await shot(page, "03-selected-bar.png");

    // The alt field focused, by the keyboard.
    const alt = page
      .getByRole("toolbar", { name: "Picture options" })
      .getByRole("textbox", { name: "Alt text" });
    await page.keyboard.press("Alt+F10");
    for (let tries = 0; tries < 14; tries += 1) {
      if (await alt.evaluate((el) => el === document.activeElement)) break;
      await page.keyboard.press("ArrowRight");
    }
    await expect(alt).toBeFocused();
    await shot(page, "04-alt-field.png");
    await page.keyboard.press("Escape");

    // An external picture: the bar says one line.
    const pier = source.locator(".cm-picture").nth(1);
    await pier.scrollIntoViewIfNeeded();
    const pierAt = await middleOf(pier);
    await page.mouse.click(pierAt.x, pierAt.y);
    await expect(pier).toHaveClass(/cm-picture-selected/);
    await expect(
      page.getByRole("toolbar", { name: "Picture options" }),
    ).toContainText("only for attached pictures");
    await settled(page);
    await shot(page, "05-external-bar.png");

    // The float next to a long paragraph: at the top, then scrolled until the
    // picture's own line has left the viewport.
    const lighthouse = source.locator(".cm-picture").nth(2);
    await lighthouse.scrollIntoViewIfNeeded();
    await page.mouse.click(0, 0);
    await settled(page);
    await shot(page, "06-float-top.png");
    const box = await lighthouse.boundingBox();
    const viewport = page.viewportSize();
    await page.mouse.move(
      (viewport?.width ?? 1280) / 2,
      (viewport?.height ?? 720) / 2,
    );
    await page.mouse.wheel(0, (box?.height ?? 200) + 80);
    await settled(page);
    await settled(page);
    await shot(page, "07-float-scrolled.png");

    // Dark.
    await page.emulateMedia({ colorScheme: "dark" });
    await edit(page, permalink, true, "Harbor notes");
    await expect(page.locator("html")).toHaveAttribute("data-theme", "dark");
    const darkPicture = source.locator(".cm-picture").first();
    const darkAt = await middleOf(darkPicture);
    await page.mouse.click(darkAt.x, darkAt.y);
    await expect(darkPicture).toHaveClass(/cm-picture-selected/);
    await settled(page);
    await shot(page, "08-dark-selected.png");
  });

  test("the room screenshots: a picture moved while a second browser types", async ({
    browser,
  }) => {
    const contextA = await browser.newContext();
    const contextB = await browser.newContext();
    try {
      await installViewHandle(contextA);
      await installViewHandle(contextB);
      const pageA = await contextA.newPage();
      const pageB = await contextB.newPage();
      await signIn(pageA);
      await signIn(pageB, PEER, PEER_PASSWORD);
      const { reference } = await attached(pageA, "Harbor");
      const permalink = await createEngram(pageA, pageOf(reference));
      const a = await edit(pageA, permalink);
      const b = await edit(pageB, permalink);
      await expect(b.picture.locator("img")).toBeVisible();
      await paragraph(b.source, "Third paragraph.").click();
      await pageB.keyboard.press("End");
      await expect(pageA.locator(".cm-ySelectionCaret")).not.toHaveCount(0);

      const start = await middleOf(a.picture);
      await pageA.mouse.move(start.x, start.y);
      await pageA.mouse.down();
      await pageA.mouse.move(start.x + 8, start.y + 8, { steps: 4 });
      const target = await middleOf(paragraph(a.source, "Third paragraph."));
      await pageA.mouse.move(target.x, target.y, { steps: 12 });
      await expect(pageA.locator(".cm-drop-line")).toHaveCount(1);
      await pageB.keyboard.type(" bravo");
      await expect
        .poll(async () => await text(pageA))
        .toContain("Third paragraph. bravo");
      await settled(pageA);
      await shot(pageA, "09-room-drag-A.png");
      await shot(pageB, "10-room-drag-B.png");
      const again = await middleOf(paragraph(a.source, "Third paragraph."));
      await pageA.mouse.move(again.x + 2, again.y, { steps: 2 });
      await pageA.mouse.up();
      await expect
        .poll(async () => await text(pageB))
        .toContain("Third paragraph. bravo\n\n![");
      await settled(pageA);
      await settled(pageB);
      await shot(pageA, "11-room-after-A.png");
      await shot(pageB, "12-room-after-B.png");
    } finally {
      await contextA.close();
      await contextB.close();
    }
  });
});
