/**
 * The rename field in a real browser: what jsdom cannot answer. Real keys, a real
 * IME composition through the DevTools protocol, the field's place over the
 * drawing, a label drawn as typed, the box leaving the window and coming back, and
 * the toolbar's starter through its real menu.
 *
 * Shares the daemon, the fixture domain and the accounts `run-smoke.sh` sets up;
 * run it the same way (`bash fluid/e2e/run-smoke.sh e2e/diagram-rename.spec.ts`).
 */

import type { Page, WebSocketRoute } from "@playwright/test";
import { expect, test } from "@playwright/test";

const USER = process.env.FLUID_E2E_USER ?? "smoke";
const PASSWORD = process.env.FLUID_E2E_PASSWORD ?? "smoke-password";
const DOMAIN = process.env.FLUID_E2E_DOMAIN ?? "fluid-smoke";
const BASE_PATH = process.env.FLUID_E2E_BASE_PATH ?? "";

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

async function openEditor(page: Page, content: string): Promise<void> {
  const me = await page.request.get(`${BASE_PATH}/api/v1/auth/me`);
  const csrf = ((await me.json()) as { csrf: string }).csrf;
  unique += 1;
  const response = await page.request.post(
    `${BASE_PATH}/api/v1/domains/${DOMAIN}/engrams`,
    {
      headers: { "X-CSRF-Token": csrf },
      data: {
        title: `Diagram rename ${String(Date.now())}${String(unique)}`,
        content,
      },
    },
  );
  expect(response.ok(), await response.text()).toBeTruthy();
  const { permalink } = (await response.json()) as { permalink: string };
  made.push(permalink);
  await page.goto(`d/${DOMAIN}/e/${permalink}`);
  await page.getByRole("link", { name: "Edit" }).click();
}

test.beforeEach(async ({ page }) => {
  await signIn(page);
});

