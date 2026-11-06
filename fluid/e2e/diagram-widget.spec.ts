/**
 * The drawn diagram in a real browser: what jsdom cannot answer. The flip from
 * the preview moves the page by the height of the source lines and by nothing
 * more; an SVG landing moves it by what the held height was off; scrolling a long
 * document with several diagrams does not jump; the label goes through
 * `drawnLabels`; mermaid leaves nothing behind.
 *
 * Measured on Chromium through a `requestAnimationFrame` sampler, so a move that
 * lasts one frame counts. Limits:
 * - FLIP_SLACK_PX: text below a flip moves by the source lines' height, within this.
 * - STILL_PX: after the flip, across a drawing landing or a scroll settling, within this.
 * - STILL_PX also: an edit's new drawing landing moves the text below in one step,
 *   by the new height less the old, and the box never drops below its old height.
 * - RESERVED_SLACK_PX: a plain chain with nothing to reuse, first drawn in the
 *   window: what its reserved height was off by.
 * - The flip back to text: the page ends where the source and the caption put it,
 *   in at most 2 positions: the box's height holds until the caption is there. It
 *   was 3 positions, jumping twice (591.06, 496.16, 528.16).
 * - The flip back with the drawing unchanged (R80): at most 2 positions, moved by the
 *   source lines within FLIP_SLACK_PX, and no empty frame in the preview.
 * - SCROLL_JUMP_PX: a line in the window, any frame of a scroll over diagrams not
 *   drawn yet, against the same walk with no diagram. Measured: 4, in the frame a
 *   600 px wheel step applied in. With the scroller's correction taken out: 12.
 *
 * Shares the daemon, the fixture domain and the accounts `run-smoke.sh` sets
 * up; run it the same way (`bash fluid/e2e/run-smoke.sh e2e/diagram-widget.spec.ts`).
 */

import type { Locator, Page } from "@playwright/test";
import { expect, test } from "@playwright/test";

const USER = process.env.FLUID_E2E_USER ?? "smoke";
const PASSWORD = process.env.FLUID_E2E_PASSWORD ?? "smoke-password";
const DOMAIN = process.env.FLUID_E2E_DOMAIN ?? "fluid-smoke";
const BASE_PATH = process.env.FLUID_E2E_BASE_PATH ?? "";

const FLIP_SLACK_PX = 2;
const STILL_PX = 2;
const SCROLL_JUMP_PX = 6;
const RESERVED_SLACK_PX = 8;

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

async function top(locator: Locator): Promise<number> {
  const box = await locator.boundingBox();
  if (box === null) throw new Error("line is not on the page");
  return box.y;
}

/** Starts logging, each frame, where the line with exactly this text stands. */
async function startSampler(page: Page, text: string): Promise<void> {
  await page.evaluate((wanted) => {
    const samples: number[] = [];
    (window as unknown as { __tops: number[] }).__tops = samples;
    const tick = () => {
      const found = [
        ...document.querySelectorAll(".cm-content > .cm-line"),
      ].find((el) => el.textContent === wanted);
      if (found) {
        samples.push(found.getBoundingClientRect().top);
      }
      requestAnimationFrame(tick);
    };
    requestAnimationFrame(tick);
  }, text);
}

/** The tops logged so far, without runs of the same value. */
async function samples(page: Page): Promise<number[]> {
  const tops = await page.evaluate(
    () => (window as unknown as { __tops: number[] }).__tops,
  );
  return tops.filter((value, i) => value !== tops[i - 1]);
}

const settle = (page: Page, ms = 1500) => page.waitForTimeout(ms);

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

