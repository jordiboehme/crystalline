/**
 * The `/π` route's size check (M4 C30): the walk that separates the route's
 * lazy chunks from the main entry's static graph, and the report it produces
 * against the budget and the guideline.
 *
 * Node-context code, like `src/theme/tokens.test.ts`: it imports the pure
 * `scripts/bundleBudget.ts` module directly rather than through a build, so
 * it typechecks under `tsconfig.node-test.json` rather than the app's own
 * `tsconfig.app.json` (see that file's exclude comment).
 */

import { describe, expect, it } from "vitest";

import {
  GAME_BUDGET_BYTES,
  MAIN_GUIDELINE_BYTES,
  gameFiles,
  mainFile,
  measure,
  reportLines,
  type Manifest,
} from "../scripts/bundleBudget.ts";

const manifest: Manifest = {
  "index.html": {
    file: "assets/index-a.js",
    isEntry: true,
    imports: ["_shared-b.js"],
    css: ["assets/index-a.css"],
    dynamicImports: [
      "src/screens/EngramPage.tsx",
      "src/screens/GraphView.tsx",
      "src/game/GameRoute.tsx",
    ],
  },
  "_shared-b.js": { file: "assets/shared-b.js" },
  "src/screens/EngramPage.tsx": {
    file: "assets/EngramPage-c.js",
    isDynamicEntry: true,
    imports: ["index.html", "_md-d.js"],
  },
  "src/screens/GraphView.tsx": {
    file: "assets/GraphView-h.js",
    isDynamicEntry: true,
    imports: ["index.html", "_heavy-i.js"],
  },
  "_heavy-i.js": { file: "assets/heavy-i.js" },
  "_md-d.js": { file: "assets/md-d.js" },
  "src/game/GameRoute.tsx": {
    file: "assets/GameRoute-e.js",
    isDynamicEntry: true,
    imports: ["index.html", "_shared-b.js", "_md-d.js", "_game-f.js"],
    dynamicImports: ["_game-g.js"],
    css: ["assets/GameRoute-e.css"],
  },
  "_game-f.js": { file: "assets/game-f.js" },
  "_game-g.js": { file: "assets/game-g.js" },
};

describe("the /π route's size (M4 C30)", () => {
  it("counts what the route loads beyond the main entry, and never walks into the entry", () => {
    // Mutation caught: the walk entering `index.html` through the route's
    // `imports` (it would count GraphView and heavy-i), dynamic imports not
    // followed, css left out, a chunk shared with another lazy route left
    // out, the main entry's static chunks counted.
    expect(gameFiles(manifest).sort()).toEqual([
      "assets/GameRoute-e.css",
      "assets/GameRoute-e.js",
      "assets/game-f.js",
      "assets/game-g.js",
      "assets/md-d.js",
    ]);
    expect(mainFile(manifest)).toBe("assets/index-a.js");
  });

  it("reports a build without the route as a notice, and as an error once it is required", () => {
    // Mutation caught: a throw on the missing key, a missing route passing
    // after Task 2 (F6).
    const without: Manifest = {
      "index.html": { file: "assets/index-a.js", isEntry: true },
    };
    expect(gameFiles(without)).toEqual([]);
    const report = measure(without, () => 1);
    expect(report.routePresent).toBe(false);
    const optional = reportLines(report, false);
    expect(optional.failed).toBe(false);
    expect(optional.annotations.some((l) => l.startsWith("::notice::"))).toBe(
      true,
    );
    const required = reportLines(report, true);
    expect(required.failed).toBe(true);
    expect(required.annotations.some((l) => l.startsWith("::error::"))).toBe(
      true,
    );
  });

  it("fails above the budget and only warns above the guideline", () => {
    // Mutation caught: `>=` for `>`, the guideline turned into a failure.
    const at = measure(manifest, (f) =>
      f.includes("index") ? MAIN_GUIDELINE_BYTES + 1 : GAME_BUDGET_BYTES / 5,
    );
    expect(at.overBudget).toBe(false);
    expect(at.overGuideline).toBe(true);
    const lines = reportLines(at);
    expect(lines.failed).toBe(false);
    expect(lines.annotations.some((l) => l.startsWith("::warning::"))).toBe(
      true,
    );
    expect(lines.annotations.some((l) => l.startsWith("::error::"))).toBe(
      false,
    );
    const over = measure(manifest, (f) =>
      f.includes("index") ? 1 : GAME_BUDGET_BYTES / 5 + 1,
    );
    expect(over.overBudget).toBe(true);
    expect(reportLines(over).failed).toBe(true);
    expect(
      reportLines(over).annotations.some((l) => l.startsWith("::error::")),
    ).toBe(true);
  });

  it("never names the route's contents in its messages", () => {
    // Mutation caught: a message that says "game" (the route stays unnamed).
    const all = [
      ...reportLines(measure(manifest, () => GAME_BUDGET_BYTES)).annotations,
      ...reportLines(measure(manifest, () => 1)).summary,
      ...reportLines(
        measure(
          { "index.html": { file: "assets/index-a.js", isEntry: true } },
          () => 1,
        ),
        true,
      ).annotations,
    ];
    expect(all.length).toBeGreaterThan(0);
    expect(all.join("\n")).not.toMatch(/game/i);
  });
});
