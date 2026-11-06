/**
 * The menus and connect mode in a real browser: what jsdom cannot answer. Real keys
 * and a real right click, whether the browser's own menu stays away, where a menu
 * stands in the window, and that the focus is never left on the body.
 *
 * Shares the daemon, the fixture domain and the accounts `run-smoke.sh` sets up;
 * run it the same way (`bash fluid/e2e/run-smoke.sh e2e/diagram-menu.spec.ts`).
 */

import type { Page } from "@playwright/test";
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
        title: `Diagram menu ${String(Date.now())}${String(unique)}`,
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
const menus = (page: Page) => page.locator('[role="menu"]');
const nodeOf = (page: Page, id: string) =>
  page.locator(`.cm-diagram g.node[id*="flowchart-${id}-"]`);
const FENCE = "```mermaid\nflowchart TD\n  A[First] --> B[Second]\n```";
const DOC = `Above\n\n${FENCE}\n\nBelow marker\n`;
const SHOTS = process.env.FLUID_E2E_SHOTS;

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

const focusOnBody = (page: Page) =>
  page.evaluate(() => document.activeElement === document.body);

const focusedName = (page: Page) =>
  page.evaluate(() => document.activeElement?.textContent ?? "");

const focusedLabel = (page: Page) =>
  page.evaluate(() => document.activeElement?.getAttribute("aria-label") ?? "");

/** The keyboard on box `label`, walking from the canvas. */
async function stopOn(page: Page, label: string): Promise<void> {
  await expect(nodeOf(page, "A")).toBeVisible();
  await nodeOf(page, "A").click();
  await page.keyboard.press("Escape");
  await expect.poll(() => focusedLabel(page)).toContain("Box: First");
  if (label !== "Box: First") {
    await page.keyboard.press("ArrowRight");
    await expect.poll(() => focusedLabel(page)).toBe(label);
  }
}

test("Shift+F10 opens one menu, walks into Shape, writes one undo step", async ({
  page,
}) => {
  await openEditor(page, DOC);
  await expect(box(page)).toBeVisible();
  const before = await docText(page);
  await stopOn(page, "Box: Second");
  await page.keyboard.press("Shift+F10");
  await expect(menus(page)).toHaveCount(1);
  await expect(page.getByRole("menu", { name: "Box actions" })).toBeVisible();
  expect(await focusedName(page)).toBe("Rename");
  for (let i = 0; i < 4; i += 1) {
    await page.keyboard.press("ArrowDown");
  }
  expect(await focusedName(page)).toBe("Shape");
  await page.keyboard.press("ArrowRight");
  await expect(menus(page)).toHaveCount(2);
  await expect(
    page.getByRole("menuitemradio", { name: "Rectangle" }),
  ).toHaveAttribute("aria-checked", "true");
  if (SHOTS !== undefined) {
    await page.screenshot({ path: `${SHOTS}/shape-submenu.png` });
  }
  await page.keyboard.press("ArrowDown");
  await page.keyboard.press("ArrowDown");
  await page.keyboard.press("Escape");
  expect(await focusedName(page)).toBe("Shape");
  await page.keyboard.press("ArrowRight");
  await page.keyboard.press("End");
  await page.keyboard.press("ArrowUp");
  expect(await focusedName(page)).toBe("Trapezoid");
  await page.keyboard.press("Home");
  await page.keyboard.press("ArrowDown");
  await page.keyboard.press("Enter");
  await expect.poll(() => docText(page)).toContain("B(Second)");
  await expect(menus(page)).toHaveCount(0);
  expect(await focusOnBody(page)).toBe(false);
  await page.keyboard.press("ControlOrMeta+z");
  await expect.poll(() => docText(page)).toBe(before);
});

