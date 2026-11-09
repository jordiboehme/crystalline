/**
 * Records what mermaid draws for every diagram fixture, in a real browser.
 *
 * jsdom cannot lay a diagram out (it has no text metrics and no SVG
 * geometry), so the tests that read mermaid's rendered DOM read these
 * recordings instead: one `svg/<fixture name>.svg` per fixture, drawn by the
 * installed mermaid in Chromium with the app's own configuration
 * (`mermaidConfig(false)`), under the svg id `rec-<fixture name>`. Beside them
 * `svg/meta.json` names the mermaid and Chromium versions the drawings came
 * from; `mermaidVersion.test.ts` fails as soon as the installed mermaid is no
 * longer the one recorded, so a bump re-runs this script.
 *
 * Every fixture under `flowchart/`, `sequence/` and `fallback/` is rendered in
 * name order, one at a time. A fixture whose name starts with `x04` or `y02`
 * is meant to be refused by mermaid: it gets no SVG, and the run fails if
 * mermaid draws it after all. Any other fixture that fails to render fails the
 * run, with every failing name listed, so this script is also the check that
 * the corpus is valid mermaid.
 *
 * Usage (from `fluid/`): `node scripts/record-diagram-svgs.ts`. Needs the
 * Playwright Chromium (`pnpm exec playwright install chromium`).
 */

import {
  existsSync,
  mkdirSync,
  readdirSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { join } from "node:path";

import { chromium } from "@playwright/test";

import { mermaidConfig } from "../src/theme/mermaid.ts";

const FIXTURES = join("src", "editor", "fixtures", "diagrams");
const SVG_DIR = join(FIXTURES, "svg");
const GROUPS = ["flowchart", "sequence", "fallback"];
const MERMAID_JS = join("node_modules", "mermaid", "dist", "mermaid.min.js");
const MERMAID_PACKAGE = join("node_modules", "mermaid", "package.json");

/** Fixtures mermaid must refuse: they are recorded as refusals, not drawings. */
function meantToFail(name: string): boolean {
  return name.startsWith("x04") || name.startsWith("y02");
}

interface Fixture {
  name: string;
  text: string;
}

function fixtures(): Fixture[] {
  const out: Fixture[] = [];
  for (const group of GROUPS) {
    const dir = join(FIXTURES, group);
    if (!existsSync(dir)) {
      continue;
    }
    for (const file of readdirSync(dir)) {
      if (file.endsWith(".mmd")) {
        out.push({
          name: file.slice(0, -".mmd".length),
          text: readFileSync(join(dir, file), "utf8"),
        });
      }
    }
  }
  return out.sort((a, b) => (a.name < b.name ? -1 : a.name > b.name ? 1 : 0));
}

/** What the page answers for one render: the SVG, or mermaid's error text. */
type Rendered = { svg: string } | { error: string };

/** The page's view of the mermaid global the script tag installs. */
interface PageMermaid {
  initialize(config: unknown): void;
  render(id: string, text: string): Promise<{ svg: string }>;
}

async function main(): Promise<void> {
  const version = (
    JSON.parse(readFileSync(MERMAID_PACKAGE, "utf8")) as { version: string }
  ).version;
  const browser = await chromium.launch();
  const failures: string[] = [];
  try {
    const page = await browser.newPage();
    await page.goto("about:blank");
    await page.addScriptTag({ path: MERMAID_JS });
    await page.evaluate(
      (config) => {
        const m = (globalThis as unknown as { mermaid: PageMermaid }).mermaid;
        m.initialize(config);
      },
      mermaidConfig(false) as unknown as Record<string, unknown>,
    );

    rmSync(SVG_DIR, { recursive: true, force: true });
    mkdirSync(SVG_DIR, { recursive: true });
    let written = 0;
    for (const fixture of fixtures()) {
      const result: Rendered = await page.evaluate(
        async ({ id, text }) => {
          const m = (globalThis as unknown as { mermaid: PageMermaid }).mermaid;
          try {
            const { svg } = await m.render(id, text);
            return { svg };
          } catch (error) {
            return { error: String(error) };
          }
        },
        { id: `rec-${fixture.name}`, text: fixture.text },
      );
      if ("error" in result) {
        if (!meantToFail(fixture.name)) {
          failures.push(`${fixture.name}: ${result.error.split("\n")[0]}`);
        }
        continue;
      }
      if (meantToFail(fixture.name)) {
        failures.push(`${fixture.name}: mermaid drew a fixture meant to fail`);
        continue;
      }
      writeFileSync(join(SVG_DIR, `${fixture.name}.svg`), `${result.svg}\n`);
      written += 1;
    }
    const meta = { mermaid: version, chromium: browser.version() };
    writeFileSync(
      join(SVG_DIR, "meta.json"),
      `${JSON.stringify(meta, null, 2)}\n`,
    );
    console.log(
      `recorded ${String(written)} SVGs with mermaid ${version} in Chromium ${meta.chromium}`,
    );
  } finally {
    await browser.close();
  }
  if (failures.length > 0) {
    console.error(`${String(failures.length)} fixtures failed to render:`);
    for (const line of failures) {
      console.error(`  ${line}`);
    }
    process.exit(1);
  }
}

await main();
