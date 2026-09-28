/**
 * The CI step for the `/π` route's size check (M4 C30, Task 11).
 *
 * Reads the Vite build manifest at `dist/.vite/manifest.json` (written only
 * when the build ran with `FLUID_BUNDLE_REPORT=1`, see `vite.config.ts`),
 * gzips every file `bundleBudget.ts`'s walk names at level 9 and prints the
 * report. The console output lists each file by the name Vite gave it, but
 * the summary appended to `$GITHUB_STEP_SUMMARY` never does: `reportLines`'
 * summary names the route `/π` and gives only a byte count and a file count,
 * because the game is never named outside the game (Global Constraints).
 *
 * `dist/.vite` is removed again once the manifest has been read, in every
 * exit path but the missing-manifest one, where there is nothing to remove:
 * the service crate embeds the whole `dist/` directory and must not end up
 * serving the manifest to a visitor.
 *
 * Usage (from `fluid/`): `FLUID_BUNDLE_REPORT=1 pnpm build && node scripts/bundle-budget.ts`.
 */

import { appendFileSync, existsSync, readFileSync, rmSync } from "node:fs";
import { join } from "node:path";
import { gzipSync } from "node:zlib";

import {
  measure,
  reportLines,
  ROUTE_REQUIRED,
  type Manifest,
} from "./bundleBudget.ts";

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

  const manifest = JSON.parse(readFileSync(MANIFEST_PATH, "utf8")) as Manifest;

  const gzipSize = (file: string): number => {
    const buf = readFileSync(join(DIST_DIR, file));
    return gzipSync(buf, { level: 9 }).length;
  };

  const report = measure(manifest, gzipSize);
  const { annotations, summary, failed } = reportLines(report, ROUTE_REQUIRED);

  for (const line of annotations) {
    console.log(line);
  }
  console.log("files:");
  for (const entry of report.files) {
    console.log(`  ${entry.file}: ${entry.bytes} B`);
  }
  for (const line of summary) {
    console.log(line);
  }

  const summaryPath = process.env.GITHUB_STEP_SUMMARY;
  if (summaryPath !== undefined && summaryPath !== "") {
    appendFileSync(summaryPath, summary.map((line) => `${line}\n`).join(""));
  }

  rmSync(VITE_DIR, { recursive: true, force: true });

  if (failed) {
    process.exit(1);
  }
}

main();
