import { describe, expect, it } from "vitest";
import { pathPermalink } from "../../permalink";
import { deckNumber } from "./decals";
import {
  SECTION_SIZE,
  childFolder,
  clampSection,
  folderDeck,
  folderName,
  folderOfPath,
  folderSlug,
  isManifestPermalink,
  parentFolder,
  sectionLabels,
  sectionOfPermalink,
  sectionsOf,
  slugPath,
} from "./folders";

const rows = (names: string[]) =>
  names.map((n) => ({ permalink: `notes/${n}` }));

describe("folders (M3 C6, C8)", () => {
  it("slugs a path exactly as the server's slugify does", () => {
    // Mutation caught: no lowercasing, runs not collapsed, segments not
    // trimmed, empty segments kept, `.md` kept. The first five vectors are
    // the Rust `slugify` tests (crates/core/tests/it/address.rs); the loop pins
    // parity with Fluid's own port, so the two cannot drift apart.
    expect(slugPath("Astronomy/Phobos Orbit.md")).toBe(
      "astronomy/phobos-orbit",
    );
    expect(slugPath("  Weird__Name!! .md")).toBe("weird-name");
    expect(slugPath("A/  B  /C.md")).toBe("a/b/c");
    expect(slugPath("--leading--/trailing--.md")).toBe("leading/trailing");
    expect(slugPath("Already/Sluggy-Path")).toBe("already/sluggy-path");
    expect(slugPath("a///b.md")).toBe("a/b");
    const inputs = [
      "My Notes/Über uns.md",
      "x.md/y.md",
      "a&b#c?/d%e.MD",
      "",
      "---",
    ];
    expect(inputs.length).toBeGreaterThan(0);
    for (const p of inputs) expect(slugPath(p)).toBe(pathPermalink(p));
  });

  it("slugs a folder the way the server slugs the paths inside it", () => {
    // Mutation caught: the folder slugged without the `/x` guard, so a
    // folder named `x.md` loses its `.md`.
    expect(folderSlug("My Notes")).toBe("my-notes");
    expect(folderSlug("a/b c/d")).toBe("a/b-c/d");
    expect(folderSlug("Über uns")).toBe("ber-uns");
    expect(folderSlug("x.md")).toBe("x-md");
    expect(folderSlug("")).toBe("");
  });

  it("numbers a deck like the stencils of its rooms (Review Focus 1)", () => {
    // Mutation caught: hashing the raw folder instead of its slug.
    for (const [folder, permalink] of [
      ["My Notes", "my-notes/ber-uns"],
      ["a/b c/d", "a/b-c/d/x"],
      ["e", "e/readme"],
      ["x.md", "x-md/y"],
      ["x&y#z?%", "x-y-z/p"],
    ] as const) {
      expect(folderDeck("dom", folder)).toBe(deckNumber("dom", permalink));
    }
    expect(folderDeck("dom", "")).toBe(1);
    expect(folderDeck("dom", "!!!")).toBe(1);
  });

  it("walks folders up and down", () => {
    // Mutation caught: the root's parent read as "" instead of null, a
    // path's folder cut at the first `/` instead of the last.
    expect(folderName("a/b/c")).toBe("c");
    expect(parentFolder("a/b/c")).toBe("a/b");
    expect(parentFolder("a")).toBe("");
    expect(parentFolder("")).toBeNull();
    expect(folderOfPath("notes/deep/x.md")).toBe("notes/deep");
    expect(folderOfPath("x.md")).toBe("");
  });

  it("nests a folder one level under its parent", () => {
    // Mutation caught: the root guard dropped, so a top-level folder's name
    // grows a leading slash instead of standing on its own.
    expect(childFolder("", "notes")).toBe("notes");
    expect(childFolder("notes", "deep")).toBe("notes/deep");
    expect(parentFolder(childFolder("notes", "deep"))).toBe("notes");
  });

  it("knows the MANIFEST at the root only", () => {
    // Mutation caught: a case-sensitive compare, a nested `manifest` taken.
    expect(isManifestPermalink("manifest")).toBe(true);
    expect(isManifestPermalink("MANIFEST")).toBe(true);
    expect(isManifestPermalink("notes/manifest")).toBe(false);
  });

  it("cuts sections at 24 and never makes an empty one (Review Focus 2)", () => {
    // Mutation caught: an off-by-one cut, an empty trailing section, input order leaking.
    for (const n of [0, 1, 24, 25, 48, 49, 500]) {
      const names = Array.from(
        { length: n },
        (_, i) => `e${String(n - i).padStart(3, "0")}`,
      );
      const sections = sectionsOf(rows(names));
      expect(sections.length).toBe(Math.ceil(n / SECTION_SIZE));
      for (const s of sections) expect(s.length).toBeGreaterThan(0);
      const flat = sections.flat().map((r) => r.permalink);
      expect(flat).toEqual(names.map((name) => `notes/${name}`).sort());
    }
    expect(clampSection(null, 3)).toBe(0);
    expect(clampSection(7, 3)).toBe(2);
    expect(clampSection(0, 0)).toBe(0);
  });

  it("labels sections by their first letters and keeps equal labels apart", () => {
    // Mutation caught: the label read from the folder part of the permalink
    // ("N-N" for every section), equal labels left equal.
    const a = sectionsOf(
      rows(["alpha", ...Array.from({ length: 23 }, (_, i) => `c${i}`), "fox"]),
    );
    expect(sectionLabels(a)).toEqual(["A-C", "F"]);
    const same = sectionsOf(
      rows(
        Array.from({ length: 30 }, (_, i) => `c${String(i).padStart(2, "0")}`),
      ),
    );
    expect(sectionLabels(same)).toEqual(["C 1", "C 2"]);
    expect(sectionOfPermalink(same, "notes/c25")).toBe(1);
    expect(sectionOfPermalink(same, "notes/zz")).toBe(-1);
  });
});
