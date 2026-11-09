/**
 * Where Tab from the text above a diagram lands, in each browser the suite has.
 *
 * This journey records and asserts nothing about the answer: Safari's own Tab
 * setting and CodeMirror's Tab handling both have a say, and the question is for
 * a person to weigh. The line it prints goes in the checkpoint's findings.
 */

import { expect, test } from "@playwright/test";

const USER = process.env.FLUID_E2E_USER ?? "smoke";
const PASSWORD = process.env.FLUID_E2E_PASSWORD ?? "smoke-password";
const DOMAIN = process.env.FLUID_E2E_DOMAIN ?? "fluid-smoke";
const BASE_PATH = process.env.FLUID_E2E_BASE_PATH ?? "";

test("Tab from the text above: where does the focus land", async ({
  page,
}, info) => {
  await page.goto("./");
  await page.getByLabel("Name", { exact: true }).fill(USER);
  await page.getByLabel("Password", { exact: true }).fill(PASSWORD);
  await page.getByRole("button", { name: "Log in" }).click();
  await expect(
    page.getByRole("heading", { name: "Home", level: 1 }),
  ).toBeVisible();
  const me = await page.request.get(`${BASE_PATH}/api/v1/auth/me`);
  const token = ((await me.json()) as { csrf: string }).csrf;
  const at = `${BASE_PATH}/api/v1/domains/${DOMAIN}/engrams`;
  const created = await page.request.post(at, {
    headers: { "X-CSRF-Token": token },
    data: {
      title: `Diagram tab ${info.project.name} ${String(Date.now())}`,
      content:
        "Above\n```mermaid\nflowchart TD\n  A[First] --> B[Second]\n```\nBelow\n",
    },
  });
  expect(created.ok(), await created.text()).toBeTruthy();
  const { permalink } = (await created.json()) as { permalink: string };

  await page.goto(`d/${DOMAIN}/e/${permalink}`);
  await page.getByRole("link", { name: "Edit" }).click();
  await expect(page.locator(".cm-diagram svg")).toBeVisible();
  await page.locator(".cm-content > .cm-line", { hasText: "Above" }).click();
  await page.keyboard.press("Tab");
  const landed = await page.evaluate(() => {
    const active = document.activeElement;
    const content = document.querySelector(".cm-content") as unknown as {
      cmTile: { view: { state: { doc: { toString(): string } } } };
    };
    return {
      tag: active?.tagName ?? "",
      classes: active?.getAttribute("class") ?? "",
      label: active?.getAttribute("aria-label") ?? "",
      onDiagramStop: active?.closest(".cm-diagram") != null,
      inTheText: active?.classList.contains("cm-content") ?? false,
      tabWritten: content.cmTile.view.state.doc.toString().includes("\t"),
    };
  });
  const line = `Tab from the text above, ${info.project.name}: ${JSON.stringify(landed)}\n`;
  process.stdout.write(line);
  info.annotations.push({ type: "tab-landing", description: line });

  await page.goto("./");
  await expect
    .poll(
      async () => {
        const detail = await page.request.get(`${at}/${permalink}`);
        const removed = await page.request.delete(`${at}/${permalink}`, {
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
});