test("the flip from the preview moves the page by the source lines and no more", async ({
  page,
}, info) => {
  // Refused at first, so it stays text; fixed by typing, so it is held; the
  // preview then holds a drawing for the body the flip draws.
  await openEditor(
    page,
    "Above\n\n```mermaid\nflowchart TD\n  A --> B\n  B --> C\n  C -->\n```\n\nBelow marker\n\nLast line\n",
  );
  await expect(line(page, "Below marker")).toBeVisible();
  await page.locator(".cm-content > .cm-line", { hasText: "C -->" }).click();
  await page.keyboard.press("End");
  await page.keyboard.type(" D");
  const preview = page.locator(".cm-mermaid-preview svg");
  await expect(preview).toBeVisible();
  await settle(page);
  await expect(page.locator(".cm-diagram")).toHaveCount(0);

  const previewId = await preview.getAttribute("id");
  const previewBox = await page.locator(".cm-mermaid-preview").boundingBox();
  const sourceHeight = await page.evaluate(() =>
    [...document.querySelectorAll(".cm-content > .cm-line")]
      .filter((el) => el.classList.contains("cm-fence-mono"))
      .reduce((sum, el) => sum + el.getBoundingClientRect().height, 0),
  );
  expect(sourceHeight).toBeGreaterThan(50);
  const before = await top(line(page, "Below marker"));
  await startSampler(page, "Below marker");

  // Off the closing line: two keys, the first lands on it.
  await page.keyboard.press("ArrowDown");
  await page.keyboard.press("ArrowDown");
  await expect(page.locator(".cm-diagram")).toHaveCount(1);
  await settle(page);

  const after = await top(line(page, "Below marker"));
  const moved = before - after;
  const seen = await samples(page);
  const boxBox = await page.locator(".cm-diagram").boundingBox();
  info.annotations.push({
    type: "flip",
    description: JSON.stringify({
      before,
      after,
      moved,
      sourceHeight,
      previewHeight: previewBox?.height,
      boxHeight: boxBox?.height,
      seen,
    }),
  });

  // The same drawing, moved over: no second render, no empty frame.
  expect(await page.locator(".cm-diagram svg").getAttribute("id")).toBe(
    previewId,
  );
  expect(Math.abs(moved - sourceHeight)).toBeLessThanOrEqual(FLIP_SLACK_PX);
  expect(
    Math.abs((boxBox?.height ?? 0) - (previewBox?.height ?? 0)),
  ).toBeLessThanOrEqual(FLIP_SLACK_PX);
  // One step: from where it stood to where it ends, nothing between.
  expect(seen.length).toBeLessThanOrEqual(2);
  expect(Math.abs((seen.at(-1) ?? 0) - after)).toBeLessThanOrEqual(STILL_PX);
});

test("a cold open comes to rest", async ({ page }, info) => {
  await page.addInitScript(() => {
    const samples: {
      top: number;
      box: number;
      svg: boolean;
      preview: boolean;
      editor: number;
      above: string;
    }[] = [];
    (window as unknown as { __cold: unknown }).__cold = samples;
    const tick = () => {
      const below = [
        ...document.querySelectorAll(".cm-content > .cm-line"),
      ].find((el) => el.textContent === "Below marker");
      if (below) {
        const box = document.querySelector(".cm-diagram");
        samples.push({
          top: below.getBoundingClientRect().top,
          box: box?.getBoundingClientRect().height ?? 0,
          svg: box?.querySelector("svg") != null,
          preview: document.querySelector(".cm-mermaid-preview svg") != null,
          editor:
            document.querySelector(".cm-editor")?.getBoundingClientRect().top ??
            0,
          // What stands above the editor in its own column, by height.
          above: [
            ...(document.querySelector("main")?.querySelectorAll("*") ?? []),
          ]
            .filter((el) => {
              const rect = el.getBoundingClientRect();
              const editor = document.querySelector(".cm-editor");
              return (
                editor !== null &&
                rect.height > 0 &&
                rect.bottom <= editor.getBoundingClientRect().top + 0.5 &&
                el.childElementCount === 0
              );
            })
            .map(
              (el) =>
                `${el.tagName}:${(el.textContent ?? "").slice(0, 18)}:${String(Math.round(el.getBoundingClientRect().height))}`,
            )
            .join("|"),
        });
      }
      requestAnimationFrame(tick);
    };
    requestAnimationFrame(tick);
  });
  await openEditor(
    page,
    "Above\n\n```mermaid\nflowchart TD\n  A --> B\n  B --> C\n  C --> D\n```\n\nBelow marker\n\nLast line\n",
  );
  await expect(page.locator(".cm-diagram svg")).toBeVisible();
  await settle(page);
  const all = await page.evaluate(
    () =>
      (
        window as unknown as {
          __cold: {
            top: number;
            box: number;
            svg: boolean;
            preview: boolean;
            editor: number;
            above: string;
          }[];
        }
      ).__cold,
  );
  const changes = all.filter(
    (s, i) =>
      i === 0 ||
      s.top !== all[i - 1]?.top ||
      s.svg !== all[i - 1]?.svg ||
      s.editor !== all[i - 1]?.editor,
  );
  info.annotations.push({ type: "cold", description: JSON.stringify(changes) });
  // At rest: the last second moved nothing.
  const end = all.at(-1)?.top ?? 0;
  for (const sample of all.slice(-30)) {
    expect(Math.abs(sample.top - end)).toBeLessThanOrEqual(STILL_PX);
  }
});

