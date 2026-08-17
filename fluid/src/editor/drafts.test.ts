import { beforeEach, describe, expect, it } from "vitest";

import { clearDraft, draftKey, readDraft, writeDraft } from "./drafts";

describe("draft snapshots", () => {
  beforeEach(() => {
    localStorage.clear();
  });

  it("keys per user and per engram", () => {
    expect(draftKey("ada", "eng", "notes/alpha")).toBe(
      "fluid.draft.ada.eng/notes/alpha",
    );
    expect(draftKey("bob", "eng", "notes/alpha")).not.toBe(
      draftKey("ada", "eng", "notes/alpha"),
    );
  });

  it("round-trips, clears, and answers null for the never-written", () => {
    expect(readDraft("ada", "eng", "alpha")).toBeNull();
    const draft = {
      content: "text",
      baseChecksum: "abc",
      savedAt: "2026-08-09T10:00:00Z",
    };
    writeDraft("ada", "eng", "alpha", draft);
    expect(readDraft("ada", "eng", "alpha")).toEqual(draft);
    clearDraft("ada", "eng", "alpha");
    expect(readDraft("ada", "eng", "alpha")).toBeNull();
  });

  it("treats an unreadable stored value as no draft", () => {
    localStorage.setItem(draftKey("ada", "eng", "alpha"), "{not json");
    expect(readDraft("ada", "eng", "alpha")).toBeNull();
  });
});
