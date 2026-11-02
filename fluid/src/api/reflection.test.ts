import { QueryClient } from "@tanstack/react-query";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { API_BASE, api } from "./client";
import {
  changedVersions,
  deleteReflectionAsset,
  discloseReflectionSection,
  fetchReflectionAssets,
  moveReflection,
  moveReflectionAsset,
  reflectionAssetHome,
  reflectionAssetsKey,
  reflectionAssetUrl,
  replaceReflectionAsset,
  uploadReflectionAsset,
  openThread,
  readDoc,
  readQueue,
  readSummary,
  readThread,
  reflectionReviewKey,
  reflectionSummaryKey,
  readTree,
  saveReflection,
  searchReflections,
  stepThread,
} from "./reflection";
import {
  reflectionEditRoute,
  reflectionPageRoute,
  reflectionReviewRoute,
} from "../paths";

vi.mock("./client", async (importOriginal) => {
  const actual = await importOriginal<typeof import("./client")>();
  return { ...actual, api: vi.fn() };
});
const apiMock = vi.mocked(api);

const THREAD = {
  id: "t-3f9a2c1b",
  path: "guides/deploying.md",
  anchor: {
    section: "7f3a9c2e",
    quote: "Deploys run at night",
    occurrence: 1,
    state: "found",
  },
  status: "open",
  waiting_on: "agent",
  opened_at: "2026-09-30T10:00:00Z",
  age_secs: 3600,
  entries: [
    {
      at: "2026-09-30T10:00:00Z",
      kind: "comment",
      author: { name: "jordi", agent: false },
      text: "Still true?",
    },
  ],
};

beforeEach(() => {
  apiMock.mockReset();
});

describe("readers", () => {
  it("reads a tree, folders and pages, and the pact apart from them", () => {
    const tree = readTree({
      domain: "eng",
      pact: {
        path: "PACT.md",
        title: "How we write",
        exists: true,
        open_threads: 1,
      },
      nodes: [
        {
          kind: "folder",
          name: "guides",
          path: "guides",
          children: [
            {
              kind: "reflection",
              name: "deploying.md",
              path: "guides/deploying.md",
              title: "Deploying",
              open_threads: 2,
              sections: 4,
            },
          ],
        },
        { kind: "mystery", path: "x" },
      ],
    });
    expect(tree.pact).toEqual({
      path: "PACT.md",
      title: "How we write",
      exists: true,
      openThreads: 1,
    });
    expect(tree.nodes).toHaveLength(1);
    expect(tree.nodes[0]).toMatchObject({ kind: "folder", path: "guides" });
  });

  it("reads a source of another domain and a count of hidden ones", () => {
    const doc = readDoc({
      domain: "eng",
      path: "a.md",
      title: "A",
      markdown: "# A\n",
      checksum: "abc",
      sections: [
        {
          id: "7f3a9c2e",
          heading: "Team",
          line: 3,
          changed: false,
          sources: [
            {
              permalink: "team/x",
              title: "X",
              state: "unavailable",
              domain: "harbor",
            },
          ],
          hidden_sources: 2,
        },
      ],
      threads: [],
    });
    expect(doc.sections[0]?.sources).toEqual([
      {
        permalink: "team/x",
        title: "X",
        state: "unavailable",
        domain: "harbor",
      },
    ]);
    expect(doc.sections[0]?.hiddenSources).toBe(2);
  });

  it("keeps absent sources absent, never empty", () => {
    const doc = readDoc({
      domain: "eng",
      path: "guides/deploying.md",
      title: "Deploying",
      markdown: "# Deploying\n",
      checksum: "abc",
      sections: [
        { id: "7f3a9c2e", heading: "Night deploys", line: 3, changed: false },
        {
          id: "0b1c2d3e",
          heading: "Other",
          line: 9,
          changed: true,
          sources: [
            {
              permalink: "runbooks/deploy",
              title: "Deploy runbook",
              state: "changed",
            },
          ],
        },
      ],
      threads: [THREAD],
    });
    expect(doc.sections[0]?.sources).toBeNull();
    expect(doc.sections[1]?.sources).toEqual([
      {
        permalink: "runbooks/deploy",
        title: "Deploy runbook",
        state: "changed",
        domain: null,
      },
    ]);
    expect(doc.threads[0]?.waitingOn).toBe("agent");
    expect(doc.livePresent).toEqual([]);
  });

  it("reads an unknown anchor state as text_changed, which never paints", () => {
    expect(
      readThread({ ...THREAD, anchor: { ...THREAD.anchor, state: "odd" } })
        ?.anchor.state,
    ).toBe("text_changed");
  });

  it("reads a queue and keeps an unreflected item's count without its engrams", () => {
    const queue = readQueue({
      total: 2,
      kinds: { feedback: 1, update: 1, review: 0 },
      queue: [
        {
          n: 1,
          kind: "feedback",
          what: "changed",
          path: "a.md",
          section: "7f3a9c2e",
          heading: "A",
          age_secs: 10,
          changed_by: ["ann"],
          diff: "@@\n-x\n+y\n",
        },
        {
          n: 2,
          kind: "update",
          what: "unreflected",
          path: "",
          age_secs: 5,
          count: 12,
        },
      ],
      guidance: "Work the oldest first.",
    });
    expect(queue.items[0]).toMatchObject({
      kind: "feedback",
      changedBy: ["ann"],
      sources: null,
    });
    expect(queue.items[1]).toMatchObject({
      what: "unreflected",
      count: 12,
      engrams: [],
    });
  });
});