interface ScrollReport {
  frames: number;
  boxes: number;
  deepest: number;
  worst: number;
  shifted: number;
  jumps: string[];
}

/** Opens `content`, walks it from the end up, then down and up, and says what moved. */
async function scrollWalk(page: Page, content: string): Promise<ScrollReport> {
  await openEditor(page, content);
  await expect(line(page, "Filler line 0")).toBeVisible();
  await settle(page);
  /** The element that scrolls the editor, whichever it is in this layout. */
  await page.evaluate(() => {
    let el: Element | null = document.querySelector(".cm-editor");
    while (el !== null) {
      const style = getComputedStyle(el);
      if (
        /auto|scroll/.test(style.overflowY) &&
        el.scrollHeight > el.clientHeight + 10
      ) {
        break;
      }
      el = el.parentElement;
    }
    (window as unknown as { __scroller: Element }).__scroller =
      el ?? document.scrollingElement ?? document.documentElement;
  });

  // Each frame: every line in the window, and where it stands in the document
  // (its place in the window plus the scroll). A wheel step changes both parts
  // and not their sum, also while it animates over several frames. A layout
  // change above the line changes the sum. That is a jump, unless the layer's own
  // `scrollBy` took it back in that frame, so that is counted and subtracted.
  await page.evaluate(() => {
    const log = {
      frames: 0,
      boxes: 0,
      deepest: 0,
      worst: 0,
      shifted: 0,
      jumps: [] as string[],
    };
    (window as unknown as { __jumps: typeof log }).__jumps = log;
    const scroller = (window as unknown as { __scroller: Element }).__scroller;
    let was = new Map<string, number>();
    let lastAt = scroller.scrollTop;
    let taken = 0;
    const scrollBy = Object.getOwnPropertyDescriptor(
      Element.prototype,
      "scrollBy",
    )?.value as (this: Element, options: ScrollToOptions) => void;
    (Element.prototype as unknown as { scrollBy: unknown }).scrollBy =
      function counted(this: Element, options: ScrollToOptions) {
        taken += options.top ?? 0;
        scrollBy.call(this, options);
      };
    const tick = () => {
      const now = new Map<string, number>();
      const at = scroller.scrollTop;
      log.deepest = Math.max(log.deepest, at);
      log.boxes = Math.max(
        log.boxes,
        document.querySelectorAll(".cm-diagram").length,
      );
      for (const el of document.querySelectorAll(".cm-content > .cm-line")) {
        const text = el.textContent ?? "";
        const rect = el.getBoundingClientRect();
        if (
          /^Filler line \d+$/.test(text) &&
          rect.bottom > 0 &&
          rect.top < window.innerHeight
        ) {
          const place = rect.top + at;
          now.set(text, place);
          const before = was.get(text);
          if (before !== undefined) {
            const moved = Math.abs(place - before - taken);
            log.worst = Math.max(log.worst, moved);
            log.shifted = Math.max(log.shifted, Math.abs(taken));
            if (moved > 2 && log.jumps.length < 8) {
              log.jumps.push(
                `${text} moved ${moved.toFixed(1)} scroll ${(at - lastAt).toFixed(1)} taken ${taken.toFixed(1)} at ${at.toFixed(0)}`,
              );
            }
          }
        }
      }
      taken = 0;
      lastAt = at;
      log.frames += 1;
      was = now;
      requestAnimationFrame(afterFrame);
    };
    // After the frame's callbacks, CodeMirror's measure among them: what is read
    // here is what was painted, not a state the same frame then corrected.
    const channel = new MessageChannel();
    channel.port1.onmessage = tick;
    const afterFrame = () => {
      channel.port2.postMessage(null);
    };
    requestAnimationFrame(afterFrame);
  });
  const read = () =>
    page.evaluate(
      () =>
        (
          window as unknown as {
            __jumps: {
              frames: number;
              boxes: number;
              deepest: number;
              worst: number;
              shifted: number;
              jumps: string[];
            };
          }
        ).__jumps,
    );

  const box = await page.locator(".cm-content").boundingBox();
  await page.mouse.move((box?.x ?? 0) + 100, 400);
  const walk = async (delta: number, steps: number) => {
    for (let step = 0; step < steps; step++) {
      await page.mouse.wheel(0, delta);
      await page.waitForTimeout(120);
    }
  };
  // Up first, from a jump to the end: each diagram is new when it comes into
  // reach above the window, where its estimate being off moves the text below.
  await page.mouse.wheel(0, 200_000);
  await settle(page);
  await walk(-600, 70);
  await page.mouse.wheel(0, -200_000);
  await settle(page);
  // Not into the end of the document: there the browser clamps the scroll as
  // the last lines' real heights come in, whatever the diagrams do.
  await walk(600, 34);
  await walk(-600, 40);
  const report = await read();
  return report;
}

