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
    // Mutation caught: a folder dropped, `rootDeck` read from the visible
    // rows instead of `total`, or the listing's fields misread.
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

  it("aborts a bridge load during the tree and listing round", async () => {
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
});
