/**
 * What a sequence op leaves of a message's room, in a browser. mermaid gives a gap
 * its width through each actor's `prevActor` and `nextActor`, and a second `addActor`
 * with a label resets them: the db's messages and labels stay right, the arrow gets
 * short, and the text runs past both its ends and out of the SVG. So a rename writes
 * its declarations on top (R69), and this draws the result beside the body before it.
 *
 * It needs no daemon: mermaid's own bundle on a blank page, with the app's config.
 */

import { readFileSync } from "node:fs";
import { join } from "node:path";

import { expect, test } from "@playwright/test";

import { mermaidConfig } from "../src/theme/mermaid.ts";

const MERMAID = join("node_modules", "mermaid", "dist", "mermaid.min.js");

/** What the op wrote for each body; `opsProperty.test.ts` pins this file against the op. */
const ROWS = JSON.parse(
  readFileSync(
    new URL("./fixtures/sequence-room.json", import.meta.url),
    "utf8",
  ),
) as { name: string; body: string; after: string }[];

interface Drawn {
  width: number;
  messages: {
    text: string;
    left: number;
    right: number;
    from: number;
    to: number;
  }[];
}

test("a rename keeps each message's room", async ({ page }) => {
  await page.goto("about:blank");
  await page.addScriptTag({ path: MERMAID });
  await page.evaluate(
    (config) => {
      (
        globalThis as unknown as {
          mermaid: { initialize(config: unknown): void };
        }
      ).mermaid.initialize(config);
    },
    mermaidConfig(false) as unknown as Record<string, unknown>,
  );

  const draw = (id: string, text: string): Promise<Drawn> =>
    page.evaluate(
      async ({ id, text }) => {
        const { svg } = await (
          globalThis as unknown as {
            mermaid: {
              render(id: string, text: string): Promise<{ svg: string }>;
            };
          }
        ).mermaid.render(id, text);
        const holder = document.createElement("div");
        holder.innerHTML = svg;
        document.body.append(holder);
        const root = holder.querySelector("svg");
        const messages: Drawn["messages"] = [];
        for (const label of holder.querySelectorAll("text.messageText")) {
          let line = label.nextElementSibling;
          while (line && line.getAttribute("data-et") !== "message") {
            line = line.nextElementSibling;
          }
          const box = label.getBoundingClientRect();
          const arrow = line?.getBoundingClientRect();
          messages.push({
            text: label.textContent,
            left: box.left,
            right: box.right,
            from: arrow?.left ?? 0,
            to: arrow?.right ?? 0,
          });
        }
        const width = root?.getBoundingClientRect().width ?? 0;
        holder.remove();
        return { width, messages };
      },
      { id, text },
    );

  let n = 0;
  for (const { name, body, after } of ROWS) {
    n++;
    const base = await draw(`base${String(n)}`, body);
    const drawn = await draw(`after${String(n)}`, after);
    expect(drawn.width, name).toBeCloseTo(base.width, 0);
    expect(drawn.messages.length, name).toBe(base.messages.length);
    for (const [i, message] of drawn.messages.entries()) {
      const was = base.messages[i];
      expect(message.text, name).toBe(was?.text);
      expect(message.from, name).toBeCloseTo(was?.from ?? 0, 0);
      expect(message.to, name).toBeCloseTo(was?.to ?? 0, 0);
      // The label stands inside its arrow, as in the body before.
      expect(message.left, name).toBeGreaterThanOrEqual(message.from - 1);
      expect(message.right, name).toBeLessThanOrEqual(message.to + 1);
    }
  }
});