test("scrolling a long document with several diagrams does not jump", async ({
  page,
}, info) => {
  test.setTimeout(300_000);
  const filler = (from: number) =>
    Array.from(
      { length: 40 },
      (_, i) => `Filler line ${String(from + i)}`,
    ).join("\n\n");
  const parts: string[] = [];
  const plain: string[] = [];
  for (let d = 0; d < 8; d++) {
    parts.push(filler(d * 100));
    plain.push(filler(d * 100));
    const edges = Array.from(
      { length: 2 + (d % 5) },
      (_, i) => `  N${String(i)} --> N${String(i + 1)}`,
    ).join("\n");
    parts.push(`\`\`\`mermaid\nflowchart TD\n${edges}\n\`\`\``);
  }
  parts.push(filler(900));
  plain.push(filler(900));

  // The same walk with no diagram: CodeMirror's height map corrects its guess for
  // prose lines it meets for the first time, and on this layout the page shows it.
  const control = await scrollWalk(page, `${plain.join("\n\n")}\n`);
  const report = await scrollWalk(page, `${parts.join("\n\n")}\n`);
  info.annotations.push({
    type: "control",
    description: JSON.stringify(control),
  });
  info.annotations.push({
    type: "scroll",
    description: JSON.stringify(report),
  });
  expect(report.frames).toBeGreaterThan(100);
  expect(report.boxes).toBeGreaterThan(0);
  expect(report.deepest).toBeGreaterThan(5000);
  expect(report.worst).toBeLessThanOrEqual(control.worst + SCROLL_JUMP_PX);
  // The walk did reach a diagram landing above the window, and the correction ran.
  expect(report.shifted).toBeGreaterThan(0);
});