test("a held Enter on a menu item runs it once", async ({ page }) => {
  await openEditor(
    page,
    "```mermaid\nflowchart TD\n  A --> B --> C\n```\n\nEnd\n",
  );
  await expect(nodeOf(page, "B")).toBeVisible();
  await nodeOf(page, "B").click({ button: "right" });
  await page.keyboard.press("End");
  expect(await focusedName(page)).toBe("Delete");
  // A key held down: its later keydowns come with repeat set.
  const cdp = await page.context().newCDPSession(page);
  const key = {
    key: "Enter",
    code: "Enter",
    windowsVirtualKeyCode: 13,
    text: "\r",
  };
  await cdp.send("Input.dispatchKeyEvent", { type: "keyDown", ...key });
  for (let i = 0; i < 4; i += 1) {
    await cdp.send("Input.dispatchKeyEvent", {
      type: "keyDown",
      autoRepeat: true,
      ...key,
    });
  }
  await cdp.send("Input.dispatchKeyEvent", { type: "keyUp", ...key });
  await expect.poll(() => docText(page)).not.toContain("--> B");
  await expect.poll(() => focusOnBody(page)).toBe(false);
  const text = await docText(page);
  expect(text).toContain("A");
  expect(text).toContain("C");
  expect(text).toContain("End");
  await expect(page.locator(".cm-diagram-label-field")).toHaveCount(0);
});

test("a pointer resting where a key's menu opens takes nothing", async ({
  page,
}) => {
  await openEditor(page, DOC);
  await expect(nodeOf(page, "A")).toBeVisible();
  const before = await docText(page);
  // Where the menu will stand: learnt by opening it once.
  await stopOn(page, "Box: First");
  await page.keyboard.press("Shift+F10");
  const rowAt = await page
    .getByRole("menuitem", { name: "Delete" })
    .boundingBox();
  if (rowAt === null) throw new Error("no row");
  await page.keyboard.press("Escape");
  await expect(menus(page)).toHaveCount(0);
  await page.mouse.move(rowAt.x + 30, rowAt.y + rowAt.height / 2);
  await page.keyboard.press("Shift+F10");
  await expect(menus(page)).toHaveCount(1);
  expect(await focusedName(page)).toBe("Rename");
  await page.waitForTimeout(400);
  expect(await focusedName(page)).toBe("Rename");
  await page.keyboard.press("ArrowDown");
  expect(await focusedName(page)).toBe("Add child");
  await page.keyboard.press("Escape");
  expect(await docText(page)).toBe(before);
  // A real move still takes it, in two steps, and the submenu opens under it.
  await page.keyboard.press("Shift+F10");
  await page.mouse.move(rowAt.x + 31, rowAt.y + rowAt.height / 2);
  await page.mouse.move(rowAt.x + 32, rowAt.y + rowAt.height / 2);
  expect(await focusedName(page)).toBe("Delete");
});

test("after Back, an Escape on the page is the page's again", async ({
  page,
}) => {
  await openEditor(page, DOC);
  await expect(nodeOf(page, "A")).toBeVisible();
  await nodeOf(page, "A").click({ button: "right" });
  await page.getByRole("menuitem", { name: "Connect to..." }).click();
  await expect(page.locator(".cm-diagram-connect")).toBeVisible();
  await page.goBack();
  await expect(page.locator(".cm-content")).toHaveCount(0);
  // The guard acts with the focus on the body, so that is where it has to be.
  const answer = await page.evaluate(() => {
    (document.activeElement as HTMLElement | null)?.blur();
    const onBody = document.activeElement === document.body;
    const event = new KeyboardEvent("keydown", {
      key: "Escape",
      bubbles: true,
      cancelable: true,
    });
    document.body.dispatchEvent(event);
    return { onBody, prevented: event.defaultPrevented };
  });
  expect(answer).toEqual({ onBody: true, prevented: false });
});

