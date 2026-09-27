/**
 * Loading one place: what the station asks the server for when it builds a
 * room, and what it makes of the answers.
 *
 * The client is a real `QueryClient` and only `api` is stubbed, so the keys
 * under test are the keys the reading screen reads: a room that fetched its
 * engram under a key of its own would pay for every request twice and could
 * show a different version of the engram than the page beside it.
 *
 * Retries are off in these tests. The app's own client retries a 500 or a
 * dropped connection once, with a backoff, and that timer is not what any
 * case below is about.
 */

import { QueryClient } from "@tanstack/react-query";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { ApiProblem, api } from "../../api/client";
import { engramDetailKey } from "../../api/engram";
import type { Answer } from "../../test/harness";
import { answersFor, domainsResponse } from "../../test/harness";
import { GAME_STALE_MS, loadPlace, TARGET_TIMEOUT_MS } from "./source";

vi.mock("../../api/client", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../../api/client")>();
  return { ...actual, api: vi.fn(), setCsrfToken: vi.fn() };
});

const apiMock = vi.mocked(api);

/** The engine's frontmatter struct, with `salience` under `extra`. */
function frontmatter(title: string, salience: number | null) {
  return {
    engram_type: "decision",
    title,
    status: "stable",
    tags: [],
    extra: salience === null ? {} : { salience },
  };
}

/** A detail payload in the engine's own shape. */
function detailResponse(
  permalink: string,
  title: string,
  salience: number | null,
  relations: unknown[] = [],
  links: unknown[] = [],
) {
  return {
    domain: "eng",
    permalink,
    title,
    path: `${permalink}.md`,
    url: `crystalline://eng/${permalink}`,
    content: `# ${title}\n`,
    checksum: "c0ffee",
    frontmatter: frontmatter(title, salience),
    observations: [],
    relations,
    links,
    inbound: { count: 1, refs: [] },
  };
}

/** Alpha, with two relations the graph locates and one bare prose link. */
const ALPHA = detailResponse(
  "alpha",
  "Alpha",
  5,
  [
    {
      line: 3,
      rel_type: "depends_on",
      resolved: true,
      target: { domain: null, target: "Beta" },
    },
    {
      line: 4,
      rel_type: "supersedes",
      resolved: true,
      target: { domain: null, target: "Gamma" },
    },
  ],
  [{ line: 5, resolved: true, target: { domain: null, target: "Beta" } }],
);

/** The neighbourhood: Alpha and its two targets, one hop out. */
const GRAPH = {
  nodes: [
    { id: 1, domain: "eng", permalink: "alpha", title: "Alpha" },
    { id: 2, domain: "eng", permalink: "notes/beta", title: "Beta" },
    { id: 3, domain: "eng", permalink: "gamma", title: "Gamma" },
  ],
  edges: [
    { from: 1, to: 2, rel_type: "depends_on" },
    { from: 1, to: 3, rel_type: "supersedes" },
  ],
  truncated: false,
};

/** One engram pointing back at Alpha. */
const INBOUND = {
  total: 1,
  page: 1,
  limit: 24,
  count: 1,
  types: [{ rel: "relates_to", count: 1 }],
  hits: [
    {
      domain: "eng",
      permalink: "delta",
      title: "Delta",
      path: "delta.md",
      rel: "relates_to",
      status: "stable",
    },
  ],
};

const BETA_PATH = "/domains/eng/engrams/notes/beta";
const GAMMA_PATH = "/domains/eng/engrams/gamma";

/** Every route a happy load asks for, overridable one at a time. */
function serve(routes: Record<string, Answer> = {}) {
  apiMock.mockImplementation(
    answersFor({
      "/domains": domainsResponse,
      "/domains/eng/engrams/alpha": () => ALPHA,
      "/graph": () => GRAPH,
      "/domains/eng/inbound/alpha": () => INBOUND,
      [BETA_PATH]: () => detailResponse("notes/beta", "Beta", 2),
      [GAMMA_PATH]: () => detailResponse("gamma", "Gamma", 8),
      ...routes,
    }),
  );
}

/** The paths `api` was called with, without their query strings. */
function requested(): string[] {
  return apiMock.mock.calls.map(([path]) => path.split("?")[0] ?? path);
}

