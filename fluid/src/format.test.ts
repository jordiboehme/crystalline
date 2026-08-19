/**
 * Saying who wrote something, in the words a reader uses.
 *
 * An OKF actor is written for a machine to sort on: `human:jordi`,
 * `process:indexer`, `claude-code/2.1`. A person reading the panel wants the
 * name first and the kind after it, and an actor written in none of those
 * conventions is somebody else's convention rather than a malformed one, so it
 * is shown as written rather than guessed at.
 */

import { describe, expect, it } from "vitest";

import { formatActor, formatBytes } from "./format";

describe("an actor, as a reader reads it", () => {
  it("names the person before the kind", () => {
    expect(formatActor("human:jordi")).toBe("jordi (human)");
  });

  it("splits an agent's version off its name", () => {
    expect(formatActor("claude-code/2.1")).toBe("claude-code (agent, 2.1)");
  });

  it("names an automated job as a process", () => {
    expect(formatActor("process:indexer")).toBe("indexer (process)");
  });

  it("shows an actor in no convention it knows as written", () => {
    expect(formatActor("teambot")).toBe("teambot");
  });
});

/**
 * A file's size, in the units its own ceiling is stated in.
 *
 * An author is told an attachment may hold 10 MiB, so the panel that lists one
 * says MiB too: a size in MB beside that sentence would be a second unit for
 * the same quantity, off by five percent at the top of the range.
 */
describe("a byte count, as a reader reads it", () => {
  it("counts small files in bytes", () => {
    expect(formatBytes(0)).toBe("0 B");
    expect(formatBytes(812)).toBe("812 B");
    expect(formatBytes(1023)).toBe("1023 B");
  });

  it("steps up a unit at the binary boundary", () => {
    expect(formatBytes(1024)).toBe("1.0 KiB");
    expect(formatBytes(1024 * 1024)).toBe("1.0 MiB");
  });

  it("keeps a decimal only while it says something", () => {
    expect(formatBytes(1536)).toBe("1.5 KiB");
    expect(formatBytes(46080)).toBe("45 KiB");
    expect(formatBytes(1258291)).toBe("1.2 MiB");
  });

  it("stops at the unit the ceiling is stated in", () => {
    expect(formatBytes(10 * 1024 * 1024)).toBe("10 MiB");
  });
});