test("a real right click opens our menu inside the window, and Escape goes back", async ({
  page,
}) => {
  await openEditor(page, DOC);
  await expect(nodeOf(page, "B")).toBeVisible();
  const before = await docText(page);
  await nodeOf(page, "B").click({ button: "right" });
  const menu = page.getByRole("menu", { name: "Box actions" });
  await expect(menu).toBeVisible();
  const area = page.viewportSize();
  const rect = await menu.boundingBox();
  expect(rect).not.toBeNull();
  if (rect !== null && area !== null) {
    expect(rect.x + rect.width).toBeLessThanOrEqual(area.width);
    expect(rect.y + rect.height).toBeLessThanOrEqual(area.height);
  }
  await page.keyboard.press("Escape");
  await expect(menus(page)).toHaveCount(0);
  expect(await focusedLabel(page)).toBe("Box: Second");
  expect(await docText(page)).toBe(before);
});

test("a menu near the window's end moves up, and its submenu stays in the window", async ({
  page,
}) => {
  await page.setViewportSize({ width: 800, height: 420 });
  await openEditor(page, DOC);
  await expect(nodeOf(page, "B")).toBeVisible();
  await nodeOf(page, "B").scrollIntoViewIfNeeded();
  const at = await nodeOf(page, "B").boundingBox();
  if (at === null) throw new Error("no box");
  await page.mouse.click(at.x + at.width / 2, at.y + at.height / 2, {
    button: "right",
  });
  const menu = page.getByRole("menu", { name: "Box actions" });
  await expect(menu).toBeVisible();
  await page.getByRole("menuitem", { name: "Shape" }).click();
  const sub = page.getByRole("menu", { name: "Shape" });
  await expect(sub).toBeVisible();
  for (const el of [menu, sub]) {
    const rect = await el.boundingBox();
    if (rect === null) throw new Error("no menu");
    expect(rect.y).toBeGreaterThanOrEqual(0);
    expect(rect.x + rect.width).toBeLessThanOrEqual(800);
  }
  // The last item can be reached, however short the room: nothing cuts it off.
  const last = page.getByRole("menuitemradio", {
    name: "Upside-down trapezoid",
  });
  await last.scrollIntoViewIfNeeded();
  await last.click({ trial: true });
  await sub.evaluate((el) => {
    el.scrollTop = 0;
  });
  // Pressed for real: nothing hides it.
  await page.getByRole("menuitemradio", { name: "Rounded" }).click();
  await expect.poll(() => docText(page)).toContain("B(Second)");
});

test("the arrow's menu says why Reverse is off, to the eye too", async ({
  page,
}) => {
  await openEditor(page, "```mermaid\nflowchart TD\n  A --> B --> C\n```\n");
  await expect(nodeOf(page, "A")).toBeVisible();
  await page.locator(".cm-diagram [data-hit-for]").first().click({
    button: "right",
    force: true,
  });
  await expect(page.getByRole("menu", { name: "Arrow actions" })).toBeVisible();
  const reverse = page.getByRole("menuitem", { name: "Reverse" });
  await expect(reverse).toHaveAttribute("aria-disabled", "true");
  // Playwright will not press a disabled item. A person can.
  await reverse.click({ force: true });
  await expect(page.locator(".cm-diagram-menu-hint")).toHaveText(
    "This arrow is part of a chain, so it cannot be reversed on its own.",
  );
  await expect(menus(page)).toHaveCount(1);
  if (SHOTS !== undefined) {
    await page.screenshot({ path: `${SHOTS}/arrow-menu.png` });
    await page.emulateMedia({ colorScheme: "dark" });
    await page.reload();
    await nodeOf(page, "A").waitFor();
    await nodeOf(page, "A").click({ button: "right" });
    await page.getByRole("menuitem", { name: "Connect to..." }).click();
    await page.screenshot({ path: `${SHOTS}/connect-dark.png` });
    await page.keyboard.press("Escape");
    await nodeOf(page, "A").click({ button: "right" });
    await page.getByRole("menuitem", { name: "Shape" }).click();
    await page.screenshot({ path: `${SHOTS}/menu-dark.png` });
  }
});