test("a new drawing landing after an edit does not collapse the box, and moves the text once", async ({
  page,
}, info) => {
  await openEditor(
    page,
    "Above\n\n```mermaid\nflowchart TD\n  A --> B\n  B --> C\n```\n\nBelow marker\n\nLast line\n",
  );
  const drawing = page.locator(".cm-diagram svg");
  await expect(drawing).toBeVisible();
  await settle(page);
  const oldId = await drawing.getAttribute("id");
  const oldHeight = (await page.locator(".cm-diagram").boundingBox())?.height;
  if (oldHeight === undefined) throw new Error("no box");

  // Sampled after each frame's callbacks: what was painted.
  await page.evaluate(() => {
    const frames: { top: number; box: number; id: string }[] = [];
    (window as unknown as { __frames: typeof frames }).__frames = frames;
    const channel = new MessageChannel();
    const after = () => {
      channel.port2.postMessage(null);
    };
    channel.port1.onmessage = () => {
      const below = [
        ...document.querySelectorAll(".cm-content > .cm-line"),
      ].find((el) => el.textContent === "Below marker");
      const box = document.querySelector(".cm-diagram");
      frames.push({
        top: below?.getBoundingClientRect().top ?? -1,
        box: box?.getBoundingClientRect().height ?? 0,
        id: box?.querySelector("svg")?.id ?? "",
      });
      requestAnimationFrame(after);
    };
    requestAnimationFrame(after);
  });
  // As a peer's edit arrives: no key, no click, and the caret stays outside.
  await page.evaluate(() => {
    const content = document.querySelector(".cm-content") as unknown as {
      cmTile: {
        view: {
          state: { doc: { toString(): string } };
          dispatch(spec: { changes: { from: number; insert: string } }): void;
        };
      };
    };
    const { view } = content.cmTile;
    const at = view.state.doc.toString().indexOf("  B --> C") + 9;
    view.dispatch({ changes: { from: at, insert: "\n  C --> D" } });
  });
  await expect(drawing).not.toHaveAttribute("id", oldId ?? "");
  await settle(page);

  const frames = await page.evaluate(
    () =>
      (
        window as unknown as {
          __frames: { top: number; box: number; id: string }[];
        }
      ).__frames,
  );
  const newHeight = frames.at(-1)?.box ?? 0;
  const tops = frames
    .map((frame) => frame.top)
    .filter((top, i, all) => top !== all[i - 1]);
  info.annotations.push({
    type: "landing",
    description: JSON.stringify({
      frames: frames.length,
      oldHeight,
      newHeight,
      tops,
      smallestBox: Math.min(...frames.map((frame) => frame.box)),
      blankFrames: frames.filter((frame) => frame.id === "").length,
    }),
  });
  expect(newHeight).toBeGreaterThan(oldHeight);
  // Never empty, never below the old height while the render runs.
  expect(frames.filter((frame) => frame.id === "")).toHaveLength(0);
  expect(Math.min(...frames.map((frame) => frame.box))).toBeGreaterThanOrEqual(
    oldHeight - STILL_PX,
  );
  // One step, by the difference.
  expect(tops.length).toBeLessThanOrEqual(2);
  expect(
    Math.abs((tops.at(-1) ?? 0) - (tops[0] ?? 0) - (newHeight - oldHeight)),
  ).toBeLessThanOrEqual(STILL_PX);
});

/** Logs each painted frame: where a line stands, and the box. */
async function sampleFrames(page: Page, text: string): Promise<void> {
  await page.evaluate((wanted) => {
    const frames: { top: number; box: number; svg: boolean }[] = [];
    (window as unknown as { __frames: typeof frames }).__frames = frames;
    const channel = new MessageChannel();
    const after = () => {
      channel.port2.postMessage(null);
    };
    channel.port1.onmessage = () => {
      const below = [
        ...document.querySelectorAll(".cm-content > .cm-line"),
      ].find((el) => el.textContent === wanted);
      const box = document.querySelector(".cm-diagram");
      frames.push({
        top: below?.getBoundingClientRect().top ?? -1,
        box: box?.getBoundingClientRect().height ?? 0,
        svg: box?.querySelector("svg") != null,
      });
      requestAnimationFrame(after);
    };
    requestAnimationFrame(after);
  }, text);
}

