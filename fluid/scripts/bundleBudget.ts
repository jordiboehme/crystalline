/**
 * The size check for the `/π` route's lazy chunks (M4 C30).
 *
 * Pure Node module, no DOM: it only reads a Vite manifest object and asks
 * for a gzip size, so it typechecks under `tsconfig.node.json` and is
 * exercised by `src/bundleBudget.test.ts` without a build. `bundle-budget.ts`
 * is the CLI that reads the real manifest, gzips the real files and drives
 * this module from a CI step; keeping the walk and the reporting here, free
 * of `fs`/`zlib`, is what lets the test fixture stand in for a build.
 *
 * The walk (F5): first the main set, `index.html` and every key reachable
 * from it through `imports` alone (a production build's static graph, what
 * every visitor downloads before any route runs). Then a walk from
 * `GAME_ENTRY` over both `imports` and `dynamicImports`, but a key already in
 * the main set is never entered: a lazy chunk imports its own entry chunk
 * (Vite's manifest lists `index.html` in `ExploreRoute`'s `imports`), and
 * entering it from here would pull in every other lazy Fluid route through
 * the entry's own `dynamicImports`. A build without the route has no
 * `GAME_ENTRY` key at all; `gameFiles` then returns `[]` rather than
 * throwing, and the report fails with an error while `ROUTE_REQUIRED` holds,
 * which it does: the route ships in every build (F6).
 */

/** The manifest key of the game's lazy route entry (Vite keys a chunk by its source path). */
export const GAME_ENTRY = "src/game/ExploreRoute.tsx";

/**
 * The output name of the route's own chunk. Every file the app ships says
 * "explore" rather than naming what the route is, and Vite would otherwise
 * name the chunk after its module, which the main chunk's lazy import and
 * preload map then carry in clear.
 */
export const EXPLORE_CHUNK = "assets/explore-[hash].js";

/** Vite's own default for every other chunk. */
const DEFAULT_CHUNK = "assets/[name]-[hash].js";

/**
 * The `chunkFileNames` rule `vite.config.ts` hands the bundler: the chunk
 * whose facade module is `GAME_ENTRY` is `EXPLORE_CHUNK`, every other one
 * keeps Vite's default. The manifest still keys the chunk by its source
 * path, so the walk below is unaffected.
 */
export function chunkFileName(chunk: {
  facadeModuleId: string | null;
}): string {
  return chunk.facadeModuleId?.endsWith(`/${GAME_ENTRY}`) === true
    ? EXPLORE_CHUNK
    : DEFAULT_CHUNK;
}

/** The manifest key of the app's main entry. */
export const MAIN_ENTRY = "index.html";

/**
 * The `/π` route's budget: everything it loads beyond the main entry, gzipped,
 * must fit on one 1.44 MB floppy disk (1,474,560 bytes). A build over this
 * fails the CI step.
 */
export const GAME_BUDGET_BYTES = 1_474_560;

/**
 * The main chunk's guideline, gzipped. This is a guideline, not a budget: a
 * build over it warns and still passes, reporting the overrun and its cause.
 */
export const MAIN_GUIDELINE_BYTES = 178_000;

/**
 * Whether a build without the `/π` route fails the check. The route ships in
 * every production build, so a build without it lost it by mistake (a gate
 * put back around the route, a broken lazy import): its absence is an error,
 * never the notice `reportLines` gives when this is false.
 */
export const ROUTE_REQUIRED = true;

/** One entry of a Vite build manifest, as `vite build --manifest` writes it. */
export interface ManifestChunk {
  file: string;
  imports?: string[];
  dynamicImports?: string[];
  css?: string[];
  isEntry?: boolean;
  isDynamicEntry?: boolean;
}

/** A Vite build manifest: source path (or a Vite-generated key) to its chunk. */
export type Manifest = Record<string, ManifestChunk>;

/**
 * The manifest keys the main entry loads up front: itself and the closure of
 * its static imports. Dynamic imports are excluded on purpose, since those
 * are the lazy routes (the game among them) that a visitor downloads only on
 * demand.
 */
