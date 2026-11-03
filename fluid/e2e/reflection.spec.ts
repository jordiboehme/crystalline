/**
 * The Reflection against a real daemon: two accounts, both modes, the live
 * room, threads on a selected passage, pages, files, search and the switch
 * that turns a Reflection on. What only a browser can prove is that a
 * selection in rendered markdown becomes a thread the server anchors, that a
 * person at knowledge none has the Reflection as their whole world, that the
 * members card sets both axes and that an image a page links to loads,
 * follows a move, is replaced and can be attached from the live editor.
 *
 * The cast `run-smoke.sh` seeds: the admin owns `smoke-pages`, a private
 * domain with its Reflection on and no pact yet; the contributor is an
 * account at knowledge none, invited into it at knowledge none with
 * Reflection contributor.
 *
 * The journeys are one serial story and build on each other, so a single
 * journey cannot run alone (`-g` on one of them fails or skips the rest).
 * The file move runs before any live room is open on its page (a move
 * refuses while a linking page is open in a room), the pact is written
 * before the contributor reads it, the review sees the thread while it is
 * still waiting for the agent, and the curator's file journey replaces the
 * file the contributor attached.
 *
 * Run it the way the whole suite runs (`bash fluid/e2e/run-smoke.sh
 * e2e/reflection.spec.ts`).
 */

import { appendFileSync, readFileSync } from "node:fs";

import type { Locator, Page } from "@playwright/test";
import { expect, test } from "@playwright/test";

const USER = process.env.FLUID_E2E_USER ?? "smoke";
const PASSWORD = process.env.FLUID_E2E_PASSWORD ?? "smoke-password";
const CONTRIBUTOR = process.env.FLUID_E2E_CONTRIBUTOR ?? "contrib";
const CONTRIBUTOR_PASSWORD =
  process.env.FLUID_E2E_CONTRIBUTOR_PASSWORD ?? "contrib-password";
const DOMAIN = process.env.FLUID_E2E_REFLECTION_DOMAIN ?? "smoke-pages";
/**
 * A domain with its Reflection off, which the enable journey switches on.
 * The members journey (`members.spec.ts`, which runs before this file) is
 * done with it by then, and nothing after this file reads it.
 */
const OFF_DOMAIN = process.env.FLUID_E2E_PRIVATE_DOMAIN ?? "smoke-vault";
/** The engram domain the welcome page cites a source from (its fixture names it). */
const ENGRAM_DOMAIN = process.env.FLUID_E2E_DOMAIN ?? "fluid-smoke";
// `run-smoke.sh` exports it; empty unless the daemon serves under a prefix.
const BASE_PATH = process.env.FLUID_E2E_BASE_PATH ?? "";

/** The fixture's image, 32 px wide: the bytes the attach journey uploads. */
const FLOW_PNG = new URL(
  "./fixtures/reflection-domain/reflections/assets/flow.png",
  import.meta.url,
);
/** Another picture, 48 px wide: the bytes the replace journey sends. */
const WIDE_PNG = new URL(
  "./fixtures/reflection-files/wide.png",
  import.meta.url,
);

const HOME = `d/${DOMAIN}/reflection`;
const WELCOME = `${HOME}/guides/welcome.md`;
const PACT_TITLE = `How we write about ${DOMAIN}`;

