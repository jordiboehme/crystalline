/**
 * Which spec files one shard of the browser smoke runs.
 *
 * CI splits the smoke over several jobs, each with its own daemon (see the
 * fluid-smoke jobs in .github/workflows/ci.yml). Playwright's own `--shard`
 * balances by the number of tests and hands out whole files in name order,
 * which leaves one shard with the 41 quick tests of smoke.spec.ts and another
 * with three slow files, and with some splits a shard with no test at all.
 * This script balances by measured time instead: every spec file in e2e/
 * goes to exactly one shard, the slowest first, each to the shard with the
 * least time so far. Both browser projects of a file stay in the same shard.
 *
 * A file is never left out. The list comes from the e2e/ folder, not from the
 * table below, and a file the table does not know gets DEFAULT_SECONDS. The
 * table only steers the balance. The CI build job checks the result as well:
 * the tests every shard lists add up to the whole suite, each test once.
 *
 * Usage (from fluid/): `node scripts/smoke-shards.ts <shard> <total>`, for
 * example `node scripts/smoke-shards.ts 2 5`. Prints one `e2e/<file>` per
 * line, the arguments run-smoke.sh hands on to Playwright:
 *
 *   bash e2e/run-smoke.sh $(node scripts/smoke-shards.ts 2 5)
 *
 * With FLUID_E2E_BASE_PATH set (the run under a path prefix) the split uses
 * the times of that run, where some files skip every test.
 */

import { readdirSync } from "node:fs";
import { fileURLToPath } from "node:url";

/** Seconds per spec file: the sum over both browser projects. */
interface Cost {
  /** The run at the root, against `vite preview`. */
  readonly root: number;
  /** The run under a path prefix, against the daemon's embedded UI. */
  readonly path: number;
}

/**
 * Measured with the list reporter in CI on 2026-11-09: the files on main in
 * run 42598433932, the diagram files (not on main yet) in run 42588072048.
 * Rounded up to whole seconds. Update the numbers when a shard runs much
 * longer than the others; a stale number only costs balance, never coverage.
 */
const COSTS: Readonly<Record<string, Cost>> = {
  "diagram-editing.spec.ts": { root: 119, path: 119 },
  "diagram-hold.spec.ts": { root: 17, path: 17 },
  "diagram-keys.spec.ts": { root: 135, path: 135 },
  "diagram-menu.spec.ts": { root: 112, path: 112 },
  // Skipped under a path prefix: the run at the root covers it.
  "diagram-png.spec.ts": { root: 210, path: 0 },
  "diagram-rename.spec.ts": { root: 195, path: 195 },
  "diagram-tab.spec.ts": { root: 15, path: 15 },
  "diagram-widget.spec.ts": { root: 83, path: 83 },
  "diagrams.spec.ts": { root: 11, path: 11 },
  "hidden-marks.spec.ts": { root: 126, path: 126 },
  "kept.spec.ts": { root: 4, path: 4 },
  "mcp-tokens.spec.ts": { root: 2, path: 2 },
  "members.spec.ts": { root: 7, path: 8 },
  "mermaid-labels.spec.ts": { root: 16, path: 16 },
  "mermaid-zoom.spec.ts": { root: 3, path: 3 },
  "page-title.spec.ts": { root: 3, path: 3 },
  "pictures.spec.ts": { root: 137, path: 130 },
  "print.spec.ts": { root: 16, path: 17 },
  "reflection.spec.ts": { root: 55, path: 60 },
  "smoke.spec.ts": { root: 66, path: 70 },
  "sso-login.spec.ts": { root: 1, path: 1 },
  "table-grid.spec.ts": { root: 75, path: 75 },
};

/** The cost of a spec file the table does not name yet. */
const DEFAULT_SECONDS = 90;

const E2E_DIR = fileURLToPath(new URL("../e2e/", import.meta.url));

function fail(message: string): never {
  console.error(`smoke-shards: ${message}`);
  process.exit(2);
}

function wholeNumber(text: string | undefined, what: string): number {
  if (text === undefined || !/^[1-9][0-9]*$/.test(text)) {
    fail(
      `the ${what} must be a whole number from 1 (usage: smoke-shards.ts <shard> <total>)`,
    );
  }
  return Number(text);
}

function main(): void {
  const [shardArg, totalArg, ...rest] = process.argv.slice(2);
  if (rest.length > 0) {
    fail("too many arguments (usage: smoke-shards.ts <shard> <total>)");
  }
  const shard = wholeNumber(shardArg, "shard");
  const total = wholeNumber(totalArg, "total");
  if (shard > total) {
    fail(`shard ${String(shard)} is past the total of ${String(total)}`);
  }

  const underPath = Boolean(process.env.FLUID_E2E_BASE_PATH);
  const cost = (file: string): number => {
    const known = COSTS[file];
    if (known === undefined) {
      return DEFAULT_SECONDS;
    }
    return underPath ? known.path : known.root;
  };

  const files = readdirSync(E2E_DIR).filter((name) =>
    name.endsWith(".spec.ts"),
  );
  if (files.length < total) {
    fail(
      `${String(files.length)} spec files cannot fill ${String(total)} shards`,
    );
  }

  // The slowest first; equal costs by name, so every job computes the same
  // split.
  files.sort((a, b) => cost(b) - cost(a) || a.localeCompare(b));
  const load = new Array<number>(total).fill(0);
  const members: string[][] = Array.from({ length: total }, () => []);
  for (const file of files) {
    let least = 0;
    for (let i = 1; i < total; i++) {
      if ((load[i] ?? 0) < (load[least] ?? 0)) {
        least = i;
      }
    }
    load[least] = (load[least] ?? 0) + cost(file);
    members[least]?.push(file);
  }

  const mine = members[shard - 1] ?? [];
  if (mine.length === 0) {
    fail(`shard ${String(shard)} of ${String(total)} has no spec file`);
  }
  console.error(
    `smoke-shards: shard ${String(shard)}/${String(total)} runs about ` +
      `${String(load[shard - 1] ?? 0)} s of tests` +
      (underPath ? " (the run under a path prefix)" : ""),
  );
  for (const file of mine.sort()) {
    console.log(`e2e/${file}`);
  }
}

main();