const framesOf = (page: Page) =>
  page.evaluate(
    () =>
      (
        window as unknown as {
          __frames: { top: number; box: number; svg: boolean }[];
        }
      ).__frames,
  );

/** Scrolls whatever scrolls the editor so the first fence stands in the window. */
async function jumpToFence(page: Page): Promise<void> {
  await page.evaluate(() => {
    const content = document.querySelector(".cm-content") as unknown as {
      cmTile: {
        view: {
          state: { doc: { toString(): string } };
          lineBlockAt(pos: number): { top: number };
          documentTop: number;
        };
      };
    };
    const { view } = content.cmTile;
    const block = view.lineBlockAt(
      view.state.doc.toString().indexOf("```mermaid"),
    );
    let el: Element | null = document.querySelector(".cm-editor");
    while (el !== null) {
      if (
        /auto|scroll/.test(getComputedStyle(el).overflowY) &&
        el.scrollHeight > el.clientHeight + 10
      ) {
        break;
      }
      el = el.parentElement;
    }
    const scroller = el ?? document.documentElement;
    scroller.scrollTop += view.documentTop + block.top - 150;
  });
}

/** A first draw inside the window: the box's blank height, and what the SVG moved. */
async function firstDrawInWindow(page: Page, body: string) {
  const filler = (from: number) =>
    Array.from(
      { length: 80 },
      (_, i) => `Filler line ${String(from + i)}`,
    ).join("\n\n");
  await openEditor(
    page,
    `${filler(0)}\n\n\`\`\`mermaid\n${body}\n\`\`\`\n\nBelow marker\n\n${filler(100)}\n`,
  );
  await expect(line(page, "Filler line 0")).toBeVisible();
  await settle(page);
  // The verdict has come, and the fence was never on screen: no preview to take.
  await expect(page.locator(".cm-diagram")).toHaveCount(0);
  await expect(page.locator(".cm-mermaid-preview")).toHaveCount(0);
  await sampleFrames(page, "Below marker");
  await jumpToFence(page);
  await expect(page.locator(".cm-diagram svg")).toBeVisible();
  await settle(page, 800);
  const frames = await framesOf(page);
  const blank = frames.filter((frame) => frame.box > 0 && !frame.svg);
  const drawn = frames.filter((frame) => frame.svg);
  return {
    blankFrames: blank.length,
    reserved: blank.at(-1)?.box ?? 0,
    height: drawn.at(-1)?.box ?? 0,
    moved: (drawn.at(-1)?.top ?? 0) - (blank.at(-1)?.top ?? 0),
  };
}

test("a first draw in the window holds its height, and its SVG moves a plain chain's text by little", async ({
  page,
}, info) => {
  const chain = await firstDrawInWindow(
    page,
    "flowchart TD\n  A --> B\n  B --> C\n  C --> D",
  );
  info.annotations.push({ type: "chain", description: JSON.stringify(chain) });
  expect(chain.blankFrames).toBeGreaterThan(0);
  expect(chain.reserved).toBeGreaterThan(48);
  expect(Math.abs(chain.moved)).toBeLessThanOrEqual(RESERVED_SLACK_PX);
});

test("a first draw in the window: how far a diagram that is no chain moves the text", async ({
  page,
}, info) => {
  // The estimate counts statements, and this one is wide: stated, with no promise
  // beyond the box holding a height and the text moving once.
  const wide = await firstDrawInWindow(
    page,
    "flowchart TD\n  A --> B\n  A --> C\n  B --> D\n  C --> D",
  );
  info.annotations.push({ type: "wide", description: JSON.stringify(wide) });
  expect(wide.blankFrames).toBeGreaterThan(0);
  expect(wide.reserved).toBeGreaterThan(48);
});

