/**
 * The consolidation queue, read the way every payload in `api/` is read.
 *
 * Two things here are worth pinning. A finding's class decides how the page
 * draws it - mechanical work an agent may just do, judgment work that is a
 * question for a person - so a class this client has never heard of has to
 * fail toward asking rather than toward acting. And the queue rows carry no
 * family of their own: the engine counts families over the whole result and
 * numbers each rule by family in its catalog, so the section a row belongs
 * under is read off its rule id, and a rule id from a catalog newer than this
 * client belongs to no section rather than to the wrong one.
 */

import { describe, expect, it, vi } from "vitest";

import { defined } from "../test/assert";
import { api } from "./client";
import {
  EVOLVE_FAMILIES,
  EVOLVE_LIMIT,
  evolveFamily,
  fetchEvolveQueue,
  readEvolveQueue,
} from "./evolve";

vi.mock("./client", async (importOriginal) => {
  const actual = await importOriginal<typeof import("./client")>();
  return { ...actual, api: vi.fn() };
});

const apiMock = vi.mocked(api);

/** One sweep, in the engine's own shape, captured from `GET /evolve`. */
function evolvePayload() {
  return {
    scope: {
      domains: ["eng", "ops"],
      families: [],
      rules: [],
      min_priority: null,
      today: "2026-08-17",
    },
    engrams_scanned: 42,
    unparsed: 0,
    total: 4,
    page: 1,
    limit: 100,
    count: 4,
    families: [
      { family: "temporal", findings: 2 },
      { family: "structure", findings: 1 },
      { family: "redundancy", findings: 1 },
    ],
    queue: [
      {
        n: 1,
        priority: 90,
        rule: "V005",
        class: "mechanical",
        domain: "eng",
        permalink: "notes/old-way",
        title: "The old way",
        line: null,
        finding: "supersedes target still current",
        evidence: "supersedes eng/new-way; new-way is stable",
        fix: "retire the old engram and wire superseded_by",
      },
      {
        n: 2,
        priority: 80,
        rule: "V201",
        class: "judgment",
        domain: "ops",
        permalink: "runbooks/restart",
        title: "Restarting the daemon",
        line: 12,
        finding: "near-duplicate content",
        evidence: "0.91 overlap with ops/runbooks/restart-service",
        fix: "merge into the richer engram and supersede the other",
      },
      {
        n: 3,
        priority: 55,
        rule: "V101",
        class: "mechanical",
        domain: "eng",
        permalink: "alpha",
        title: "Alpha",
        line: 7,
        finding: "live reference to retired",
        evidence: "links_to eng/old-way, which is deprecated",
        fix: "repoint at the successor named in the evidence",
      },
      {
        n: 4,
        priority: 50,
        // A class from a catalog this client has never seen.
        rule: "V006",
        class: "somethingelse",
        domain: "eng",
        permalink: "human-capture",
        title: "Incident capture",
        line: null,
        finding: "captured by a person and never reviewed",
        evidence: "generated.by human:jordi; no verified entry",
        fix: "review, then record a verified entry",
      },
    ],
    actions: [
      { rule: "V005", instruction: "Complete the retirement." },
      { rule: "V006", instruction: "Read it and verify the claim." },
      { rule: "V101", instruction: "Repoint it at the successor." },
      { rule: "V201", instruction: "Merge into the richest one." },
    ],
    guidance: "This queue changes nothing by itself.",
    truncations: ["eng - findings capped at 200"],
  };
}

