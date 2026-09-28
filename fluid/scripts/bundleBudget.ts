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
 * (Vite's manifest lists `index.html` in `GameRoute`'s `imports`), and
 * entering it from here would pull in every other lazy Fluid route through
 * the entry's own `dynamicImports`. A build made before the route ships has
 * no `GAME_ENTRY` key at all; `gameFiles` then returns `[]` rather than
 * throwing, and the report says so instead of failing (F6).
 */

/** The manifest key of the game's lazy route entry (Vite keys a chunk by its source path). */
export const GAME_ENTRY = "src/game/GameRoute.tsx";

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
export const MAIN_GUIDELINE_BYTES = 176_584;

/**
 * Whether a build without the `/π` route fails the check. Before the route
 * ships (M4 Task 2) its absence is expected and only a notice; Task 2 sets
 * this true, after which a build that lost the route is an error.
 */
export const ROUTE_REQUIRED = false;

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
  const files: string[] = [];
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
    files.push(chunk.file);
    for (const css of chunk.css ?? []) {
      files.push(css);
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
        `::error::the /π route's lazy chunks are ${report.gameBytes} B gzipped, over the ${GAME_BUDGET_BYTES} B budget`,
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
