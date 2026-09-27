import { describe, expect, it } from "vitest";

import type { EngramChange } from "../api/events";
import { keysFor, keysForDomain, keysForEngram } from "./invalidation";

function change(overrides: Partial<EngramChange> = {}): EngramChange {
  return {
    domain: "eng",
    permalink: "alpha",
    path: "alpha.md",
    kind: "modified",
    from: null,
    checksum: "9f",
    actor: null,
    draftOf: null,
    ...overrides,
  };
}

const has = (keys: readonly (readonly unknown[])[], key: readonly unknown[]) =>
  keys.some((k) => JSON.stringify(k) === JSON.stringify(key));

describe("the invalidation table", () => {
  it("an engram event of any kind reaches the detail, the graph prefix, the tree, the listing, search, title matches and the sync card", () => {
    const keys = keysForEngram(change());
    for (const key of [
      ["engram", "eng", "alpha"],
      ["graph", "eng", "alpha"],
      ["domain-tree", "eng"],
      ["domain-engrams", "eng"],
      ["search"],
      ["title-matches"],
      ["domains", "eng", "sync"],
      ["share-plan", "eng"],
    ]) {
      expect(has(keys, key), JSON.stringify(key)).toBe(true);
    }
    expect(has(keys, ["domains"])).toBe(false);
  });

  it("a move also reaches the old permalink's detail and graph", () => {
    const keys = keysForEngram(
      change({
        kind: "moved",
        permalink: "new",
        from: { path: "alpha.md", permalink: "alpha" },
      }),
    );
    expect(has(keys, ["engram", "eng", "new"])).toBe(true);
    expect(has(keys, ["engram", "eng", "alpha"])).toBe(true);
    expect(has(keys, ["graph", "eng", "alpha"])).toBe(true);
  });

  it("an add or a delete also reaches the domain switcher, which shows counts", () => {
    expect(has(keysForEngram(change({ kind: "added" })), ["domains"])).toBe(
      true,
    );
    expect(has(keysForEngram(change({ kind: "deleted" })), ["domains"])).toBe(
      true,
    );
    expect(
      has(keysForEngram(change({ kind: "deleted" })), [
        "engram",
        "eng",
        "alpha",
      ]),
    ).toBe(true);
  });

  it("a modified MANIFEST.md also reaches the domain switcher, so domainSpellings follows a canonical-name change (Jordi, 2026-09-27)", () => {
    expect(
      has(keysForEngram(change({ path: "MANIFEST.md" })), ["domains"]),
    ).toBe(true);
    expect(
      has(keysForEngram(change({ path: "MANIFEST.md", kind: "added" })), [
        "domains",
      ]),
      "already covered by the add/delete row",
    ).toBe(true);
    expect(
      has(keysForEngram(change()), ["domains"]),
      "an ordinary path does not",
    ).toBe(false);
  });

  it("a domain event reaches every key of the domain, the switcher and the sync summary", () => {
    const keys = keysForDomain({ domain: "eng", actor: null });
    for (const key of [
      ["engram", "eng"],
      ["graph", "eng"],
      ["domain-tree", "eng"],
      ["domain-engrams", "eng"],
      ["search"],
      ["title-matches"],
      ["domains", "eng", "sync"],
      ["share-plan", "eng"],
      ["domains"],
      ["sync-summary"],
    ]) {
      expect(has(keys, key), JSON.stringify(key)).toBe(true);
    }
  });

  it("a reset is everything", () => {
    expect(keysFor({ event: "reset" })).toBe("everything");
  });
});