describe("the evolve payload", () => {
  it("reads the sweep, its queue and its per-rule instructions", () => {
    const queue = readEvolveQueue(evolvePayload());

    expect(queue.engramsScanned).toBe(42);
    expect(queue.total).toBe(4);
    expect(queue.families).toEqual([
      { family: "temporal", findings: 2 },
      { family: "structure", findings: 1 },
      { family: "redundancy", findings: 1 },
    ]);
    expect(queue.queue).toHaveLength(4);
    const first = defined(queue.queue[0], "the first finding");
    expect(first).toEqual({
      n: 1,
      priority: 90,
      rule: "V005",
      class: "mechanical",
      domain: "eng",
      permalink: "notes/old-way",
      title: "The old way",
      line: null,
      finding: "supersedes target still current",
      evidence: "supersedes eng/new-way; new-way is stable",
      fix: "retire the old engram and wire superseded_by",
    });
    expect(defined(queue.queue[1], "the second finding").line).toBe(12);
    expect(queue.actions).toContainEqual({
      rule: "V006",
      instruction: "Read it and verify the claim.",
    });
    expect(queue.truncations).toEqual(["eng - findings capped at 200"]);
  });

  it("reads a class it has never heard of as judgment", () => {
    const queue = readEvolveQueue(evolvePayload());

    // Fail safe toward asking: an unknown class drawn as mechanical would
    // invite somebody to apply a change nobody has judged.
    expect(defined(queue.queue[3], "the last finding").class).toBe("judgment");
  });

  it("drops a row that carries no address and survives a payload of nonsense", () => {
    const queue = readEvolveQueue({
      engrams_scanned: 3,
      total: 2,
      families: [{ family: "temporal" }, null, "nonsense"],
      queue: [
        { n: 1, priority: 40, rule: "V003", class: "judgment", domain: "eng" },
        null,
        {
          n: 2,
          priority: 30,
          rule: "V104",
          class: "mechanical",
          domain: "eng",
          permalink: "orphan",
          title: "Orphan",
          line: null,
          finding: "orphan",
          evidence: "no inbound or outbound reference",
          fix: "link it into the neighbourhood its tags suggest",
        },
      ],
      actions: [{ rule: "V104" }, { instruction: "no rule" }],
      truncations: [null, "eng - capped"],
    });

    expect(queue.queue.map((row) => row.permalink)).toEqual(["orphan"]);
    expect(queue.families).toEqual([]);
    expect(queue.actions).toEqual([]);
    expect(queue.truncations).toEqual(["eng - capped"]);
  });

  it("answers an empty queue for a payload that is not one at all", () => {
    expect(readEvolveQueue(null)).toEqual({
      engramsScanned: 0,
      total: 0,
      families: [],
      queue: [],
      actions: [],
      truncations: [],
    });
  });
});

/**
 * Which section a finding is drawn under. The rule id says it - the catalog
 * numbers temporal rules `V0xx`, structure `V1xx` and redundancy `V2xx` - and
 * a row carries no family of its own to read instead.
 */
describe("the family of a rule", () => {
  it("reads the family off the rule id", () => {
    expect(evolveFamily("V001")).toBe("temporal");
    expect(evolveFamily("V006")).toBe("temporal");
    expect(evolveFamily("V105")).toBe("structure");
    expect(evolveFamily("V203")).toBe("redundancy");
  });

  it("puts a rule from a newer catalog under no section at all", () => {
    // Never guessed into a section: a finding filed under the wrong heading
    // reads as a claim about what kind of work it is.
    expect(evolveFamily("V301")).toBeNull();
    expect(evolveFamily("")).toBeNull();
    expect(evolveFamily("nonsense")).toBeNull();
  });

  it("lists the families in the catalog's own order", () => {
    expect(EVOLVE_FAMILIES).toEqual(["temporal", "structure", "redundancy"]);
  });
});

describe("fetching the queue", () => {
  it("asks for a full page of every domain by default", async () => {
    apiMock.mockResolvedValue(evolvePayload());

    const queue = await fetchEvolveQueue();

    expect(queue.total).toBe(4);
    expect(apiMock).toHaveBeenCalledWith(
      `/evolve?limit=${String(EVOLVE_LIMIT)}`,
    );
  });

  it("names the domains it was scoped to, comma separated", async () => {
    apiMock.mockResolvedValue(evolvePayload());

    await fetchEvolveQueue({ domains: ["eng", "ops"], limit: 25 });

    expect(apiMock).toHaveBeenCalledWith("/evolve?domains=eng%2Cops&limit=25");
  });
});
