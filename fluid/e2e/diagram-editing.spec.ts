/**
 * Editing a flowchart by its boxes and arrows, from the first click to the file.
 *
 * Every journey that writes holds the file to its exact bytes through the REST
 * API: what the page shows is not the question, what was saved is. The fixture
 * has a comment, a chain and odd spacing, so a write that tidied anything shows.
 *
 * Shares the daemon, the fixture domain and the accounts `run-smoke.sh` sets up;
 * run it the same way (`bash fluid/e2e/run-smoke.sh e2e/diagram-editing.spec.ts`).
 * With `FLUID_E2E_SHOTS` set to a folder, the two picture journeys fill it.
 */

import { readFileSync } from "node:fs";

import type { Page } from "@playwright/test";
import { expect, test } from "@playwright/test";

const USER = process.env.FLUID_E2E_USER ?? "smoke";
const PASSWORD = process.env.FLUID_E2E_PASSWORD ?? "smoke-password";
const PEER = process.env.FLUID_E2E_PEER ?? "peer";
const PEER_PASSWORD = process.env.FLUID_E2E_PEER_PASSWORD ?? "peer-password";
const DOMAIN = process.env.FLUID_E2E_DOMAIN ?? "fluid-smoke";
const PAGES = process.env.FLUID_E2E_REFLECTION_DOMAIN ?? "smoke-pages";
const BASE_PATH = process.env.FLUID_E2E_BASE_PATH ?? "";
const SHOTS = process.env.FLUID_E2E_SHOTS;

/** The fixture's body: every journey that writes edits a copy of its own. */
const BODY = readFileSync(
  new URL("./fixtures/domain/diagram-editing.md", import.meta.url),
  "utf8",
)
  .split("---\n")
  .slice(2)
  .join("---\n")
  .trimStart();

const READABLE = 4.5;

let unique = 0;
const made: string[] = [];
const madePages: string[] = [];

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
        title: `Flow journey ${String(Date.now())}${String(unique)}`,
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
  for (const path of madePages.splice(0)) {
    await expect
      .poll(
        async () => {
          const removed = await page.request.delete(
            `${BASE_PATH}/api/v1/domains/${PAGES}/reflections/${path}`,
            { headers: { "X-CSRF-Token": token } },
          );
          return removed.status();
        },
        { timeout: 15_000 },
      )
      .toBeLessThan(300);
  }
});

const field = (page: Page) => page.locator(".cm-diagram-label-field");
const menus = (page: Page) => page.locator('[role="menu"]');
const nodeOf = (page: Page, id: string) =>
  page.locator(`.cm-diagram g.node[id*="flowchart-${id}-"]`);
const edgeOf = (page: Page, from: string, to: string) =>
  page.locator(`.cm-diagram path[data-et="edge"][data-id^="L_${from}_${to}_"]`);
const focusedLabel = (page: Page) =>
  page.evaluate(() => document.activeElement?.getAttribute("aria-label") ?? "");
const focusIsOnEditor = (page: Page) =>
  page.evaluate(
    () => document.activeElement?.classList.contains("cm-content") ?? false,
  );

const docText = (page: Page) =>
  page.evaluate(() => {
    const content = document.querySelector(".cm-content") as unknown as {
      cmTile: { view: { state: { doc: { toString(): string } } } };
    };
    return content.cmTile.view.state.doc.toString();
  });

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

/** Where on the screen a point of an arrow's path stands. */
async function onArrow(
  page: Page,
  from: string,
  to: string,
  part: number,
): Promise<{ x: number; y: number }> {
  return edgeOf(page, from, to).evaluate((el, at) => {
    const path = el as SVGGeometryElement;
    const point = path.getPointAtLength(path.getTotalLength() * at);
    const screen = point.matrixTransform(
      path.getScreenCTM() ?? new DOMMatrix(),
    );
    return { x: screen.x, y: screen.y };
  }, part);
}

