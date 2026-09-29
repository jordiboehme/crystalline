/**
 * The CI step for the `/π` route's size check (M4 C30, Task 11).
 *
 * Reads the Vite build manifest at `dist/.vite/manifest.json` (written only
 * when the build ran with `FLUID_BUNDLE_REPORT=1`, see `vite.config.ts`),
 * gzips every file `bundleBudget.ts`'s walk names and prints the report. The
 * console output lists each file by the name Vite gave it, but the summary
 * appended to `$GITHUB_STEP_SUMMARY` never does: `reportLines`' summary names
 * the route `/π` and gives only a byte count and a file count, because the
 * game is never named outside the game (Global Constraints).
 *
 * Gzip is `zlib.gzipSync(buf, { level: 9 })`, equivalent to `gzip -9nc`: it
 * omits the gzip header's filename field that plain `gzip -9c` writes by
 * default. That field costs a fixed `filename.length + 1` bytes (the NUL
 * terminator) per file, so every number this script reports runs about 18 B
 * below what `gzip -9c` on the same file would say (measured on the real
 * build's main chunk, M4 Task 11 fix round 1). The budget and the guideline
 * are numbers of record measured the same way, so this is consistent, not a
 * bug; it only matters if someone cross-checks a byte count by hand with
 * plain `gzip -9c` and finds it 18 B (or a multiple of it, one per file)
 * higher than this script's own number.
 *
 * `runBudgetCheck` (in `bundleBudget.ts`) wraps the manifest read, the
 * measurement and the summary write in a `try`/`finally`: whatever throws in
 * between, a malformed `manifest.json`, `measure` reading a missing asset
 * file, or an unwritable `$GITHUB_STEP_SUMMARY` path, `dist/.vite` is still
 * removed before the error is let through (M4 Task 11 fix round 1). Only the
 * missing-manifest case below, checked before any of that runs, has nothing
 * to remove.
 *
 * Usage (from `fluid/`): `FLUID_BUNDLE_REPORT=1 pnpm build && node scripts/bundle-budget.ts`.
 */

import { appendFileSync, existsSync, readFileSync, rmSync } from "node:fs";
import { join } from "node:path";
import { gzipSync } from "node:zlib";

import { ROUTE_REQUIRED, runBudgetCheck } from "./bundleBudget.ts";

const DIST_DIR = "dist";
const VITE_DIR = join(DIST_DIR, ".vite");
const MANIFEST_PATH = join(VITE_DIR, "manifest.json");

function main(): void {
  if (!existsSync(MANIFEST_PATH)) {
    console.error(
      `no manifest at ${MANIFEST_PATH}. Build with FLUID_BUNDLE_REPORT=1 (for example ` +
        `"FLUID_BUNDLE_REPORT=1 pnpm build") to produce one before running this script.`,
    );
    process.exit(2);
  }

  const result = runBudgetCheck({
    readManifest: () => readFileSync(MANIFEST_PATH, "utf8"),
    gzipSize: (file) =>
      gzipSync(readFileSync(join(DIST_DIR, file)), { level: 9 }).length,
    writeSummary: (lines) => {
      const summaryPath = process.env.GITHUB_STEP_SUMMARY;
      if (summaryPath !== undefined && summaryPath !== "") {
        appendFileSync(summaryPath, lines.map((line) => `${line}\n`).join(""));
      }
    },
    cleanup: () => rmSync(VITE_DIR, { recursive: true, force: true }),
    routeRequired: ROUTE_REQUIRED,
  });

  for (const line of result.annotations) {
    console.log(line);
  }
  console.log("files:");
  for (const entry of result.files) {
    console.log(`  ${entry.file}: ${entry.bytes} B`);
  }
  for (const line of result.summary) {
    console.log(line);
  }

  if (result.failed) {
    process.exit(1);
  }
}

main();