test("Tab and a press on nothing never leave the focus on the body", async ({
  page,
}) => {
  await openEditor(page, DOC);
  await stopOn(page, "Box: Second");
  await page.keyboard.press("Shift+F10");
  await page.keyboard.press("Tab");
  await expect(menus(page)).toHaveCount(0);
  expect(await focusedLabel(page)).toBe("Box: Second");
  await page.keyboard.press("Shift+F10");
  await expect(menus(page)).toHaveCount(1);
  await page.mouse.click(2, 2);
  await expect(menus(page)).toHaveCount(0);
  await expect.poll(() => focusOnBody(page)).toBe(false);
  // A press in the text takes the menu, the ring and the keyboard there.
  await page.keyboard.press("Shift+F10");
  await expect(menus(page)).toHaveCount(1);
  await page.getByText("Below marker").click();
  await expect(menus(page)).toHaveCount(0);
  await expect(page.locator(".cm-diagram-focus-ring")).toHaveCount(0);
  await expect(page.locator(".cm-content")).toBeFocused();
});

test("a redraw under an open menu keeps it; Escape lands on the new drawing", async ({
  page,
}) => {
  await openEditor(page, DOC);
  await stopOn(page, "Box: Second");
  await page.keyboard.press("Shift+F10");
  await page.keyboard.press("ArrowDown");
  await peerEdit(page, "  A[First]", "  A --> D\n");
  await expect(nodeOf(page, "D")).toBeVisible();
  await expect(menus(page)).toHaveCount(1);
  expect(await focusedName(page)).toBe("Add child");
  await page.keyboard.press("Escape");
  expect(await focusedLabel(page)).toBe("Box: Second");
  await page.keyboard.press("ArrowLeft");
  await expect.poll(() => focusedLabel(page)).not.toBe("Box: Second");
});

test("a dismissing press opens no field, and a menu's box scrolled away does not strand the keys", async ({
  page,
}) => {
  await openEditor(page, `${DOC}${"line\n\n".repeat(120)}`);
  await expect(nodeOf(page, "B")).toBeVisible();
  await nodeOf(page, "B").click({ button: "right" });
  await expect(menus(page)).toHaveCount(1);
  await nodeOf(page, "A").click();
  await expect(menus(page)).toHaveCount(0);
  await expect(page.locator(".cm-diagram-label-field")).toHaveCount(0);
  await stopOn(page, "Box: Second");
  await page.keyboard.press("Shift+F10");
  await expect(menus(page)).toHaveCount(1);
  await page.evaluate(() => {
    document.querySelector(".cm-scroller")?.scrollBy({ top: 100000 });
    document.scrollingElement?.scrollBy({ top: 100000 });
    for (const el of document.querySelectorAll("main, [class*=overflow]")) {
      el.scrollBy({ top: 100000 });
    }
  });
  await expect(box(page)).toHaveCount(0);
  await expect(menus(page)).toHaveCount(0);
  await page.evaluate(() => {
    document.querySelector(".cm-scroller")?.scrollTo({ top: 0 });
    document.scrollingElement?.scrollTo({ top: 0 });
    for (const el of document.querySelectorAll("main, [class*=overflow]")) {
      el.scrollTo({ top: 0 });
    }
  });
  await expect(box(page)).toHaveCount(1);
  await expect.poll(() => focusedLabel(page)).toBe("Box: Second");
  expect(await docText(page)).toContain("A[First] --> B[Second]");
});