test("a click on a box, a name and Enter change only that label", async ({
  page,
}) => {
  const permalink = await scratch(page, BODY);
  await edit(page, permalink);
  const before = await saved(page, permalink);
  // The whole file, frontmatter too, so a save that stamped anything would show.
  expect(before.startsWith("---\n")).toBe(true);
  await nodeOf(page, "B").click();
  await expect(field(page)).toHaveValue("Middle");
  await page.keyboard.type("Renamed");
  await page.keyboard.press("Enter");
  await expect(field(page)).toHaveCount(0);
  await savedIs(page, permalink, swap(before, "B[Middle]", "B[Renamed]"));
});

test("Add child by right click gains one line and keeps the comment and the spacing", async ({
  page,
}) => {
  const permalink = await scratch(page, BODY);
  await edit(page, permalink);
  const before = await saved(page, permalink);
  await nodeOf(page, "E").click({ button: "right" });
  await page.getByRole("menuitem", { name: "Add child" }).click();
  await expect(field(page)).toBeVisible();
  await page.keyboard.type("Kid");
  await page.keyboard.press("Enter");
  await expect(field(page)).toHaveCount(0);
  await savedIs(
    page,
    permalink,
    swap(
      before,
      "  D    --> E[Last]\n",
      "  D    --> E[Last]\n  E --> F[Kid]\n",
    ),
  );
});

test("by keys alone: from the line above to a box, Shape, Decision changes only the delimiters", async ({
  page,
}) => {
  const permalink = await scratch(page, BODY);
  await edit(page, permalink);
  const before = await saved(page, permalink);
  await page.locator(".cm-content").focus();
  // Down the page to the line above the fence, the way a person walks it.
  for (let i = 0; i < 40; i += 1) {
    if ((await caretLine(page)) === "Text above the flowchart.") {
      break;
    }
    await page.keyboard.press("ArrowDown");
  }
  expect(await caretLine(page)).toBe("Text above the flowchart.");
  await page.keyboard.press("ArrowDown");
  expect(await focusIsOnEditor(page)).toBe(false);
  await page.keyboard.press("ArrowRight");
  expect(await focusedLabel(page)).toBe("Box: Start");
  await page.keyboard.press("Shift+F10");
  await expect(page.getByRole("menu", { name: "Box actions" })).toBeVisible();
  for (let i = 0; i < 4; i += 1) {
    await page.keyboard.press("ArrowDown");
  }
  await page.keyboard.press("ArrowRight");
  await expect(page.getByRole("menu", { name: "Shape" })).toBeVisible();
  const decision = page.getByRole("menuitemradio", { name: "Decision" });
  for (let i = 0; i < 20; i += 1) {
    if (await decision.evaluate((el) => el === document.activeElement)) {
      break;
    }
    await page.keyboard.press("ArrowDown");
  }
  await expect(decision).toBeFocused();
  await page.keyboard.press("Enter");
  await savedIs(page, permalink, swap(before, "A[Start]", "A{Start}"));
});

test("a click on an arrow takes a label, and Line, Dotted changes only the arrow's token", async ({
  page,
}) => {
  const permalink = await scratch(page, BODY);
  await edit(page, permalink);
  const before = await saved(page, permalink);
  const middle = await onArrow(page, "D", "E", 0.5);
  await page.mouse.click(middle.x, middle.y);
  await expect(field(page)).toHaveAccessibleName("Arrow label");
  await page.keyboard.type("next");
  await page.keyboard.press("Enter");
  await expect(field(page)).toHaveCount(0);
  const labelled = swap(before, "D    --> E[Last]", "D    -->|next| E[Last]");
  await savedIs(page, permalink, labelled);
  // On the new drawing, and away from its middle, where the label stands.
  await expect(
    page.locator(".cm-diagram g.edgeLabel", { hasText: "next" }),
  ).toBeVisible();
  const near = await onArrow(page, "D", "E", 0.1);
  await page.mouse.click(near.x, near.y, { button: "right" });
  await expect(page.getByRole("menu", { name: "Arrow actions" })).toBeVisible();
  await page.getByRole("menuitem", { name: "Line" }).click();
  await page.getByRole("menuitemradio", { name: "Dotted" }).click();
  await savedIs(page, permalink, swap(labelled, "-->|next|", "-.->|next|"));
});

