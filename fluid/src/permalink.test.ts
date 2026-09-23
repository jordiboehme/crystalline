import { describe, expect, it } from "vitest";

import {
  defaultMovedPermalink,
  destinationFile,
  pathPermalink,
  permalinkFolder,
} from "./permalink";

describe("the permalink a path derives", () => {
  it("matches the engine's slug for the shapes a move meets", () => {
    expect(pathPermalink("projects/velog/alpha.md")).toBe(
      "projects/velog/alpha",
    );
    expect(pathPermalink("Projects/Velog/Alpha Notes.md")).toBe(
      "projects/velog/alpha-notes",
    );
    expect(pathPermalink("a//b/-c-.MD")).toBe("a/b/c");
    expect(pathPermalink("notes/Über uns!.md")).toBe("notes/ber-uns");
  });

  it("names the folder part and nothing else", () => {
    expect(permalinkFolder("projects/velog/alpha")).toBe("projects/velog");
    expect(permalinkFolder("alpha")).toBe("");
  });

  it("reads a destination with or without its suffix", () => {
    expect(destinationFile("guides/alpha")).toBe("guides/alpha.md");
    expect(destinationFile(" /guides//alpha.md ")).toBe("guides/alpha.md");
    expect(destinationFile("  ")).toBe("");
  });

  it("follows an in-step permalink and keeps a custom one", () => {
    expect(defaultMovedPermalink("alpha", "alpha.md", "guides/alpha")).toBe(
      "guides/alpha",
    );
    expect(
      defaultMovedPermalink(
        "velog/alpha",
        "projects/velog/alpha.md",
        "x/alpha",
      ),
    ).toBe("velog/alpha");
  });
});