describe("calls", () => {
  it("saves with the checksum it read and asks for a create when told", async () => {
    apiMock.mockResolvedValue({
      checksum: "new",
      created: true,
      ids_assigned: [],
    });
    await saveReflection("eng", "guides/new page.md", "# New\n", {
      create: true,
    });
    expect(apiMock).toHaveBeenCalledWith(
      "/domains/eng/reflections/guides/new%20page.md",
      {
        method: "PUT",
        body: JSON.stringify({ markdown: "# New\n", create: true }),
      },
    );
  });

  it("opens a thread with its quote and occurrence, and steps it by action", async () => {
    apiMock.mockResolvedValue(THREAD);
    await openThread("eng", {
      path: "guides/deploying.md",
      section: "7f3a9c2e",
      quote: "Deploys run at night",
      occurrence: 2,
      text: "Still true?",
    });
    expect(apiMock).toHaveBeenLastCalledWith(
      "/domains/eng/reflection/threads",
      {
        method: "POST",
        body: JSON.stringify({
          path: "guides/deploying.md",
          section: "7f3a9c2e",
          quote: "Deploys run at night",
          occurrence: 2,
          text: "Still true?",
        }),
      },
    );
    await stepThread("eng", "t-3f9a2c1b", "resolve", {
      path: "guides/deploying.md",
      outcome: "Yes.",
    });
    expect(apiMock).toHaveBeenLastCalledWith(
      "/domains/eng/reflection/threads/t-3f9a2c1b/resolve",
      {
        method: "POST",
        body: JSON.stringify({ path: "guides/deploying.md", outcome: "Yes." }),
      },
    );
  });

  it("searches with the mode asked for and reads which mode ran", async () => {
    apiMock.mockResolvedValue({
      domain: "eng",
      query: "night",
      mode: "text",
      total: 21,
      page: 2,
      limit: 20,
      count: 1,
      hits: [
        {
          path: "guides/deploying.md",
          title: "Deploying",
          section: "_top",
          heading: "",
          snippet: "Short intro.",
        },
      ],
    });
    const found = await searchReflections("eng", {
      query: "night",
      mode: "semantic",
      page: 2,
    });
    expect(apiMock).toHaveBeenCalledWith(
      "/domains/eng/reflection/search?q=night&limit=20&search_type=semantic&page=2",
    );
    expect(found).toMatchObject({
      mode: "text",
      total: 21,
      page: 2,
      limit: 20,
    });
    expect(found.hits[0]).toEqual({
      path: "guides/deploying.md",
      title: "Deploying",
      section: "_top",
      heading: "",
      snippet: "Short intro.",
    });
    // The default mode and the first page are left out of the address.
    await searchReflections("eng", { query: "night" });
    expect(apiMock).toHaveBeenLastCalledWith(
      "/domains/eng/reflection/search?q=night&limit=20",
    );
  });

  it("shares a section's text on purpose and reads who confirmed it", async () => {
    apiMock.mockResolvedValue({
      domain: "eng",
      path: "overview.md",
      section: "7f3a9c2e",
      disclosed: { by: "keeper", at: "2026-10-28T09:00:00Z" },
    });
    expect(
      await discloseReflectionSection("eng", "overview.md", "7f3a9c2e", "b1"),
    ).toEqual({ by: "keeper", at: "2026-10-28T09:00:00Z" });
    expect(apiMock).toHaveBeenCalledWith("/domains/eng/reflection/disclose", {
      method: "POST",
      body: JSON.stringify({
        path: "overview.md",
        section: "7f3a9c2e",
        basis: "b1",
      }),
    });
  });

  it("reads the question a too wide item carries", () => {
    const queue = readQueue({
      total: 1,
      kinds: { feedback: 0, update: 1, review: 0 },
      queue: [
        {
          n: 1,
          kind: "update",
          what: "source_too_wide",
          path: "overview.md",
          section: "7f3a9c2e",
          heading: "Team",
          age_secs: 1,
          question: "Share text from harbor on a page more people may read?",
          basis: "b1",
        },
      ],
    });
    expect(queue.items[0]?.question).toBe(
      "Share text from harbor on a page more people may read?",
    );
    expect(queue.items[0]?.basis).toBe("b1");
  });

  it("reads the pages a move could not relink", async () => {
    apiMock.mockResolvedValue({
      from: "a.md",
      to: "b.md",
      links_rewritten: 1,
      threads_moved: true,
      links_failed: [{ path: "c.md", reason: "it is open in a room" }],
    });
    expect(await moveReflection("eng", "a.md", "b.md")).toEqual({
      to: "b.md",
      linksFailed: [{ path: "c.md", reason: "it is open in a room" }],
    });
  });
});

