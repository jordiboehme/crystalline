import { describe, expect, it } from "vitest";

import type { EngramChange } from "../api/events";
import {
  keysFor,
  keysForDomain,
  keysForEngram,
  keysForReflection,
} from "./invalidation";

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

  it("a files event reaches the attachment listing and treats assets/... as no engram page", () => {
    const keys = keysForEngram(
      change({
        kind: "files",
        permalink: "assets/a.png",
        path: "assets/a.png",
      }),
    );
    expect(has(keys, ["attachments", "eng"])).toBe(true);
    expect(has(keys, ["engram", "eng", "assets/a.png"])).toBe(false);
    expect(has(keys, ["graph", "eng", "assets/a.png"])).toBe(false);
    expect(has(keys, ["domain-tree", "eng"])).toBe(false);
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

  it("a reflection event reaches the whole Reflection of its domain and the listing", () => {
    const keys = keysForReflection({
      domain: "eng",
      path: "guides/a.md",
      kind: "threads",
      from: null,
      actor: null,
    });
    expect(has(keys, ["reflection", "eng"])).toBe(true);
    expect(has(keys, ["reflection-status", "eng"])).toBe(true);
    expect(has(keys, ["reflection-domains"])).toBe(true);
    expect(has(keys, ["reflection", "ops"])).toBe(false);
    const moved = {
      domain: "eng",
      path: "a.md",
      kind: "moved" as const,
      from: "b.md",
      actor: null,
    };
    expect(keysFor({ event: "reflection", change: moved })).toEqual(
      keysForReflection(moved),
    );
  });

  it("a MANIFEST.md change also reaches the Reflection status and listing, so a switch made elsewhere shows", () => {
    const keys = keysForEngram(
      change({ permalink: "manifest", path: "MANIFEST.md", kind: "modified" }),
    );
    expect(has(keys, ["reflection-status", "eng"])).toBe(true);
    expect(has(keys, ["reflection-domains"])).toBe(true);
  });
});
