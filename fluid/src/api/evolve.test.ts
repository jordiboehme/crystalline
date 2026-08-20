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

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { defined } from "../test/assert";
import { api } from "./client";
import {
  EVOLVE_FAMILIES,
  EVOLVE_LIMIT,
  acknowledgeFinding,
  evolveFamily,
  evolveKey,
  fetchEvolveQueue,
  readEvolveQueue,
  unacknowledgeFinding,
} from "./evolve";

vi.mock("./client", async (importOriginal) => {
  const actual = await importOriginal<typeof import("./client")>();
  return { ...actual, api: vi.fn() };
});

const apiMock = vi.mocked(api);

/**
 * The real transport, so the two acknowledgment writes can be checked over
 * `fetch` itself.
 *
 * The queue reads are stubbed at {@link api}, which is where a payload reader
 * is worth pinning. A write is a different claim - the method, the JSON body
 * and the CSRF header the server refuses without - and none of those exist
 * above the client, so those tests run the genuine one against a stubbed
 * `fetch`. Both halves address the same module instance, so the token one sets
 * is the token the other reads.
 */
const realClient = await vi.importActual<typeof import("./client")>("./client");

/** Install a fetch stub and hand back the spy the assertions read. */
function stubFetch(...responses: Response[]) {
  const queue = [...responses];
  const spy = vi.fn((_input: string | URL | Request, _init?: RequestInit) => {
    const next = queue.shift();
    if (!next) {
      throw new Error("fetch called more times than the test stubbed");
    }
    return Promise.resolve(next);
  });
  vi.stubGlobal("fetch", spy);
  return spy;
}

/** The JSON body a stubbed call went out with. */
function sentBody(init: RequestInit | undefined): unknown {
  const body = init?.body;
  if (typeof body !== "string") {
    throw new Error("expected the call to carry a JSON body");
  }
  return JSON.parse(body) as unknown;
}

beforeEach(() => {
  apiMock.mockReset();
  apiMock.mockImplementation(realClient.api);
});