it("builds the Reflection addresses with the path's slashes kept", () => {
  expect(reflectionPageRoute("eng", "guides/new page.md")).toBe(
    "/d/eng/reflection/guides/new%20page.md",
  );
  expect(reflectionEditRoute("eng", "PACT.md")).toBe(
    "/d/eng/reflection-edit/PACT.md",
  );
  expect(reflectionReviewRoute("eng")).toBe("/d/eng/reflection/review");
});

describe("more reads", () => {
  it("reads who is in the live room", () => {
    expect(
      readDoc({ path: "a.md", live: { present: ["Ada"] } }).livePresent,
    ).toEqual(["Ada"]);
  });

  it("reads a save that landed live, with who is present", async () => {
    apiMock.mockResolvedValue({
      checksum: "c",
      created: false,
      ids_assigned: [],
      curated: true,
      landed: "live",
      present: ["Ada"],
    });
    expect(await saveReflection("eng", "a.md", "# A\n")).toEqual({
      checksum: "c",
      created: false,
      idsAssigned: [],
      curated: true,
      landedLive: true,
      present: ["Ada"],
      room: null,
      roomNote: null,
    });
  });

  it("reads the room note of a save that is on disk only", async () => {
    apiMock.mockResolvedValue({
      checksum: "c",
      created: false,
      ids_assigned: [],
      curated: false,
      landed: "file",
      present: ["Ada"],
      room: "conflict",
      room_note: "The room will ask.",
    });
    expect(await saveReflection("eng", "a.md", "# A\n")).toMatchObject({
      landedLive: false,
      room: "conflict",
      roomNote: "The room will ask.",
    });
  });
});

