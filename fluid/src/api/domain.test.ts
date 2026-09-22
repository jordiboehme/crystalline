/**
 * What a browse row is allowed to carry, read the way every list in this app
 * reads a row.
 *
 * The tree is the one source whose rows are navigation rather than description,
 * and the one field navigation cannot do without is `status`: it is what fades
 * a retired engram in a sidebar. The endpoint answers with it, so this pins
 * that it survives the read, and that a row without one still reads as a row
 * rather than throwing three components deep.
 */

import { describe, expect, it, vi } from "vitest";

import { api } from "./client";
import {
  fetchManifestDetail,
  readManifestSections,
  readTree,
  saveManifest,
  setDomainPolicies,
} from "./domain";
import { defined } from "../test/assert";

vi.mock("./client", async (importOriginal) => {
  const actual = await importOriginal<typeof import("./client")>();
  return { ...actual, api: vi.fn() };
});

const apiMock = vi.mocked(api);

/** One folder of a domain, in the shape the endpoint answers with. */
function browsePayload() {
  return {
    domain: "eng",
    path: "/",
    folders: ["notes"],
    engrams: [
      {
        permalink: "alpha",
        title: "Alpha",
        type: "engram",
        status: "stable",
        path: "alpha.md",
      },
      {
        permalink: "old",
        title: "Old Way",
        type: "decision",
        status: "deprecated",
        path: "old.md",
      },
    ],
  };
}

describe("a browse payload", () => {
  it("carries each engram's status into the row", () => {
    const tree = readTree(browsePayload(), "eng", "");

    expect(tree.folders).toEqual(["notes"]);
    expect(tree.engrams.map((row) => row.status)).toEqual([
      "stable",
      "deprecated",
    ]);
    // The domain of the request rides along, because a tree row names only
    // itself and a link out of one needs both halves of the address.
    const alpha = defined(tree.engrams[0], "the first row");
    expect(alpha.domain).toBe("eng");
    expect(alpha.type).toBe("engram");
  });

  it("carries the level's bound: whether it was cut, and what it holds", () => {
    const tree = readTree(
      { ...browsePayload(), truncated: true, total: 501 },
      "eng",
      "",
    );

    // The server caps a level rather than answering with a folder of tens of
    // thousands, and says so. A sidebar that ignored this would draw the first
    // page of a folder as if it were the folder.
    expect(tree.truncated).toBe(true);
    expect(tree.total).toBe(501);
  });

  it("reads a payload that predates the bound as a whole level", () => {
    // Additive fields: a daemon that never wrote them answered with everything
    // it had, so the level is not cut and holds exactly what it carried.
    const tree = readTree(browsePayload(), "eng", "");

    expect(tree.truncated).toBe(false);
    expect(tree.total).toBe(2);
  });

  it("leaves the status null when a row does not say", () => {
    const tree = readTree(
      { domain: "eng", path: "/", folders: [], engrams: [{ permalink: "a" }] },
      "eng",
      "",
    );

    // Null rather than a plausible default: a row claiming a state nobody
    // wrote would be a lie a reader cannot see through, and it would fade or
    // fail to fade on that lie.
    const row = defined(tree.engrams[0], "the first row");
    expect(row.status).toBeNull();
    expect(row.title).toBe("a");
  });
});

describe("a manifest detail", () => {
  it("fetches the route and carries the checksum for editing", async () => {
    apiMock.mockResolvedValueOnce({ markdown: "# eng", checksum: "abc123" });
    const detail = await fetchManifestDetail("eng");
    expect(apiMock).toHaveBeenLastCalledWith("/domains/eng/manifest");
    expect(detail).toEqual({ markdown: "# eng", checksum: "abc123" });
  });

  it("saves with a quoted If-Match", async () => {
    apiMock.mockResolvedValueOnce({ markdown: "# eng v2", checksum: "def456" });
    const saved = await saveManifest("eng", "# eng v2", "abc123");
    expect(apiMock).toHaveBeenLastCalledWith(
      "/domains/eng/manifest",
      expect.objectContaining({
        method: "PUT",
        headers: { "If-Match": '"abc123"' },
        body: JSON.stringify({ markdown: "# eng v2" }),
      }),
    );
    expect(saved.checksum).toBe("def456");
  });
});

describe("the manifest policies", () => {
  const row = {
    key: "sharing",
    declared: "direct",
    effective: "direct",
    values: ["proposal", "direct"],
    default: "proposal",
    meaning:
      "Whether a share opens a proposal for review or commits straight to the branch.",
    changed_by: "owner",
  };

  it("reads every registry row and drops one without a key or an effective value", () => {
    const sections = readManifestSections({
      scope: [],
      when_to_use: [],
      routing: "none",
      missing: [],
      provisioning: null,
      tag_aliases: null,
      policies: [
        row,
        // A row the registry sent without the three describing fields: the
        // key and what holds are what a card draws, and the rest has
        // defaults of its own.
        { key: "generated_indexes", effective: "local" },
        // Neither of these is a row anything could be drawn from.
        { declared: "x", effective: "y" },
        { key: "orphan" },
      ],
    });

    expect(sections?.policies).toEqual([
      {
        key: "sharing",
        declared: "direct",
        effective: "direct",
        values: ["proposal", "direct"],
        default: "proposal",
        meaning: row.meaning,
        changedBy: "owner",
      },
      {
        key: "generated_indexes",
        declared: null,
        effective: "local",
        values: [],
        // No default on the wire reads as "what holds is the default", which
        // is true of every key nobody declared.
        default: "local",
        meaning: "",
        changedBy: "owner",
      },
    ]);
    expect(sections).not.toHaveProperty("generatedIndexes");
  });

  it("reads an older payload with no policies as an empty list", () => {
    expect(
      readManifestSections({
        scope: [],
        when_to_use: [],
        routing: "none",
        missing: [],
      })?.policies,
    ).toEqual([]);
  });

  it("patches the policies and reads the answer back, draft flag included", async () => {
    apiMock.mockResolvedValueOnce({
      domain: "eng",
      markdown: "---\nsharing: direct\n---\n",
      checksum: "def456",
      draft: true,
      sections: {
        scope: [],
        when_to_use: [],
        routing: "none",
        missing: [],
        provisioning: null,
        tag_aliases: null,
        policies: [row],
      },
    });
    const written = await setDomainPolicies("eng", { sharing: "direct" });

    expect(apiMock).toHaveBeenLastCalledWith(
      "/domains/eng/manifest",
      expect.objectContaining({
        method: "PATCH",
        body: JSON.stringify({ sharing: "direct" }),
      }),
    );
    // The write landed in the caller's own draft, which is a different
    // sentence from the write landing in the domain.
    expect(written.draft).toBe(true);
    expect(written.checksum).toBe("def456");
    expect(written.sections?.policies[0]?.declared).toBe("direct");
  });
});