let client: QueryClient;

beforeEach(() => {
  apiMock.mockReset();
  client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
});

afterEach(() => {
  vi.useRealTimers();
  client.clear();
});

describe("loadPlace", () => {
  it("builds the place and sets each door target's salience", async () => {
    serve();
    const loaded = await loadPlace(client, "eng", "alpha");
    expect(loaded.kind).toBe("place");
    if (loaded.kind !== "place") {
      return;
    }
    const { place } = loaded;
    expect(place.title).toBe("Alpha");
    expect(
      place.relations.map((r) => [r.address?.permalink, r.targetSalience]),
    ).toEqual([
      ["notes/beta", 2],
      ["gamma", 8],
    ]);
    expect(place.inbound.map((h) => h.address.permalink)).toEqual(["delta"]);
    // Shared with the reading screen, under its own key.
    expect(client.getQueryData(engramDetailKey("eng", "alpha"))).toMatchObject({
      domain: "eng",
      permalink: "alpha",
      title: "Alpha",
    });
    expect(client.getQueryData(engramDetailKey("eng", "gamma"))).toMatchObject({
      permalink: "gamma",
    });
    // The inbound page asks for as many hatches as a room can hold.
    const inboundCall = apiMock.mock.calls.find(([path]) =>
      path.startsWith("/domains/eng/inbound/alpha"),
    );
    expect(inboundCall?.[0]).toContain("limit=24");
    expect(inboundCall?.[0]).toContain("page=1");
  });

  it("routes a door written with a domain's canonical name", async () => {
    // Registered here as `moonbase`, its MANIFEST calls it `moon`: the door
    // is written the way the content spells it and the route is the local one.
    const listing = domainsResponse();
    serve({
      "/domains": () => ({
        ...listing,
        domains: [
          ...listing.domains,
          {
            name: "moonbase",
            kind: "file",
            canonical_name: "moon",
            aliases: [],
            shadowed: false,
          },
        ],
      }),
      "/domains/eng/engrams/alpha": () =>
        detailResponse("alpha", "Alpha", 5, [
          {
            line: 3,
            rel_type: "depends_on",
            resolved: true,
            target: { domain: "moon", target: "Crater Base" },
          },
        ]),
      "/graph": () => ({
        ...GRAPH,
        nodes: [
          ...GRAPH.nodes,
          {
            id: 4,
            domain: "moonbase",
            permalink: "crater-base",
            title: "Crater Base",
          },
        ],
      }),
      "/domains/moonbase/engrams/crater-base": () =>
        detailResponse("crater-base", "Crater Base", 3),
    });
    const loaded = await loadPlace(client, "eng", "alpha");
    expect(loaded.kind).toBe("place");
    if (loaded.kind !== "place") {
      return;
    }
    expect(loaded.place.relations.map((r) => r.address)).toEqual([
      { domain: "moonbase", permalink: "crater-base" },
    ]);
  });

  it("still builds the room when the graph fails, with no address known", async () => {
    serve({
      "/graph": () => {
        throw new ApiProblem(500, "boom", "graph failed");
      },
    });
    const loaded = await loadPlace(client, "eng", "alpha");
    expect(loaded.kind).toBe("place");
    if (loaded.kind !== "place") {
      return;
    }
    expect(loaded.place.relations).toHaveLength(2);
    expect(loaded.place.relations.every((r) => r.address === null)).toBe(true);
    // Nothing located, so no target was fetched either.
    expect(requested()).not.toContain(BETA_PATH);
    expect(requested()).not.toContain(GAMMA_PATH);
  });

  it("still builds the room when the inbound page fails, with no hatches", async () => {
    serve({
      "/domains/eng/inbound/alpha": () => {
        throw new ApiProblem(500, "boom", "inbound failed");
      },
    });
    const loaded = await loadPlace(client, "eng", "alpha");
    expect(loaded.kind).toBe("place");
    if (loaded.kind === "place") {
      expect(loaded.place.inbound).toEqual([]);
    }
  });

  it.each([
    [404, "missing"],
    [403, "denied"],
    [0, "offline"],
  ] as const)("reads a detail answered %i as %s", async (status, kind) => {
    serve({
      "/domains/eng/engrams/alpha": () => {
        throw new ApiProblem(status, "no", "detail failed");
      },
    });
    await expect(loadPlace(client, "eng", "alpha")).resolves.toEqual({ kind });
  });

  it("rethrows any other detail failure", async () => {
    serve({
      "/domains/eng/engrams/alpha": () => {
        throw new ApiProblem(500, "boom", "detail failed");
      },
    });
    await expect(loadPlace(client, "eng", "alpha")).rejects.toMatchObject({
      status: 500,
    });
  });

  it("gives up on a target that never answers after the timeout", async () => {
    vi.useFakeTimers();
    serve({ [BETA_PATH]: () => new Promise(() => {}) });
    let settled = false;
    const pending = loadPlace(client, "eng", "alpha").then((loaded) => {
      settled = true;
      return loaded;
    });
    await vi.advanceTimersByTimeAsync(TARGET_TIMEOUT_MS - 1);
    expect(settled).toBe(false);
    await vi.advanceTimersByTimeAsync(1);
    const loaded = await pending;
    expect(loaded.kind).toBe("place");
    if (loaded.kind !== "place") {
      return;
    }
    expect(
      loaded.place.relations.map((r) => [
        r.address?.permalink,
        r.targetSalience,
      ]),
    ).toEqual([
      ["notes/beta", null],
      ["gamma", 8],
    ]);
  });

  it("rejects with an AbortError when aborted before the target fetches", async () => {
    const controller = new AbortController();
    serve({
      "/domains/eng/inbound/alpha": () => {
        controller.abort();
        return INBOUND;
      },
    });
    const error: unknown = await loadPlace(
      client,
      "eng",
      "alpha",
      controller.signal,
    ).catch((caught: unknown) => caught);
    expect(error).toBeInstanceOf(DOMException);
    expect((error as DOMException).name).toBe("AbortError");
    expect(requested()).not.toContain(BETA_PATH);
    expect(requested()).not.toContain(GAMMA_PATH);
  });

  it("rejects at once when the signal is already aborted", async () => {
    serve();
    const controller = new AbortController();
    controller.abort();
    await expect(
      loadPlace(client, "eng", "alpha", controller.signal),
    ).rejects.toMatchObject({ name: "AbortError" });
    expect(apiMock).not.toHaveBeenCalled();
  });
  it("still builds the room when the domain listing fails", async () => {
    serve({
      "/domains": () => {
        throw new ApiProblem(500, "boom", "listing failed");
      },
    });
    const loaded = await loadPlace(client, "eng", "alpha");
    expect(loaded.kind).toBe("place");
    if (loaded.kind !== "place") {
      return;
    }
    // A bare link still resolves without the listing.
    expect(loaded.place.links[0]?.address).toEqual({
      domain: "eng",
      permalink: "notes/beta",
    });
  });

  it("reuses a fresh detail on a second load within the stale window", async () => {
    serve();
    await loadPlace(client, "eng", "alpha");
    await loadPlace(client, "eng", "alpha");
    const detailCalls = requested().filter(
      (path) => path === "/domains/eng/engrams/alpha",
    );
    expect(detailCalls).toHaveLength(1);
    expect(requested().filter((path) => path === GAMMA_PATH)).toHaveLength(1);
    expect(GAME_STALE_MS).toBe(30_000);
  });

  it("rejects at once when aborted while the detail still hangs", async () => {
    const controller = new AbortController();
    serve({ "/domains/eng/engrams/alpha": () => new Promise(() => {}) });
    const pending = loadPlace(client, "eng", "alpha", controller.signal);
    await Promise.resolve();
    controller.abort();
    await expect(pending).rejects.toMatchObject({ name: "AbortError" });
  });

  it("rejects with an AbortError when aborted during the target round", async () => {
    const controller = new AbortController();
    serve({
      [GAMMA_PATH]: () => {
        controller.abort();
        return detailResponse("gamma", "Gamma", 8);
      },
    });
    const error: unknown = await loadPlace(
      client,
      "eng",
      "alpha",
      controller.signal,
    ).catch((caught: unknown) => caught);
    expect(error).toBeInstanceOf(DOMException);
    expect((error as DOMException).name).toBe("AbortError");
  });
});
