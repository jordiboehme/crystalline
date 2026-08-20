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

// The shared corpus, as text, out of the very file the core's own test reads:
// `crates/core/tests/fixtures/asset_ref_corpus.json`. `?raw` rather than
// `node:fs` because this program is browser scoped and carries no Node types;
// vite.config.ts allows the one folder it lives in for the test run.
import corpusJson from "../../../crates/core/tests/fixtures/asset_ref_corpus.json?raw";

import {
  assetPath,
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
    // The core scanner strips a leading "./" before it tests the prefix, so a
    // reference written that way IS a reference and has to resolve here too.
    expect(isAssetTarget("./assets/a.png")).toBe(true);
    // Absolute and external targets belong to whoever wrote them.
    expect(isAssetTarget("/assets/a.png")).toBe(false);
    expect(isAssetTarget("https://example.com/assets/a.png")).toBe(false);
    expect(isAssetTarget("//cdn.example.com/assets/a.png")).toBe(false);
    expect(isAssetTarget("notes/assets/a.png")).toBe(false);
    expect(isAssetTarget("")).toBe(false);
  });

  test("a path that names no file is not a target either", () => {
    // The prefix alone, and the prefix with nothing but a fragment after it:
    // the core drops both, because neither names a file.
    expect(isAssetTarget("assets/")).toBe(false);
    expect(isAssetTarget("assets/#left")).toBe(false);
  });

  test("a dot segment resolves to nothing, because the core would refuse it", () => {
    // `validate_asset_path` refuses `.` and `..` segments outright, so no such
    // path can be stored - and a URL built from one would ask the browser for
    // a different address than the one written.
    expect(isAssetTarget("assets/../../evil.png")).toBe(false);
    expect(isAssetTarget("assets/2026/../08/a.png")).toBe(false);
    expect(isAssetTarget("assets/./a.png")).toBe(false);
  });
});

describe("assetPath", () => {
  test("hands back the stored path a target names, or nothing", () => {
    expect(assetPath("./assets/a.png#right,w=50%")).toBe("assets/a.png");
    expect(assetPath("assets/2026/08/deck.pdf")).toBe(
      "assets/2026/08/deck.pdf",
    );
    expect(assetPath("https://example.com/assets/a.png")).toBeNull();
  });

  test("decodes once, so a written escape and a rendered one are one file", () => {
    // The reading view is handed a micromark-normalized URL and the rail is
    // handed the raw source; both come through here, so both have to answer
    // the same path for the same file.
    expect(assetPath("assets/2026/08/%E8%A8%AD%E8%A8%88.png")).toBe(
      "assets/2026/08/設計.png",
    );
    expect(assetPath("assets/2026/08/設計.png")).toBe(
      "assets/2026/08/設計.png",
    );
    // A stray percent is not an escape: it survives rather than throwing.
    expect(assetPath("assets/a%b.png")).toBe("assets/a%b.png");
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

  test("a reference written with a leading ./ is the same reference", () => {
    const line = "![Shot](./assets/a.png#left)";
    const [ref] = imageRefsIn(line);
    expect(ref?.path).toBe("assets/a.png");
    // What the author wrote is what a rewrite has to put back: the ./ is
    // theirs, and normalizing it away would be an edit nobody asked for.
    expect(ref?.written).toBe("./assets/a.png");
    expect(line.slice(ref?.targetFrom ?? 0, ref?.targetTo ?? 0)).toBe(
      "./assets/a.png#left",
    );
  });

  test("a title clause after the target is dropped, as the core drops it", () => {
    const line = '![Shot](assets/a.png#right "Q3 deck")';
    const [ref] = imageRefsIn(line);
    expect(ref?.path).toBe("assets/a.png");
    expect(ref?.format).toEqual({ align: "right" });
    // The span is the target token alone, so a rewrite leaves the title in
    // place rather than eating it.
    expect(line.slice(ref?.targetFrom ?? 0, ref?.targetTo ?? 0)).toBe(
      "assets/a.png#right",
    );
  });

  test("brackets inside the alt text do not hide the reference", () => {
    expect(imageRefsIn("![a [b] c](assets/a.png)").map((r) => r.path)).toEqual([
      "assets/a.png",
    ]);
  });

  test("a dot segment is no picture to draw", () => {
    expect(imageRefsIn("![x](assets/../../evil.png)")).toEqual([]);
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

  test("a leading ./ is stripped, so both spellings are one file", () => {
    expect(assetRefsIn("![a](./assets/a.png) and [b](assets/a.png)")).toEqual([
      "assets/a.png",
    ]);
  });

  test("a title clause after the target is not part of the path", () => {
    expect(assetRefsIn('[deck](assets/deck.pdf "Q3 deck")')).toEqual([
      "assets/deck.pdf",
    ]);
  });

  test("brackets inside the label do not hide the reference", () => {
    expect(assetRefsIn("![a [b] c](assets/a.png)")).toEqual(["assets/a.png"]);
  });

  test("balanced parentheses inside a destination close at the right one", () => {
    // The core walks to the depth-zero `)`, so a destination carrying a
    // balanced pair is read whole rather than cut at the first one.
    expect(assetRefsIn("[x](assets/a(1).png)")).toEqual(["assets/a(1).png"]);
  });

  test("a closing fence carrying an info string does not close the fence", () => {
    // The core requires the remainder of a closing fence line to be empty, so
    // this whole block is code and neither reference counts.
    expect(
      assetRefsIn(
        ["```", "![a](assets/a.png)", "```js", "[b](assets/b.pdf)", "```"].join(
          "\n",
        ),
      ),
    ).toEqual([]);
  });

  test("a target that names no file is dropped", () => {
    expect(assetRefsIn("![x](assets/#left) [y](assets/)")).toEqual([]);
  });

  test("external and absolute targets are somebody else's", () => {
    expect(
      assetRefsIn("![x](https://example.com/a.png) [y](/assets/b.pdf)"),
    ).toEqual([]);
  });
});

/** One case out of the shared fixture: a body and every ref it holds. */
interface CorpusCase {
  name: string;
  body: string;
  refs: string[];
}

const CORPUS = JSON.parse(corpusJson) as CorpusCase[];

/**
 * The one file both scanners are held to.
 *
 * `crates/core/src/attachment.rs` is the authority and this module is its
 * mirror, so the mirror is checked against the authority's own fixture rather
 * than against a second set of examples that could drift out from under it.
 * Since 0.15.1 the core percent-decodes a target the way {@link decodeTarget}
 * does, so the agreement is case for case with nothing excused.
 *
 * A change to either scanner's answer therefore changes this fixture, and the
 * fixture is shared: the Rust side moves in the same commit or one of the two
 * suites goes red.
 */
describe("the shared asset-ref corpus", () => {
  test("the fixture carries the whole corpus, not a truncated read of it", () => {
    expect(CORPUS.length).toBeGreaterThanOrEqual(16);
  });

  test.each(CORPUS)("$name", ({ body, refs }) => {
    expect(assetRefsIn(body)).toEqual(refs);
  });
});
