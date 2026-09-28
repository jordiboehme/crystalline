/**
 * Loading every station address kind from real data (M3 C22, C23, C29):
 * `loadStation` for the airlock, a bridge, a deck and an engram, and what
 * each failure turns into.
 *
 * Same convention as `source.test.ts`: a real `QueryClient`, only `api`
 * stubbed, so the keys under test are the keys the reading screen and
 * `loadPlace` themselves read - a bridge's MANIFEST fetch shares the
 * engram detail cache, and never asks twice for what `loadPlace` already
 * holds.
 */

import { QueryClient } from "@tanstack/react-query";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { ApiProblem, api } from "../../api/client";
import { engramDetailKey } from "../../api/engram";
import type { Answer } from "../../test/harness";
import { answersFor } from "../../test/harness";
import { loadStation } from "./station";

vi.mock("../../api/client", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../../api/client")>();
  return { ...actual, api: vi.fn(), setCsrfToken: vi.fn() };
});

const apiMock = vi.mocked(api);

/** An engine frontmatter block, decision typed, stable. */
function frontmatter(title: string) {
  return {
    engram_type: "decision",
    title,
    status: "stable",
    tags: [],
    extra: {},
  };
}

/** An engram detail payload, in the engine's own wire shape. */
function detailResponse(
  domain: string,
  permalink: string,
  title: string,
  path: string | null,
) {
  return {
    domain,
    permalink,
    title,
    path,
    url: `crystalline://${domain}/${permalink}`,
    content: `# ${title}\n`,
    checksum: "c0ffee",
    frontmatter: frontmatter(title),
    observations: [],
    relations: [],
    links: [],
    inbound: { count: 0, refs: [] },
  };
}

/** One row of a tree level, in the engine's own wire shape. */
function treeRow(
  permalink: string,
  title = permalink,
  type: string | null = "engram",
  status: string | null = "stable",
) {
  return { permalink, title, type, status };
}

/** One folder level of a domain's tree. */
function treeResponse(
  domain: string,
  path: string,
  folders: string[],
  engrams: ReturnType<typeof treeRow>[],
  opts: { truncated?: boolean; total?: number } = {},
) {
  return {
    domain,
    path,
    folders,
    engrams,
    truncated: opts.truncated ?? false,
    total: opts.total ?? engrams.length,
  };
}

/** One row of the domain listing, in the engine's own wire shape. */
function domainRow(
  name: string,
  overrides: {
    private?: boolean;
    canonicalName?: string | null;
    engrams?: number | null;
  } = {},
) {
  return {
    name,
    kind: "file",
    engrams: overrides.engrams ?? null,
    private: overrides.private ?? false,
    canonical_name: overrides.canonicalName ?? null,
    aliases: [],
  };
}

/** A domain listing payload. */
function listingResponse(rows: ReturnType<typeof domainRow>[]) {
  return { behavior: [], domains: rows };
}

function serve(routes: Record<string, Answer>) {
  apiMock.mockImplementation(answersFor(routes));
}

/** The paths `api` was called with, without their query strings. */
function requested(): string[] {
  return apiMock.mock.calls.map(([path]) => path.split("?")[0] ?? path);
}

let client: QueryClient;

beforeEach(() => {
  apiMock.mockReset();
  client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
});

afterEach(() => {
  client.clear();
});

describe("loadStation: the airlock (M3 C24, C29)", () => {
  it("builds the airlock from the domain listing", async () => {
    // Mutation caught: a name or a private flag dropped or swapped.
    serve({
      "/domains": () =>
        listingResponse([
          domainRow("eng", { private: false }),
          domainRow("cargo", { private: true }),
        ]),
    });
    const loaded = await loadStation(client, { kind: "airlock" });
    expect(loaded.kind).toBe("airlock");
    if (loaded.kind !== "airlock") return;
    expect(loaded.input).toEqual({
      domains: [
        { name: "eng", private: false },
        { name: "cargo", private: true },
      ],
      here: null,
    });
  });

  it("gives the airlock domains: null on any listing failure, never offline (C29)", async () => {
    // Mutation caught: the failure read as `offline` instead of degrading
    // to "cannot tell" (`domains: null`).
    serve({
      "/domains": () => {
        throw new ApiProblem(0, "no", "dropped");
      },
    });
    const loaded = await loadStation(client, { kind: "airlock" });
    expect(loaded).toEqual({
      kind: "airlock",
      input: { domains: null, here: null },
    });
  });
});