test("Connect to with the mouse writes one line and keeps the rest", async ({
  page,
}) => {
  const permalink = await scratch(page, BODY);
  await edit(page, permalink);
  const before = await saved(page, permalink);
  await nodeOf(page, "A").click({ button: "right" });
  await page.getByRole("menuitem", { name: "Connect to..." }).click();
  await expect(page.locator(".cm-diagram-connect [role=status]")).toBeVisible();
  await nodeOf(page, "E").click();
  await expect(page.locator(".cm-diagram-connect [role=status]")).toHaveCount(
    0,
  );
  await savedIs(
    page,
    permalink,
    swap(before, "  D    --> E[Last]\n", "  D    --> E[Last]\n  A --> E\n"),
  );
});

test("two browsers: a comment typed under Raw and a rename by the box both end everywhere", async ({
  page,
  browser,
}) => {
  const permalink = await scratch(page, BODY);
  const before = await saved(page, permalink);
  const contextA = await browser.newContext();
  const contextB = await browser.newContext();
  try {
    const pageA = await contextA.newPage();
    const pageB = await contextB.newPage();
    await signIn(pageA);
    await signIn(pageB, PEER, PEER_PASSWORD);
    await edit(pageA, permalink);
    await edit(pageB, permalink);

    // B has its field open and a name typed before A starts, so the two overlap.
    await nodeOf(pageB, "C").click();
    await expect(field(pageB)).toHaveValue("End");
    await pageB.keyboard.type("Finish");

    await pageA.getByRole("button", { name: "Raw", exact: true }).click();
    const comment = pageA.locator(".cm-line", {
      hasText: "%% keep this comment",
    });
    await comment.click();
    await pageA.keyboard.press("End");
    await pageA.keyboard.press("Enter");
    await pageA.keyboard.type("%% from raw");
    // The redraw this brings on must leave B's field and its draft alone.
    await expect.poll(() => docText(pageB)).toContain("%% from raw");
    await expect(field(pageB)).toHaveValue("Finish");
    await pageB.keyboard.press("Enter");
    await expect(field(pageB)).toHaveCount(0);

    const expected = swap(
      swap(before, "C[End]", "C[Finish]"),
      "  %% keep this comment\n",
      "  %% keep this comment\n  %% from raw\n",
    );
    await savedIs(page, permalink, expected);
    await expect.poll(() => docText(pageA)).toContain("C[Finish]");
    expect(await docText(pageA)).toBe(await docText(pageB));
    expect(await docText(pageA)).toContain(
      expected.slice(expected.indexOf("Text above")),
    );
    await expect(nodeOf(pageB, "C")).toContainText("Finish");
  } finally {
    await contextA.close();
    await contextB.close();
  }
});

test("Raw with the rename field open saves the draft and shows the fence's text", async ({
  page,
}) => {
  const permalink = await scratch(page, BODY);
  await edit(page, permalink);
  const before = await saved(page, permalink);
  await nodeOf(page, "D").click();
  await expect(field(page)).toHaveValue("Other");
  await page.keyboard.type("Drafted");
  await page.getByRole("button", { name: "Raw", exact: true }).click();
  await expect(page.locator(".cm-diagram")).toHaveCount(0);
  await expect(
    page.locator(".cm-line", { hasText: "D[Drafted]" }),
  ).toBeVisible();
  await savedIs(page, permalink, swap(before, "D[Other]", "D[Drafted]"));
});

