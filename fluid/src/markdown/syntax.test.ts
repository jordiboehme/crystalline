/**
 * The footnote read and the reading page share one list of syntax plugins.
 * This fails if the page adds a plugin that changes how text is parsed
 * without it being in the shared list.
 */

import { describe, expect, it } from "vitest";

import { remarkPipeline } from "./pipeline";
import { markdownSyntax } from "./syntax";

type Store = Record<string, unknown[]>;

/** The parse extensions a plugin registers when it is attached. */
function registers(plugin: unknown): number {
  const store: Store = {};
  const self = {
    data: (key?: string) => (key === undefined ? store : store[key]),
  };
  const entry: unknown[] = Array.isArray(plugin) ? plugin : [plugin];
  const attacher = entry[0] as (this: unknown, ...args: unknown[]) => unknown;
  attacher.call(self, ...entry.slice(1));
  return Object.values(store).reduce((sum, list) => sum + list.length, 0);
}

describe("the shared syntax", () => {
  it("is the start of the page's pipeline, in the same order", () => {
    const pipeline = remarkPipeline("");
    expect(pipeline.slice(0, markdownSyntax.length)).toEqual([
      ...markdownSyntax,
    ]);
  });

  it("is everything in the pipeline that changes how text is parsed", () => {
    const rest = remarkPipeline("").slice(markdownSyntax.length);
    expect(rest.length).toBeGreaterThan(0);
    for (const plugin of rest) {
      expect(registers(plugin)).toBe(0);
    }
  });

  it("does register syntax in its own plugins", () => {
    expect(markdownSyntax.every((plugin) => registers(plugin) > 0)).toBe(true);
  });
});
