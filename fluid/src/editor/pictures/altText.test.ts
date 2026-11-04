/**
 * A picture's alt text between the source an author wrote and the words a
 * field shows: escapes out for display, and an edit that writes back only
 * what changed, encoded so the reference still parses as written.
 */

import { describe, expect, it } from "vitest";

import { altDisplay, altEdit } from "./altText";

/** The source after `altEdit`'s change, or the source itself. */
function applied(source: string, next: string): string {
  const edit = altEdit(source, next);
  if (edit === null) return source;
  return source.slice(0, edit.from) + edit.insert + source.slice(edit.to);
}

describe("altDisplay", () => {
  it("drops backslash escapes of ASCII punctuation", () => {
    expect(altDisplay("a \\[b\\]")).toBe("a [b]");
    expect(altDisplay("x \\\\ y \\( z")).toBe("x \\ y ( z");
  });

  it("keeps a backslash before anything else", () => {
    expect(altDisplay("a\\b \\é")).toBe("a\\b \\é");
  });
});

describe("altEdit", () => {
  it("inserts only what was typed", () => {
    expect(altEdit("Shot", "Shot 2")).toEqual({ from: 4, to: 4, insert: " 2" });
  });

  it("replaces only the changed character and keeps brackets as written", () => {
    expect(altEdit("a [b] c", "a [b] d")).toEqual({
      from: 6,
      to: 7,
      insert: "d",
    });
  });

  it("keeps the escapes of untouched characters", () => {
    expect(applied("a \\[b\\] c", "a [b] d")).toBe("a \\[b\\] d");
    expect(altEdit("a \\[b\\] c", "a [b] d")).toEqual({
      from: 8,
      to: 9,
      insert: "d",
    });
  });

  it("encodes brackets, backslashes and a parenthesis after a bracket", () => {
    expect(altEdit("x", "a](b")).toEqual({
      from: 0,
      to: 1,
      insert: "a\\]\\(b",
    });
    expect(applied("x", "[\\]")).toBe("\\[\\\\\\]");
  });

  it("never leaves a bracket and a parenthesis side by side", () => {
    // A typed `]` before an untouched `(`.
    expect(applied("a (b)", "a] (b)").includes("](")).toBe(false);
    expect(applied("a(b", "a](b")).toBe("a\\]\\(b");
    // A typed `(` after an untouched `]`.
    expect(applied("[a] b", "[a](b")).toBe("[a]\\(b");
    // A deleted space that brings the two together.
    expect(applied("[a] (b)", "[a](b)")).toBe("[a]\\(b)");
  });

  it("writes a typed line break or tab as one space", () => {
    expect(altEdit("a", "a\nb")).toEqual({ from: 1, to: 1, insert: " b" });
    expect(applied("a", "a\r\nb\tc")).toBe("a b c");
  });

  it("answers null when nothing changed", () => {
    expect(altEdit("x", "x")).toBeNull();
    expect(altEdit("a \\[b\\]", "a [b]")).toBeNull();
  });
});
