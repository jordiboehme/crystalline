/**
 * What the diff pane decides before it draws anything: which of the two
 * layouts an editor gets, and when a sentence stands in for an editor because
 * there is nothing an editor could show.
 */

import { describe, expect, it } from "vitest";

import type { ChangeDetail } from "../api/admin";
import { kindWord, paneFace, sizeSentence } from "./diffFace";

function detail(overrides: Partial<ChangeDetail> = {}): ChangeDetail {
  return {
    path: "notes/a.md",
    kind: "modified",
    sha: "9f2c",
    sizeBefore: 1204,
    sizeAfter: 1388,
    binary: false,
    engram: null,
    base: "old\n",
    current: "new\n",
    tooLarge: false,
    ...overrides,
  };
}

describe("the diff pane's face", () => {
  it("is a unified editor by default and split when wide", () => {
    expect(paneFace(detail(), false)).toEqual({
      kind: "editor",
      layout: "unified",
      base: "old\n",
      current: "new\n",
    });
    expect(paneFace(detail(), true)).toEqual({
      kind: "editor",
      layout: "split",
      base: "old\n",
      current: "new\n",
    });
  });

  it("reads an addition against empty and a deletion as base against empty", () => {
    expect(
      paneFace(detail({ kind: "added", base: null, sizeBefore: null }), false),
    ).toEqual({
      kind: "editor",
      layout: "unified",
      base: "",
      current: "new\n",
    });
    expect(
      paneFace(
        detail({ kind: "deleted", current: null, sha: null, sizeAfter: null }),
        false,
      ),
    ).toEqual({
      kind: "editor",
      layout: "unified",
      base: "old\n",
      current: "",
    });
  });

  it("says the sizes for a binary change and nothing else", () => {
    expect(
      paneFace(
        detail({
          binary: true,
          base: null,
          current: null,
          sizeBefore: 20480,
          sizeAfter: 24576,
        }),
        true,
      ),
    ).toEqual({
      kind: "sentence",
      text: "Changed, 20 KiB to 24 KiB",
      hint: null,
    });
    expect(sizeSentence("added", null, 20480)).toBe("Added, 20 KiB");
    expect(sizeSentence("deleted", 20480, null)).toBe("Deleted, 20 KiB");
  });

  it("says too large with the CLI hint", () => {
    expect(
      paneFace(
        detail({
          tooLarge: true,
          base: null,
          current: null,
          sizeBefore: 1468006,
          sizeAfter: 1572864,
        }),
        false,
      ),
    ).toEqual({
      kind: "sentence",
      text: "Too large to show here, 1.4 MiB to 1.5 MiB",
      hint: "crystalline origin diff <domain> --path notes/a.md",
    });
  });

  it("says too large on its own when neither size is known", () => {
    // Not a shape this server answers - a side big enough to withhold is a
    // side whose length it read - so what is pinned is that a reader taking
    // the payload at its word never runs a word into the sentence.
    expect(
      paneFace(
        detail({
          kind: "renamed",
          tooLarge: true,
          base: null,
          current: null,
          sizeBefore: null,
          sizeAfter: null,
        }),
        false,
      ),
    ).toEqual({
      kind: "sentence",
      text: "Too large to show here",
      hint: "crystalline origin diff <domain> --path notes/a.md",
    });
  });

  it("names the kinds", () => {
    expect(kindWord("added")).toBe("Added");
    expect(kindWord("modified")).toBe("Modified");
    expect(kindWord("deleted")).toBe("Deleted");
    expect(kindWord("renamed")).toBe("Renamed");
  });
});