describe("loadStation: a bridge (M3 C20, C22, C23, C29)", () => {
  it("builds a bridge from its domain's tree root and MANIFEST", async () => {
    // Mutation caught: a folder dropped, or the listing's fields misread
    // (the count, the private flag or the display name). `rootDeck`'s own
    // total-vs-visible-rows distinction is pinned separately below, where
    // the two readings can actually disagree.
    serve({
      "/domains/eng/tree": () =>
        treeResponse(
          "eng",
          "",
          ["notes", "drafts"],
          [
            treeRow("MANIFEST", "Manifest", "manifest", "stable"),
            treeRow("intro"),
          ],
        ),
      "/domains": () => listingResponse([domainRow("eng", { engrams: 5 })]),
      "/domains/eng/engrams/MANIFEST": () =>
        detailResponse("eng", "MANIFEST", "Manifest", "MANIFEST.md"),
    });
    const loaded = await loadStation(client, { kind: "bridge", domain: "eng" });
    expect(loaded.kind).toBe("bridge");
    if (loaded.kind !== "bridge") return;
    expect(loaded.place.permalink).toBe("MANIFEST");
    expect(loaded.bridge).toEqual({
      domain: "eng",
      display: "eng",
      engrams: 5,
      private: false,
      folders: ["notes", "drafts"],
      rootDeck: true,
    });
  });

  it("reads a MANIFEST that declares another permalink from the tree root (C22)", async () => {
    // Mutation caught: the loader hardcoding the lowercase permalink
    // "manifest" instead of the tree row's own spelling.
    serve({
      "/domains/eng/tree": () =>
        treeResponse("eng", "", [], [treeRow("Manifest", "Manifest")]),
      "/domains": () => listingResponse([domainRow("eng")]),
      "/domains/eng/engrams/Manifest": () =>
        detailResponse("eng", "Manifest", "Manifest", "Manifest.md"),
    });
    const loaded = await loadStation(client, { kind: "bridge", domain: "eng" });
    expect(loaded.kind).toBe("bridge");
    if (loaded.kind !== "bridge") return;
    expect(loaded.place.permalink).toBe("Manifest");
    expect(requested()).toContain("/domains/eng/engrams/Manifest");
    expect(requested()).not.toContain("/domains/eng/engrams/manifest");
  });

  it("gives a domain without a MANIFEST a stand-in place (C23)", async () => {
    // Mutation caught: a 404 on the MANIFEST read as `missing` for the
    // whole bridge instead of the stand-in place.
    serve({
      "/domains/eng/tree": () => treeResponse("eng", "", [], []),
      "/domains": () => listingResponse([domainRow("eng")]),
      "/domains/eng/engrams/manifest": () => {
        throw new ApiProblem(404, "no", "not found");
      },
    });
    const loaded = await loadStation(client, { kind: "bridge", domain: "eng" });
    expect(loaded.kind).toBe("bridge");
    if (loaded.kind !== "bridge") return;
    expect(loaded.place).toEqual({
      domain: "eng",
      permalink: "manifest",
      title: "eng",
      type: "manifest",
      status: null,
      salience: null,
      validFrom: null,
      validTo: null,
      tags: [],
      content: "",
      relations: [],
      links: [],
      inbound: [],
      inboundTotal: 0,
      observations: [],
    });
  });

  it("denies a bridge whose MANIFEST answers 403 (C23)", async () => {
    // Mutation caught: a 403 on the MANIFEST read as the stand-in place
    // instead of `denied`.
    serve({
      "/domains/eng/tree": () =>
        treeResponse("eng", "", [], [treeRow("manifest")]),
      "/domains": () => listingResponse([domainRow("eng")]),
      "/domains/eng/engrams/manifest": () => {
        throw new ApiProblem(403, "no", "forbidden");
      },
    });
    await expect(
      loadStation(client, { kind: "bridge", domain: "eng" }),
    ).resolves.toEqual({ kind: "denied" });
  });

  it("gives no root deck stop for a domain that holds only its MANIFEST (C12)", async () => {
    // Mutation caught: `rootDeck` read from the tree's total alone,
    // without excluding the MANIFEST row from the count.
    serve({
      "/domains/eng/tree": () =>
        treeResponse("eng", "", [], [treeRow("manifest")]),
      "/domains": () => listingResponse([domainRow("eng")]),
      "/domains/eng/engrams/manifest": () =>
        detailResponse("eng", "manifest", "Eng", "manifest.md"),
    });
    const loaded = await loadStation(client, { kind: "bridge", domain: "eng" });
    expect(loaded.kind).toBe("bridge");
    if (loaded.kind !== "bridge") return;
    expect(loaded.bridge.rootDeck).toBe(false);
  });

  it("derives rootDeck from the tree's true total, not the rows it happened to send (M1)", async () => {
    // Mutation caught: `rootDeck` computed from `tree.engrams.length`
    // instead of `tree.total` - a truncated root level (more rows exist
    // than the level sent) would then read as having no root deck even
    // though the level plainly holds more than the MANIFEST.
    serve({
      "/domains/eng/tree": () =>
        treeResponse("eng", "", [], [treeRow("manifest")], {
          truncated: true,
          total: 5,
        }),
      "/domains": () => listingResponse([domainRow("eng")]),
      "/domains/eng/engrams/manifest": () =>
        detailResponse("eng", "manifest", "Eng", "manifest.md"),
    });
    const loaded = await loadStation(client, { kind: "bridge", domain: "eng" });
    expect(loaded.kind).toBe("bridge");
    if (loaded.kind !== "bridge") return;
    expect(loaded.bridge.rootDeck).toBe(true);
  });

  it("marks a bridge's domain private when the listing says so (I2)", async () => {
    // Mutation caught: `priv` read as `false` regardless of the listing's
    // own `private` flag - the key pictogram on the bridge screen (C20)
    // would then never appear for a private domain.
    serve({
      "/domains/eng/tree": () =>
        treeResponse("eng", "", [], [treeRow("manifest")]),
      "/domains": () => listingResponse([domainRow("eng", { private: true })]),
      "/domains/eng/engrams/manifest": () =>
        detailResponse("eng", "manifest", "Eng", "manifest.md"),
    });
    const loaded = await loadStation(client, { kind: "bridge", domain: "eng" });
    expect(loaded.kind).toBe("bridge");
    if (loaded.kind !== "bridge") return;
    expect(loaded.bridge.private).toBe(true);
  });

  // Mutation caught: `faultOf`'s 404 and 403 mapped to the wrong kind (both
  // cases below turn red at once from that one swap).
  it.each([
    [404, "missing"],
    [403, "denied"],
    [0, "offline"],
  ] as const)(
    "reads a tree root answered %i as %s (C29)",
    async (status, kind) => {
      serve({
        "/domains/eng/tree": () => {
          throw new ApiProblem(status, "no", "tree failed");
        },
        "/domains": () => listingResponse([domainRow("eng")]),
      });
      await expect(
        loadStation(client, { kind: "bridge", domain: "eng" }),
      ).resolves.toEqual({ kind });
    },
  );

  it("degrades a bridge's panel on any other tree failure instead of failing the whole bridge", async () => {
    // Mutation caught: any other tree failure rethrown, or the panel kept
    // as if the tree had answered (folders still an array).
    serve({
      "/domains/eng/tree": () => {
        throw new ApiProblem(500, "no", "boom");
      },
      "/domains": () => listingResponse([domainRow("eng")]),
      "/domains/eng/engrams/manifest": () => {
        throw new ApiProblem(404, "no", "not found");
      },
    });
    const loaded = await loadStation(client, { kind: "bridge", domain: "eng" });
    expect(loaded.kind).toBe("bridge");
    if (loaded.kind !== "bridge") return;
    expect(loaded.bridge.folders).toBeNull();
    expect(loaded.bridge.rootDeck).toBe(false);
    expect(requested()).toContain("/domains/eng/engrams/manifest");
  });

  it("gives the bridge a failed-listing default: no count, public, display falls back to the domain key (C23)", async () => {
    // Mutation caught: a failed listing left the bridge without a
    // fallback display name, or carried `private: true` through.
    serve({
      "/domains/eng/tree": () =>
        treeResponse("eng", "", [], [treeRow("manifest")]),
      "/domains": () => {
        throw new ApiProblem(500, "no", "listing failed");
      },
      "/domains/eng/engrams/manifest": () =>
        detailResponse("eng", "manifest", "Eng", "manifest.md"),
    });
    const loaded = await loadStation(client, { kind: "bridge", domain: "eng" });
    expect(loaded.kind).toBe("bridge");
    if (loaded.kind !== "bridge") return;
    expect(loaded.bridge).toMatchObject({
      domain: "eng",
      display: "eng",
      engrams: null,
      private: false,
    });
  });

  it("reads a failed domain listing once, not twice (M2)", async () => {
    // Mutation caught: `loadBridge` fetching the listing itself ahead of
    // `loadPlace`'s own attempt (a second `fetchQuery` call for the same
    // key on a failed listing is not reused - TanStack Query retries a
    // failed fetch rather than treating it as fresh - so the domain
    // listing would be requested twice instead of once).
    const listingCalls = vi.fn(() => {
      throw new ApiProblem(500, "no", "listing failed");
    });
    serve({
      "/domains/eng/tree": () =>
        treeResponse("eng", "", [], [treeRow("manifest")]),
      "/domains": listingCalls,
      "/domains/eng/engrams/manifest": () =>
        detailResponse("eng", "manifest", "Eng", "manifest.md"),
    });
    await loadStation(client, { kind: "bridge", domain: "eng" });
    expect(listingCalls).toHaveBeenCalledTimes(1);
  });

  it("shows the domain's canonical name on the bridge screen, kept apart from its routing key", async () => {
    // Mutation caught: the canonical name written into `domain` (breaking
    // every deck stop's `to.domain`) instead of into `display`.
    serve({
      "/domains/eng/tree": () =>
        treeResponse("eng", "", [], [treeRow("manifest")]),
      "/domains": () =>
        listingResponse([
          domainRow("eng", { canonicalName: "Engineering Bay", engrams: 3 }),
        ]),
      "/domains/eng/engrams/manifest": () =>
        detailResponse("eng", "manifest", "Eng", "manifest.md"),
    });
    const loaded = await loadStation(client, { kind: "bridge", domain: "eng" });
    expect(loaded.kind).toBe("bridge");
    if (loaded.kind !== "bridge") return;
    expect(loaded.bridge.domain).toBe("eng");
    expect(loaded.bridge.display).toBe("Engineering Bay");
  });
});