test("connect by key: a prompt in the window, arrows to a box, Enter writes the line", async ({
  page,
}) => {
  await openEditor(page, DOC);
  await stopOn(page, "Box: First");
  await page.keyboard.press("Shift+F10");
  await page.getByRole("menuitem", { name: "Connect to..." }).click();
  const prompt = page.getByRole("status").filter({
    hasText: "Choose the box to connect to. Press Escape to cancel.",
  });
  await expect(prompt).toBeVisible();
  if (SHOTS !== undefined) {
    await page.screenshot({ path: `${SHOTS}/connect.png` });
  }
  await page.keyboard.press("ArrowRight");
  expect(await focusedLabel(page)).toBe("Box: Second");
  // A peer's line and a redraw: the mode stays, and the focus is on the new element.
  const old = await nodeOf(page, "B").elementHandle();
  await peerEdit(page, "  A[First]", "  %% peer\n");
  await expect.poll(() => docText(page)).toContain("%% peer");
  await expect(prompt).toBeVisible();
  // The new drawing, as a key before it lands is told "try again" (R75).
  await expect.poll(() => old?.evaluate((el) => el.isConnected)).toBe(false);
  await expect.poll(() => focusedLabel(page)).toBe("Box: Second");
  await page.keyboard.press("Enter");
  await expect.poll(() => docText(page)).toContain("A --> B");
  await expect(prompt).toHaveCount(0);
  expect(await focusOnBody(page)).toBe(false);
  await page.keyboard.press("ControlOrMeta+z");
  await expect.poll(() => docText(page)).not.toContain("A --> B");
  expect(await docText(page)).toContain("%% peer");
});

test("connect by mouse, and every way out of it writes nothing", async ({
  page,
}) => {
  await openEditor(page, DOC);
  await expect(box(page)).toBeVisible();
  const before = await docText(page);
  const prompt = () => page.locator(".cm-diagram-connect [role=status]");
  const start = async () => {
    await nodeOf(page, "A").click({ button: "right" });
    await page.getByRole("menuitem", { name: "Connect to..." }).click();
    await expect(prompt()).toBeVisible();
  };
  const gone = async () => {
    await expect(prompt()).toHaveCount(0);
    expect(await docText(page)).toBe(before);
    expect(await focusOnBody(page)).toBe(false);
  };
  await expect(nodeOf(page, "A")).toBeVisible();
  await start();
  await page.keyboard.press("Escape");
  await gone();
  expect(await focusedLabel(page)).toBe("Box: First");
  await start();
  await page.getByRole("button", { name: "Cancel" }).click();
  await gone();
  await start();
  const frame = await box(page).boundingBox();
  if (frame === null) throw new Error("no box");
  await page.mouse.click(frame.x + 4, frame.y + frame.height - 4);
  await gone();
  await start();
  await nodeOf(page, "B").click();
  await expect.poll(() => docText(page)).toContain("A --> B");
  await expect(prompt()).toHaveCount(0);
  await expect(page.locator(".cm-diagram-label-field")).toHaveCount(0);
});

test("connect mode through its box scrolled away: Escape gets out, and the prompt comes back", async ({
  page,
}) => {
  await openEditor(page, `${DOC}${"line\n\n".repeat(120)}`);
  const prompt = page.locator(".cm-diagram-connect [role=status]");
  const scroll = (top: number) =>
    page.evaluate((to) => {
      document.querySelector(".cm-scroller")?.scrollTo({ top: to });
      document.scrollingElement?.scrollTo({ top: to });
      for (const el of document.querySelectorAll("main, [class*=overflow]")) {
        el.scrollTo({ top: to });
      }
    }, top);
  await expect(nodeOf(page, "A")).toBeVisible();
  const before = await docText(page);
  const start = async () => {
    await nodeOf(page, "A").click({ button: "right" });
    await page.getByRole("menuitem", { name: "Connect to..." }).click();
    await expect(prompt).toBeVisible();
  };
  await start();
  await scroll(100000);
  await expect(box(page)).toHaveCount(0);
  await scroll(0);
  await expect(prompt).toBeVisible();
  await expect.poll(() => focusedLabel(page)).toBe("Box: First");
  await page.keyboard.press("ArrowRight");
  expect(await focusedLabel(page)).toBe("Box: Second");
  await scroll(100000);
  await expect(box(page)).toHaveCount(0);
  expect(await focusOnBody(page)).toBe(true);
  await page.keyboard.press("Escape");
  await expect(box(page)).toHaveCount(1);
  await expect(prompt).toHaveCount(0);
  await expect.poll(() => focusedLabel(page)).toBe("Box: First");
  expect(await docText(page)).toBe(before);
});