test("the state fence and the quoted flowchart show their text and the drawing", async ({
  page,
}) => {
  await page.goto(`d/${DOMAIN}/e/diagram-editing`);
  await page.getByRole("link", { name: "Edit" }).click();
  await expect(
    page.locator(".cm-line", { hasText: "stateDiagram-v2" }),
  ).toBeVisible();
  await expect(
    page.locator(".cm-line", { hasText: "flowchart LR" }),
  ).toBeVisible();
  await expect(
    page.locator(".cm-line", { hasText: "Waiting --> Running" }),
  ).toBeVisible();
  await expect(
    page.locator(".cm-line", { hasText: "Q[Quoted] --> R[Reply]" }),
  ).toBeVisible();
  const previews = page.locator(".cm-mermaid-preview svg");
  await expect(previews).toHaveCount(2);
  await expect(previews.nth(0)).toContainText("Running");
  await expect(previews.nth(1)).toContainText("Quoted");
  // Drawn as a picture of the text, not as something to edit by its boxes.
  await expect(page.locator(".cm-diagram")).toHaveCount(1);
});

async function newPage(page: Page, markdown: string): Promise<string> {
  unique += 1;
  const path = `diagram-journeys/flow-${String(Date.now())}${String(unique)}.md`;
  const response = await page.request.put(
    `${BASE_PATH}/api/v1/domains/${PAGES}/reflections/${path}`,
    {
      headers: { "X-CSRF-Token": await csrf(page) },
      data: { markdown, create: true },
    },
  );
  expect(response.ok(), await response.text()).toBeTruthy();
  madePages.push(path);
  return path;
}

async function pageSource(page: Page, path: string): Promise<string> {
  const response = await page.request.get(
    `${BASE_PATH}/api/v1/domains/${PAGES}/reflections/${path}`,
  );
  return ((await response.json()) as { markdown: string }).markdown;
}

const REFLECTION =
  "# Flow\n\nThe way through.\n\n```mermaid\nflowchart TD\n  %% keep this comment\n  A[Start]   -->   B[End]\n```\n\nThe rest.\n";

test("in a Reflection the flowchart draws and Add child writes one line", async ({
  page,
}) => {
  const path = await newPage(page, REFLECTION);
  const before = await pageSource(page, path);
  await page.goto(`d/${PAGES}/reflection/${path}`);
  await expect(
    page.locator('figure[aria-label="Diagram"] svg').first(),
  ).toBeVisible();
  await page.getByRole("link", { name: "Edit", exact: true }).click();
  await expect(page.locator(".cm-diagram svg")).toBeVisible();
  await nodeOf(page, "B").click({ button: "right" });
  await page.getByRole("menuitem", { name: "Add child" }).click();
  await expect(field(page)).toBeVisible();
  await page.keyboard.type("Kid");
  await page.keyboard.press("Enter");
  await expect(field(page)).toHaveCount(0);
  // A Reflection stamps who changed it and when; everything else stays as it was.
  const unstamped = (text: string) => text.replace(/changed_at: \S+\n/, "");
  const expected = swap(
    before,
    "  A[Start]   -->   B[End]\n",
    "  A[Start]   -->   B[End]\n  B --> C[Kid]\n",
  );
  await expect
    .poll(async () => unstamped(await pageSource(page, path)), {
      timeout: 20_000,
    })
    .toBe(unstamped(expected));
});

test("in a Reflection a fence that ends the page takes a line below by Enter (R104)", async ({
  page,
}) => {
  const markdown =
    "# Flow\n\nThe way through.\n\n```mermaid\nflowchart TD\n  A[Start] --> B[End]\n```";
  const path = await newPage(page, markdown);
  expect((await pageSource(page, path)).endsWith("```")).toBe(true);
  await page.goto(`d/${PAGES}/reflection/${path}`);
  await page.getByRole("link", { name: "Edit", exact: true }).click();
  await expect(page.locator(".cm-diagram svg")).toBeVisible();
  await nodeOf(page, "A").click();
  await page.keyboard.press("Escape");
  await expect.poll(() => focusedLabel(page)).toBe("Box: Start");
  // The last stop: ArrowDown there writes nothing.
  await page.keyboard.press("End");
  await page.keyboard.press("ArrowDown");
  expect(await docText(page)).toBe(markdown);
  await page.keyboard.press("Home");
  await page.keyboard.press("ArrowLeft");
  await expect.poll(() => focusedLabel(page)).toBe("Flowchart");
  await page.keyboard.press("Enter");
  await expect.poll(() => focusIsOnEditor(page)).toBe(true);
  expect(await caretLine(page)).toBe("");
  await page.keyboard.type("Below it");
  await expect.poll(() => docText(page)).toBe(`${markdown}\nBelow it`);
  await expect
    .poll(
      async () => (await pageSource(page, path)).endsWith("```\nBelow it"),
      {
        timeout: 20_000,
      },
    )
    .toBe(true);
});