async function signIn(
  page: Page,
  name: string,
  password: string,
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

/** An image that loaded: the browser decoded it, so it has a width. */
async function expectLoaded(image: Locator): Promise<void> {
  await expect(image).toBeVisible();
  await expect
    .poll(() => image.evaluate((el) => (el as HTMLImageElement).naturalWidth))
    .toBeGreaterThan(0);
}

/** The page's source as the server holds it. */
async function pageSource(page: Page, path: string): Promise<string> {
  const response = await page.request.get(
    `${BASE_PATH}/api/v1/domains/${DOMAIN}/reflections/${path}`,
  );
  return ((await response.json()) as { markdown: string }).markdown;
}

/** The sidebar tree of the Reflection. */
function tree(page: Page): Locator {
  return page.getByRole("navigation", { name: `Reflection of ${DOMAIN}` });
}

/** The pathname the browser is on, with the base path taken off. */
function at(page: Page, path: string): Promise<void> {
  return expect(page).toHaveURL(
    (url) => url.pathname === `${BASE_PATH}/${path}`,
  );
}

test.describe.serial("the Reflection journeys", () => {
  test("the owner switches modes and reads a page", async ({ page }) => {
    await signIn(page, USER, PASSWORD);
    // The domain opens on its Reflection, which comes first in the switch;
    // the engrams are one click away and have their own address.
    await page.goto(`d/${DOMAIN}`);
    await at(page, HOME);
    const modes = page.getByRole("navigation", { name: "Domain view" });
    const reflectionMode = modes.getByRole("link", {
      name: "Reflection",
      exact: true,
    });
    const engramsMode = modes.getByRole("link", {
      name: "Engrams",
      exact: true,
    });
    await expect(reflectionMode).toHaveAttribute("aria-current", "page");
    await expect(modes.getByRole("link")).toHaveText(["Reflection", "Engrams"]);
    await engramsMode.click();
    await at(page, `d/${DOMAIN}/engrams`);
    await expect(engramsMode).toHaveAttribute("aria-current", "page");
    await reflectionMode.click();
    await tree(page).getByRole("button", { name: "Expand guides" }).click();
    await tree(page).getByRole("link", { name: "Welcome" }).click();
    await expect(
      page.getByRole("heading", { name: "Welcome", level: 1 }),
    ).toBeVisible();
    await expect(page.locator("main")).not.toContainText("r:c5a0e001");
    await page
      .getByRole("button", { name: /^\d+ sources? for Night deploys$/ })
      .click();
    await expect(page.getByText("missing", { exact: true })).toBeVisible();
    // A source of another domain links into that domain and names it.
    const foreign = page.getByRole("link", { name: "Tide Tables" });
    await expect(foreign).toHaveAttribute(
      "href",
      new RegExp(`/d/${ENGRAM_DOMAIN}/e/tide-tables$`),
    );
    await expect(foreign.locator("xpath=..")).toContainText(
      `Tide Tables in domain ${ENGRAM_DOMAIN}`,
    );
    // On a phone, in light and in dark, the list with the marker fits.
    await page.setViewportSize({ width: 390, height: 844 });
    for (const colorScheme of ["light", "dark"] as const) {
      await page.emulateMedia({ colorScheme });
      await expect(foreign).toBeVisible();
      const box = await foreign.locator("xpath=ancestor::ul[1]").boundingBox();
      expect(box).not.toBeNull();
      expect((box?.x ?? 0) + (box?.width ?? 0)).toBeLessThanOrEqual(390);
      expect(box?.x ?? -1).toBeGreaterThanOrEqual(0);
    }
  });

  test("a curator writes the pact in the live editor", async ({ page }) => {
    await signIn(page, USER, PASSWORD);
    await page.goto(HOME);
    await page.getByRole("button", { name: "Write the pact" }).click();
    await at(page, `d/${DOMAIN}/reflection-edit/PACT.md`);
    const editor = page.getByRole("textbox", { name: "Page source" });
    await expect(editor).toContainText(PACT_TITLE);
    await editor.click();
    await page.keyboard.press("ControlOrMeta+A");
    await page.keyboard.press("ArrowRight");
    await editor.pressSequentially(
      "\n\n## Plain words\n\nPlain words, short sentences.",
    );
    await expect
      .poll(() => pageSource(page, "PACT.md"))
      .toContain("Plain words, short sentences.");
    await page.getByRole("link", { name: "Done", exact: true }).click();
    await expect(
      page.getByRole("heading", { name: PACT_TITLE, level: 1 }),
    ).toBeVisible();
    await expect(page.getByText("Plain words, short sentences.")).toBeVisible();
    // The pact now exists, so the home links it instead of offering it.
    await page.goto(HOME);
    await expect(
      page.getByRole("main").getByRole("link", { name: PACT_TITLE }),
    ).toBeVisible();
    await expect(
      page.getByRole("button", { name: "Write the pact" }),
    ).toHaveCount(0);
  });

  test("a curator moves a file and the page follows", async ({ page }) => {
    await signIn(page, USER, PASSWORD);
    await page.goto(WELCOME);
    await expectLoaded(page.getByRole("img", { name: "Flow" }));

    await page
      .getByRole("region", { name: "Attachments" })
      .getByRole("button", { name: "Move flow.png", exact: true })
      .click();
    const dialog = page.getByRole("dialog", { name: "Move flow.png" });
    await dialog
      .getByRole("textbox", { name: "New path" })
      .fill("diagrams/flow.png");
    await dialog.getByRole("button", { name: "Move file" }).click();
    await expect(dialog.getByRole("status")).toContainText(
      "Moved. 1 link updated.",
    );
    await dialog.getByRole("button", { name: "Close" }).click();
    await expect(dialog).toHaveCount(0);

    // The page link was rewritten on disk, and the image loads from there.
    const image = page.getByRole("img", { name: "Flow" });
    await expect(image).toHaveAttribute("src", /diagrams\/flow\.png/);
    await expectLoaded(image);
    await expect
      .poll(() => pageSource(page, "guides/welcome.md"))
      .toContain("../assets/diagrams/flow.png");
  });

  test("a knowledge-none contributor reads a reflection's image", async ({
    page,
  }) => {
    await signIn(page, CONTRIBUTOR, CONTRIBUTOR_PASSWORD);
    await page.goto(WELCOME);
    await expectLoaded(page.getByRole("img", { name: "Flow" }));
    const files = page.getByRole("region", { name: "Attachments" });
    await expect(files).toContainText("flow.png");
    // A contributor edits pages, but files are a curator's: no control on
    // the row, and no move dialog to reach.
    for (const name of [
      "Remove flow.png",
      "Move flow.png",
      "Replace flow.png",
      "Move file",
    ]) {
      await expect(page.getByRole("button", { name, exact: true })).toHaveCount(
        0,
      );
    }
    // Nothing on the page leads to an engram.
    await expect(page.locator('a[href*="/e/"]')).toHaveCount(0);
  });

  test("a contributor lands in the Reflection, comments on a passage and edits live", async ({
    page,
  }) => {
    await signIn(page, CONTRIBUTOR, CONTRIBUTOR_PASSWORD);
    await expect(
      page
        .getByRole("region", { name: "Reflection", exact: true })
        .getByRole("link", { name: DOMAIN }),
    ).toBeVisible();
    // An engram address of the domain: nothing is said about engrams, and the
    // domain's own address leads into the Reflection.
    await page.goto(`d/${DOMAIN}`);
    await at(page, HOME);
    await expect(
      page.getByRole("navigation", { name: "Domain view" }),
    ).toHaveCount(0);

    await page.goto(WELCOME);
    const passage = page.getByText(
      "Deploys run at night, after the batch window.",
    );
    await expect(passage).toBeVisible();
    // No source engram reaches a contributor: not as a control, not in the page.
    await expect(
      page.getByRole("button", { name: /^\d+ sources? for / }),
    ).toHaveCount(0);
    expect(await page.content()).not.toContain("notes/smoke-source");
    expect(await page.content()).not.toContain("tide-tables");
    // Select "Deploys run at night" the way a drag would leave it. The
    // paragraph's first child is the stamped span, whose first child is the
    // text node.
    await passage.evaluate((el) => {
      const text = el.firstChild?.firstChild ?? el.firstChild;
      if (text === null || text === undefined) {
        throw new Error("no text node");
      }
      const range = document.createRange();
      range.setStart(text, 0);
      range.setEnd(text, "Deploys run at night".length);
      const selection = window.getSelection();
      selection?.removeAllRanges();
      selection?.addRange(range);
    });
    const threads = page.getByRole("complementary", { name: "Threads" });
    await threads.getByRole("button", { name: "Comment", exact: true }).click();
    await page
      .getByRole("textbox", { name: "Your comment" })
      .fill("Is this still true?");
    await page
      .getByRole("form", { name: "New comment" })
      .getByRole("button", { name: "Comment", exact: true })
      .click();
    const card = page.getByRole("article", { name: "Thread on Night deploys" });
    await expect(card).toContainText("Deploys run at night");
    await expect(card).toContainText("Is this still true?");

    // PACT.md is written by curators: the contributor reads it and gets no
    // Edit, and proposes a change through a comment instead.
    await page.goto(`${HOME}/PACT.md`);
    await expect(
      page.getByRole("heading", { name: PACT_TITLE, level: 1 }),
    ).toBeVisible();
    await expect(
      page.getByRole("link", { name: "Edit", exact: true }),
    ).toHaveCount(0);

    // The live room: typed text lands in the file, and the ids stay hidden.
    await page.goto(WELCOME);
    await page.getByRole("link", { name: "Edit", exact: true }).click();
    const editor = page.getByRole("textbox", { name: "Page source" });
    await expect(editor).toBeVisible();
    await expect(editor).toContainText("Deploys run at night");
    await expect(editor).not.toContainText("r:c5a0e001");
    await editor.click();
    await page.keyboard.press("ControlOrMeta+A");
    await page.keyboard.press("ArrowRight");
    await editor.pressSequentially("\nWritten in the smoke.");
    await expect
      .poll(() => pageSource(page, "guides/welcome.md"))
      .toContain("Written in the smoke.");
    await page.getByRole("link", { name: "Done", exact: true }).click();
    await expect(page.getByText("Written in the smoke.")).toBeVisible();
  });

  test("the review screen lists the open thread as waiting for the agent", async ({
    page,
  }) => {
    await signIn(page, USER, PASSWORD);
    await page.goto(`${HOME}/review`);
    const agent = page.getByRole("region", { name: "Waiting for the agent" });
    await expect(agent.getByText("Is this still true?")).toBeVisible();
    await expect(
      page.getByRole("region", { name: "Reflect queue" }),
    ).toBeVisible();
  });

  test("the owner answers, resolves and reopens the thread", async ({
    page,
  }) => {
    await signIn(page, USER, PASSWORD);
    await page.goto(WELCOME);
    const card = page.getByRole("article", { name: "Thread on Night deploys" });
    await expect(card).toContainText("Is this still true?");

    await card.getByRole("button", { name: "Answer", exact: true }).click();
    await card
      .getByRole("textbox", { name: "Your answer" })
      .fill("Still true.");
    await card.getByRole("button", { name: "Answer", exact: true }).click();
    await expect(card).toContainText("Still true.");
    await expect(
      card.getByRole("textbox", { name: "Your answer" }),
    ).toHaveCount(0);

    await card.getByRole("button", { name: "Resolve", exact: true }).click();
    await card
      .getByRole("textbox", { name: "Outcome" })
      .fill("Checked with the night shift.");
    await card.getByRole("button", { name: "Resolve", exact: true }).click();
    // Resolved threads leave the column until somebody asks for them.
    await expect(card).toHaveCount(0);

    await page.getByRole("switch", { name: "Show resolved threads" }).click();
    await expect(card).toContainText("Resolved: Checked with the night shift.");
    await card.getByRole("button", { name: "Reopen" }).click();
    await expect(card).toContainText("Reopened");
    await expect(
      card.getByRole("button", { name: "Answer", exact: true }),
    ).toBeVisible();
  });

  test("on a phone the threads are a sheet behind a pill, and a tap on a quote opens it at its thread", async ({
    page,
  }) => {
    await page.setViewportSize({ width: 390, height: 844 });
    await signIn(page, USER, PASSWORD);
    await page.goto(WELCOME);
    const passage = page.getByText(
      "Deploys run at night, after the batch window.",
    );
    await expect(passage).toBeVisible();
    await expect(
      page.getByRole("complementary", { name: "Threads" }),
    ).toHaveCount(0);
    const pill = page.getByRole("button", { name: "Show 1 thread" });
    await pill.click();
    const sheet = page.getByRole("dialog", { name: "Threads" });
    await expect(
      sheet.getByRole("article", { name: "Thread on Night deploys" }),
    ).toBeVisible();
    await page.keyboard.press("Escape");
    await expect(sheet).toHaveCount(0);
    await expect(pill).toBeFocused();
    // The quote is "Deploys run at night": a tap on its first word.
    const box = await passage.boundingBox();
    if (box === null) {
      throw new Error("the passage has no box");
    }
    await page.mouse.click(box.x + 8, box.y + 10);
    const card = sheet.getByRole("article", {
      name: "Thread on Night deploys",
    });
    await expect(card).toBeFocused();
    await page.keyboard.press("Escape");
    await expect(sheet).toHaveCount(0);

    // A comment on a phone: select on the page, then Comment in the sheet.
    await passage.evaluate((el) => {
      const text = el.firstChild?.firstChild ?? el.firstChild;
      if (text === null || text === undefined) {
        throw new Error("no text node");
      }
      const start = (text.textContent ?? "").indexOf("after the batch window");
      const range = document.createRange();
      range.setStart(text, start);
      range.setEnd(text, start + "after the batch window".length);
      const selection = window.getSelection();
      selection?.removeAllRanges();
      selection?.addRange(range);
    });
    await pill.click();
    await sheet.getByRole("button", { name: "Comment", exact: true }).click();
    await sheet
      .getByRole("textbox", { name: "Your comment" })
      .fill("Which batch window?");
    await sheet
      .getByRole("form", { name: "New comment" })
      .getByRole("button", { name: "Comment", exact: true })
      .click();
    const added = sheet
      .getByRole("article", { name: "Thread on Night deploys" })
      .filter({ hasText: "Which batch window?" });
    await expect(added).toContainText("after the batch window");
    // Resolved again, so the journeys after this one see one open thread.
    await added.getByRole("button", { name: "Resolve", exact: true }).click();
    await added.getByRole("textbox", { name: "Outcome" }).fill("Answered.");
    await added.getByRole("button", { name: "Resolve", exact: true }).click();
    await expect(added).toHaveCount(0);
  });

  test("a keyboard-only reader selects with Shift and the arrow keys, comments, and sees the highlight", async ({
    page,
  }) => {
    await signIn(page, USER, PASSWORD);
    await page.goto(WELCOME);
    const passage = page.getByText(
      "Deploys run at night, after the batch window.",
    );
    await expect(passage).toBeVisible();
    const phrase = "after the batch window";
    const painted = () =>
      page.evaluate(() => CSS.highlights.get("reflection-quote")?.size ?? 0);
    await expect.poll(painted).toBeGreaterThan(0);
    const before = await painted();

    // The page has to be quiet first. A refetch is NOT what drops the
    // selection: ReflectionPage.test.tsx shows that a window-focus refetch,
    // with the same page or with only the threads changed, replaces no text
    // node and keeps the selection. What was seen is a Shift+Arrow run that
    // came up short in a full run, so the keys are checked by polling and the
    // whole selection is retried from its first letter.
    await page.waitForLoadState("networkidle");
    const selected = () =>
      page.evaluate(() => window.getSelection()?.toString() ?? "");
    // Start on the first letter of "after" (a caret alone is not kept in text
    // that cannot be edited) and grow the selection one key at a time. If the
    // selection is lost on the way, start again from the first letter.
    await expect(async () => {
      await passage.evaluate((el, word) => {
        const text = el.firstChild?.firstChild ?? el.firstChild;
        if (text === null || text === undefined) {
          throw new Error("no text node");
        }
        const at = (text.textContent ?? "").indexOf(word);
        const range = document.createRange();
        range.setStart(text, at);
        range.setEnd(text, at + 1);
        const selection = window.getSelection();
        selection?.removeAllRanges();
        selection?.addRange(range);
      }, phrase);
      await expect.poll(selected, { timeout: 2_000 }).toBe(phrase[0]);
      for (let i = 1; i < phrase.length; i += 1) {
        await page.keyboard.press("Shift+ArrowRight");
      }
      await expect.poll(selected, { timeout: 2_000 }).toBe(phrase);
    }).toPass({ timeout: 20_000 });

    // Tab on to the Comment button and open it with Enter; the selection stays.
    const threads = page.getByRole("complementary", { name: "Threads" });
    const open = threads.getByRole("button", { name: "Comment", exact: true });
    await expect(open).toBeVisible();
    for (let i = 0; i < 40; i += 1) {
      if (await open.evaluate((el) => el === document.activeElement)) {
        break;
      }
      await page.keyboard.press("Tab");
    }
    await expect(open).toBeFocused();
    await page.keyboard.press("Enter");
    const field = page.getByRole("textbox", { name: "Your comment" });
    await expect(field).toBeFocused();
    await page.keyboard.type("Which batch window?");
    // The Label menu stands left of the send button.
    await page.keyboard.press("Tab");
    await expect(
      page
        .getByRole("form", { name: "New comment" })
        .getByRole("button", { name: "Label", exact: true }),
    ).toBeFocused();
    await page.keyboard.press("Tab");
    const post = page
      .getByRole("form", { name: "New comment" })
      .getByRole("button", { name: "Comment", exact: true });
    await expect(post).toBeFocused();
    await page.keyboard.press("Enter");

    const added = page
      .getByRole("article", { name: "Thread on Night deploys" })
      .filter({ hasText: "Which batch window?" });
    await expect(added).toContainText(phrase);
    await expect.poll(painted).toBeGreaterThan(before);
    await expect
      .poll(() =>
        page.evaluate(() => {
          const ranges = [...(CSS.highlights.get("reflection-quote") ?? [])];
          return ranges.map((range) => (range as Range).toString());
        }),
      )
      .toContain(phrase);

    // Resolved again, so the journeys after this one see one open thread.
    await added.getByRole("button", { name: "Resolve", exact: true }).click();
    await added.getByRole("textbox", { name: "Outcome" }).fill("Answered.");
    await added.getByRole("button", { name: "Resolve", exact: true }).click();
    await expect(added).toHaveCount(0);
  });

  test("a reader comments from the bar at the selected text", async ({
    page,
  }) => {
    await signIn(page, USER, PASSWORD);
    await page.goto(WELCOME);
    const passage = page.getByText(
      "Deploys run at night, after the batch window.",
    );
    await expect(passage).toBeVisible();
    const phrase = "after the batch window";
    await passage.evaluate((el, words) => {
      const text = el.firstChild?.firstChild ?? el.firstChild;
      if (text === null || text === undefined) {
        throw new Error("no text node");
      }
      const start = (text.textContent ?? "").indexOf(words);
      const range = document.createRange();
      range.setStart(text, start);
      range.setEnd(text, start + words.length);
      const selection = window.getSelection();
      selection?.removeAllRanges();
      selection?.addRange(range);
    }, phrase);
    // The bar stands just above the passage, not over it.
    const bar = page.getByRole("toolbar", { name: "Selection actions" });
    await expect(bar).toBeVisible();
    const barBox = await bar.boundingBox();
    const textBox = await passage.boundingBox();
    expect(barBox).not.toBeNull();
    expect(textBox).not.toBeNull();
    expect((barBox?.y ?? 0) + (barBox?.height ?? 0)).toBeLessThanOrEqual(
      textBox?.y ?? 0,
    );
    await bar
      .getByRole("button", { name: "Comment on selection", exact: true })
      .click();
    const form = page.getByRole("form", { name: "New comment" });
    await expect(form).toContainText(phrase);
    const field = form.getByRole("textbox", { name: "Your comment" });
    await expect(field).toBeFocused();
    await expect(bar).toHaveCount(0);
    await field.fill("Is the window fixed?");
    await form.getByRole("button", { name: "Comment", exact: true }).click();
    const added = page
      .getByRole("complementary", { name: "Threads" })
      .getByRole("article", { name: "Thread on Night deploys" })
      .filter({ hasText: "Is the window fixed?" });
    await expect(added).toContainText(phrase);

    // Resolved again, so the journeys after this one see one open thread.
    await added.getByRole("button", { name: "Resolve", exact: true }).click();
    await added.getByRole("textbox", { name: "Outcome" }).fill("Answered.");
    await added.getByRole("button", { name: "Resolve", exact: true }).click();
    await expect(added).toHaveCount(0);
  });

  test("a reader labels a comment from the menu or by typing, and filters to blocking threads", async ({
    page,
  }) => {
    // On a phone first: the Label menu opens inside the sheet and fits.
    await page.setViewportSize({ width: 390, height: 844 });
    await signIn(page, USER, PASSWORD);
    await page.goto(WELCOME);
    const passage = page.getByText(
      "Deploys run at night, after the batch window.",
    );
    await expect(passage).toBeVisible();
    const selectWords = (words: string) =>
      passage.evaluate((el, phrase) => {
        const text = el.firstChild?.firstChild ?? el.firstChild;
        if (text === null || text === undefined) {
          throw new Error("no text node");
        }
        const start = (text.textContent ?? "").indexOf(phrase);
        const range = document.createRange();
        range.setStart(text, start);
        range.setEnd(text, start + phrase.length);
        const selection = window.getSelection();
        selection?.removeAllRanges();
        selection?.addRange(range);
      }, words);
    await selectWords("after the batch window");
    await page.getByRole("button", { name: "Show 1 thread" }).click();
    const sheet = page.getByRole("dialog", { name: "Threads" });
    await sheet.getByRole("button", { name: "Comment", exact: true }).click();
    const form = sheet.getByRole("form", { name: "New comment" });
    await form.getByRole("button", { name: "Label", exact: true }).click();
    const menu = page.getByRole("menu");
    await expect(menu).toBeVisible();
    const box = await menu.boundingBox();
    expect(box).not.toBeNull();
    expect(box?.x ?? -1).toBeGreaterThanOrEqual(0);
    expect((box?.x ?? 0) + (box?.width ?? 0)).toBeLessThanOrEqual(390);
    await menu.getByRole("menuitemradio", { name: /^question/ }).click();
    await menu.getByRole("menuitemcheckbox", { name: /^non-blocking/ }).click();
    await page.keyboard.press("Escape");
    await expect(menu).toHaveCount(0);
    const field = form.getByRole("textbox", { name: "Your comment" });
    await expect(field).toBeFocused();
    await expect(
      form.getByRole("button", { name: "Remove the label" }),
    ).toBeVisible();
    await field.fill("Which batch window?");
    await form.getByRole("button", { name: "Comment", exact: true }).click();
    const asked = sheet
      .getByRole("article", { name: "Thread on Night deploys" })
      .filter({ hasText: "Which batch window?" });
    const pill = asked.locator("[data-label-pill]");
    await expect(pill).toContainText("question");
    await expect(pill).toContainText("non-blocking");
    await page.keyboard.press("Escape");
    await expect(sheet).toHaveCount(0);

    // Beside the page: the exact head typed at the start becomes the chip.
    await page.setViewportSize({ width: 1280, height: 720 });
    const threads = page.getByRole("complementary", { name: "Threads" });
    await selectWords("Deploys run at night");
    await threads.getByRole("button", { name: "Comment", exact: true }).click();
    const typed = page.getByRole("form", { name: "New comment" });
    const words = typed.getByRole("textbox", { name: "Your comment" });
    await words.pressSequentially("issue (blocking): The batch moved.");
    await expect(words).toHaveValue("The batch moved.");
    await expect(
      typed.getByRole("button", { name: "Remove the label" }),
    ).toBeVisible();
    await typed.getByRole("button", { name: "Comment", exact: true }).click();
    const blocking = threads
      .getByRole("article", { name: "Thread on Night deploys" })
      .filter({ hasText: "The batch moved." });
    await expect(blocking.locator("[data-label-pill]")).toContainText("issue");
    await expect(
      page.getByRole("article", { name: "Thread on Night deploys" }),
    ).toHaveCount(3);

    // Blocking only: the other threads leave the list until it is off.
    const only = threads.getByRole("switch", {
      name: "Show blocking threads only",
    });
    await only.click();
    await expect(
      page.getByRole("article", { name: "Thread on Night deploys" }),
    ).toHaveCount(1);
    await expect(blocking).toBeVisible();
    await only.click();
    await expect(
      page.getByRole("article", { name: "Thread on Night deploys" }),
    ).toHaveCount(3);

    // Resolved again, so the journeys after this one see one open thread.
    const asked2 = threads
      .getByRole("article", { name: "Thread on Night deploys" })
      .filter({ hasText: "Which batch window?" });
    for (const added of [blocking, asked2]) {
      await added.getByRole("button", { name: "Resolve", exact: true }).click();
      await added.getByRole("textbox", { name: "Outcome" }).fill("Answered.");
      await added.getByRole("button", { name: "Resolve", exact: true }).click();
      await expect(added).toHaveCount(0);
    }
    await expect(only).toHaveCount(0);
  });

  test("the quote highlight paints in light and in dark", async ({ page }) => {
    await signIn(page, USER, PASSWORD);
    for (const scheme of ["light", "dark"] as const) {
      await page.emulateMedia({ colorScheme: scheme });
      await page.goto(WELCOME);
      await expect
        .poll(() =>
          page.evaluate(
            () => CSS.highlights.get("reflection-quote")?.size ?? 0,
          ),
        )
        .toBeGreaterThan(0);
      // What the stylesheet paints for `::highlight(reflection-quote)`: the
      // passage with the quotes registered differs from the same passage
      // with them taken away.
      const passage = page.getByText(
        "Deploys run at night, after the batch window.",
      );
      const painted = await passage.screenshot();
      await page.evaluate(() => CSS.highlights.delete("reflection-quote"));
      expect(painted.equals(await passage.screenshot())).toBe(false);
    }
  });

  test("at 1100 wide the threads are a sheet too, beside the sidebar", async ({
    page,
  }) => {
    await page.setViewportSize({ width: 1100, height: 900 });
    await signIn(page, USER, PASSWORD);
    await page.goto(WELCOME);
    await expect(
      page.getByText("Deploys run at night, after the batch window."),
    ).toBeVisible();
    await expect(
      page.getByRole("complementary", { name: "Threads" }),
    ).toHaveCount(0);
    const pill = page.getByRole("button", { name: "Show 1 thread" });
    await pill.click();
    const sheet = page.getByRole("dialog", { name: "Threads" });
    await expect(
      sheet.getByRole("article", { name: "Thread on Night deploys" }),
    ).toBeVisible();
    await page.keyboard.press("Escape");
    await expect(sheet).toHaveCount(0);
    await expect(pill).toBeFocused();
  });

  test("a contributor attaches a file in the live editor", async ({ page }) => {
    await signIn(page, CONTRIBUTOR, CONTRIBUTOR_PASSWORD);
    await page.goto(WELCOME);
    await page.getByRole("link", { name: "Edit", exact: true }).click();
    const editor = page.getByRole("textbox", { name: "Page source" });
    await expect(editor).toContainText("Written in the smoke.");
    // No caret placed: the link goes to the end of the text, not above the
    // title where the untouched caret sits.
    const attach = page.getByRole("button", { name: "Attach a file" });
    await expect(attach).toBeEnabled();
    const chooser = page.waitForEvent("filechooser");
    await attach.click();
    await (
      await chooser
    ).setFiles({
      name: "sketch.png",
      mimeType: "image/png",
      buffer: readFileSync(FLOW_PNG),
    });
    await expect(editor).toContainText(/\.\.\/assets\/\S*sketch\.png/);
    await expect
      .poll(() => pageSource(page, "guides/welcome.md"))
      .toMatch(/Written in the smoke\.\n\n!\[sketch\.png\]\([^)]*\)\n$/);
    await page.getByRole("link", { name: "Done", exact: true }).click();
    await expectLoaded(page.getByRole("img", { name: "sketch.png" }));
  });

  test("a curator replaces and removes a file", async ({ page }) => {
    await signIn(page, USER, PASSWORD);
    await page.goto(WELCOME);
    const sketch = page.getByRole("img", { name: "sketch.png" });
    await expectLoaded(sketch);
    expect(
      await sketch.evaluate((el) => (el as HTMLImageElement).naturalWidth),
    ).toBe(32);
    const files = page.getByRole("region", { name: "Attachments" });

    const chooser = page.waitForEvent("filechooser");
    await files
      .getByRole("button", { name: "Replace sketch.png", exact: true })
      .click();
    await (
      await chooser
    ).setFiles({
      name: "sketch.png",
      mimeType: "image/png",
      buffer: readFileSync(WIDE_PNG),
    });
    await files.getByRole("button", { name: "Replace", exact: true }).click();
    // The new bytes under the same path: the address names the version, and
    // the browser draws the new picture.
    await expect(sketch).toHaveAttribute("src", /sketch\.png\?v=/);
    await expect
      .poll(() =>
        sketch.evaluate((el) => (el as HTMLImageElement).naturalWidth),
      )
      .toBe(48);

    await files
      .getByRole("button", { name: "Remove sketch.png", exact: true })
      .click();
    await files.getByRole("button", { name: "Remove", exact: true }).click();
    // The page still links it, so the row stays and says the file is gone.
    await expect(
      files.getByRole("button", { name: "Remove sketch.png", exact: true }),
    ).toHaveCount(0);
    await expect(files.getByText("missing", { exact: true })).toBeVisible();
    // The file the curator moved earlier is untouched.
    await expectLoaded(page.getByRole("img", { name: "Flow" }));
  });

  test("a curator creates, moves and deletes a page", async ({ page }) => {
    await signIn(page, USER, PASSWORD);
    await page.goto(HOME);
    await tree(page).getByRole("button", { name: "New page" }).click();
    const create = page.getByRole("dialog", { name: "New page" });
    await create.getByRole("textbox", { name: "Folder" }).fill("scratch");
    await create.getByRole("textbox", { name: "Title" }).fill("Tmp");
    await create.getByRole("button", { name: "Create page" }).click();
    // A new page opens in the editor.
    await at(page, `d/${DOMAIN}/reflection-edit/scratch/tmp.md`);
    await expect(
      page.getByRole("textbox", { name: "Page source" }),
    ).toContainText("Tmp");
    await page.getByRole("link", { name: "Done", exact: true }).click();
    await at(page, `${HOME}/scratch/tmp.md`);
    await expect(
      page.getByRole("heading", { name: "Tmp", level: 1 }),
    ).toBeVisible();
    await expect(
      tree(page).getByRole("button", { name: "Collapse scratch" }),
    ).toBeVisible();
    await expect(tree(page).getByRole("link", { name: "Tmp" })).toBeVisible();

    // Download as Markdown hands over the page under its own file name.
    await page.getByRole("button", { name: "More actions" }).click();
    const [download] = await Promise.all([
      page.waitForEvent("download"),
      page.getByRole("menuitem", { name: "Download as Markdown" }).click(),
    ]);
    expect(download.suggestedFilename()).toBe("tmp.md");

    await page.getByRole("button", { name: "More actions" }).click();
    await page.getByRole("menuitem", { name: "Move", exact: true }).click();
    const move = page.getByRole("dialog", { name: "Move Tmp" });
    await move
      .getByRole("textbox", { name: "New path" })
      .fill("archive/tmp.md");
    await move.getByRole("button", { name: "Move page" }).click();
    await expect(move).toHaveCount(0);
    await at(page, `${HOME}/archive/tmp.md`);
    await expect(
      page.getByRole("heading", { name: "Tmp", level: 1 }),
    ).toBeFocused();
    await expect(
      tree(page).getByRole("button", { name: "Collapse archive" }),
    ).toBeVisible();
    await expect(tree(page).getByRole("link", { name: "Tmp" })).toBeVisible();
    await expect(
      tree(page).getByRole("button", { name: /^(Expand|Collapse) scratch$/ }),
    ).toHaveCount(0);

    await page.getByRole("button", { name: "More actions" }).click();
    await page.getByRole("menuitem", { name: "Delete", exact: true }).click();
    await page.getByRole("button", { name: "Confirm delete" }).click();
    await at(page, HOME);
    await expect(tree(page).getByRole("link", { name: "Tmp" })).toHaveCount(0);
    await expect(
      tree(page).getByRole("button", { name: /^(Expand|Collapse) archive$/ }),
    ).toHaveCount(0);

    // Every page the story made or kept is still in the tree, and nothing
    // else: the pact, the two guides, no assets folder and no left-over
    // folder from the move.
    await expect(
      tree(page).getByRole("link", { name: "How we write", exact: true }),
    ).toBeVisible();
    await tree(page).getByRole("button", { name: "Expand guides" }).click();
    await expect(
      tree(page).getByRole("link", { name: "Welcome" }),
    ).toBeVisible();
    await expect(
      tree(page).getByRole("link", { name: "Diagram" }),
    ).toBeVisible();
    for (const folder of ["assets", "diagrams", "scratch", "archive"]) {
      await expect(
        tree(page).getByRole("button", {
          name: new RegExp(`^(Expand|Collapse) ${folder}$`),
        }),
      ).toHaveCount(0);
    }
  });

  test("a page moved while somebody edits it opens at its new path", async ({
    page,
    browser,
  }) => {
    // One tab writes a new page and keeps its editor open.
    await signIn(page, USER, PASSWORD);
    await page.goto(HOME);
    await tree(page).getByRole("button", { name: "New page" }).click();
    const create = page.getByRole("dialog", { name: "New page" });
    await create.getByRole("textbox", { name: "Folder" }).fill("scratch");
    await create.getByRole("textbox", { name: "Title" }).fill("Live");
    await create.getByRole("button", { name: "Create page" }).click();
    await at(page, `d/${DOMAIN}/reflection-edit/scratch/live.md`);
    const editor = page.getByRole("textbox", { name: "Page source" });
    await editor.click();
    await page.keyboard.press("ControlOrMeta+End");
    await page.keyboard.type("\nTyped while it moved.");
    await expect(page.getByText("Saved", { exact: true })).toBeVisible();

    // A second browser moves the page from its reading view.
    const other = await browser.newContext();
    const mover = await other.newPage();
    await signIn(mover, USER, PASSWORD);
    await mover.goto(`${HOME}/scratch/live.md`);
    await mover.getByRole("button", { name: "More actions" }).click();
    await mover.getByRole("menuitem", { name: "Move", exact: true }).click();
    const move = mover.getByRole("dialog", { name: "Move Live" });
    await move
      .getByRole("textbox", { name: "New path" })
      .fill("archive/live.md");
    await move.getByRole("button", { name: "Move page" }).click();
    await expect(move).toHaveCount(0);

    // The editor says where the page went and opens it there.
    await expect(
      page.getByText("This page was moved to archive/live.md."),
    ).toBeVisible();
    await page.getByRole("button", { name: "Open it there" }).click();
    await at(page, `d/${DOMAIN}/reflection-edit/archive/live.md`);
    await expect(
      page.getByRole("textbox", { name: "Page source" }),
    ).toContainText("Typed while it moved.");
    await page.getByRole("link", { name: "Done", exact: true }).click();
    await at(page, `${HOME}/archive/live.md`);

    // The story leaves no page behind.
    await page.getByRole("button", { name: "More actions" }).click();
    await page.getByRole("menuitem", { name: "Delete", exact: true }).click();
    await page.getByRole("button", { name: "Confirm delete" }).click();
    await at(page, HOME);
    await other.close();
  });

  test("the Reflection search finds a page by its words", async ({ page }) => {
    await signIn(page, USER, PASSWORD);
    await page.goto(HOME);
    const search = page.getByRole("search", { name: "Search the Reflection" });
    await search
      .getByRole("searchbox", { name: "Search the Reflection" })
      .fill("batch window");
    // Text mode reads the full-text index alone, so no embedding model is
    // needed.
    await search.getByRole("combobox", { name: "Mode" }).selectOption("text");
    await search.getByRole("button", { name: "Search", exact: true }).click();
    const hit = page.getByRole("link", { name: "Welcome: Night deploys" });
    await expect(hit).toBeVisible();
    await expect(page.getByText(/ranked by text\./)).toBeVisible();
    await hit.click();
    await expect(
      page.getByRole("heading", { name: "Welcome", level: 1 }),
    ).toBeVisible();
  });

  test("a thread an agent answered waits for the person who opened it, at 390 in light and dark", async ({
    page,
  }) => {
    await signIn(page, USER, PASSWORD);
    await page.setViewportSize({ width: 390, height: 844 });
    for (const scheme of ["light", "dark"] as const) {
      await page.emulateMedia({ colorScheme: scheme });
      await page.goto(`${HOME}/guides/diagram.md`);
      await page.getByRole("button", { name: "Show 1 thread" }).click();
      const card = page.getByRole("article", { name: "Thread on Old diagram" });
      // Open, so it shows without Show resolved.
      const cue = card.getByText("Answered, waiting for you");
      await expect(cue).toBeVisible();
      const box = await cue.boundingBox();
      expect((box?.x ?? 0) + (box?.width ?? 0)).toBeLessThanOrEqual(390);
      const colour = await cue.evaluate((el) => getComputedStyle(el).color);
      const background = await page.evaluate(
        () => getComputedStyle(document.body).backgroundColor,
      );
      expect(colour).not.toBe(background);
    }
  });

  test("a page refuses to show an engram's file", async ({ page }) => {
    await signIn(page, USER, PASSWORD);
    await page.goto(`${HOME}/guides/diagram.md`);
    const article = page.locator("main article").first();
    await expect(article).toContainText("Old harbor diagram");
    await expect(article).toContainText(
      "This file is not in the Reflection. A curator or an agent copies it into the Reflection's files.",
    );
    await expect(
      page.getByRole("img", { name: "Old harbor diagram" }),
    ).toHaveCount(0);
    await expect(page.locator('img[src*="old-harbor"]')).toHaveCount(0);
  });

  test("a comment written to the threads file on disk shows on the open page", async ({
    page,
  }) => {
    // `run-smoke.sh` exports where the daemon's copy of the fixture lives.
    const root = process.env.FLUID_E2E_REFLECTION_ROOT;
    test.skip(!root, "run through run-smoke.sh, which names the domain root");
    await signIn(page, USER, PASSWORD);
    await page.goto(`${HOME}/guides/diagram.md`);
    const card = page.getByRole("article", { name: "Thread on Old diagram" });
    await expect(card).toContainText("It is on the welcome page now.");
    // Another process appends to the file, as an agent outside the daemon
    // or a pull would. No reload: the change stream brings it.
    appendFileSync(
      `${root}/reflections/guides/diagram.threads.md`,
      "- 2026-10-19T09:00:00Z, comment by scribe (agent): The legend moved under the picture.\n",
    );
    await expect(card).toContainText("The legend moved under the picture.", {
      timeout: 10_000,
    });
  });

  test("the domain policies card turns a Reflection on", async ({ page }) => {
    await signIn(page, USER, PASSWORD);
    // The policies live on the domain's settings screen.
    await page.goto(`d/${OFF_DOMAIN}/settings`);
    await expect(
      page.getByRole("navigation", { name: "Domain view" }),
    ).toHaveCount(0);
    const policies = page.getByRole("region", { name: "Domain policies" });
    const reflection = policies.getByRole("combobox", { name: "reflection" });
    await expect(reflection).toHaveValue("off");
    await reflection.selectOption("on");
    await policies
      .getByRole("button", { name: "Turn on the Reflection" })
      .click();
    await expect(reflection).toHaveValue("on");
    const modes = page.getByRole("navigation", { name: "Domain view" });
    await modes.getByRole("link", { name: "Reflection", exact: true }).click();
    await at(page, `d/${OFF_DOMAIN}/reflection`);
    // Switching it on wrote a first pact.
    await expect(
      page
        .getByRole("navigation", { name: `Reflection of ${OFF_DOMAIN}` })
        .getByRole("link", { name: "How we write", exact: true }),
    ).toBeVisible();
  });

  test("the members card and the users screen set both axes side by side", async ({
    page,
  }) => {
    await signIn(page, USER, PASSWORD);
    // The members card is on the domain's settings screen.
    await page.goto(`d/${DOMAIN}/settings`);
    await expect(
      page.getByRole("combobox", { name: `Level for ${CONTRIBUTOR}` }),
    ).toHaveValue("none");
    await expect(
      page.getByRole("combobox", { name: `Reflection for ${CONTRIBUTOR}` }),
    ).toHaveValue("contributor");
    await page.goto("users");
    await expect(
      page.getByRole("combobox", { name: `Role for ${CONTRIBUTOR}` }),
    ).toHaveValue("none");
    // The smoke's own admin curates: the cell says so and offers no choice.
    await expect(
      page.getByRole("combobox", { name: `Reflection for ${USER}` }),
    ).toHaveCount(0);
  });
});
