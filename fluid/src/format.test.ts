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

import {
  formatActor,
  formatBytes,
  formatDay,
  formatInstant,
  localDay,
  relativeTime,
} from "./format";

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

  it("never states a full 1024 of a unit it has a name above", () => {
    // One byte under a megabyte rounds up to the next unit's own boundary, so
    // the unit is picked against the rounded figure rather than the exact one.
    expect(formatBytes(1024 * 1024 - 1)).toBe("1.0 MiB");
    // A figure that genuinely rounds to 1023 is 1023, not a coy megabyte.
    expect(formatBytes(1024 * 1024 - 600)).toBe("1023 KiB");
    // The top unit has nothing above it, so it keeps counting.
    expect(formatBytes(2048 * 1024 * 1024)).toBe("2048 MiB");
  });
});

/**
 * An instant, in the local date and time a reader's own clock shows.
 *
 * Unlike a plain day, an instant is meant to be parsed: it names a precise
 * moment rather than a day somebody wrote down, so the expected string here
 * is built off the same `Date` the code under test parses - `getHours()` and
 * friends - rather than a string hardcoded against one timezone, so the
 * suite passes on whatever zone the machine running it is in.
 */
describe("an instant, as a reader reads it", () => {
  const INSTANT = "2026-08-10T08:00:00Z";

  it("parses into the local date and time, built from the Date fields", () => {
    const parsed = new Date(INSTANT);
    const pad = (n: number) => String(n).padStart(2, "0");
    const expected = `${localDay(parsed)} ${pad(parsed.getHours())}:${pad(parsed.getMinutes())}`;
    expect(formatInstant(INSTANT)).toBe(expected);
  });

  it("falls back to formatDay's answer for a plain day", () => {
    expect(formatInstant("2026-08-04")).toBe(formatDay("2026-08-04"));
  });

  it("falls back to formatDay's answer for a value with no date in it", () => {
    expect(formatInstant("not a date")).toBe(formatDay("not a date"));
  });

  it("falls back to formatDay's answer for an instant-shaped string that does not parse", () => {
    const value = "2026-13-40T99:99:00Z";
    expect(formatInstant(value)).toBe(formatDay(value));
  });
});

/**
 * How long ago an instant was, in the words a reader reads.
 *
 * Every case is built off explicit `now` values offset from one fixed
 * instant, so the unit boundaries (a minute, an hour, a day) are exercised
 * without depending on the wall clock or a mocked timezone.
 */
describe("how long ago an instant was, in the words a reader reads", () => {
  const INSTANT = "2026-08-10T08:00:00Z";
  const base = new Date(INSTANT).getTime();
  const after = (ms: number) => new Date(base + ms);

  it("says just now for anything under a minute", () => {
    expect(relativeTime(INSTANT, after(0))).toBe("just now");
    expect(relativeTime(INSTANT, after(30_000))).toBe("just now");
    expect(relativeTime(INSTANT, after(59_000))).toBe("just now");
  });

  it("counts whole minutes once a minute has passed", () => {
    expect(relativeTime(INSTANT, after(60_000))).toBe("1 minute ago");
    expect(relativeTime(INSTANT, after(13 * 60_000))).toBe("13 minutes ago");
  });

  it("steps up to hours once sixty minutes have passed", () => {
    expect(relativeTime(INSTANT, after(2 * 3_600_000))).toBe("2 hours ago");
  });

  it("steps up to days once twenty-four hours have passed", () => {
    expect(relativeTime(INSTANT, after(3 * 86_400_000))).toBe("3 days ago");
  });

  it("reads the other direction for an instant still ahead of now", () => {
    expect(relativeTime(INSTANT, after(-13 * 60_000))).toBe("in 13 minutes");
  });

  it("returns null when the value does not parse", () => {
    expect(relativeTime("not a date", after(0))).toBeNull();
  });
});
