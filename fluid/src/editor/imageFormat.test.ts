/**
 * The fragment convention, held to the one property that makes it safe to
 * write into somebody's engram: what is parsed rebuilds to what was there.
 *
 * The convention is prose in the file - a fragment on an image target - so the
 * round trip is not a nicety. A menu that rewrote `#right,w=50%` into
 * `#w=50%,right` would churn the document on every click, and one that kept an
 * unknown directive it does not honor would promise a rendering nobody
 * implements.
 */

import { describe, expect, test } from "vitest";

import {
  assetRefsIn,
  buildImageTarget,
  imageRefsIn,
  isAssetTarget,
  parseImageFragment,
} from "./imageFormat";

describe("parseImageFragment", () => {
  test("a bare target is a centered image with no width", () => {
    expect(parseImageFragment("assets/2026/08/shot.png")).toEqual({
      path: "assets/2026/08/shot.png",
      format: { align: "center" },
    });
  });

  test("each placement directive is read", () => {
    for (const align of ["center", "full", "left", "right"] as const) {
      expect(parseImageFragment(`assets/a.png#${align}`).format).toEqual({
        align,
      });
    }
  });

  test("a width is read in pixels and in percent", () => {
    expect(parseImageFragment("assets/a.png#w=300").format).toEqual({
      align: "center",
      width: "300",
    });
    expect(parseImageFragment("assets/a.png#w=50%").format).toEqual({
      align: "center",
      width: "50%",
    });
  });

  test("directives combine in any order and the path never carries the fragment", () => {
    expect(parseImageFragment("assets/a.png#right,w=50%")).toEqual({
      path: "assets/a.png",
      format: { align: "right", width: "50%" },
    });
    expect(parseImageFragment("assets/a.png#w=50%,right")).toEqual({
      path: "assets/a.png",
      format: { align: "right", width: "50%" },
    });
  });

  test("an unknown directive is ignored rather than honored or kept", () => {
    expect(parseImageFragment("assets/a.png#zoom")).toEqual({
      path: "assets/a.png",
      format: { align: "center" },
    });
    // A width nobody can render is not a width: only digits, optionally a
    // percent sign, mean anything here.
    expect(parseImageFragment("assets/a.png#w=huge").format).toEqual({
      align: "center",
    });
  });

  test("an empty fragment is the same as no fragment", () => {
    expect(parseImageFragment("assets/a.png#")).toEqual({
      path: "assets/a.png",
      format: { align: "center" },
    });
  });

  test("the last placement wins, so a contradictory fragment still renders", () => {
    expect(parseImageFragment("assets/a.png#left,right").format).toEqual({
      align: "right",
    });
  });
});

describe("buildImageTarget", () => {
  test("center with no width is a bare path - the default writes nothing", () => {
    expect(buildImageTarget("assets/a.png", { align: "center" })).toBe(
      "assets/a.png",
    );
  });

  test("what parses rebuilds identically", () => {
    for (const target of [
      "assets/a.png",
      "assets/a.png#full",
      "assets/a.png#left",
      "assets/a.png#right",
      "assets/a.png#w=300",
      "assets/a.png#w=50%",
      "assets/a.png#right,w=50%",
      "assets/2026/08/deck-2.png#left,w=25%",
    ]) {
      const { path, format } = parseImageFragment(target);
      expect(buildImageTarget(path, format)).toBe(target);
    }
  });

  test("an unknown directive is dropped on the rebuild", () => {
    const { path, format } = parseImageFragment("assets/a.png#zoom");
    expect(buildImageTarget(path, format)).toBe("assets/a.png");
  });

  test("a centered width states only the width, since center is the default", () => {
    expect(
      buildImageTarget("assets/a.png", { align: "center", width: "50%" }),
    ).toBe("assets/a.png#w=50%");
  });

  test("building is idempotent through a second parse", () => {
    const once = buildImageTarget("assets/a.png", {
      align: "left",
      width: "25%",
    });
    const { path, format } = parseImageFragment(once);
    expect(buildImageTarget(path, format)).toBe(once);
  });
});

describe("isAssetTarget", () => {
  test("only a relative path under the reserved prefix is one of ours", () => {
    expect(isAssetTarget("assets/a.png")).toBe(true);
    expect(isAssetTarget("assets/2026/08/a.png#right")).toBe(true);
    // Absolute and external targets belong to whoever wrote them.
    expect(isAssetTarget("/assets/a.png")).toBe(false);
    expect(isAssetTarget("https://example.com/assets/a.png")).toBe(false);
    expect(isAssetTarget("//cdn.example.com/assets/a.png")).toBe(false);
    expect(isAssetTarget("notes/assets/a.png")).toBe(false);
    expect(isAssetTarget("")).toBe(false);
  });
});

describe("imageRefsIn", () => {
  test("finds an image reference and where its target sits", () => {
    const line = "before ![Shot](assets/a.png#right) after";
    const [ref] = imageRefsIn(line);
    expect(ref).toBeDefined();
    expect(ref?.path).toBe("assets/a.png");
    expect(ref?.format).toEqual({ align: "right" });
    expect(line.slice(ref?.targetFrom ?? 0, ref?.targetTo ?? 0)).toBe(
      "assets/a.png#right",
    );
  });

  test("a line may carry several, in the order they are written", () => {
    const refs = imageRefsIn("![a](assets/a.png) and ![b](assets/b.png#full)");
    expect(refs.map((ref) => ref.path)).toEqual([
      "assets/a.png",
      "assets/b.png",
    ]);
  });

  test("an external image and a plain link are not attachments", () => {
    expect(imageRefsIn("![x](https://example.com/a.png)")).toEqual([]);
    expect(imageRefsIn("[deck](assets/deck.pdf)")).toEqual([]);
    // An image reference to a non-image attachment carries no preview either.
    expect(imageRefsIn("![deck](assets/deck.pdf)")).toEqual([]);
  });

  test("the alt text may be empty, which is what an upload writes for a blank name", () => {
    expect(imageRefsIn("![](assets/a.png)").map((ref) => ref.path)).toEqual([
      "assets/a.png",
    ]);
  });
});

describe("assetRefsIn", () => {
  test("collects both kinds of reference, fragment stripped and deduplicated", () => {
    expect(
      assetRefsIn(
        [
          "![Shot](assets/a.png#right,w=50%)",
          "",
          "The [deck](assets/2026/08/deck.pdf) says more.",
          "",
          "And the same picture again: ![again](assets/a.png)",
        ].join("\n"),
      ),
    ).toEqual(["assets/a.png", "assets/2026/08/deck.pdf"]);
  });

  test("a path inside a fence is an example rather than a reference", () => {
    expect(
      assetRefsIn(
        [
          "```md",
          "![a](assets/a.png)",
          "```",
          "",
          "~~~",
          "[b](assets/b.pdf)",
          "~~~",
        ].join("\n"),
      ),
    ).toEqual([]);
  });

  test("a fence closes only on its own character", () => {
    expect(
      assetRefsIn(["```", "~~~", "![a](assets/a.png)", "```"].join("\n")),
    ).toEqual([]);
  });

  test("external and absolute targets are somebody else's", () => {
    expect(
      assetRefsIn("![x](https://example.com/a.png) [y](/assets/b.pdf)"),
    ).toEqual([]);
  });
});
