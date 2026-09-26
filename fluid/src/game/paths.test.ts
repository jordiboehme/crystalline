import { describe, expect, it } from "vitest";

import { engramRoute } from "../paths";
import {
  MANIFEST_PERMALINK,
  bridgeAddress,
  fluidRouteOf,
  gameEngramRoute,
  placeKeyOf,
} from "./paths";

describe("gameEngramRoute", () => {
  it("mirrors the engram route under the encoded π prefix", () => {
    expect(gameEngramRoute("eng", "notes/deep/gamma")).toBe(
      "/%CF%80/d/eng/e/notes/deep/gamma",
    );
  });

  it("encodes segments the way engramRoute does", () => {
    expect(gameEngramRoute("my eng", "a b/c d")).toBe(
      "/%CF%80/d/my%20eng/e/a%20b/c%20d",
    );
    expect(gameEngramRoute("my eng", "a b/c d")).toBe(
      `/%CF%80${engramRoute("my eng", "a b/c d")}`,
    );
  });

  it("matches what a browser's own URL parser settles a raw π prefix into", () => {
    // This is the decision this module documents: `window.location.pathname`
    // is always percent-encoded, never the raw character, whether the
    // browser was handed the raw form or the encoded one. Building with the
    // raw character below would therefore never string-equal what
    // `session.ts` compares it against - the very next test pins that as a
    // failure, not just an assertion here.
    const rawPathname = new URL("/π/d/eng/e/x", "http://example.test").pathname;
    const encodedPathname = new URL("/%CF%80/d/eng/e/x", "http://example.test")
      .pathname;
    expect(rawPathname).toBe(encodedPathname);
    expect(gameEngramRoute("eng", "x")).toBe(rawPathname);
  });

  it("would NOT match window.location.pathname if built from the raw character", () => {
    // Evidence for the decision: a builder that used the raw "π" instead of
    // its encoding would produce a string that never equals the pathname a
    // real browser (or jsdom) reports, even for the exact same address.
    const rawBuilt = `/${"π"}${engramRoute("eng", "x")}`;
    const browserPathname = new URL(rawBuilt, "http://example.test").pathname;
    expect(rawBuilt).not.toBe(browserPathname);
    expect(gameEngramRoute("eng", "x")).toBe(browserPathname);
  });
});

describe("fluidRouteOf", () => {
  const cases: [label: string, input: string, expected: string][] = [
    ["raw π prefix", "/π/d/eng/e/a", "/d/eng/e/a"],
    ["encoded π prefix, uppercase hex", "/%CF%80/d/eng/e/a", "/d/eng/e/a"],
    ["encoded π prefix, lowercase hex", "/%cf%80/d/eng/e/a", "/d/eng/e/a"],
    ["bare raw prefix", "/π", "/"],
    ["bare encoded prefix", "/%CF%80", "/"],
    ["raw prefix with trailing slash", "/π/", "/"],
    ["encoded prefix with trailing slash", "/%CF%80/", "/"],
    [
      "a game engram route round-tripped",
      gameEngramRoute("eng", "x y/z"),
      engramRoute("eng", "x y/z"),
    ],
    [
      "an encoded permalink segment survives untouched",
      "/%CF%80/d/eng/e/a%2Fb%20c",
      "/d/eng/e/a%2Fb%20c",
    ],
    ["a path that only starts with the same letters, raw", "/πx/y", "/πx/y"],
    [
      "a path that only starts with the same letters, encoded",
      "/%CF%80x/y",
      "/%CF%80x/y",
    ],
    ["a malformed escape, handed back rather than thrown", "/%E0/x", "/%E0/x"],
    ["a plain Fluid path, unchanged", "/d/eng/e/a", "/d/eng/e/a"],
    [
      "the old /game prefix, no alias, unchanged",
      "/game/d/eng/e/a",
      "/game/d/eng/e/a",
    ],
  ];

  it.each(cases)("%s", (_label, input, expected) => {
    expect(fluidRouteOf(input)).toBe(expected);
  });
});

describe("placeKeyOf", () => {
  it("joins domain and permalink with a NUL", () => {
    expect(placeKeyOf("eng", "a/b")).toBe("eng\u0000a/b");
  });
});

describe("bridgeAddress", () => {
  it("names a domain's MANIFEST room, whose game route mirrors its engram route", () => {
    expect(MANIFEST_PERMALINK).toBe("manifest");
    const bridge = bridgeAddress("platform eng");
    expect(bridge).toEqual({ domain: "platform eng", permalink: "manifest" });
    expect(gameEngramRoute(bridge.domain, bridge.permalink)).toBe(
      "/%CF%80/d/platform%20eng/e/manifest",
    );
  });
});