test("in a room the line below a last fence cannot be selected away (R104)", async ({
  page,
}) => {
  const content =
    "Top\n\n```mermaid\nflowchart TD\n  A[Start] --> B[End]\n```\n";
  const permalink = await scratch(page, content);
  await edit(page, permalink);
  const before = await docText(page);
  expect(before.endsWith("```\n")).toBe(true);
  await page.locator(".cm-content").focus();
  await page.keyboard.press("ControlOrMeta+End");
  await page.keyboard.press("Shift+ArrowLeft");
  await page.keyboard.press("Delete");
  await expect(
    page.getByText("That would break a diagram here", { exact: false }).first(),
  ).toBeVisible();
  expect(await docText(page)).toBe(before);
});

/** A page long enough that its drawing leaves the window. */
const FAR =
  "Top\n\n```mermaid\nflowchart TD\n  A[First] --> B[Second]\n```\n\n" +
  Array.from({ length: 160 }, (_, i) => `Filler ${String(i)}.`).join("\n\n") +
  "\n";

async function wheelAway(page: Page): Promise<void> {
  await page.mouse.move(400, 400);
  await page.mouse.wheel(0, 12_000);
  await expect(page.locator(".cm-diagram")).toHaveCount(0);
  await expect
    .poll(() => page.evaluate(() => document.activeElement === document.body))
    .toBe(true);
}

test("a key on a stop whose box left the window brings it back and does nothing else (R105)", async ({
  page,
}) => {
  const permalink = await scratch(page, FAR);
  await edit(page, permalink);
  const before = await docText(page);
  await nodeOf(page, "A").click();
  await page.keyboard.press("Escape");
  await expect.poll(() => focusedLabel(page)).toBe("Box: First");
  await wheelAway(page);
  await page.keyboard.press("Delete");
  await expect(page.locator(".cm-diagram")).toHaveCount(1);
  await expect.poll(() => focusedLabel(page)).toBe("Box: First");
  expect(await docText(page)).toBe(before);
});

test("connect mode with its box away: a key brings it back, still waiting (R105)", async ({
  page,
}) => {
  const permalink = await scratch(page, FAR);
  await edit(page, permalink);
  const before = await docText(page);
  await nodeOf(page, "A").click({ button: "right" });
  await page.getByRole("menuitem", { name: "Connect to..." }).click();
  const prompt = page.locator(".cm-diagram-connect [role=status]");
  await expect(prompt).toBeVisible();
  await wheelAway(page);
  await page.keyboard.press("ArrowRight");
  await expect(prompt).toBeVisible();
  await expect.poll(() => focusedLabel(page)).toBe("Box: First");
  await page.keyboard.press("ArrowRight");
  await expect.poll(() => focusedLabel(page)).toBe("Box: Second");
  await page.keyboard.press("Escape");
  await expect(prompt).toHaveCount(0);
  expect(await docText(page)).toBe(before);
});

/** A flowchart with a group, a loop, a twin arrow and shapes: each must map. */
const RICH =
  "```mermaid\nflowchart LR\n  subgraph S [Group]\n    P[One] --> Q((Two))\n  end\n  Q --> Q\n  P --> R{Three}\n  P --> R\n  R -. dotted .-> T & U\n  T --> S\n```\n";

