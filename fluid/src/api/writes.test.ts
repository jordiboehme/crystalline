import { describe, expect, it, vi } from "vitest";

import { ApiProblem, api } from "./client";
import {
  conflictOf,
  createEngram,
  deleteEngram,
  moveEngram,
  retireEngram,
  saveEngram,
  validateDocument,
} from "./writes";

vi.mock("./client", async (importOriginal) => {
  const actual = await importOriginal<typeof import("./client")>();
  return { ...actual, api: vi.fn() };
});

const apiMock = vi.mocked(api);

const DETAIL = {
  domain: "eng",
  permalink: "alpha",
  title: "Alpha",
  content: "---\ntitle: Alpha\n---\n\nBody.\n",
  checksum: "abc123",
};

describe("the engram writes module", () => {
  it("saves with a quoted If-Match and reads the detail back", async () => {
    apiMock.mockResolvedValueOnce(DETAIL);
    const saved = await saveEngram("eng", "notes/alpha", "text", "abc123");
    expect(apiMock).toHaveBeenLastCalledWith(
      "/domains/eng/engrams/notes/alpha",
      expect.objectContaining({
        method: "PUT",
        headers: { "If-Match": '"abc123"' },
        body: JSON.stringify({ content: "text" }),
      }),
    );
    expect(saved.checksum).toBe("abc123");
  });

  it("follows a rename: the returned detail carries the new permalink", async () => {
    apiMock.mockResolvedValueOnce({ ...DETAIL, permalink: "renamed" });
    const saved = await saveEngram("eng", "alpha", "text", "abc123");
    expect(saved.permalink).toBe("renamed");
  });

  it("creates, retires, moves, deletes and validates on their routes", async () => {
    apiMock.mockResolvedValueOnce(DETAIL);
    await createEngram("eng", { title: "Alpha", content: "Body." });
    expect(apiMock).toHaveBeenLastCalledWith(
      "/domains/eng/engrams",
      expect.objectContaining({ method: "POST" }),
    );

    apiMock.mockResolvedValueOnce({
      domain: "eng",
      permalink: "alpha",
      status: "superseded",
      successor: "beta",
    });
    const retired = await retireEngram("eng", {
      permalink: "alpha",
      status: "superseded",
      successor: "beta",
    });
    expect(apiMock).toHaveBeenLastCalledWith(
      "/domains/eng/retire",
      expect.objectContaining({ method: "POST" }),
    );
    expect(retired).toEqual({
      permalink: "alpha",
      status: "superseded",
      successor: "beta",
    });

    apiMock.mockResolvedValueOnce({
      from: { domain: "eng", permalink: "alpha", path: "alpha.md" },
      to: { domain: "eng", path: "guides/alpha.md" },
      cross_domain: false,
      links_rewritten: 2,
    });
    const moved = await moveEngram("eng", {
      permalink: "alpha",
      destination: "guides/alpha",
    });
    expect(moved).toEqual({
      domain: "eng",
      permalink: "guides/alpha",
      crossDomain: false,
      linksRewritten: 2,
      attachmentWarnings: [],
    });

    apiMock.mockResolvedValueOnce(undefined);
    await deleteEngram("eng", "alpha", "abc123");
    expect(apiMock).toHaveBeenLastCalledWith(
      "/domains/eng/engrams/alpha",
      expect.objectContaining({
        method: "DELETE",
        headers: { "If-Match": '"abc123"' },
      }),
    );

    apiMock.mockResolvedValueOnce({ findings: [], errors: 0 });
    const report = await validateDocument({ content: "---\nt: x\n---\n" });
    expect(apiMock).toHaveBeenLastCalledWith(
      "/validate",
      expect.objectContaining({ method: "POST" }),
    );
    expect(report.errors).toBe(0);
  });

  it("carries a move's attachment warnings, and answers none for a server that sends none", async () => {
    apiMock.mockResolvedValueOnce({
      from: { domain: "eng", permalink: "alpha", path: "alpha.md" },
      to: { domain: "other", path: "guides/alpha.md" },
      cross_domain: true,
      links_rewritten: 0,
      // The stray number is the point of reading rather than casting: a shape
      // this app did not expect drops out instead of reaching a list renderer.
      attachment_warnings: [
        "assets/2026/08/shot.png did not follow the move",
        7,
      ],
    });
    const warned = await moveEngram("eng", {
      permalink: "alpha",
      destination: "guides/alpha",
      destination_domain: "other",
    });
    expect(warned.attachmentWarnings).toEqual([
      "assets/2026/08/shot.png did not follow the move",
    ]);

    // An older daemon does not send the key at all; a clean move is the answer
    // rather than a crash three components deep.
    apiMock.mockResolvedValueOnce({
      to: { domain: "eng", path: "guides/alpha.md" },
      cross_domain: false,
      links_rewritten: 0,
    });
    const quiet = await moveEngram("eng", {
      permalink: "alpha",
      destination: "guides/alpha",
    });
    expect(quiet.attachmentWarnings).toEqual([]);
  });

  it("reads a 412 into a SaveConflict and nothing else into one", () => {
    const conflict = conflictOf(
      new ApiProblem(412, "precondition failed", "stale edit: changed", {
        current_etag: '"def456"',
        current_content: "theirs",
      }),
    );
    expect(conflict).toEqual({
      currentChecksum: "def456",
      currentContent: "theirs",
      detail: "stale edit: changed",
    });
    expect(conflictOf(new ApiProblem(409, "conflict", "taken"))).toBeNull();
    expect(conflictOf(new Error("network"))).toBeNull();
  });
});
