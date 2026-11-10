/**
 * The kept texts client talks to three routes: this pins the paths, the
 * method, that the listing is unwrapped, that a refusal reads as "nothing to
 * show", and which entry an editor offers for its page.
 */

import { describe, expect, it, vi } from "vitest";

import { ApiProblem, api } from "./client";
import {
  dropKeptText,
  fetchKeptText,
  fetchKeptTexts,
  isNoKeptRight,
  keptForPage,
  keptFrom,
  keptKey,
  keptKindLabel,
  keptNoticeSentence,
  keptQuery,
  keptTextKey,
} from "./kept";
import type { KeptTextEntry } from "./model";

vi.mock("./client", async (importOriginal) => {
  const actual = await importOriginal<typeof import("./client")>();
  return { ...actual, api: vi.fn() };
});

const apiMock = vi.mocked(api);

function entry(over: Partial<KeptTextEntry> = {}): KeptTextEntry {
  return {
    id: "3f9a1c2b7d4e",
    draft: false,
    kind: "engram",
    path: "guides/a.md",
    file: false,
    kept_at: "2026-10-15T08:00:00Z",
    bytes: 12,
    sender: null,
    ...over,
  };
}

describe("the kept texts api module", () => {
  it("carries stable cache keys under one prefix", () => {
    expect(keptKey("eng")).toEqual(["kept", "eng"]);
    expect(keptTextKey("eng", "abc")).toEqual(["kept", "eng", "abc"]);
  });

  it("lists a domain's kept texts and unwraps the envelope", async () => {
    apiMock.mockResolvedValueOnce({ domain: "my eng", kept: [entry()] });
    const kept = await fetchKeptTexts("my eng");
    expect(apiMock).toHaveBeenCalledWith("/domains/my%20eng/kept");
    expect(kept).toEqual([entry()]);
  });

  it("reads one kept text by id", async () => {
    apiMock.mockResolvedValueOnce({ domain: "eng", ...entry(), text: "x" });
    const kept = await fetchKeptText("eng", "3f9a1c2b7d4e");
    expect(apiMock).toHaveBeenCalledWith("/domains/eng/kept/3f9a1c2b7d4e");
    expect(kept.text).toBe("x");
  });

  it("drops one kept text by id and returns nothing", async () => {
    apiMock.mockResolvedValueOnce(undefined);
    const result = await dropKeptText("eng", "3f9a1c2b7d4e");
    expect(apiMock).toHaveBeenCalledWith("/domains/eng/kept/3f9a1c2b7d4e", {
      method: "DELETE",
    });
    expect(result).toBeUndefined();
  });

  it("never retries the listing", () => {
    const options = keptQuery("eng");
    expect(options.queryKey).toEqual(["kept", "eng"]);
    expect(options.retry).toBe(false);
    expect(keptQuery("eng", false).enabled).toBe(false);
  });

  it("reads 401, 403 and 404 as nothing to show, and other failures as failures", () => {
    for (const status of [401, 403, 404]) {
      expect(isNoKeptRight(new ApiProblem(status, "t", "d"))).toBe(true);
    }
    expect(isNoKeptRight(new ApiProblem(500, "t", "d"))).toBe(false);
    expect(isNoKeptRight(new Error("offline"))).toBe(false);
    expect(isNoKeptRight(null)).toBe(false);
  });

  it("offers the newest room text of the same kind, path and draft side", () => {
    const old = entry({ id: "old" });
    const newer = entry({ id: "new", kept_at: "2026-10-16T08:00:00Z" });
    const entries = [
      old,
      entry({ id: "comment", kind: "comment" }),
      entry({ id: "page", kind: "reflection" }),
      entry({ id: "other", path: "guides/b.md" }),
      entry({ id: "draft", draft: true }),
      newer,
    ];
    expect(keptForPage(entries, "engram", "guides/a.md", false)?.id).toBe(
      "new",
    );
    expect(keptForPage(entries, "engram", "guides/a.md", true)?.id).toBe(
      "draft",
    );
    expect(keptForPage(entries, "reflection", "guides/a.md", false)?.id).toBe(
      "page",
    );
    expect(keptForPage(entries, "engram", "guides/c.md", false)).toBeNull();
    expect(keptForPage(undefined, "engram", "guides/a.md", false)).toBeNull();
  });

  it("names an entry's kind and where it came from", () => {
    expect(keptKindLabel("engram")).toBe("Engram text");
    expect(keptKindLabel("reflection")).toBe("Page text");
    expect(keptKindLabel("comment")).toBe("Comment");
    expect(keptKindLabel("something new")).toBe("Kept text");
    expect(keptFrom(entry({ sender: "ben" }))).toBe("From ben");
    expect(keptFrom(entry({ draft: true }))).toBe("From your draft");
    // A room's text records nobody, and nobody is invented for it.
    expect(keptFrom(entry())).toBeNull();
  });

  it("words the editors' note by sender and draft", () => {
    expect(keptNoticeSentence(entry())).toBe(
      "The server kept unsaved text for this page.",
    );
    expect(keptNoticeSentence(entry({ sender: "ben" }))).toBe(
      "The server kept unsaved text for this page. From ben.",
    );
    expect(keptNoticeSentence(entry({ draft: true }))).toBe(
      "The server kept unsaved text from your draft.",
    );
  });
});