afterEach(() => {
  vi.unstubAllGlobals();
  realClient.setCsrfToken(null);
});

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
    acknowledged: {
      total: 2,
      by_family: { temporal: 0, structure: 2, redundancy: 0 },
    },
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
      // Nothing to delete: only the orphaned-attachment rule names a file.
      attachmentPath: null,
      line: null,
      finding: "supersedes target still current",
      evidence: "supersedes eng/new-way; new-way is stable",
      fix: "retire the old engram and wire superseded_by",
      acknowledged: false,
      ackStale: false,
      ackNote: null,
    });
    expect(defined(queue.queue[1], "the second finding").line).toBe(12);
    expect(queue.actions).toContainEqual({
      rule: "V006",
      instruction: "Read it and verify the claim.",
    });
    expect(queue.truncations).toEqual(["eng - findings capped at 200"]);
  });

  it("reads the guidance the whole queue is worked under", () => {
    // The engine's own mechanical-versus-judgment sentence, which the chips on
    // every row are shorthand for. It rides the sweep rather than the rows,
    // so it is read once and said once.
    expect(readEvolveQueue(evolvePayload()).guidance).toBe(
      "This queue changes nothing by itself.",
    );
  });

  it("reads no guidance at all as none rather than as an empty line", () => {
    expect(readEvolveQueue({ total: 1 }).guidance).toBeNull();
    expect(readEvolveQueue({ guidance: 7 }).guidance).toBeNull();
  });

  it("reads a class it has never heard of as judgment", () => {
    const queue = readEvolveQueue(evolvePayload());

    // Fail safe toward asking: an unknown class drawn as mechanical would
    // invite somebody to apply a change nobody has judged.
    expect(defined(queue.queue[3], "the last finding").class).toBe("judgment");
  });

  it("keeps a finding that names no engram, and survives a payload of nonsense", () => {
    const queue = readEvolveQueue({
      engrams_scanned: 3,
      total: 2,
      families: [{ family: "temporal" }, null, "nonsense"],
      queue: [
        // An orphaned attachment: the subject is the file, so there is no
        // permalink to carry. Dropping it counted it in `total` and drew none
        // of it, which is a queue that says two and shows one.
        {
          n: 1,
          priority: 55,
          rule: "V108",
          class: "judgment",
          domain: "eng",
          permalink: "",
          title: "assets/2026/08/orphan.png",
          line: null,
          finding: "no engram references or claims this attachment",
          evidence: "12 KiB, image/png; no engram references or claims it",
          fix: "delete assets/2026/08/orphan.png or analyze it into an engram",
        },
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

    expect(queue.queue.map((row) => row.permalink)).toEqual(["", "orphan"]);
    const anchorless = defined(queue.queue[0], "the anchorless finding");
    expect(anchorless.title).toBe("assets/2026/08/orphan.png");
    // The path a delete is addressed to is read straight off the row, with no
    // fallback of its own: the same string today as the title, and a separate
    // field so it stays the path if the title ever stops being one.
    expect(anchorless.attachmentPath).toBe("assets/2026/08/orphan.png");
    expect(
      defined(queue.queue[1], "the orphan engram finding").attachmentPath,
    ).toBeNull();
    expect(queue.families).toEqual([]);
    expect(queue.actions).toEqual([]);
    expect(queue.truncations).toEqual(["eng - capped"]);
  });

  it("names an anchorless finding by its domain when it has no title either", () => {
    // Tag drift is about a domain's vocabulary rather than about any one
    // engram, so it arrives with neither a permalink nor a title. The domain
    // is the only subject there is, and a blank row would be worse.
    const queue = readEvolveQueue({
      queue: [{ n: 1, priority: 30, rule: "V203", domain: "eng" }],
    });

    expect(defined(queue.queue[0], "the tag drift finding").title).toBe("eng");
  });

  it("gives an orphan row that names no path nothing to delete", () => {
    // The title falls back to the domain; the delete path never does. A row
    // with no path names no file, and `/files/eng` is not a file.
    const queue = readEvolveQueue({
      queue: [{ n: 1, priority: 55, rule: "V108", domain: "eng" }],
    });

    const row = defined(queue.queue[0], "the pathless orphan finding");
    expect(row.title).toBe("eng");
    expect(row.attachmentPath).toBeNull();
  });

  it("still refuses a row with no rule or no domain", () => {
    // Neither can be defaulted: the rule decides the section and the action,
    // and the domain is the address every write to it needs.
    const queue = readEvolveQueue({
      queue: [
        { n: 1, priority: 40, domain: "eng", permalink: "alpha" },
        { n: 2, priority: 40, rule: "V003", permalink: "alpha" },
      ],
    });

    expect(queue.queue).toEqual([]);
  });

  it("answers an empty queue for a payload that is not one at all", () => {
    expect(readEvolveQueue(null)).toEqual({
      engramsScanned: 0,
      total: 0,
      families: [],
      queue: [],
      actions: [],
      truncations: [],
      acknowledged: { total: 0, byFamily: {} },
      guidance: null,
    });
  });
});

/**
 * What acknowledgments kept out of the queue, and what an acknowledgment that
 * stopped matching left on the finding it no longer silences.
 */
describe("the acknowledgment fields", () => {
  it("reads the suppressed counts, whole and per family", () => {
    const queue = readEvolveQueue(evolvePayload());

    expect(queue.acknowledged).toEqual({
      total: 2,
      byFamily: { temporal: 0, structure: 2, redundancy: 0 },
    });
  });

  it("counts nothing suppressed when the sweep reports no counts at all", () => {
    // A count that never arrived is zero silenced findings, never an unknown
    // number: the line it feeds only appears above zero.
    expect(readEvolveQueue({ total: 1 }).acknowledged).toEqual({
      total: 0,
      byFamily: {},
    });
    expect(
      readEvolveQueue({ acknowledged: { total: 3, by_family: "nonsense" } })
        .acknowledged,
    ).toEqual({ total: 3, byFamily: {} });
  });

  it("reads a suppressed finding and the note that silenced it", () => {
    const queue = readEvolveQueue({
      queue: [
        {
          n: 1,
          priority: 55,
          rule: "V101",
          class: "mechanical",
          domain: "eng",
          permalink: "alpha",
          title: "Alpha",
          acknowledged: true,
          ack_note: "lineage citation, keep",
        },
      ],
    });

    const finding = defined(queue.queue[0], "the suppressed finding");
    expect(finding.acknowledged).toBe(true);
    expect(finding.ackStale).toBe(false);
    expect(finding.ackNote).toBe("lineage citation, keep");
  });

  it("reads an acknowledgment that no longer matches the evidence", () => {
    const queue = readEvolveQueue({
      queue: [
        {
          n: 1,
          priority: 55,
          rule: "V101",
          class: "mechanical",
          domain: "eng",
          permalink: "alpha",
          title: "Alpha",
          ack_stale: true,
          ack_note: "lineage citation, keep",
        },
      ],
    });

    const finding = defined(queue.queue[0], "the stale finding");
    // Returned rather than suppressed: the evidence moved, so the queue says
    // so instead of pretending the acknowledgment never happened.
    expect(finding.acknowledged).toBe(false);
    expect(finding.ackStale).toBe(true);
    expect(finding.ackNote).toBe("lineage citation, keep");
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

  it("asks for the suppressed findings only when told to", async () => {
    apiMock.mockResolvedValue(evolvePayload());

    await fetchEvolveQueue({ includeAcknowledged: true });

    expect(apiMock).toHaveBeenCalledWith(
      `/evolve?limit=${String(EVOLVE_LIMIT)}&include_acknowledged=true`,
    );

    apiMock.mockClear();
    await fetchEvolveQueue({ includeAcknowledged: false });

    // Absent rather than `false`: the default is the server's, and a sweep
    // that never asked reads as one that never asked.
    expect(apiMock).toHaveBeenCalledWith(
      `/evolve?limit=${String(EVOLVE_LIMIT)}`,
    );
  });

  it("keys a sweep by what it asked for, the suppressed rows included", () => {
    // Two different questions, so two different cached answers: showing the
    // silenced findings must not read back the sweep that left them out.
    expect(evolveKey()).not.toEqual(evolveKey([], true));
    expect(evolveKey([], true)).toEqual(evolveKey([], true));
  });
});

/**
 * The two writes: ruling a finding intentional, and taking that back.
 *
 * Both go out over the genuine client, because everything worth pinning about
 * them lives there - the method, the JSON body and the CSRF header the server
 * refuses a write without.
 */
describe("acknowledging a finding", () => {
  it("POSTs the engram, the rule and the note, with the CSRF header", async () => {
    realClient.setCsrfToken("token-1");
    const spy = stubFetch(new Response(null, { status: 204 }));

    await acknowledgeFinding("eng", "notes/beta", "V101", "lineage, keep");

    expect(spy.mock.calls[0]?.[0]).toBe("/api/v1/domains/eng/evolve/ack");
    const init = spy.mock.calls[0]?.[1];
    expect(init?.method).toBe("POST");
    expect(new Headers(init?.headers).get(realClient.CSRF_HEADER)).toBe(
      "token-1",
    );
    expect(sentBody(init)).toEqual({
      permalink: "notes/beta",
      rule: "V101",
      note: "lineage, keep",
    });
  });

  it("leaves an empty note out rather than storing a blank one", async () => {
    const spy = stubFetch(
      new Response(null, { status: 204 }),
      new Response(null, { status: 204 }),
    );

    await acknowledgeFinding("eng", "notes/beta", "V101");
    await acknowledgeFinding("eng", "notes/beta", "V101", "   ");

    expect(sentBody(spy.mock.calls[0]?.[1])).toEqual({
      permalink: "notes/beta",
      rule: "V101",
    });
    expect(sentBody(spy.mock.calls[1]?.[1])).toEqual({
      permalink: "notes/beta",
      rule: "V101",
    });
  });

  it("encodes a domain name that is not URL safe", async () => {
    const spy = stubFetch(new Response(null, { status: 204 }));

    await acknowledgeFinding("my domain", "notes/beta", "V101");

    expect(spy.mock.calls[0]?.[0]).toBe(
      "/api/v1/domains/my%20domain/evolve/ack",
    );
  });

  it("DELETEs the same shape to take an acknowledgment back", async () => {
    realClient.setCsrfToken("token-2");
    const spy = stubFetch(new Response(null, { status: 204 }));

    await unacknowledgeFinding("eng", "notes/beta", "V101");

    expect(spy.mock.calls[0]?.[0]).toBe("/api/v1/domains/eng/evolve/ack");
    const init = spy.mock.calls[0]?.[1];
    expect(init?.method).toBe("DELETE");
    expect(new Headers(init?.headers).get(realClient.CSRF_HEADER)).toBe(
      "token-2",
    );
    // No note: a removal names the entry, and the endpoint ignores one anyway.
    expect(sentBody(init)).toEqual({ permalink: "notes/beta", rule: "V101" });
  });
});
