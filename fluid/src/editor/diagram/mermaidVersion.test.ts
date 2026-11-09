// The pin on the mermaid the recorded SVGs and the id rules came from.
//
// `diagram/mermaidDb.ts` reads mermaid's own flowchart db through
// `mermaidAPI.getDiagramFromText`, which mermaid marks `@internal`, and the
// hit mapping reads the element ids and `data-id` attributes mermaid writes.
// Neither is a promise mermaid makes, so any version change has to re-run the
// checklists in Tasks 1 and 14 of the Part 4 plan and record the SVGs again
// (`node scripts/record-diagram-svgs.ts`). This test is what makes a bump
// stop and say so.

import { describe, expect, test } from "vitest";

import metaText from "../fixtures/diagrams/svg/meta.json?raw";

const meta = JSON.parse(metaText) as { mermaid: string; chromium: string };

interface Fs {
  readFileSync: (path: string, encoding: "utf8") => string;
}

/** The installed mermaid's version, read off disk (no Node types here). */
function installedMermaid(): string {
  const node = globalThis as unknown as {
    process: { getBuiltinModule(id: string): unknown; cwd(): string };
  };
  const { readFileSync } = node.process.getBuiltinModule("node:fs") as Fs;
  const pkg = readFileSync(
    `${node.process.cwd()}/node_modules/mermaid/package.json`,
    "utf8",
  );
  return (JSON.parse(pkg) as { version: string }).version;
}

describe("the recorded mermaid", () => {
  test("is the installed mermaid", () => {
    expect(
      meta.mermaid,
      "mermaid changed: re-run the checklists in Tasks 1 and 14 of the Part 4 plan and re-record the SVGs",
    ).toBe(installedMermaid());
  });

  test("names the browser it was drawn in", () => {
    expect(meta.chromium).toMatch(/^\d+\.\d+/);
  });
});