describe("a Reflection's files", () => {
  it("the asset calls use the wildcard under reflections/assets/", async () => {
    apiMock.mockResolvedValue({
      path: "assets/2026/10/flow.png",
      mime: "image/png",
      size: 3,
      sha256: "ab",
      created: true,
    });
    const file = new Blob(["abc"], { type: "image/png" });
    expect(
      await uploadReflectionAsset("eng", "assets/2026/10/flow.png", file),
    ).toEqual({
      path: "assets/2026/10/flow.png",
      mime: "image/png",
      size: 3,
      sha256: "ab",
      created: true,
    });
    // A new upload only creates: the server refuses a taken path with 412.
    expect(apiMock).toHaveBeenCalledWith(
      "/domains/eng/reflection/assets/2026/10/flow.png",
      {
        method: "PUT",
        headers: { "Content-Type": "image/png", "If-None-Match": "*" },
        body: file,
      },
    );

    apiMock.mockResolvedValue(undefined);
    await deleteReflectionAsset("eng", "assets/a b.png");
    expect(apiMock).toHaveBeenLastCalledWith(
      "/domains/eng/reflection/assets/a%20b.png",
      { method: "DELETE" },
    );

    apiMock.mockResolvedValue({
      from: "assets/a.png",
      to: "assets/b.png",
      links_rewritten: 2,
    });
    await moveReflectionAsset("eng", "assets/a.png", "assets/b.png");
    expect(apiMock).toHaveBeenLastCalledWith(
      "/domains/eng/reflection/move-asset",
      {
        method: "POST",
        body: JSON.stringify({ from: "assets/a.png", to: "assets/b.png" }),
      },
    );

    apiMock.mockResolvedValue({
      assets: [
        {
          path: "assets/a.png",
          mime: "image/png",
          size: 1,
          modified: "2026-10-01T00:00:00Z",
          sha256: "x",
        },
        { size: 2 },
      ],
    });
    expect(await fetchReflectionAssets("eng")).toEqual([
      {
        path: "assets/a.png",
        mime: "image/png",
        size: 1,
        modified: "2026-10-01T00:00:00Z",
        sha256: "x",
      },
    ]);
    expect(apiMock).toHaveBeenLastCalledWith("/domains/eng/reflection/assets");
  });

  it("names the listing under the domain's Reflection key", () => {
    expect(reflectionAssetsKey("eng")).toEqual(["reflection", "eng", "assets"]);
    expect(reflectionAssetUrl("eng", "assets/x/y.png")).toBe(
      `${API_BASE}/domains/eng/reflection/assets/x/y.png`,
    );
  });

  it("a move receipt keeps the server's note and failed links as sent", async () => {
    apiMock.mockResolvedValue({
      domain: "eng",
      from: "assets/a.png",
      to: "assets/b.png",
      links_rewritten: 1,
      links_failed: [{ path: "c.md", reason: "it is open in a room" }],
      note: "The file moved; its index row follows at the next sync.",
    });
    expect(
      await moveReflectionAsset("eng", "assets/a.png", "assets/b.png"),
    ).toEqual({
      from: "assets/a.png",
      to: "assets/b.png",
      linksRewritten: 1,
      linksFailed: [{ path: "c.md", reason: "it is open in a room" }],
      note: "The file moved; its index row follows at the next sync.",
    });
    apiMock.mockResolvedValue({ from: "assets/a.png", to: "assets/b.png" });
    expect(
      await moveReflectionAsset("eng", "assets/a.png", "assets/b.png"),
    ).toEqual({
      from: "assets/a.png",
      to: "assets/b.png",
      linksRewritten: 0,
      linksFailed: [],
      note: null,
    });
  });

  it("the Reflection's home links from the page and refuses engram files", () => {
    const home = reflectionAssetHome("eng", "guides/a.md");
    expect(home.key).toEqual(reflectionAssetsKey("eng"));
    expect(home.linkTo("assets/x.png")).toBe("../assets/x.png");
    expect(home.resolve("../assets/x.png#left")).toBe("assets/x.png");
    expect(home.resolve("../../assets/x.png")).toBeNull();
    expect(home.resolve("peer.md")).toBeNull();
    expect(home.refused("../../assets/x.png")).toBe(
      "This file is not in the Reflection. A curator or an agent copies it into the Reflection's files.",
    );
    expect(home.refused("../assets/x.png")).toBeNull();
    expect(home.url("assets/x.png")).toMatch(/\/reflection\/assets\/x\.png$/);
  });
});

describe("replacing a Reflection file", () => {
  it("puts the new bytes to the same path with no precondition", async () => {
    apiMock.mockResolvedValue({
      path: "assets/flow.png",
      mime: "image/png",
      size: 3,
      sha256: "cd",
      created: false,
    });
    const file = new Blob(["abc"], { type: "image/png" });
    const home = reflectionAssetHome("eng", "guides/deploying.md");
    expect(await home.replace?.("assets/flow.png", file)).toEqual({
      path: "assets/flow.png",
      mime: "image/png",
      size: 3,
      sha256: "cd",
      created: false,
    });
    // Only the content type: a create-only header on new uploads must never
    // reach a replace, which overwrites on purpose.
    expect(apiMock).toHaveBeenLastCalledWith(
      "/domains/eng/reflection/assets/flow.png",
      {
        method: "PUT",
        headers: { "Content-Type": "image/png" },
        body: file,
      },
    );
    expect(
      await replaceReflectionAsset("eng", "assets/flow.png", file),
    ).toMatchObject({ path: "assets/flow.png" });
  });
});

