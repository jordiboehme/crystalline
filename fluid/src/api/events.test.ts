import { describe, expect, it, vi } from "vitest";

import { parseFrame, readEngramChange } from "./events";

describe("the change frames", () => {
  it("reads an engram frame with every field and tolerates the optional ones", () => {
    const change = readEngramChange({
      domain: "eng",
      permalink: "alpha",
      path: "alpha.md",
      kind: "moved",
      from: { path: "old.md", permalink: "old" },
      checksum: "9f",
      actor: "ada",
      draft_of: null,
    });
    expect(change).toEqual({
      domain: "eng",
      permalink: "alpha",
      path: "alpha.md",
      kind: "moved",
      from: { path: "old.md", permalink: "old" },
      checksum: "9f",
      actor: "ada",
      draftOf: null,
    });
    expect(
      readEngramChange({
        domain: "eng",
        permalink: "a",
        path: "a.md",
        kind: "added",
      }),
    ).toMatchObject({ from: null, checksum: null, actor: null, draftOf: null });
  });

  it("drops a frame that does not parse, with a warning and no throw", () => {
    const warn = vi.spyOn(console, "warn").mockImplementation(() => undefined);
    expect(parseFrame("engram", "{not json")).toBeNull();
    expect(parseFrame("engram", JSON.stringify({ domain: "eng" }))).toBeNull();
    expect(
      parseFrame(
        "engram",
        JSON.stringify({
          domain: "eng",
          permalink: "a",
          path: "a.md",
          kind: "exploded",
        }),
      ),
    ).toBeNull();
    expect(parseFrame("presence", "{}")).toBeNull();
    warn.mockRestore();
  });

  it("reads the three event names", () => {
    expect(parseFrame("reset", "{}")).toEqual({ event: "reset" });
    expect(
      parseFrame("domain", JSON.stringify({ domain: "eng", actor: null })),
    ).toEqual({ event: "domain", change: { domain: "eng", actor: null } });
    expect(
      parseFrame(
        "engram",
        JSON.stringify({
          domain: "eng",
          permalink: "a",
          path: "a.md",
          kind: "deleted",
        }),
      )?.event,
    ).toBe("engram");
  });
});