test("every box and every arrow in a drawing is mapped", async ({ page }) => {
  const permalink = await scratch(page, `${BODY}\n${RICH}`);
  await page.goto(`d/${DOMAIN}/e/${permalink}`);
  await page.getByRole("link", { name: "Edit" }).click();
  await expect(page.locator(".cm-diagram svg")).toHaveCount(2);
  await page.locator(".cm-diagram").nth(1).scrollIntoViewIfNeeded();
  const counts = await page.evaluate(() =>
    Array.from(document.querySelectorAll(".cm-diagram")).map((diagram) => {
      const edges = Array.from(
        diagram.querySelectorAll('path[data-et="edge"]'),
      );
      return {
        nodes: diagram.querySelectorAll("g.node").length,
        nodeButtons: diagram.querySelectorAll('g.node[role="button"]').length,
        edges: edges.length,
        edgesWithHit: edges.filter((edge) =>
          edge.parentElement?.querySelector(
            `:scope > [data-hit-for="${edge.getAttribute("data-id") ?? ""}"]`,
          ),
        ).length,
      };
    }),
  );
  expect(counts.length).toBe(2);
  for (const row of counts) {
    expect(row.nodes).toBeGreaterThan(0);
    expect(row.edges).toBeGreaterThan(0);
    expect(row.nodeButtons).toBe(row.nodes);
    expect(row.edgesWithHit).toBe(row.edges);
  }
  expect(counts.map((row) => row.nodes)).toEqual([5, 5]);
  expect(counts.map((row) => row.edges)).toEqual([4, 7]);
});

/** The label's painted colour and its box's, read in the page. */
function labelAndBox(page: Page, id: string) {
  return nodeOf(page, id).evaluate((node) => {
    const run = node.querySelector("text tspan") ?? node.querySelector("text");
    const shape = node.querySelector("rect, polygon, circle, path");
    return {
      text: run === null ? "" : getComputedStyle(run).fill,
      box: shape === null ? "" : getComputedStyle(shape).fill,
    };
  });
}

function contrast(a: string, b: string): number {
  const rgb = (css: string): number[] => {
    const m = /rgba?\(([^)]+)\)/.exec(css);
    return (m?.[1] ?? "").split(/[ ,/]+/).map(Number);
  };
  const lum = (css: string): number => {
    const [r = 0, g = 0, bl = 0] = rgb(css).map((v) => {
      const s = v / 255;
      return s <= 0.03928 ? s / 12.92 : ((s + 0.055) / 1.055) ** 2.4;
    });
    return 0.2126 * r + 0.7152 * g + 0.0722 * bl;
  };
  const [hi = 0, lo = 0] = [lum(a), lum(b)].sort((x, y) => y - x);
  return (hi + 0.05) / (lo + 0.05);
}

for (const theme of ["light", "dark"] as const) {
  test(`Colour, Blue: a white label on a blue box in ${theme}`, async ({
    page,
  }) => {
    await page.addInitScript((value) => {
      window.localStorage.setItem("fluid-theme", value);
    }, theme);
    const permalink = await scratch(page, BODY);
    await edit(page, permalink);
    const before = await saved(page, permalink);
    await nodeOf(page, "B").click({ button: "right" });
    await page.getByRole("menuitem", { name: "Colour" }).click();
    await page.getByRole("menuitemradio", { name: "Blue" }).click();
    await savedIs(
      page,
      permalink,
      swap(
        before,
        "  D    --> E[Last]\n",
        "  D    --> E[Last]\n  style B fill:#1d4ed8,stroke:#1e3a8a,color:#ffffff\n",
      ),
    );
    await expect
      .poll(async () => (await labelAndBox(page, "B")).text)
      .toBe("rgb(255, 255, 255)");
    const painted = await labelAndBox(page, "B");
    expect(painted.box).toBe("rgb(29, 78, 216)");
    expect(contrast(painted.text, painted.box)).toBeGreaterThanOrEqual(
      READABLE,
    );
  });
}

/** One group arrow: Line refuses, with the sentence a person sees on the box. */
const NOTICE_FENCE = `Above\n\n\`\`\`mermaid\nflowchart TD\n  A[One] & B[Two] --> C[Three]\n\`\`\`\n\nBelow\n\n${Array.from(
  { length: 60 },
  (_, i) => `Filler paragraph ${String(i)}.`,
).join("\n\n")}\n`;