describe("a Reflection file's address carries its version", () => {
  it("only the URL gets the version, never a link or a resolved path", () => {
    const home = reflectionAssetHome(
      "eng",
      "guides/deploying.md",
      new Map([["assets/flow.png", "0123456789abcdef0123"]]),
    );
    expect(home.url("assets/flow.png")).toBe(
      `${API_BASE}/domains/eng/reflection/assets/flow.png?v=0123456789ab`,
    );
    // A file the listing does not know is addressed plainly.
    expect(home.url("assets/new.png")).toBe(
      `${API_BASE}/domains/eng/reflection/assets/new.png`,
    );
    expect(home.linkTo("assets/flow.png")).toBe("../assets/flow.png");
    expect(home.resolve("../assets/flow.png")).toBe("assets/flow.png");
  });
});

describe("which Reflection files changed since this document first saw them", () => {
  function row(path: string, sha256: string) {
    return { path, mime: "image/png", size: 1, modified: "", sha256 };
  }

  it("names nothing at first and a file once its bytes differ", () => {
    const client = new QueryClient();
    expect(
      changedVersions(client, "eng", [
        row("assets/a.png", "1"),
        row("assets/b.png", "1"),
      ]),
    ).toEqual(new Map());
    expect(changedVersions(client, "eng", undefined)).toEqual(new Map());
    expect(
      changedVersions(client, "eng", [
        row("assets/a.png", "2"),
        row("assets/b.png", "1"),
      ]),
    ).toEqual(new Map([["assets/a.png", "2"]]));
    // The baseline stays the first sha: back to it is no change.
    expect(changedVersions(client, "eng", [row("assets/a.png", "1")])).toEqual(
      new Map(),
    );
    // A file first seen later takes its own baseline then.
    expect(changedVersions(client, "eng", [row("assets/c.png", "9")])).toEqual(
      new Map(),
    );
  });

  it("keeps domains and documents apart", () => {
    const client = new QueryClient();
    changedVersions(client, "eng", [row("assets/a.png", "1")]);
    expect(changedVersions(client, "ops", [row("assets/a.png", "2")])).toEqual(
      new Map(),
    );
    expect(
      changedVersions(new QueryClient(), "eng", [row("assets/a.png", "2")]),
    ).toEqual(new Map());
  });
});

describe("readSummary", () => {
  it("reads the counts and the threads by page, people included", () => {
    const summary = readSummary({
      domain: "eng",
      waiting_for_the_agent: 2,
      waiting_for_people: 1,
      pages_to_update: 4,
      open_by_page: [
        {
          path: "guides/deploying.md",
          title: "Deploying",
          threads: [
            {
              id: "t-1",
              quote: null,
              waiting_on: "people",
              opened_at: "2026-10-01T10:00:00Z",
              people: [
                { name: "ann", agent: false },
                { name: "claude-code", agent: true },
              ],
            },
            { quote: "no id, dropped" },
          ],
        },
        { title: "no path, dropped" },
      ],
    });
    expect(summary).toEqual({
      waitingForAgent: 2,
      waitingForPeople: 1,
      pagesToUpdate: 4,
      openByPage: [
        {
          path: "guides/deploying.md",
          title: "Deploying",
          threads: [
            {
              id: "t-1",
              quote: null,
              waitingOn: "people",
              openedAt: "2026-10-01T10:00:00Z",
              people: [
                { name: "ann", agent: false },
                { name: "claude-code", agent: true },
              ],
            },
          ],
        },
      ],
    });
  });

  it("reads an empty answer as nothing waiting", () => {
    expect(readSummary({})).toEqual({
      waitingForAgent: 0,
      waitingForPeople: 0,
      pagesToUpdate: 0,
      openByPage: [],
    });
  });

  it("keys the summary under the review, so one invalidation refreshes both", () => {
    const review = reflectionReviewKey("eng");
    expect(reflectionSummaryKey("eng").slice(0, review.length)).toEqual(review);
  });
});
