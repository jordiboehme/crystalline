/**
 * The keys and the pointer on a drawn diagram in a real browser: what jsdom
 * cannot answer. Real focus and the tab order, a click landing on a part of a
 * real drawing, and a key on the box not reaching the text.
 *
 * Shares the daemon, the fixture domain and the accounts `run-smoke.sh` sets
 * up; run it the same way (`bash fluid/e2e/run-smoke.sh e2e/diagram-keys.spec.ts`).
 */

import type { Locator, Page } from "@playwright/test";
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
        title: `Diagram widget ${String(Date.now())}${String(unique)}`,
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

function line(page: Page, text: string): Locator {
  return page.locator(".cm-content > .cm-line", {
    hasText: new RegExp(`^${text}$`),
  });
}

test.beforeEach(async ({ page }) => {
  await signIn(page);
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

const box = (page: Page) => page.locator(".cm-diagram");
const focusedLabel = (page: Page) =>
  page.evaluate(() => document.activeElement?.getAttribute("aria-label") ?? "");
const focusedIsBox = (page: Page) =>
  page.evaluate(
    () => document.activeElement?.classList.contains("cm-diagram") ?? false,
  );
const FENCE = "```mermaid\nflowchart TD\n  A[First] --> B[Second]\n```";

test("the keys walk the stops, with one Tab stop, and Escape leaves", async ({
  page,
}) => {
  await openEditor(page, `Above\n${FENCE}\nBelow marker\n`);
  await expect(box(page).locator("svg")).toBeVisible();
  await page.locator(".cm-content > .cm-line", { hasText: "Above" }).click();
  await page.keyboard.press("ArrowDown");
  expect(await focusedIsBox(page)).toBe(true);
  await page.keyboard.press("ArrowRight");
  expect(await focusedLabel(page)).toBe("Box: First");
  await expect(box(page).locator(".cm-diagram-focus-ring")).toBeVisible();
  await page.keyboard.press("End");
  expect(await focusedLabel(page)).toBe("Arrow from First to Second");
  await expect(box(page).locator('[tabindex="0"]')).toHaveCount(1);
  await page.keyboard.press("x");
  expect(await focusedLabel(page)).toBe("Arrow from First to Second");
  await page.keyboard.press("Escape");
  await expect(line(page, "Below marker")).toBeVisible();
  await page.keyboard.type("Q");
  await expect(line(page, "QBelow marker")).toBeVisible();
});

test("Tab goes in and out in one stop", async ({ page }) => {
  await openEditor(page, `Above\n${FENCE}\nBelow marker\n`);
  await expect(box(page).locator("svg")).toBeVisible();
  await box(page).focus();
  expect(await focusedIsBox(page)).toBe(true);
  await page.keyboard.press("ArrowRight");
  await page.keyboard.press("Tab");
  const inside = await page.evaluate(
    () => document.activeElement?.closest(".cm-diagram") != null,
  );
  expect(inside).toBe(false);
  await page.keyboard.press("Shift+Tab");
  expect(await focusedLabel(page)).toBe("Box: First");
});

test("a click maps to its box, and duplicates and odd ids get their own hit paths", async ({
  page,
}) => {
  await openEditor(
    page,
    "Top\n\n```mermaid\nflowchart LR\n  step-1[One] --> a_b[Two]\n  step-1 --> a_b\n```\n\nEnd\n",
  );
  await expect(box(page).locator("svg")).toBeVisible();
  await box(page).locator("g.node", { hasText: "Two" }).click();
  expect(await focusedLabel(page)).toBe("Box name");
  await page.keyboard.press("Escape");
  expect(await focusedLabel(page)).toBe("Box: Two");
  const paths = box(page).locator('path[data-et="edge"]');
  await expect(paths).toHaveCount(2);
  expect(await paths.first().getAttribute("aria-label")).toBe(
    "Arrow from One to Two",
  );
  await expect(box(page).locator("[data-hit-for]")).toHaveCount(2);
  const ids = await box(page)
    .locator("[data-hit-for]")
    .evaluateAll((els) => els.map((el) => el.getAttribute("data-hit-for")));
  // Mermaid's own numbering, which skips: a hit path exists only for an id that
  // matched a model edge exactly, so both arrows here mapped.
  expect([...ids].sort()).toEqual(["L_step-1_a_b_0", "L_step-1_a_b_2"]);
});

test("Backspace from the line below goes to the canvas, and the second press deletes", async ({
  page,
}) => {
  await openEditor(page, `Above\n${FENCE}\nBelow marker\n`);
  await expect(box(page).locator("svg")).toBeVisible();
  await line(page, "Below marker").click();
  await page.keyboard.press("Home");
  await page.keyboard.press("Backspace");
  expect(await focusedIsBox(page)).toBe(true);
  await expect(box(page)).toHaveCount(1);
  await page.keyboard.press("Backspace");
  await expect(box(page)).toHaveCount(0);
  await page.keyboard.press("ControlOrMeta+z");
  await expect(box(page)).toHaveCount(1);
});

test("Delete on a box takes it out, and the focus stays on the drawing", async ({
  page,
}) => {
  await openEditor(page, `Above\n${FENCE}\nBelow marker\n`);
  await expect(box(page).locator("svg")).toBeVisible();
  await box(page).locator("g.node", { hasText: "Second" }).click();
  await page.keyboard.press("Escape");
  await page.keyboard.press("Delete");
  await expect(box(page).locator("g.node")).toHaveCount(1);
  await expect
    .poll(() =>
      page.evaluate(
        () => document.activeElement?.closest(".cm-diagram") != null,
      ),
    )
    .toBe(true);
  await page.keyboard.press("ControlOrMeta+z");
  await expect(box(page).locator("g.node")).toHaveCount(2);
});

test("a self-loop's arrow is a stop like any other", async ({ page }) => {
  await openEditor(
    page,
    "Top\n\n```mermaid\nflowchart TD\n  A[One] --> A\n```\n\nEnd\n",
  );
  await expect(box(page).locator("svg")).toBeVisible();
  await expect(box(page).locator("[data-hit-for]")).toHaveCount(1);
  await box(page).focus();
  await page.keyboard.press("End");
  expect(await focusedLabel(page)).toBe("Arrow from One to One");
});

test("a click on an arrow's wide hit path selects that arrow", async ({
  page,
}) => {
  await openEditor(page, `Above\n${FENCE}\nBelow marker\n`);
  await expect(box(page).locator("svg")).toBeVisible();
  const arrow = await box(page).locator("[data-hit-for]").boundingBox();
  if (arrow === null) throw new Error("no arrow");
  await page.mouse.click(
    arrow.x + arrow.width / 2 + 4,
    arrow.y + arrow.height / 2,
  );
  expect(await focusedLabel(page)).toBe("Arrow label");
  await page.keyboard.press("Escape");
  expect(await focusedLabel(page)).toBe("Arrow from First to Second");
});

/** The document's exact bytes, as the view holds them. */
const docText = (page: Page) =>
  page.evaluate(() => {
    const content = document.querySelector(".cm-content") as unknown as {
      cmTile: { view: { state: { doc: { toString(): string } } } };
    };
    return content.cmTile.view.state.doc.toString();
  });

const sourceShown = (page: Page) =>
  page.locator(".cm-content > .cm-line", { hasText: "A[First]" });

for (const key of ["Backspace", "ControlOrMeta+x"]) {
  test(`a search hit left with Escape shows the fence as text, and ${key} takes only the hit`, async ({
    page,
  }) => {
    await openEditor(page, `Above\n${FENCE}\nBelow marker\n`);
    await expect(box(page).locator("svg")).toBeVisible();
    await page.locator(".cm-content > .cm-line", { hasText: "Above" }).click();
    const before = await docText(page);
    await page.keyboard.press("ControlOrMeta+f");
    await page.keyboard.type("Second");
    await page.keyboard.press("Enter");
    await page.keyboard.press("Escape");
    await expect(sourceShown(page)).toBeVisible();
    await expect(box(page)).toHaveCount(0);
    await page.keyboard.press(key);
    await expect.poll(() => docText(page)).toBe(before.replace("Second", ""));
    await expect(sourceShown(page)).toBeVisible();
    await page.keyboard.press("ControlOrMeta+z");
    await expect.poll(() => docText(page)).toBe(before);
  });
}

test("a fence shown as text draws again when the caret leaves", async ({
  page,
}) => {
  await openEditor(page, `Above\n${FENCE}\nBelow marker\n`);
  await expect(box(page).locator("svg")).toBeVisible();
  await page.locator(".cm-content > .cm-line", { hasText: "Above" }).click();
  await page.keyboard.press("ControlOrMeta+f");
  await page.keyboard.type("Second");
  await page.keyboard.press("Enter");
  await page.keyboard.press("Escape");
  await expect(box(page)).toHaveCount(0);
  await page.locator(".cm-content > .cm-line", { hasText: "Above" }).click();
  await expect(box(page).locator("svg")).toBeVisible();
});

test("Mod-End then Shift+Tab lands on a fence shown as text", async ({
  page,
}) => {
  await openEditor(page, `Above\n${FENCE}\n`);
  await expect(box(page).locator("svg")).toBeVisible();
  // A hand-written file with no final line break; the app writes one.
  await page.evaluate(() => {
    const content = document.querySelector(".cm-content") as unknown as {
      cmTile: {
        view: {
          state: { doc: { length: number; toString(): string } };
          dispatch(spec: { changes: { from: number; to: number } }): void;
        };
      };
    };
    const { view } = content.cmTile;
    const end = view.state.doc.length;
    if (view.state.doc.toString().endsWith("\n")) {
      view.dispatch({ changes: { from: end - 1, to: end } });
    }
  });
  await page.locator(".cm-content > .cm-line", { hasText: "Above" }).click();
  await page.keyboard.press("ControlOrMeta+End");
  await expect.poll(() => focusedIsBox(page)).toBe(true);
  await page.keyboard.press("Shift+Tab");
  await expect(sourceShown(page)).toBeVisible();
  await expect(box(page)).toHaveCount(0);
  await expect(page.locator(".cm-content")).toContainText("flowchart TD");
  // A visible edit of one character; the fence going whole was the defect.
  const before = await docText(page);
  // Past the history's grouping time, so the setup's edit is an undo step apart.
  await page.waitForTimeout(600);
  await page.keyboard.press("Backspace");
  await expect.poll(() => docText(page)).toBe(before.slice(0, -1));
  await page.keyboard.press("ControlOrMeta+z");
  await expect.poll(() => docText(page)).toBe(before);
});

test("holding Backspace stops at the fence, and one undo gives back the word", async ({
  page,
}) => {
  await openEditor(page, `Para above\n${FENCE}\nBelow\n`);
  await expect(box(page).locator("svg")).toBeVisible();
  await page.locator(".cm-content > .cm-line", { hasText: /^Below$/ }).click();
  await page.keyboard.press("End");
  const before = await docText(page);
  for (let n = 0; n < 13; n += 1) {
    await page.keyboard.down("Backspace");
  }
  await page.keyboard.up("Backspace");
  await expect
    .poll(() => docText(page))
    .toBe(before.replace("\nBelow\n", "\n\n"));
  await expect(box(page).locator("g.node")).toHaveCount(2);
  expect(await focusedIsBox(page)).toBe(false);
  await page.keyboard.press("ControlOrMeta+z");
  await expect.poll(() => docText(page)).toBe(before);
  await expect(box(page).locator("g.node")).toHaveCount(2);
});

test("holding Delete on a box takes only that box", async ({ page }) => {
  await openEditor(
    page,
    "Above\n```mermaid\nflowchart TD\n  A[First] --> B[Second]\n  B --> C[Third]\n```\nBelow\n",
  );
  await expect(box(page).locator("g.node")).toHaveCount(3);
  const before = await docText(page);
  await box(page).locator("g.node", { hasText: "Third" }).click();
  await page.keyboard.press("Escape");
  await page.keyboard.down("Delete");
  await expect(box(page).locator("g.node")).toHaveCount(2);
  for (let n = 0; n < 8; n += 1) {
    await page.keyboard.down("Delete");
  }
  await page.keyboard.up("Delete");
  await expect(box(page).locator("g.node")).toHaveCount(2);
  await page.keyboard.press("ControlOrMeta+z");
  await expect.poll(() => docText(page)).toBe(before);
  await expect(box(page).locator("g.node")).toHaveCount(3);
});

const SENTENCE = "That would break a diagram here, so nothing was changed.";

/** Whether the sentence stands inside the window, where a person can read it. */
const sentenceInWindow = (page: Page) =>
  page.evaluate(
    (sentence) =>
      [...document.querySelectorAll('[role="status"]')].some((el) => {
        const rect = el.getBoundingClientRect();
        return (
          el.textContent === sentence &&
          rect.height > 0 &&
          rect.top >= 0 &&
          rect.bottom <= window.innerHeight
        );
      }),
    SENTENCE,
  );

/** Replace all of `mermaid`, through the real search panel. */
async function replaceAllMermaid(page: Page): Promise<void> {
  await page.keyboard.press("ControlOrMeta+f");
  await page.locator('.cm-search input[name="search"]').fill("mermaid");
  await page.locator('.cm-search input[name="replace"]').fill("chart");
  await page.locator('.cm-search button[name="replaceAll"]').click();
}

const fillers = (count: number) =>
  Array.from({ length: count }, (_, n) => `Line ${String(n)}\n`).join("\n");

for (const [name, doc, start] of [
  [
    "below the window",
    `Above mermaid\n${fillers(17)}\n${FENCE}\nEnd\n`,
    "Above mermaid",
  ],
  [
    "above the window",
    `${FENCE}\n${fillers(17)}\nLast mermaid\n`,
    "Last mermaid",
  ],
  [
    "far off, its box not mounted",
    `Above mermaid\n${fillers(150)}\n${FENCE}\nEnd\n`,
    "Above mermaid",
  ],
] as const) {
  test(`Replace all refused with the fence ${name}: the sentence shows, and the page stays`, async ({
    page,
  }) => {
    await openEditor(page, doc);
    const here = line(page, start);
    if (start === "Last mermaid") {
      // The box drawn first: this is about the refusal, not about Ctrl+End racing
      // the first draw (that has a journey of its own below).
      await expect(box(page).locator("svg")).toBeVisible();
      // Far down the page, so not in the DOM until the caret goes there.
      await page.locator(".cm-content").focus();
      await page.keyboard.press("ControlOrMeta+End");
    } else {
      await here.click();
      await page.keyboard.press("End");
    }
    await expect(here).toBeInViewport();
    // Long enough for the verdict: a fence not drawn yet would take the edit.
    await page.waitForTimeout(1_500);
    const before = await docText(page);
    await replaceAllMermaid(page);
    const top = (await here.boundingBox())?.y;
    await expect.poll(() => sentenceInWindow(page)).toBe(true);
    expect(await docText(page)).toBe(before);
    expect((await here.boundingBox())?.y).toBe(top);
  });
}

test("Ctrl+End racing the first draw of a fence above: the end stays in the window", async ({
  page,
}) => {
  // The diagram chunk held, so the fence is still its source at the key.
  let release = () => {};
  const held = new Promise<void>((resolve) => {
    release = resolve;
  });
  await page.route(/assets\/(diagram-|mermaid\.core)/, async (route) => {
    await held;
    await route.continue();
  });
  await openEditor(page, `${FENCE}\n${fillers(17)}\nLast mermaid\n`);
  const here = line(page, "Last mermaid");
  await page.locator(".cm-content").focus();
  await page.keyboard.press("ControlOrMeta+End");
  await expect(here).toBeInViewport();
  await expect(box(page)).toHaveCount(0);
  await page.evaluate(
    () =>
      new Promise((resolve) => {
        requestAnimationFrame(() => requestAnimationFrame(resolve));
      }),
  );
  // The line's place in the editor less the page's scroll: what a person sees
  // of the text, whatever moves the editor itself (the findings line above it
  // comes in at no fixed time). The box growing above the line adds to the
  // first part; only a scroll by the same amount keeps the sum.
  const seen = () =>
    here.evaluate(
      (el) =>
        el.getBoundingClientRect().top -
        (el.closest(".cm-editor")?.getBoundingClientRect().top ?? Number.NaN) -
        (document.scrollingElement?.scrollTop ?? Number.NaN),
    );
  const was = await seen();
  // Something above the editor grows after CodeMirror's last pass and before
  // the pass that measures the new box (as the findings line can, coming in),
  // so that pass keeps no anchor of its own.
  const ABOVE_PX = 8;
  await page.evaluate((px) => {
    const content = document.querySelector(".cm-content");
    if (content === null) return;
    const watch = new MutationObserver(() => {
      if (content.querySelector(".cm-diagram") === null) return;
      watch.disconnect();
      const above = document.createElement("div");
      above.style.height = `${String(px)}px`;
      document.querySelector(".cm-editor")?.before(above);
    });
    watch.observe(content, { childList: true });
  }, ABOVE_PX);
  release();
  await expect(box(page).locator("svg")).toBeVisible();
  await page.waitForTimeout(500);
  // The box replaced its source above the window; the text in the window stays.
  await expect(here).toBeInViewport();
  expect(Math.abs((await seen()) - was)).toBeLessThanOrEqual(2);
});

/** A hit taken with main focused: the box stays drawn, the selection in its body. */
async function hitWithMainFocused(page: Page): Promise<string> {
  await openEditor(page, `Above\n${FENCE}\nBelow marker\n`);
  await expect(box(page).locator("svg")).toBeVisible();
  await page.locator(".cm-content > .cm-line", { hasText: "Above" }).click();
  await page.keyboard.press("ControlOrMeta+f");
  await page.keyboard.type("Second");
  await page.keyboard.press("Enter");
  await page.keyboard.press("Escape");
  await page.locator(".cm-content > .cm-line", { hasText: "Above" }).click();
  await expect(box(page).locator("svg")).toBeVisible();
  await page.keyboard.press("ControlOrMeta+Home");
  await page.keyboard.press("F3");
  await expect(box(page).locator("svg")).toBeVisible();
  expect(
    await page.evaluate(
      () => document.activeElement?.classList.contains("cm-content") ?? false,
    ),
  ).toBe(true);
  return docText(page);
}

test("after a hit taken with main focused, a letter writes nothing and the box takes the keyboard", async ({
  page,
}) => {
  const before = await hitWithMainFocused(page);
  await page.keyboard.press("q");
  await expect.poll(() => focusedIsBox(page)).toBe(true);
  expect(await docText(page)).toBe(before);
  await expect(page.locator(".cm-content")).not.toContainText("qBelow");
});

test("after a hit taken with main focused, Enter writes nothing", async ({
  page,
}) => {
  const before = await hitWithMainFocused(page);
  await page.keyboard.press("Enter");
  await expect.poll(() => focusedIsBox(page)).toBe(true);
  expect(await docText(page)).toBe(before);
});

test("after a hit taken with main focused, a paste writes nothing", async ({
  page,
}) => {
  const before = await hitWithMainFocused(page);
  await page.evaluate(() => {
    const data = new DataTransfer();
    data.setData("text/plain", "pasted");
    document.querySelector(".cm-content")?.dispatchEvent(
      new ClipboardEvent("paste", {
        clipboardData: data,
        bubbles: true,
        cancelable: true,
      }),
    );
  });
  await expect.poll(() => focusedIsBox(page)).toBe(true);
  expect(await docText(page)).toBe(before);
});

test("a press held after the canvas delete takes nothing more", async ({
  page,
}) => {
  await openEditor(page, `Para above\n${FENCE}\nBelow\n`);
  await expect(box(page).locator("svg")).toBeVisible();
  await page.locator(".cm-content > .cm-line", { hasText: /^Below$/ }).click();
  await page.keyboard.press("Home");
  const before = await docText(page);
  await page.keyboard.press("Backspace");
  expect(await focusedIsBox(page)).toBe(true);
  for (let n = 0; n < 12; n += 1) {
    await page.keyboard.down("Backspace");
  }
  await page.keyboard.up("Backspace");
  await expect.poll(() => docText(page)).toBe(before.replace(`${FENCE}\n`, ""));
});

const WITH_TABLE = `Above\n\n| one | two |\n|---|---|\n| a | b |\n\n${FENCE}\n\nBelow marker\n`;

/** Whether the focus is in a table cell's editor. */
const focusInCell = (page: Page) =>
  page.evaluate(() => document.activeElement?.closest(".cm-grid") != null);

for (const [name, query] of [
  ["no search", null],
  ["a hit outside the fence", "marker"],
  ["a hit in the fence", "Second"],
] as const) {
  test(`typing in a table cell with ${name} writes there, and the focus stays`, async ({
    page,
  }) => {
    await openEditor(page, WITH_TABLE);
    await expect(box(page).locator("svg")).toBeVisible();
    await page.locator(".cm-grid").getByText("one").waitFor();
    await page.locator(".cm-content > .cm-line", { hasText: "Above" }).click();
    if (query !== null) {
      await page.keyboard.press("ControlOrMeta+f");
      await page.keyboard.type(query);
      await page.keyboard.press("Enter");
    }
    const before = await docText(page);
    await page.locator(".cm-grid").getByText("one").click();
    await expect.poll(() => focusInCell(page)).toBe(true);
    await page.keyboard.type("ZZ");
    await expect
      .poll(() => docText(page))
      .toMatch(/\| o?n?e?ZZo?n?e? \| two \|/);
    expect(await focusInCell(page)).toBe(true);
    expect((await docText(page)).replace("ZZ", "")).toBe(before);
    await expect(box(page).locator("svg")).toBeVisible();
  });
}

test("a paste into a table cell with a hit in the fence writes there", async ({
  page,
}) => {
  await openEditor(page, WITH_TABLE);
  await expect(box(page).locator("svg")).toBeVisible();
  await page.locator(".cm-content > .cm-line", { hasText: "Above" }).click();
  await page.keyboard.press("ControlOrMeta+f");
  await page.keyboard.type("Second");
  await page.keyboard.press("Enter");
  const before = await docText(page);
  await page.locator(".cm-grid").getByText("one").click();
  await expect.poll(() => focusInCell(page)).toBe(true);
  await page.evaluate(() => {
    const data = new DataTransfer();
    data.setData("text/plain", "PP");
    document.activeElement?.dispatchEvent(
      new ClipboardEvent("paste", {
        clipboardData: data,
        bubbles: true,
        cancelable: true,
      }),
    );
  });
  await expect.poll(() => docText(page)).toContain("PP");
  expect(await focusInCell(page)).toBe(true);
  expect((await docText(page)).replace("PP", "")).toBe(before);
});