for (const theme of ["light", "dark"] as const) {
  test(`the checkpoint's pictures in ${theme}`, async ({ page }) => {
    test.skip(SHOTS === undefined, "only with FLUID_E2E_SHOTS");
    await page.addInitScript((value) => {
      window.localStorage.setItem("fluid-theme", value);
    }, theme);
    const shot = async (name: string) => {
      // The app's short READY. flash after a load is not what a picture shows.
      await expect(page.locator(".ready-flash")).toHaveCount(0);
      await page.screenshot({ path: `${SHOTS ?? ""}/${name}-${theme}.png` });
    };

    const permalink = await scratch(page, BODY);
    await edit(page, permalink);
    await shot("flowchart");

    await nodeOf(page, "B").click();
    await expect(field(page)).toBeVisible();
    await page.keyboard.type("A new name");
    await shot("rename-field");
    await page.keyboard.press("Escape");
    await expect(field(page)).toHaveCount(0);

    await nodeOf(page, "B").click();
    await expect(field(page)).toBeVisible();
    await page.keyboard.type("a %%{ b");
    await page.keyboard.press("Enter");
    await expect(field(page)).toHaveAttribute("aria-invalid", "true");
    await expect(page.locator(".cm-diagram-notice")).toBeVisible();
    await shot("refused-name");
    await page.keyboard.press("Escape");
    await expect(field(page)).toHaveCount(0);

    await nodeOf(page, "B").click({ button: "right" });
    await page.getByRole("menuitem", { name: "Shape" }).click();
    await expect(page.getByRole("menu", { name: "Shape" })).toBeVisible();
    await shot("box-menu-shape");
    await page.keyboard.press("Escape");
    await page.keyboard.press("Escape");
    await expect(menus(page)).toHaveCount(0);

    await nodeOf(page, "A").click({ button: "right" });
    await page.getByRole("menuitem", { name: "Connect to..." }).click();
    await expect(
      page.locator(".cm-diagram-connect [role=status]"),
    ).toBeVisible();
    await shot("connect-mode");
    await page.keyboard.press("Escape");

    const near = await onArrow(page, "D", "E", 0.3);
    await page.mouse.click(near.x, near.y, { button: "right" });
    const arrow = page.getByRole("menu", { name: "Arrow actions" });
    await expect(arrow).toBeVisible();
    await page.getByRole("menuitem", { name: "Line" }).click();
    await expect(page.getByRole("menu", { name: "Line" })).toBeVisible();
    await shot("arrow-menu");
    await page.keyboard.press("Escape");
    await page.keyboard.press("Escape");

    const notice = await scratch(page, NOTICE_FENCE);
    await edit(page, notice);
    const group = await onArrow(page, "A", "C", 0.3);
    await page.mouse.click(group.x, group.y, { button: "right" });
    await page.getByRole("menuitem", { name: "Line" }).click();
    await page.getByRole("menuitemradio", { name: "Dotted" }).click();
    await expect(page.locator(".cm-diagram-notice")).toBeVisible();
    await shot("notice");
    // The same refusal with the drawing out of the window: the panel says it.
    await expect.poll(() => focusedLabel(page)).toContain("Arrow from");
    await page.mouse.wheel(0, 450);
    await expect(page.locator(".cm-diagram")).not.toBeInViewport();
    await page.keyboard.press("Delete");
    await expect(page.locator(".cm-diagram-refusal")).toBeInViewport();
    await shot("refusal-panel");

    const path = await newPage(page, REFLECTION);
    await page.goto(`d/${PAGES}/reflection/${path}`);
    await expect(
      page.locator('figure[aria-label="Diagram"] svg').first(),
    ).toBeVisible();
    await shot("reflection-read");
    await page.getByRole("link", { name: "Edit", exact: true }).click();
    await expect(page.locator(".cm-diagram svg")).toBeVisible();
    await shot("reflection-edit");
  });
}