describe("loadStation: a deck (M3 C8, C11, C12, C29)", () => {
  it("passes a deck's tree level through unchanged, dropping nothing itself (M3 A17)", async () => {
    // Mutation caught: the root MANIFEST row dropped here too (it is
    // `generateDeck`'s job alone), or a field mapped from the wrong key.
    serve({
      "/domains/eng/tree": () =>
        treeResponse(
          "eng",
          "notes",
          ["deep"],
          [
            treeRow("notes/a", "A", "engram", "stable"),
            treeRow("notes/b", "B", null, null),
          ],
          { truncated: true, total: 40 },
        ),
    });
    const loaded = await loadStation(client, {
      kind: "deck",
      domain: "eng",
      folder: "notes",
      section: 2,
    });
    expect(loaded.kind).toBe("deck");
    if (loaded.kind !== "deck") return;
    expect(loaded.section).toBe(2);
    expect(loaded.input).toEqual({
      domain: "eng",
      folder: "notes",
      rows: [
        { permalink: "notes/a", title: "A", type: "engram", status: "stable" },
        { permalink: "notes/b", title: "B", type: null, status: null },
      ],
      subfolders: ["deep"],
      total: 40,
      truncated: true,
    });
  });

  // Mutation caught: `faultOf`'s 404 and 403 mapped to the wrong kind (both
  // cases below turn red at once from that one swap).
  it.each([
    [404, "missing"],
    [403, "denied"],
    [0, "offline"],
  ] as const)(
    "reads a deck's tree level answered %i as %s (C29)",
    async (status, kind) => {
      serve({
        "/domains/eng/tree": () => {
          throw new ApiProblem(status, "no", "tree failed");
        },
      });
      await expect(
        loadStation(client, {
          kind: "deck",
          domain: "eng",
          folder: "notes",
          section: null,
        }),
      ).resolves.toEqual({ kind });
    },
  );

  it("rethrows any other deck tree failure", async () => {
    // Mutation caught: a deck degrading its level like a bridge's tree
    // instead of rethrowing (a deck has no note to degrade to).
    serve({
      "/domains/eng/tree": () => {
        throw new ApiProblem(500, "no", "boom");
      },
    });
    await expect(
      loadStation(client, {
        kind: "deck",
        domain: "eng",
        folder: "notes",
        section: null,
      }),
    ).rejects.toMatchObject({ status: 500 });
  });
});

