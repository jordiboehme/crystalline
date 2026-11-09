/**
 * The theme key holds only if every `initialize` in the app counts (R79 N2):
 * mermaid reads its configuration when a queued render's turn comes, so any
 * caller can put a render under the other theme.
 */

import { describe, expect, it, vi } from "vitest";

import {
  initializeMermaid,
  mermaidConfig,
  mermaidThemeChanges,
} from "./mermaid";

const SOURCES = import.meta.glob<string>(
  ["../**/*.ts", "../**/*.tsx", "!../**/*.test.ts", "!../**/*.test.tsx"],
  { query: "?raw", import: "default", eager: true },
);

describe("initializeMermaid", () => {
  it("hands mermaid the app's configuration", () => {
    const mermaid = { initialize: vi.fn() };
    initializeMermaid(mermaid, true);
    expect(mermaid.initialize).toHaveBeenCalledWith(mermaidConfig(true));
  });

  it("counts a change of theme, and not the same theme again", () => {
    const mermaid = { initialize: vi.fn() };
    initializeMermaid(mermaid, false);
    const start = mermaidThemeChanges();
    expect(initializeMermaid(mermaid, false)).toBe(start);
    expect(initializeMermaid(mermaid, true)).toBe(start + 1);
    expect(mermaidThemeChanges()).toBe(start + 1);
  });

  it("is the only way the app initializes mermaid", () => {
    const direct = Object.entries(SOURCES)
      .filter(([, text]) => /\bmermaid\.initialize\(/.test(text))
      .map(([path]) => path);
    expect(direct).toEqual(["./mermaid.ts"]);
  });
});