test.afterEach(async ({ page }) => {
  const me = await page.request.get(`${BASE_PATH}/api/v1/auth/me`);
  const token = ((await me.json()) as { csrf: string }).csrf;
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

const box = (page: Page) => page.locator(".cm-diagram");
const field = (page: Page) => page.locator(".cm-diagram-label-field");
const FENCE = "```mermaid\nflowchart TD\n  A[First] --> B[Second]\n```";
const DOC = `Above\n\n${FENCE}\n\nBelow marker\n`;

const docText = (page: Page) =>
  page.evaluate(() => {
    const content = document.querySelector(".cm-content") as unknown as {
      cmTile: { view: { state: { doc: { toString(): string } } } };
    };
    return content.cmTile.view.state.doc.toString();
  });

/** An edit no key made here, as a peer's arrives: no user event. */
async function peerEdit(page: Page, before: string, insert: string) {
  await page.evaluate(
    ([marker, text]) => {
      const content = document.querySelector(".cm-content") as unknown as {
        cmTile: {
          view: {
            state: { doc: { toString(): string } };
            dispatch(spec: { changes: { from: number; insert: string } }): void;
          };
        };
      };
      const { view } = content.cmTile;
      view.dispatch({
        changes: {
          from: view.state.doc.toString().indexOf(marker ?? ""),
          insert: text ?? "",
        },
      });
    },
    [before, insert],
  );
}

async function fieldIsFocused(page: Page): Promise<boolean> {
  return page.evaluate(() =>
    document.activeElement?.classList.contains("cm-diagram-label-field"),
  ) as Promise<boolean>;
}

/** Marks the field so a later read can tell it is the same element. */
const markField = (page: Page) =>
  field(page).evaluate((el) => {
    (el as unknown as { marked: boolean }).marked = true;
  });
const fieldIsMarked = (page: Page) =>
  field(page).evaluate((el) => (el as unknown as { marked?: boolean }).marked);

test("a click opens the field over the box, a name and Enter write it, and it draws", async ({
  page,
}) => {
  await openEditor(page, DOC);
  const second = box(page).locator("g.node", { hasText: "Second" });
  await expect(second).toBeVisible();
  const shape = await second.boundingBox();
  await second.click();
  await expect(field(page)).toBeVisible();
  await expect(field(page)).toHaveAccessibleName("Box name");
  await expect(field(page)).toHaveValue("Second");
  const place = await field(page).boundingBox();
  if (shape === null || place === null) throw new Error("no box");
  // Over the box: their middles agree.
  expect(
    Math.abs(place.x + place.width / 2 - (shape.x + shape.width / 2)),
  ).toBeLessThanOrEqual(2);
  expect(
    Math.abs(place.y + place.height / 2 - (shape.y + shape.height / 2)),
  ).toBeLessThanOrEqual(2);
  await expect(box(page).locator(".cm-diagram-focus-ring")).toHaveCount(0);
  await page.keyboard.type("Done");
  await page.keyboard.press("Enter");
  await expect(field(page)).toHaveCount(0);
  await expect
    .poll(() => docText(page))
    .toContain(DOC.replace("B[Second]", "B[Done]"));
  await expect(box(page).locator("g.node", { hasText: "Done" })).toBeVisible();
  await expect
    .poll(() =>
      page.evaluate(() => document.activeElement?.getAttribute("aria-label")),
    )
    .toBe("Box: Done");
  await page.keyboard.press("ControlOrMeta+z");
  await expect.poll(() => docText(page)).toContain(DOC);
});

test("an arrow's field stands at the path's middle, and its label takes the place after", async ({
  page,
}) => {
  await openEditor(page, DOC);
  const hit = box(page).locator("[data-hit-for]");
  await expect(hit).toHaveCount(1);
  const middle = await box(page)
    .locator('path[data-et="edge"]')
    .evaluate((el) => {
      const path = el as SVGGeometryElement;
      const point = path.getPointAtLength(path.getTotalLength() / 2);
      const screen = point.matrixTransform(
        path.getScreenCTM() ?? new DOMMatrix(),
      );
      return { x: screen.x, y: screen.y };
    });
  await page.mouse.click(middle.x, middle.y);
  await expect(field(page)).toHaveAccessibleName("Arrow label");
  await expect(field(page)).toHaveValue("");
  const place = await field(page).boundingBox();
  if (place === null) throw new Error("no field");
  expect(Math.abs(place.x + place.width / 2 - middle.x)).toBeLessThanOrEqual(2);
  expect(Math.abs(place.y + place.height / 2 - middle.y)).toBeLessThanOrEqual(
    2,
  );
  await page.keyboard.type("yes");
  await page.keyboard.press("Enter");
  await expect
    .poll(() => docText(page))
    .toContain("A[First] -->|yes| B[Second]");
});

test("what is typed is what is drawn and what the field shows again", async ({
  page,
}) => {
  await openEditor(page, DOC);
  const typed = [
    'say "hi"',
    "a [b] (c) {d}",
    "#1 and #35; here",
    "&amp; and #quot; stay",
    "party 🎉 time",
    "한국어 이름",
    "x > y < z",
  ];
  let shown = "Second";
  for (const text of typed) {
    await box(page).locator("g.node", { hasText: shown }).click();
    await expect(field(page)).toHaveValue(shown);
    await field(page).fill(text);
    await page.keyboard.press("Enter");
    await expect(field(page)).toHaveCount(0);
    await expect(
      box(page).locator("g.node", { hasText: text }),
      `drawn: ${text}`,
    ).toBeVisible();
    shown = text;
  }
});

test("Shift+Enter makes two rows in the drawing", async ({ page }) => {
  await openEditor(page, DOC);
  await box(page).locator("g.node", { hasText: "Second" }).click();
  await page.keyboard.type("one");
  await page.keyboard.press("Shift+Enter");
  await page.keyboard.type("two");
  await page.keyboard.press("Enter");
  await expect
    .poll(() => docText(page))
    .toContain(DOC.replace("B[Second]", "B[one<br>two]"));
  const node = box(page).locator("g.node", { hasText: "two" });
  await expect(node).toContainText("one");
  await node.click();
  await expect(field(page)).toHaveValue("one\ntwo");
  await page.keyboard.press("Escape");
});

test("a composition goes on through a peer's edits and a new drawing", async ({
  page,
}) => {
  await openEditor(page, DOC);
  await box(page).locator("g.node", { hasText: "Second" }).click();
  await markField(page);
  const cdp = await page.context().newCDPSession(page);
  await cdp.send("Input.imeSetComposition", {
    text: "ㄱ",
    selectionStart: 1,
    selectionEnd: 1,
  });
  await expect(field(page)).toHaveValue("ㄱ");
  const composing = () =>
    field(page).evaluate((el) => {
      const seen = (el as unknown as { ended?: boolean }).ended === true;
      return !seen;
    });
  await field(page).evaluate((el) => {
    el.addEventListener("compositionend", () => {
      (el as unknown as { ended: boolean }).ended = true;
    });
  });

  // In the body, with a new drawing to land.
  await peerEdit(page, "  A[First]", "  %% note\n  C[Third]\n");
  await expect(box(page).locator("g.node")).toHaveCount(3);
  // Right at the first mark, which makes CodeMirror ask for a new element.
  await peerEdit(page, "```mermaid", "peer line\n\n");
  await expect(
    page.locator(".cm-line", { hasText: "peer line" }),
  ).toBeVisible();
  await page.waitForTimeout(300);
  expect(await fieldIsMarked(page)).toBe(true);
  expect(await fieldIsFocused(page)).toBe(true);
  expect(await composing()).toBe(true);

  await cdp.send("Input.imeSetComposition", {
    text: "가",
    selectionStart: 1,
    selectionEnd: 1,
  });
  await cdp.send("Input.insertText", { text: "가" });
  await expect(field(page)).toHaveValue("가");
  await page.keyboard.press("Enter");
  await expect(field(page)).toHaveCount(0);
  await expect.poll(() => docText(page)).toContain("B[가]");
  const text = await docText(page);
  expect(text).toContain("peer line");
  expect(text).toContain("%% note");
});

test("a field whose box leaves the window comes back with its draft and the focus", async ({
  page,
}) => {
  const filler = Array.from(
    { length: 300 },
    (_, i) => `Filler line ${String(i)}`,
  ).join("\n\n");
  await openEditor(page, `Above\n\n${FENCE}\n\n${filler}\n`);
  await box(page).locator("g.node", { hasText: "Second" }).click();
  await page.keyboard.type("Draft");
  await markField(page);
  const before = await docText(page);
  const content = await page.locator(".cm-content").boundingBox();
  await page.mouse.move((content?.x ?? 0) + 100, 400);
  await page.mouse.wheel(0, 100_000);
  await expect(box(page)).toHaveCount(0);
  await page.waitForTimeout(200);
  expect(await docText(page)).toBe(before);
  await page.mouse.wheel(0, -100_000);
  await expect(field(page)).toBeVisible();
  await expect(field(page)).toHaveValue("Draft");
  expect(await fieldIsMarked(page)).toBe(true);
  expect(await fieldIsFocused(page)).toBe(true);
  await page.keyboard.type("ed");
  await page.keyboard.press("Enter");
  await expect.poll(() => docText(page)).toContain("B[Drafted]");
});

test("a blur commits, a click in the text leaves its caret there", async ({
  page,
}) => {
  await openEditor(page, DOC);
  await box(page).locator("g.node", { hasText: "Second" }).click();
  await page.keyboard.type("Done");
  await page.locator(".cm-line", { hasText: "Below marker" }).click();
  await expect(field(page)).toHaveCount(0);
  await expect
    .poll(() => docText(page))
    .toContain(DOC.replace("B[Second]", "B[Done]"));
  await page.keyboard.press("End");
  await page.keyboard.type("!");
  await expect.poll(() => docText(page)).toContain("Below marker!");
});

test("Raw on commits the draft and the text shows", async ({ page }) => {
  await openEditor(page, DOC);
  await box(page).locator("g.node", { hasText: "Second" }).click();
  await page.keyboard.type("New");
  await page.getByRole("button", { name: "Raw", exact: true }).click();
  await expect(box(page)).toHaveCount(0);
  await expect.poll(() => docText(page)).toContain("B[New]");
  await expect(page.locator(".cm-line", { hasText: "B[New]" })).toBeVisible();
});

test("the toolbar's flowchart opens the field on its first box, and typing goes there", async ({
  page,
}) => {
  await openEditor(page, "Intro\n");
  await page.locator(".cm-line", { hasText: "Intro" }).click();
  await page.getByRole("button", { name: "Insert", exact: true }).click();
  await page.getByRole("menuitem", { name: "Diagram" }).click();
  await page.getByRole("menuitem", { name: "Flowchart" }).click();
  await expect(field(page)).toBeVisible();
  await expect(field(page)).toHaveValue("First step");
  await expect.poll(() => fieldIsFocused(page)).toBe(true);
  await page.keyboard.type("Go");
  await page.keyboard.press("Enter");
  await expect.poll(() => docText(page)).toContain("A[Go] --> B[Next step]");
  await expect(field(page)).toHaveCount(0);
  await expect(box(page).locator("g.node", { hasText: "Go" })).toBeVisible();
});

async function insertFlowchart(page: Page): Promise<void> {
  await page.getByRole("button", { name: "Insert", exact: true }).click();
  await page.getByRole("menuitem", { name: "Diagram" }).click();
  await page.getByRole("menuitem", { name: "Flowchart" }).click();
}

test("a starter by mouse on a page with a drawn fence opens the field, and every key lands", async ({
  page,
}) => {
  await openEditor(page, `${DOC}\nTail\n`);
  await expect(box(page).locator("svg")).toBeVisible();
  await page.locator(".cm-line", { hasText: "Tail" }).click();
  await insertFlowchart(page);
  await expect(field(page)).toHaveValue("First step");
  await expect.poll(() => fieldIsFocused(page)).toBe(true);
  await page.waitForTimeout(800);
  await page.keyboard.type("Typed");
  await expect(field(page)).toHaveValue("Typed");
  await page.keyboard.press("Enter");
  await expect.poll(() => docText(page)).toContain("A[Typed] --> B[Next step]");

  // A second one, with its verdict known by now.
  await page.keyboard.press("Escape");
  await page.locator(".cm-line", { hasText: "Tail" }).click();
  await insertFlowchart(page);
  await expect(field(page)).toHaveValue("First step");
  await expect.poll(() => fieldIsFocused(page)).toBe(true);
  await page.keyboard.type("Again");
  await page.keyboard.press("Enter");
  await expect.poll(() => docText(page)).toContain("A[Again] --> B[Next step]");
});

async function savedContent(page: Page): Promise<string> {
  const response = await page.request.get(
    `${BASE_PATH}/api/v1/domains/${DOMAIN}/engrams/${made.at(-1) ?? ""}`,
  );
  return ((await response.json()) as { content: string }).content;
}

/** A press and a release with nothing between them, on what is at `locator`. */
async function zeroClick(page: Page, name: string): Promise<void> {
  const where = await page
    .getByRole("link", { name, exact: true })
    .boundingBox();
  if (where === null) throw new Error(`no ${name}`);
  await page.mouse.move(where.x + where.width / 2, where.y + where.height / 2);
  await page.evaluate(() => undefined);
  await Promise.all([page.mouse.down(), page.mouse.up()]);
}

test("Done at 0 ms writes a good draft before the page goes", async ({
  page,
}) => {
  await openEditor(page, DOC);
  await box(page).locator("g.node", { hasText: "Second" }).click();
  await page.keyboard.type("Quick");
  await zeroClick(page, "Done");
  await expect(page.locator(".cm-content")).toHaveCount(0);
  await expect.poll(() => savedContent(page)).toContain("B[Quick]");
});

test("Back writes a good draft before the page goes", async ({ page }) => {
  await openEditor(page, DOC);
  await box(page).locator("g.node", { hasText: "Second" }).click();
  await page.keyboard.type("Backed");
  await page.goBack();
  await expect(page.locator(".cm-content")).toHaveCount(0);
  await expect.poll(() => savedContent(page)).toContain("B[Backed]");
});

test("a refused draft stays on the page through Done and Back, and the page stays usable", async ({
  page,
}) => {
  await openEditor(page, DOC);
  const editing = page.url();
  await box(page).locator("g.node", { hasText: "Second" }).click();
  await page.keyboard.type("a %%{ b");
  await zeroClick(page, "Done");
  const status = box(page).locator(".cm-diagram-notice");
  await expect(status).toContainText("Escape");
  await expect(field(page)).toHaveValue("a %%{ b");
  await expect(field(page)).toHaveAttribute("aria-invalid", "true");
  expect(page.url()).toBe(editing);

  await page.goBack();
  await expect(field(page)).toHaveValue("a %%{ b");
  await expect.poll(() => page.url()).toBe(editing);
  await expect(field(page)).toHaveValue("a %%{ b");

  // The sentence outlasts any timer.
  await page.waitForTimeout(4_500);
  await expect(status).toContainText("Escape");

  // Nothing holds the keyboard: the text takes the click and the letters.
  await page.locator(".cm-line", { hasText: "Below marker" }).click();
  await page.keyboard.press("End");
  await page.keyboard.type("!");
  expect(await fieldIsFocused(page)).toBe(false);
  await expect.poll(() => docText(page)).toContain("Below marker!");
  await expect(field(page)).toHaveValue("a %%{ b");

  // Back in the field, Escape drops it, and Done then goes.
  await field(page).click();
  await page.keyboard.press("Escape");
  await expect(field(page)).toHaveCount(0);
  await page.getByRole("link", { name: "Done", exact: true }).click();
  await expect(page.locator(".cm-content")).toHaveCount(0);
});

test("Tab after a refused name goes on, and the field does not take it back", async ({
  page,
}) => {
  await openEditor(page, DOC);
  await box(page).locator("g.node", { hasText: "Second" }).click();
  await page.keyboard.type("a %%{ b");
  await page.keyboard.press("Tab");
  await expect(box(page).locator(".cm-diagram-notice")).toContainText("Escape");
  expect(await fieldIsFocused(page)).toBe(false);
  await page.keyboard.press("Tab");
  expect(await fieldIsFocused(page)).toBe(false);
  await page.keyboard.press("Shift+Tab");
  await page.keyboard.press("Shift+Tab");
  await page.waitForTimeout(100);
  await expect(field(page)).toHaveValue("a %%{ b");
});

test("leaving the page asks while a draft is unsaved", async ({ page }) => {
  await openEditor(page, DOC);
  await box(page).locator("g.node", { hasText: "Second" }).click();
  await page.keyboard.type("Unsaved");
  const asked: string[] = [];
  page.on("dialog", (dialog) => {
    asked.push(dialog.type());
    void dialog.dismiss();
  });
  await page.reload({ timeout: 2_000 }).catch(() => undefined);
  expect(asked).toEqual(["beforeunload"]);
  await expect(field(page)).toHaveValue("Unsaved");
});

test("Raw with a refused name: a line says so, and the field is back when Raw goes off", async ({
  page,
}) => {
  await openEditor(page, DOC);
  await box(page).locator("g.node", { hasText: "Second" }).click();
  await page.keyboard.type("a %%{ b");
  const raw = page.getByRole("button", { name: "Raw", exact: true });
  await raw.click();
  await expect(box(page)).toHaveCount(0);
  const note = page.locator(".cm-diagram-raw-note");
  await expect(note).toContainText("a %%{ b");
  await raw.click();
  await expect(field(page)).toHaveValue("a %%{ b");
  await expect(note).toHaveCount(0);
  await expect(box(page).locator(".cm-diagram-notice")).toContainText("Escape");
  await page.keyboard.press("Escape");
  await expect(field(page)).toHaveCount(0);
  expect(await docText(page)).toContain("B[Second]");
});

test("Shift+Tab from a good draft leaves the focus on the stop, through the redraw", async ({
  page,
}) => {
  await openEditor(page, DOC);
  await box(page).locator("g.node", { hasText: "Second" }).click();
  await page.keyboard.type("Good");
  await page.keyboard.press("Shift+Tab");
  await expect
    .poll(() =>
      page.evaluate(() => document.activeElement?.getAttribute("aria-label")),
    )
    .toBe("Box: Good");
  await page.keyboard.press("ArrowLeft");
  await expect
    .poll(() =>
      page.evaluate(() => document.activeElement?.getAttribute("aria-label")),
    )
    .toBe("Box: First");
});

const FILLER = Array.from(
  { length: 300 },
  (_, i) => `Filler line ${String(i)}`,
).join("\n\n");

test("a letter typed with the box scrolled away brings it back and goes in", async ({
  page,
}) => {
  await openEditor(page, `Above\n\n${FENCE}\n\n${FILLER}\n`);
  await box(page).locator("g.node", { hasText: "Second" }).click();
  await page.keyboard.type("Dra");
  const content = await page.locator(".cm-content").boundingBox();
  await page.mouse.move((content?.x ?? 0) + 100, 400);
  await page.mouse.wheel(0, 100_000);
  await expect(box(page)).toHaveCount(0);
  // Chromium animates a wheel step: past that, so nothing scrolls the page away.
  await page.waitForTimeout(600);
  await page.keyboard.type("ft");
  await expect(field(page)).toBeVisible();
  await expect(field(page)).toHaveValue("Draft");
  expect(await fieldIsFocused(page)).toBe(true);
  await expect(field(page)).toBeInViewport();
});

test("a composition keeps the page where it is, and its letter is whole", async ({
  page,
}) => {
  await openEditor(page, `Above\n\n${FENCE}\n\n${FILLER}\n`);
  await box(page).locator("g.node", { hasText: "Second" }).click();
  await page.keyboard.type("Dr");
  await markField(page);
  const cdp = await page.context().newCDPSession(page);
  await cdp.send("Input.imeSetComposition", {
    text: "ㄱ",
    selectionStart: 1,
    selectionEnd: 1,
  });
  // A peer's lines move the field, and from there the page holds (N8).
  await peerEdit(page, "```mermaid", "peer\n\npeer\n\npeer\n\n");
  await page.waitForTimeout(300);
  const moved = (await field(page).boundingBox())?.y ?? 0;
  const content = await page.locator(".cm-content").boundingBox();
  await page.mouse.move((content?.x ?? 0) + 100, 400);
  await page.mouse.wheel(0, 40);
  await page.waitForTimeout(400);
  expect(
    Math.abs(((await field(page).boundingBox())?.y ?? 0) - moved),
  ).toBeLessThanOrEqual(2);
  await page.mouse.wheel(0, 100_000);
  await page.waitForTimeout(400);
  await expect(box(page)).toHaveCount(1);
  expect(await fieldIsMarked(page)).toBe(true);
  expect(await fieldIsFocused(page)).toBe(true);
  await cdp.send("Input.imeSetComposition", {
    text: "가",
    selectionStart: 1,
    selectionEnd: 1,
  });
  await cdp.send("Input.insertText", { text: "가" });
  await expect(field(page)).toHaveValue("Dr가");
});

test("the field keeps its text in the middle of a tall box", async ({
  page,
}) => {
  await openEditor(
    page,
    "Above\n\n```mermaid\nflowchart TD\n  A[First] --> B{Second}\n```\n\nBelow\n",
  );
  const node = box(page).locator("g.node", { hasText: "Second" });
  await expect(node).toBeVisible();
  const shape = await node.boundingBox();
  await node.click();
  const place = await field(page).boundingBox();
  if (shape === null || place === null) throw new Error("no box");
  expect(place.height).toBeLessThan(shape.height / 2);
  expect(
    Math.abs(place.y + place.height / 2 - (shape.y + shape.height / 2)),
  ).toBeLessThanOrEqual(2);
});

test("a refused draft and the search palette: the page goes, and a notice names the text", async ({
  page,
}) => {
  await openEditor(page, DOC);
  await box(page).locator("g.node", { hasText: "Second" }).click();
  await page.keyboard.type("a %%{ b");
  await page.keyboard.press("ControlOrMeta+k");
  await page.keyboard.type("Home");
  await page.waitForTimeout(400);
  await page.keyboard.press("ArrowDown");
  await page.keyboard.press("Enter");
  const note = page.locator(".cm-diagram-lost-draft");
  await expect(note).toContainText("a %%{ b");
  await note.getByRole("button", { name: "Dismiss" }).click();
  await expect(note).toHaveCount(0);
});

test("under Raw a refused draft comes back once the caret leaves, and a second leave goes", async ({
  page,
}) => {
  await openEditor(page, DOC);
  await box(page).locator("g.node", { hasText: "Second" }).click();
  await page.keyboard.type("a %%{ b");
  const raw = page.getByRole("button", { name: "Raw", exact: true });
  await raw.click();
  const note = page.locator(".cm-diagram-raw-note");
  await expect(note).toContainText("a %%{ b");
  await page.locator(".cm-line", { hasText: "B[Second]" }).click();
  await raw.click();
  await expect(note).toContainText("caret");
  await expect(field(page)).toHaveCount(0);
  await page.locator(".cm-line", { hasText: "Below marker" }).click();
  await expect(field(page)).toHaveValue("a %%{ b");
  await expect(note).toHaveCount(0);

  await raw.click();
  await expect(note).toContainText("a %%{ b");
  const done = page.getByRole("link", { name: "Done", exact: true });
  await done.click();
  await expect(note).toBeVisible();
  await expect(page.locator(".cm-content")).toHaveCount(1);
  await done.click();
  await expect(page.locator(".cm-content")).toHaveCount(0);
  await expect(page.locator(".cm-diagram-lost-draft")).toContainText("a %%{ b");
});

/** Passes a room's socket through, and can say to the page the room closed. */
async function closableRoom(page: Page): Promise<() => void> {
  const sockets: WebSocketRoute[] = [];
  await page.routeWebSocket(/\/collab\//, (socket) => {
    const server = socket.connectToServer();
    socket.onMessage((message) => {
      server.send(message);
    });
    server.onMessage((message) => {
      socket.send(message);
    });
    sockets.push(socket);
  });
  return () => {
    const json = Buffer.from(
      JSON.stringify({ kind: "closed", reason: "deleted" }),
    );
    for (const socket of sockets) {
      socket.send(Buffer.concat([Buffer.from([4, json.length]), json]));
    }
  };
}

const storedDrafts = (page: Page) =>
  page.evaluate(() => JSON.stringify({ ...window.localStorage }));

test("a room closing writes a good draft into the draft the banner promises", async ({
  page,
}) => {
  const close = await closableRoom(page);
  await openEditor(page, DOC);
  await box(page).locator("g.node", { hasText: "Second" }).click();
  await page.keyboard.type("MyDraftWord");
  close();
  await expect(page.getByText("your text is kept as a draft")).toBeVisible();
  await expect.poll(() => storedDrafts(page)).toContain("B[MyDraftWord]");
  await expect(page.locator(".cm-content")).toHaveCount(0, { timeout: 5_000 });
  await expect(page.locator(".cm-diagram-lost-draft")).toHaveCount(0);
});

test("a room closing with a refused draft shows the text and stays until the person goes", async ({
  page,
}) => {
  const close = await closableRoom(page);
  await openEditor(page, DOC);
  await box(page).locator("g.node", { hasText: "Second" }).click();
  await page.keyboard.type("a %%{ b");
  close();
  const said = page.getByRole("alert").filter({ hasText: "a %%{ b" });
  await expect(said).toBeVisible();
  await page.waitForTimeout(2_000);
  await expect(page.locator(".cm-content")).toHaveCount(1);
  await expect(said).toBeVisible();
  await expect(field(page)).toHaveCount(0);
  await page.getByRole("link", { name: "Done", exact: true }).click();
  await expect(page.locator(".cm-content")).toHaveCount(0);
});
