import { describe, expect, it } from "vitest";

import {
  readScalar,
  readTagList,
  writeScalar,
  writeTagList,
} from "./frontmatterFields";

const DOC = `---
title: Alpha
permalink: alpha
type: engram
status: stable
tags:
  - eng
  - deep
valid_from: 2026-01-01
salience: 0.7
---

# Alpha

Body text with status: decoy outside the block.
`;

function applied(
  doc: string,
  edit: { from: number; to: number; insert: string } | null,
): string {
  if (!edit) {
    throw new Error("expected an edit");
  }
  return doc.slice(0, edit.from) + edit.insert + doc.slice(edit.to);
}

describe("scalar fields", () => {
  it("reads only inside the block", () => {
    expect(readScalar(DOC, "status")).toBe("stable");
    expect(readScalar(DOC, "valid_to")).toBeNull();
    expect(readScalar("no block", "status")).toBeNull();
  });

  it("rewrites exactly one line and nothing else", () => {
    const next = applied(DOC, writeScalar(DOC, "status", "draft"));
    expect(next).toContain("status: draft\n");
    // Every other byte is untouched, including the decoy in the body.
    expect(next.replace("status: draft\n", "status: stable\n")).toBe(DOC);
  });

  it("adds a missing key before the closing fence", () => {
    const next = applied(DOC, writeScalar(DOC, "valid_to", "2026-12-01"));
    expect(next).toContain("valid_to: 2026-12-01\n---\n");
  });

  it("removing a key removes its line: absent means unbounded, never a sentinel", () => {
    const next = applied(DOC, writeScalar(DOC, "valid_from", null));
    expect(next).not.toContain("valid_from");
    expect(readScalar(next, "valid_from")).toBeNull();
  });

  it("quotes a value that yaml would misread", () => {
    const next = applied(DOC, writeScalar(DOC, "title", "Alpha: the rule"));
    expect(next).toContain('title: "Alpha: the rule"\n');
  });
});

describe("tag lists", () => {
  it("reads block and inline forms", () => {
    expect(readTagList(DOC)).toEqual(["eng", "deep"]);
    expect(readTagList("---\ntags: [a, b]\n---\n")).toEqual(["a", "b"]);
    expect(readTagList("---\ntitle: x\n---\n")).toEqual([]);
  });

  it("replaces the whole entry with a block list", () => {
    const next = applied(DOC, writeTagList(DOC, ["eng", "editor"]));
    expect(next).toContain("tags:\n  - eng\n  - editor\n");
    expect(next).not.toContain("- deep");
  });

  it("an empty list removes the entry", () => {
    const next = applied(DOC, writeTagList(DOC, []));
    expect(next).not.toContain("tags:");
  });
});
