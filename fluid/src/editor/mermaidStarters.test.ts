// NO vi.mock("mermaid") here: this file is the one place the real parser
// runs, because "every starter parses" is the module's entire warranty.
// This is also the tree's FIRST real-mermaid import under vitest (both
// existing suites mock it); the import alone measures ~150 ms, so a slow
// first test here is load cost, not a hang - do not mock it away.
import mermaid from "mermaid";
import { beforeAll, describe, expect, test } from "vitest";

import { mermaidConfig } from "../theme/mermaid";
import { MERMAID_STARTER_GROUPS, mermaidFence } from "./mermaidStarters";

const ALL = MERMAID_STARTER_GROUPS.flatMap((group) => group.starters);

beforeAll(() => {
  mermaid.initialize(mermaidConfig(false));
});

describe("the starter library", () => {
  test("exactly sixteen starters in Jordi's three groups", () => {
    expect(
      MERMAID_STARTER_GROUPS.map((g) => [g.label, g.starters.length]),
    ).toEqual([
      ["Everyday", 7],
      ["Planning and product", 4],
      ["Technical", 5],
    ]);
    expect(ALL.map((s) => s.label)).toEqual([
      "Flowchart",
      "Sequence",
      "State",
      "Class",
      "Entity relationship",
      "Gantt",
      "Pie",
      "Timeline",
      "User journey",
      "Quadrant chart",
      "Mindmap",
      "C4 context",
      "Requirement",
      "Architecture",
      "XY chart",
      "Radar",
    ]);
  });

  test.each(ALL.map((s) => [s.label, s] as const))(
    "%s parses under the pinned mermaid",
    async (_label, starter) => {
      await expect(
        mermaid.parse(starter.lines.join("\n")),
      ).resolves.toBeTruthy();
    },
  );

  test.each(ALL.map((s) => [s.label, s] as const))(
    "%s selects its first editable token",
    (_label, starter) => {
      const { lines, select } = mermaidFence(starter);
      expect(select).not.toBeNull();
      expect(lines[select?.line ?? 0]?.slice(select?.from, select?.to)).toBe(
        starter.token,
      );
      expect(select?.line).toBeGreaterThan(0); // never the fence line itself
    },
  );
});
