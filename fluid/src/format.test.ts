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

import { formatActor } from "./format";

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