describe("loadStation: an engram (M3 C28, C29)", () => {
  it("gives an engram its deck folder from its path", async () => {
    // Mutation caught: the folder read from the permalink even when the
    // path is known (they are picked to differ here on purpose).
    serve({
      "/domains/eng/engrams/custom/x": () =>
        detailResponse("eng", "custom/x", "X", "notes/deep/x.md"),
      "/domains": () => listingResponse([domainRow("eng")]),
      "/graph": () => ({ nodes: [], edges: [], truncated: false }),
      "/domains/eng/inbound/custom/x": () => ({
        total: 0,
        page: 1,
        limit: 24,
        count: 0,
        types: [],
        hits: [],
      }),
    });
    const loaded = await loadStation(client, {
      kind: "engram",
      domain: "eng",
      permalink: "custom/x",
    });
    expect(loaded.kind).toBe("engram");
    if (loaded.kind !== "engram") return;
    expect(loaded.folder).toBe("notes/deep");
  });

  it("gives an engram outside any folder the root deck (M3)", async () => {
    // Mutation caught: a root-level path or permalink read as its own
    // one-segment folder instead of the empty root (`folderOfPath`/
    // `folderOfPermalink` cutting at the segment itself rather than at the
    // last `/`, which a root-level path or permalink has none of).
    serve({
      "/domains/eng/engrams/x": () => detailResponse("eng", "x", "X", "x.md"),
      "/domains": () => listingResponse([domainRow("eng")]),
      "/graph": () => ({ nodes: [], edges: [], truncated: false }),
      "/domains/eng/inbound/x": () => ({
        total: 0,
        page: 1,
        limit: 24,
        count: 0,
        types: [],
        hits: [],
      }),
    });
    const loaded = await loadStation(client, {
      kind: "engram",
      domain: "eng",
      permalink: "x",
    });
    expect(loaded.kind).toBe("engram");
    if (loaded.kind !== "engram") return;
    expect(loaded.folder).toBe("");
  });

  it("gives an engram its deck folder from its permalink in a virtual domain, when it has no path", async () => {
    // Mutation caught: `folderOfPath` called on a null path instead of
    // falling back to `folderOfPermalink`.
    serve({
      "/domains/eng/engrams/custom/x": () =>
        detailResponse("eng", "custom/x", "X", null),
      "/domains": () => listingResponse([domainRow("eng")]),
      "/graph": () => ({ nodes: [], edges: [], truncated: false }),
      "/domains/eng/inbound/custom/x": () => ({
        total: 0,
        page: 1,
        limit: 24,
        count: 0,
        types: [],
        hits: [],
      }),
    });
    const loaded = await loadStation(client, {
      kind: "engram",
      domain: "eng",
      permalink: "custom/x",
    });
    expect(loaded.kind).toBe("engram");
    if (loaded.kind !== "engram") return;
    expect(loaded.folder).toBe("custom");
  });

  it("reads the folder back from the cache instead of fetching the detail twice", async () => {
    // Mutation caught: a second detail fetch of its own for the folder.
    const detailCalls = vi.fn(() =>
      detailResponse("eng", "alpha", "Alpha", "alpha.md"),
    );
    serve({
      "/domains/eng/engrams/alpha": detailCalls,
      "/domains": () => listingResponse([domainRow("eng")]),
      "/graph": () => ({ nodes: [], edges: [], truncated: false }),
      "/domains/eng/inbound/alpha": () => ({
        total: 0,
        page: 1,
        limit: 24,
        count: 0,
        types: [],
        hits: [],
      }),
    });
    await loadStation(client, {
      kind: "engram",
      domain: "eng",
      permalink: "alpha",
    });
    expect(detailCalls).toHaveBeenCalledTimes(1);
    expect(client.getQueryData(engramDetailKey("eng", "alpha"))).toMatchObject({
      permalink: "alpha",
    });
  });

  // Mutation caught: `loadEngram` not passing `loadPlace`'s own
  // missing/denied/offline result straight through (a mapping of its own
  // that got the codes wrong, or that swallowed the distinction).
  it.each([
    [404, "missing"],
    [403, "denied"],
    [0, "offline"],
  ] as const)(
    "reads an engram detail answered %i as %s",
    async (status, kind) => {
      serve({
        "/domains/eng/engrams/alpha": () => {
          throw new ApiProblem(status, "no", "detail failed");
        },
      });
      await expect(
        loadStation(client, {
          kind: "engram",
          domain: "eng",
          permalink: "alpha",
        }),
      ).resolves.toEqual({ kind });
    },
  );
});

