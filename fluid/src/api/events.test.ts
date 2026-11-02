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

  it("reads an engram files frame for an attachment path", () => {
    expect(
      parseFrame(
        "engram",
        JSON.stringify({
          domain: "eng",
          permalink: "assets/a.png",
          path: "assets/a.png",
          kind: "files",
          from: null,
          checksum: null,
          actor: "ada",
          draft_of: null,
        }),
      ),
    ).toMatchObject({
      event: "engram",
      change: {
        kind: "files",
        path: "assets/a.png",
        permalink: "assets/a.png",
        actor: "ada",
      },
    });
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

  it("reads a reflection frame and reads an unknown kind as changed", () => {
    expect(
      parseFrame(
        "reflection",
        JSON.stringify({
          domain: "eng",
          path: "b.md",
          kind: "moved",
          from: "a.md",
          actor: "jordi",
        }),
      ),
    ).toEqual({
      event: "reflection",
      change: {
        domain: "eng",
        path: "b.md",
        kind: "moved",
        from: "a.md",
        actor: "jordi",
      },
    });
    // `toMatchObject`, not `?.change?.kind`: the `reset` arm of `ChangeEvent` has no `change`, and the tests are typechecked.
    expect(
      parseFrame(
        "reflection",
        JSON.stringify({ domain: "eng", path: "assets/x.png", kind: "files" }),
      ),
    ).toMatchObject({
      event: "reflection",
      change: { kind: "files", path: "assets/x.png" },
    });
    expect(
      parseFrame(
        "reflection",
        JSON.stringify({ domain: "eng", path: "b.md", kind: "renamed" }),
      ),
    ).toEqual({
      event: "reflection",
      change: {
        domain: "eng",
        path: "b.md",
        kind: "changed",
        from: null,
        actor: null,
      },
    });
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
    expect(
      parseFrame(
        "reflection",
        JSON.stringify({ path: "b.md", kind: "changed" }),
      ),
    ).toBeNull();
    warn.mockRestore();
    // The switch event (`switched`, also sent when the host lock moves) has an empty path and is kept;
    // a missing `from` or `actor` reads as null.
    expect(
      parseFrame(
        "reflection",
        JSON.stringify({ domain: "eng", path: "", kind: "switched" }),
      ),
    ).toEqual({
      event: "reflection",
      change: {
        domain: "eng",
        path: "",
        kind: "switched",
        from: null,
        actor: null,
      },
    });
  });

  it("reads the four event names", () => {
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
