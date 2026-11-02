import { QueryClient } from "@tanstack/react-query";
import { describe, expect, it } from "vitest";

import { changedVersions } from "./assetVersions";

describe("which files changed since this document first saw them", () => {
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

  it("keeps scopes apart, so a Reflection and the engrams of one domain do not share a baseline", () => {
    const client = new QueryClient();
    changedVersions(client, "eng", [row("assets/a.png", "1")]);
    expect(
      changedVersions(client, "engram:eng", [row("assets/a.png", "2")]),
    ).toEqual(new Map());
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
