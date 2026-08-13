// NO vi.mock("mermaid") here: this file is the one place the real parser
// runs, because "every starter parses" is the module's entire warranty.
// This is also the tree's FIRST real-mermaid import under vitest (both
// existing suites mock it); the import alone measures ~150 ms, so a slow
// first test here is load cost, not a hang - do not mock it away.
import mermaid from "mermaid";
import { beforeAll, describe, expect, test } from "vitest";

import { mermaidConfig } from "../theme/mermaid";
import { MERMAID_STARTER_GROUPS, mermaidFence } from "./mermaidStarters";
import { MERMAID_SKELETON } from "./toolbar";

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

  test("the flowchart fence is the toolbar's skeleton, byte for byte", () => {
    // Decision 16, pinned rather than merely commented: keyboard-opening the
    // diagram menu highlights Flowchart first, so Enter Enter through the
    // picker has to insert exactly what today's mermaid button inserts. If
    // either side's body moves, this fails instead of the parity silently
    // dying. It also pins the fence spelling itself - "```mermaid" open, a
    // closing fence - which nothing else in this file asserts.
    expect(mermaidFence(ALL[0]!).lines).toEqual([...MERMAID_SKELETON]);
    // The same spelling for all sixteen, body untouched in between.
    for (const starter of ALL) {
      expect(mermaidFence(starter).lines).toEqual([
        "```mermaid",
        ...starter.lines,
        "```",
      ]);
    }
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
    "%s mentions its token exactly once",
    (_label, starter) => {
      // The rule that keeps type-over honest: the caret lands on the token
      // selected, and one keystroke replaces every mention there is. A body
      // that names its token twice leaves the second one behind as a phantom
      // state or a dangling edge, which is a diagram nobody asked for.
      const body = starter.lines.join("\n");
      expect(body.split(starter.token)).toHaveLength(2);
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