describe("loadStation: cancellation (M3 C29)", () => {
  it("rejects at once when the signal is already aborted, for every address kind", async () => {
    // Mutation caught: `checkAborted` dropped from `loadStation`'s entry
    // point (or from any one loader's first line), so a signal that had
    // already fired before the call was made would fall through to the
    // first fetch instead of rejecting before any request is made.
    serve({});
    const controller = new AbortController();
    controller.abort();
    const addresses = [
      { kind: "airlock" } as const,
      { kind: "bridge", domain: "eng" } as const,
      { kind: "deck", domain: "eng", folder: "", section: null } as const,
      { kind: "engram", domain: "eng", permalink: "alpha" } as const,
    ];
    for (const address of addresses) {
      await expect(
        loadStation(client, address, controller.signal),
      ).rejects.toMatchObject({ name: "AbortError" });
    }
    expect(apiMock).not.toHaveBeenCalled();
  });

  it("aborts a bridge load during the tree fetch", async () => {
    // Mutation caught: the trailing `checkAborted` after the tree fetch
    // dropped, so a signal that fired while the tree was still in flight
    // would go unnoticed and the load would carry on to the MANIFEST fetch.
    const controller = new AbortController();
    serve({
      "/domains/eng/tree": () => {
        controller.abort();
        return treeResponse("eng", "", [], [treeRow("manifest")]);
      },
      "/domains": () => listingResponse([domainRow("eng")]),
    });
    await expect(
      loadStation(client, { kind: "bridge", domain: "eng" }, controller.signal),
    ).rejects.toMatchObject({ name: "AbortError" });
    expect(requested()).not.toContain("/domains/eng/engrams/manifest");
  });

  it("aborts a bridge load during the MANIFEST fetch", async () => {
    // Mutation caught: the trailing `checkAborted` after `loadPlace`
    // dropped, so a signal that fired while the MANIFEST was still loading
    // would go unnoticed and the load would resolve with a stale bridge.
    const controller = new AbortController();
    serve({
      "/domains/eng/tree": () =>
        treeResponse("eng", "", [], [treeRow("manifest")]),
      "/domains": () => listingResponse([domainRow("eng")]),
      "/domains/eng/engrams/manifest": () => {
        controller.abort();
        return detailResponse("eng", "manifest", "Eng", "manifest.md");
      },
    });
    await expect(
      loadStation(client, { kind: "bridge", domain: "eng" }, controller.signal),
    ).rejects.toMatchObject({ name: "AbortError" });
  });

  it("aborts a deck load during the tree fetch", async () => {
    // Mutation caught: the trailing `checkAborted` after the tree fetch
    // dropped, so a signal that fired while the tree was still in flight
    // would go unnoticed and the load would resolve with a stale deck.
    const controller = new AbortController();
    serve({
      "/domains/eng/tree": () => {
        controller.abort();
        return treeResponse("eng", "notes", [], []);
      },
    });
    await expect(
      loadStation(
        client,
        { kind: "deck", domain: "eng", folder: "notes", section: null },
        controller.signal,
      ),
    ).rejects.toMatchObject({ name: "AbortError" });
  });

  it("aborts an airlock load during the listing fetch", async () => {
    // Mutation caught: the trailing `checkAborted` after the listing fetch
    // dropped, so a signal that fired while the listing was still loading
    // would go unnoticed and the load would resolve with a stale airlock.
    const controller = new AbortController();
    serve({
      "/domains": () => {
        controller.abort();
        return listingResponse([domainRow("eng")]);
      },
    });
    await expect(
      loadStation(client, { kind: "airlock" }, controller.signal),
    ).rejects.toMatchObject({ name: "AbortError" });
  });

  it("aborts an engram load during the detail fetch", async () => {
    // Mutation caught: `loadEngram` swallowing `loadPlace`'s own
    // `AbortError` (a `.catch` that maps every rejection to a fault kind,
    // say) instead of letting it propagate.
    const controller = new AbortController();
    serve({
      "/domains/eng/engrams/alpha": () => {
        controller.abort();
        return detailResponse("eng", "alpha", "Alpha", "alpha.md");
      },
    });
    await expect(
      loadStation(
        client,
        { kind: "engram", domain: "eng", permalink: "alpha" },
        controller.signal,
      ),
    ).rejects.toMatchObject({ name: "AbortError" });
  });

  it("rejects at once, rather than waiting forever, while the airlock's listing hangs (I3)", async () => {
    // Mutation caught: the `abortable` race dropped from `loadAirlock`
    // (`await client.fetchQuery(domainsQuery()).catch(...)` with no race
    // against the signal) - a request that never settles would then hang
    // the whole call past the point the player gave up, instead of
    // rejecting the moment the signal fires.
    const controller = new AbortController();
    serve({ "/domains": () => new Promise(() => {}) });
    const pending = loadStation(client, { kind: "airlock" }, controller.signal);
    await Promise.resolve();
    controller.abort();
    await expect(pending).rejects.toMatchObject({ name: "AbortError" });
  });

  it("rejects at once, rather than waiting forever, while a bridge's tree hangs (I3)", async () => {
    // Mutation caught: the `abortable` race dropped from the tree fetch in
    // `loadBridge` - a tree request that never settles would then hang the
    // whole bridge load past the point the player gave up.
    const controller = new AbortController();
    serve({ "/domains/eng/tree": () => new Promise(() => {}) });
    const pending = loadStation(
      client,
      { kind: "bridge", domain: "eng" },
      controller.signal,
    );
    await Promise.resolve();
    controller.abort();
    await expect(pending).rejects.toMatchObject({ name: "AbortError" });
  });

  it("rejects at once, rather than waiting forever, while a deck's tree hangs (I3)", async () => {
    // Mutation caught: the `abortable` race dropped from the tree fetch in
    // `loadDeck` - a tree request that never settles would then hang the
    // whole deck load past the point the player gave up.
    const controller = new AbortController();
    serve({ "/domains/eng/tree": () => new Promise(() => {}) });
    const pending = loadStation(
      client,
      { kind: "deck", domain: "eng", folder: "notes", section: null },
      controller.signal,
    );
    await Promise.resolve();
    controller.abort();
    await expect(pending).rejects.toMatchObject({ name: "AbortError" });
  });

  it("rejects at once, rather than waiting forever, while an engram's detail hangs (I3)", async () => {
    // Mutation caught: `loadPlace`'s own `abortable` race dropped from its
    // detail fetch - a detail request that never settles would then hang
    // the whole engram load past the point the player gave up.
    const controller = new AbortController();
    serve({ "/domains/eng/engrams/alpha": () => new Promise(() => {}) });
    const pending = loadStation(
      client,
      { kind: "engram", domain: "eng", permalink: "alpha" },
      controller.signal,
    );
    await Promise.resolve();
    controller.abort();
    await expect(pending).rejects.toMatchObject({ name: "AbortError" });
  });
});