/** How many places a line stood in, leaving out moves within STILL_PX. */
function positions(tops: number[]): number {
  let count = 0;
  let last = Number.NaN;
  for (const top of tops) {
    if (!(Math.abs(top - last) <= STILL_PX)) {
      count += 1;
      last = top;
    }
  }
  return count;
}

test("the flip back to text ends where the source and the caption put the page", async ({
  page,
}, info) => {
  await openEditor(
    page,
    "Above\n\n```mermaid\nflowchart TD\n  A --> B\n  B --> C\n```\n\nBelow marker\n\nLast line\n",
  );
  await expect(page.locator(".cm-diagram svg")).toBeVisible();
  await settle(page);
  const boxHeight = (await page.locator(".cm-diagram").boundingBox())?.height;
  if (boxHeight === undefined) throw new Error("no box");
  await sampleFrames(page, "Below marker");
  // A peer's edit that mermaid refuses: the last arrow has no end.
  await page.evaluate(() => {
    const content = document.querySelector(".cm-content") as unknown as {
      cmTile: {
        view: {
          state: { doc: { toString(): string } };
          dispatch(spec: { changes: { from: number; insert: string } }): void;
        };
      };
    };
    const { view } = content.cmTile;
    const at = view.state.doc.toString().indexOf("  B --> C") + 9;
    view.dispatch({ changes: { from: at, insert: "\n  C -->" } });
  });
  await expect(page.locator(".cm-diagram")).toHaveCount(0);
  await expect(page.locator(".cm-mermaid-error")).toBeVisible();
  await settle(page, 800);
  const source = await page.evaluate(() =>
    [...document.querySelectorAll(".cm-content > .cm-line")]
      .filter((el) => el.classList.contains("cm-fence-mono"))
      .reduce((sum, el) => sum + el.getBoundingClientRect().height, 0),
  );
  const preview = (await page.locator(".cm-mermaid-preview").boundingBox())
    ?.height;
  if (preview === undefined) throw new Error("no preview");
  const frames = await framesOf(page);
  const tops = frames
    .map((frame) => frame.top)
    .filter((top, i, all) => top !== all[i - 1]);
  const moved = (tops.at(-1) ?? 0) - (tops[0] ?? 0);
  info.annotations.push({
    type: "flip back",
    description: JSON.stringify({ boxHeight, source, preview, tops, moved }),
  });
  expect(Math.abs(moved - (source + preview - boxHeight))).toBeLessThanOrEqual(
    STILL_PX,
  );
  expect(positions(tops)).toBeLessThanOrEqual(2);
});

test("the flip back with its body unchanged moves the page by the source lines, in one step", async ({
  page,
}, info) => {
  await openEditor(
    page,
    "Above\n\n```mermaid\nflowchart TD\n  A --> B\n  B --> C\n```\n\nBelow marker\n\nLast line\n",
  );
  await expect(page.locator(".cm-diagram svg")).toBeVisible();
  await settle(page);
  // A search hit in the fence, left with Escape: R80's route, by real keys.
  await page.locator(".cm-content > .cm-line", { hasText: "Above" }).click();
  await page.keyboard.press("ControlOrMeta+f");
  await page.keyboard.type("B --> C");
  await page.keyboard.press("Enter");
  await settle(page, 300);
  // The search panel stands above the text and closes in the same step.
  const panel =
    (await page.locator(".cm-panels-top").boundingBox())?.height ?? 0;
  await sampleFrames(page, "Below marker");
  await page.keyboard.press("Escape");
  await expect(page.locator(".cm-diagram")).toHaveCount(0);
  await expect(page.locator(".cm-mermaid-preview svg")).toBeVisible();
  await settle(page, 800);
  const source = await page.evaluate(() =>
    [...document.querySelectorAll(".cm-content > .cm-line")]
      .filter((el) => el.classList.contains("cm-fence-mono"))
      .reduce((sum, el) => sum + el.getBoundingClientRect().height, 0),
  );
  const frames = await framesOf(page);
  const tops = frames
    .map((frame) => frame.top)
    .filter((top, i, all) => top !== all[i - 1]);
  const moved = (tops.at(-1) ?? 0) - (tops[0] ?? 0);
  info.annotations.push({
    type: "flip back, same body",
    description: JSON.stringify({ source, panel, tops, moved }),
  });
  expect(Math.abs(moved - (source - panel))).toBeLessThanOrEqual(FLIP_SLACK_PX);
  expect(positions(tops)).toBeLessThanOrEqual(2);
});