export function mainKeys(manifest: Manifest): Set<string> {
  const keys = new Set<string>();
  const stack: string[] = [MAIN_ENTRY];
  while (stack.length > 0) {
    const key = stack.pop();
    if (key === undefined || keys.has(key)) {
      continue;
    }
    const chunk = manifest[key];
    if (chunk === undefined) {
      continue;
    }
    keys.add(key);
    for (const imported of chunk.imports ?? []) {
      stack.push(imported);
    }
  }
  return keys;
}

/**
 * The files the `/π` route loads that the main entry does not (M4 C30):
 * the JS `file` and every `css[]` entry of every manifest key reachable from
 * `GAME_ENTRY` over `imports` and `dynamicImports`, stopping at any key
 * already in the main set (see the module doc's walk). `[]` when the route
 * is not in the build (F6).
 */
export function gameFiles(manifest: Manifest): string[] {
  if (manifest[GAME_ENTRY] === undefined) {
    return [];
  }
  const main = mainKeys(manifest);
  const visited = new Set<string>();
  // Deduped by output file, not by manifest key (M4 Task 11 fix round 1): two
  // different keys inside the game's own walk can list the same physical
  // file (most plausibly a `css[]` entry a coarser code split shares between
  // chunks), and counting it twice would overstate the route's bytes.
  const seenFiles = new Set<string>();
  const files: string[] = [];
  const addFile = (file: string): void => {
    if (seenFiles.has(file)) {
      return;
    }
    seenFiles.add(file);
    files.push(file);
  };
  const stack: string[] = [GAME_ENTRY];
  while (stack.length > 0) {
    const key = stack.pop();
    if (key === undefined || visited.has(key) || main.has(key)) {
      continue;
    }
    const chunk = manifest[key];
    if (chunk === undefined) {
      continue;
    }
    visited.add(key);
    addFile(chunk.file);
    for (const css of chunk.css ?? []) {
      addFile(css);
    }
    for (const imported of chunk.imports ?? []) {
      stack.push(imported);
    }
    for (const imported of chunk.dynamicImports ?? []) {
      stack.push(imported);
    }
  }
  return files;
}

/** The main entry's file. */
export function mainFile(manifest: Manifest): string {
  const chunk = manifest[MAIN_ENTRY];
  if (chunk === undefined) {
    throw new Error(`the manifest has no "${MAIN_ENTRY}" entry`);
  }
  return chunk.file;
}

/**
 * Groups a non-negative integer's digits by thousands with commas
 * (`1474560` -> `"1,474,560"`), the way the Global Constraints quote the
 * budget literally in the over-budget message (M4 Task 11 fix round 1). A
 * plain regex rather than `toLocaleString` so the output never depends on
 * the running process's locale.
 */
function grouped(n: number): string {
  return Math.trunc(n)
    .toString()
    .replace(/\B(?=(\d{3})+(?!\d))/g, ",");
}

/** The measured sizes of a build, and whether they cross the budget or the guideline. */
export interface BudgetReport {
  routePresent: boolean;
  gameBytes: number;
  mainBytes: number;
  files: { file: string; bytes: number }[];
  overBudget: boolean;
  overGuideline: boolean;
}

/**
 * Measures a manifest with the given gzip function (real gzip of the built
 * file in the CLI, a stub in tests). Sums `gameFiles`' bytes against
 * `GAME_BUDGET_BYTES` and the main entry's bytes against
 * `MAIN_GUIDELINE_BYTES`.
 */
export function measure(
  manifest: Manifest,
  gzipSize: (file: string) => number,
): BudgetReport {
  const routePresent = manifest[GAME_ENTRY] !== undefined;
  const files = gameFiles(manifest).map((file) => ({
    file,
    bytes: gzipSize(file),
  }));
  const gameBytes = files.reduce((sum, entry) => sum + entry.bytes, 0);
  const mainBytes = gzipSize(mainFile(manifest));
  return {
    routePresent,
    gameBytes,
    mainBytes,
    files,
    overBudget: gameBytes > GAME_BUDGET_BYTES,
    overGuideline: mainBytes > MAIN_GUIDELINE_BYTES,
  };
}

