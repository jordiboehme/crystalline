/**
 * The vocabulary payload mixes two counting conventions: tags carry their
 * count under `engrams` (the older field, read by `readTags`), categories and
 * relation types carry theirs under `count`. `readVocabulary` reads both
 * without either shape leaking into the other's field.
 */

import { describe, expect, it, vi } from "vitest";

import { api } from "./client";
import { fetchVocabulary, readVocabulary } from "./vocabulary";

vi.mock("./client", async (importOriginal) => {
  const actual = await importOriginal<typeof import("./client")>();
  return { ...actual, api: vi.fn() };
});

const apiMock = vi.mocked(api);

describe("readVocabulary", () => {
  it("reads tags by their engrams count and categories/relation types by count", () => {
    const vocabulary = readVocabulary({
      tags: [{ name: "alpha", engrams: 3 }],
      categories: [{ name: "decision", count: 5 }],
      relation_types: [{ name: "supersedes", count: 2 }],
    });

    expect(vocabulary.tags).toEqual([{ name: "alpha", engrams: 3 }]);
    expect(vocabulary.categories).toEqual([{ name: "decision", count: 5 }]);
    expect(vocabulary.relationTypes).toEqual([
      { name: "supersedes", count: 2 },
    ]);
  });

  it("reads the type and status words the domain actually uses", () => {
    const vocabulary = readVocabulary({
      types: [
        { name: "engram", count: 12 },
        { name: "playbook", count: 3 },
      ],
      statuses: [{ name: "brewing", count: 2 }],
    });

    expect(vocabulary.types).toEqual([
      { name: "engram", count: 12 },
      { name: "playbook", count: 3 },
    ]);
    expect(vocabulary.statuses).toEqual([{ name: "brewing", count: 2 }]);
  });

  it("answers empty lists when the server enumerates neither", () => {
    // An older server has no `types` or `statuses` in its payload at all, and
    // a reader that yielded undefined would make every caller guard for it.
    const vocabulary = readVocabulary({ tags: [] });

    expect(vocabulary.types).toEqual([]);
    expect(vocabulary.statuses).toEqual([]);
  });

  it("drops an entry with no name and sorts the rest commonest first", () => {
    const vocabulary = readVocabulary({
      categories: [
        { name: "b", count: 1 },
        { count: 9 },
        { name: "a", count: 1 },
        { name: "c", count: 3 },
      ],
    });

    expect(vocabulary.categories).toEqual([
      { name: "c", count: 3 },
      { name: "a", count: 1 },
      { name: "b", count: 1 },
    ]);
  });
});

describe("fetchVocabulary", () => {
  it("requests the whole instance's vocabulary for a null domain", async () => {
    apiMock.mockResolvedValueOnce({
      tags: [],
      categories: [],
      relation_types: [],
    });
    await fetchVocabulary(null);
    expect(apiMock).toHaveBeenLastCalledWith("/vocabulary");
  });

  it("scopes the request to one domain", async () => {
    apiMock.mockResolvedValueOnce({
      tags: [],
      categories: [],
      relation_types: [],
    });
    await fetchVocabulary("eng");
    expect(apiMock).toHaveBeenLastCalledWith("/vocabulary?domain=eng");
  });
});