test("a label goes through drawnLabels, a fence coming back has its drawing at once, and nothing is left behind", async ({
  page,
}) => {
  const filler = Array.from(
    { length: 200 },
    (_, i) => `Filler line ${String(i)}`,
  ).join("\n\n");
  await openEditor(
    page,
    `Above\n\n\`\`\`mermaid\nflowchart TD\n  A["say #quot;hi#quot;"] --> B\n\`\`\`\n\n\`\`\`mermaid\nflowchart TD\n  A["say #quot;hi#quot;"] --> B\n\`\`\`\n\n${filler}\n`,
  );
  const drawing = page.locator(".cm-diagram svg");
  await expect(drawing).toHaveCount(2);
  await expect(drawing.first()).toContainText('say "hi"');
  const content = await page.locator(".cm-diagram").first().textContent();
  expect(content).not.toContain("&quot;");
  expect(content).not.toContain("#quot;");

  const left = await page.evaluate(() => ({
    temp: document.querySelectorAll(
      '[id^="dcm-diagram-"], [id^="dcm-mermaid-"]',
    ).length,
    ids: [...document.querySelectorAll(".cm-diagram svg")].map((svg) => svg.id),
  }));
  expect(left.temp).toBe(0);
  expect(new Set(left.ids).size).toBe(left.ids.length);

  // Away and back: the frame the box is back already has its drawing.
  await page.evaluate(() => {
    const blank = { frames: 0 };
    (window as unknown as { __blank: { frames: number } }).__blank = blank;
    const tick = () => {
      for (const box of document.querySelectorAll(".cm-diagram")) {
        if (box.querySelector("svg") === null) blank.frames += 1;
      }
      requestAnimationFrame(tick);
    };
    requestAnimationFrame(tick);
  });
  const box = await page.locator(".cm-content").boundingBox();
  await page.mouse.move((box?.x ?? 0) + 100, 400);
  await page.mouse.wheel(0, 100_000);
  await expect(page.locator(".cm-diagram")).toHaveCount(0);
  await page.mouse.wheel(0, -100_000);
  await expect(page.locator(".cm-diagram svg")).toHaveCount(2);
  const frames = await page.evaluate(
    () => (window as unknown as { __blank: { frames: number } }).__blank.frames,
  );
  expect(frames).toBe(0);
});

test("Raw on and off gives the text and then the drawing back", async ({
  page,
}) => {
  await openEditor(
    page,
    "Above\n\n```mermaid\nflowchart TD\n  A --> B\n```\n\nBelow marker\n",
  );
  await expect(page.locator(".cm-diagram svg")).toBeVisible();
  const raw = page.getByRole("button", { name: "Raw", exact: true });
  await raw.click();
  await expect(page.locator(".cm-diagram")).toHaveCount(0);
  await expect(page.locator(".cm-content")).toContainText("flowchart TD");
  await raw.click();
  await expect(page.locator(".cm-diagram svg")).toBeVisible();
  await expect(page.locator(".cm-content")).not.toContainText("A --> B");
});