/**
 * The GitHub annotation and summary lines for a report. `annotations` are
 * the `::notice::`/`::warning::`/`::error::` lines a CI log turns into
 * inline annotations; `summary` is a handful of plain lines fit for
 * `$GITHUB_STEP_SUMMARY`. Neither names a file or says what the route is:
 * both speak only of "the /π route" and its byte counts. `routeRequired`
 * defaults to `ROUTE_REQUIRED`; the CLI reads the same constant, so passing
 * it explicitly (as the tests do) only matters when the route is absent.
 */
export function reportLines(
  report: BudgetReport,
  routeRequired: boolean = ROUTE_REQUIRED,
): { annotations: string[]; summary: string[]; failed: boolean } {
  const annotations: string[] = [];
  const summary: string[] = [];
  let failed = false;

  if (!report.routePresent) {
    if (routeRequired) {
      annotations.push("::error::the /π route is missing from the build");
      summary.push("the /π route is missing from the build.");
      failed = true;
    } else {
      annotations.push("::notice::the /π route is not in this build");
      summary.push("the /π route is not in this build.");
    }
  } else {
    summary.push(
      `the /π route: ${report.gameBytes} B gzipped across ${report.files.length} files.`,
    );
    if (report.overBudget) {
      annotations.push(
        `::error::the /π route's lazy chunks are ${report.gameBytes} B gzipped, over the ${grouped(GAME_BUDGET_BYTES)} B budget`,
      );
      failed = true;
    }
  }

  summary.push(`the main chunk: ${report.mainBytes} B gzipped.`);
  if (report.overGuideline) {
    const over = report.mainBytes - MAIN_GUIDELINE_BYTES;
    annotations.push(
      `::warning::the main chunk is ${report.mainBytes} B gzipped, ${over} B over the ${MAIN_GUIDELINE_BYTES} B guideline`,
    );
  }

  return { annotations, summary, failed };
}

/** What `runBudgetCheck` reports back to its caller: the same shape `reportLines` returns, plus the per-file byte list the CLI's console output lists. */
export interface BudgetCheckResult {
  annotations: string[];
  summary: string[];
  files: { file: string; bytes: number }[];
  failed: boolean;
}

/**
 * Runs the full report against injected effects: parses the manifest text
 * `readManifest` returns, measures it with `gzipSize`, turns the result into
 * report lines with `reportLines`, and hands the summary to `writeSummary`.
 *
 * `cleanup` always runs, in a `finally`, whichever of `readManifest`,
 * `JSON.parse`, `measure` or `writeSummary` throws or not (M4 Task 11 fix
 * round 1): the caller (`bundle-budget.ts`) creates `dist/.vite` for this one
 * report alone, and it must not survive a bad manifest, a manifest entry
 * whose file is missing from `dist/assets`, or an unwritable
 * `$GITHUB_STEP_SUMMARY` path any less than it survives a clean run. A throw
 * still propagates once `cleanup` has run; this function cannot recover from
 * a caller's bad input, only make sure the directory it made along the way
 * is gone before the error reaches the caller.
 *
 * Kept here, free of `fs`/`zlib` like the rest of this module, so the
 * cleanup guarantee is exercised by injecting plain functions in
 * `src/bundleBudget.test.ts` rather than by touching a real filesystem.
 */
export function runBudgetCheck(deps: {
  readManifest: () => string;
  gzipSize: (file: string) => number;
  writeSummary: (lines: string[]) => void;
  cleanup: () => void;
  routeRequired?: boolean;
}): BudgetCheckResult {
  try {
    const manifest = JSON.parse(deps.readManifest()) as Manifest;
    const report = measure(manifest, deps.gzipSize);
    const lines = reportLines(report, deps.routeRequired ?? ROUTE_REQUIRED);
    deps.writeSummary(lines.summary);
    return {
      annotations: lines.annotations,
      summary: lines.summary,
      files: report.files,
      failed: lines.failed,
    };
  } finally {
    deps.cleanup();
  }
}
