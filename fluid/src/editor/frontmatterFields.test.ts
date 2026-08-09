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

/**
 * Every YAML shape this module does not understand, in one block, plus the
 * decoys each of them can plant: a comment, a block scalar whose text is a
 * `status:` line, an anchor and its alias, a flow map with a `status` inside
 * it, a key with no value. Nothing here has to be edited through the form -
 * the raw buffer is always there for that - but everything here has to
 * survive an edit to the one key that is a plain top-level scalar.
 */
const HOSTILE = `---
# a note about status: not this one
title: Alpha
description: |
  status: decoy inside a block scalar
  and a second line of it
base: &shared
  status: nested under an anchor
other: *shared
flow: { status: mapped, keep: true }
empty:
status: stable
tags: []
---

status: body decoy
`;

describe("frontmatter this module does not pretend to parse", () => {
  it("reads the top-level scalar and none of the decoys", () => {
    expect(readScalar(HOSTILE, "status")).toBe("stable");
    expect(readScalar(HOSTILE, "empty")).toBeNull();
    expect(readTagList(HOSTILE)).toEqual([]);
  });

  it("changes exactly one line and leaves every other byte alone", () => {
    const next = applied(HOSTILE, writeScalar(HOSTILE, "status", "draft"));
    expect(next).toContain("\nstatus: draft\n");
    expect(next.replace("\nstatus: draft\n", "\nstatus: stable\n")).toBe(
      HOSTILE,
    );
    // The decoys are all still exactly where they were.
    expect(next).toContain("# a note about status: not this one");
    expect(next).toContain("  status: decoy inside a block scalar");
    expect(next).toContain("  status: nested under an anchor");
    expect(next).toContain("flow: { status: mapped, keep: true }");
    expect(next).toContain("\nstatus: body decoy\n");
  });
});

describe("an empty but present block", () => {
  const EMPTY = "---\n---\n\n# Alpha\n";

  it("takes a first scalar and a first tag list", () => {
    expect(applied(EMPTY, writeScalar(EMPTY, "status", "draft"))).toBe(
      "---\nstatus: draft\n---\n\n# Alpha\n",
    );
    expect(applied(EMPTY, writeTagList(EMPTY, ["eng"]))).toBe(
      "---\ntags:\n  - eng\n---\n\n# Alpha\n",
    );
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
