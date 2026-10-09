/**
 * Mermaid labels under Safari's page zoom: what only WebKit can answer.
 *
 * Runs in the `webkit` project alone (see playwright.config.ts). The zoom is
 * set before the app renders anything, on every document, so the diagram is
 * laid out under it exactly as a Safari reader with page zoom sees it. Every
 * node's label text has to fit inside the node's own shape.
 */

import { expect, test } from "@playwright/test";

/** The account and the domain `run-smoke.sh` set up. Kept in step with it. */
const USER = process.env.FLUID_E2E_USER ?? "smoke";
const PASSWORD = process.env.FLUID_E2E_PASSWORD ?? "smoke-password";
const DOMAIN = process.env.FLUID_E2E_DOMAIN ?? "fluid-smoke";

/** A pixel of slack for sub-pixel rounding between the two boxes. */
const SLACK = 1;

test("flowchart labels fit their nodes under a 125 percent page zoom", async ({
  page,
}) => {
  await page.addInitScript(() => {
    document.documentElement.style.zoom = "1.25";
  });

  await page.goto("./");
  await page.getByLabel("Name", { exact: true }).fill(USER);
  await page.getByLabel("Password", { exact: true }).fill(PASSWORD);
  await page.getByRole("button", { name: "Log in" }).click();
  await expect(
    page.getByRole("heading", { name: "Home", level: 1 }),
  ).toBeVisible();

  await page.goto(`d/${DOMAIN}/e/berth-assignment-flow`);
  const diagram = page.locator('article svg[id^="mermaid-"]');
  await expect(diagram).toBeVisible();

  const nodes = await diagram.evaluate((svg) =>
    Array.from(svg.querySelectorAll("g.node")).map((node) => {
      const shape = node.querySelector("rect, polygon, path, circle, ellipse");
      // SVG text when labels are SVG; the text's own element inside the
      // foreignObject when they are HTML, which is what overflows.
      const text =
        node.querySelector(".label text") ??
        node.querySelector(".label foreignObject span, .label foreignObject p");
      const box = (el: Element | null) => {
        const r = el?.getBoundingClientRect();
        return r
          ? { left: r.left, right: r.right, top: r.top, bottom: r.bottom }
          : null;
      };
      // Each SVG label line is a direct `tspan` row of the label's `text`
      // (mermaid's `createText` with HTML labels off); an HTML label has none.
      const lines = node.querySelectorAll(".label text > tspan").length;
      return {
        id: node.id,
        label: node.textContent?.trim() ?? "",
        lines,
        shape: box(shape),
        text: box(text),
      };
    }),
  );

  expect(nodes.length).toBe(4);
  for (const node of nodes) {
    expect(node.shape, node.id).not.toBeNull();
    expect(node.text, node.id).not.toBeNull();
    if (node.shape === null || node.text === null) {
      continue;
    }
    expect(node.text.left, node.label).toBeGreaterThanOrEqual(
      node.shape.left - SLACK,
    );
    expect(node.text.right, node.label).toBeLessThanOrEqual(
      node.shape.right + SLACK,
    );
    expect(node.text.top, node.label).toBeGreaterThanOrEqual(
      node.shape.top - SLACK,
    );
    expect(node.text.bottom, node.label).toBeLessThanOrEqual(
      node.shape.bottom + SLACK,
    );
    // Not broken across lines: the identifier is one label line.
    expect(node.lines, node.label).toBe(1);
  }
});
