import { describe, expect, it } from "vitest";

import { engramRoute } from "../paths";
import type { PlaceAddress, StationAddress } from "./world/types";
import {
  MANIFEST_PERMALINK,
  addressOfGameLocation,
  bridgeAddress,
  canonicalStation,
  domainOf,
  fluidRouteOf,
  fluidRouteOfStation,
  gameRouteOf,
  placeKeyOf,
  sameStation,
  stationOfPlace,
} from "./paths";

/** An engram address, for the tests that only care about its route. */
function engramAt(domain: string, permalink: string): StationAddress {
  return { kind: "engram", domain, permalink };
}

describe("gameRouteOf, an engram address", () => {
  it("mirrors the engram route under the encoded π prefix", () => {
    expect(gameRouteOf(engramAt("eng", "notes/deep/gamma"))).toBe(
      "/%CF%80/d/eng/e/notes/deep/gamma",
    );
  });

  it("encodes segments the way engramRoute does", () => {
    expect(gameRouteOf(engramAt("my eng", "a b/c d"))).toBe(
      "/%CF%80/d/my%20eng/e/a%20b/c%20d",
    );
    expect(gameRouteOf(engramAt("my eng", "a b/c d"))).toBe(
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
    expect(gameRouteOf(engramAt("eng", "x"))).toBe(rawPathname);
  });

  it("would NOT match window.location.pathname if built from the raw character", () => {
    // Evidence for the decision: a builder that used the raw "π" instead of
    // its encoding would produce a string that never equals the pathname a
    // real browser (or jsdom) reports, even for the exact same address.
    const rawBuilt = `/${"π"}${engramRoute("eng", "x")}`;
    const browserPathname = new URL(rawBuilt, "http://example.test").pathname;
    expect(rawBuilt).not.toBe(browserPathname);
    expect(gameRouteOf(engramAt("eng", "x"))).toBe(browserPathname);
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
      gameRouteOf(engramAt("eng", "x y/z")),
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
  it("names a domain's bridge, whose game route is the domain's own (M3 C3)", () => {
    // Mutation caught: the bridge still addressed as the MANIFEST engram
    // (`/e/manifest` in its route), or its domain's space written as `+`.
    expect(MANIFEST_PERMALINK).toBe("manifest");
    expect(bridgeAddress("eng")).toEqual({ kind: "bridge", domain: "eng" });
    expect(gameRouteOf(bridgeAddress("eng"))).toBe("/%CF%80/d/eng");
    expect(gameRouteOf(bridgeAddress("platform eng"))).toBe(
      "/%CF%80/d/platform%20eng",
    );
  });
});

describe("station addresses (M3 C1 to C5)", () => {
  const cases: [StationAddress, string, string][] = [
    [{ kind: "airlock" }, "/%CF%80", "/"],
    [{ kind: "bridge", domain: "eng" }, "/%CF%80/d/eng", "/d/eng"],
    [
      { kind: "deck", domain: "eng", folder: "", section: 0 },
      "/%CF%80/d/eng?path=",
      "/d/eng",
    ],
    [
      { kind: "deck", domain: "eng", folder: "My Notes", section: 0 },
      "/%CF%80/d/eng?path=My%20Notes",
      "/d/eng?path=My%20Notes",
    ],
    [
      { kind: "deck", domain: "eng", folder: "a&b#c", section: 2 },
      "/%CF%80/d/eng?path=a%26b%23c&section=3",
      "/d/eng?path=a%26b%23c",
    ],
    [
      { kind: "engram", domain: "e", permalink: "e/x" },
      "/%CF%80/d/e/e/e/x",
      "/d/e/e/e/x",
    ],
    [
      { kind: "deck", domain: "eng", folder: "?", section: 0 },
      "/%CF%80/d/eng?path=%3F",
      "/d/eng?path=%3F",
    ],
    [
      { kind: "deck", domain: "eng", folder: "%", section: 0 },
      "/%CF%80/d/eng?path=%25",
      "/d/eng?path=%25",
    ],
    [
      { kind: "deck", domain: "eng", folder: "'", section: 0 },
      "/%CF%80/d/eng?path=%27",
      "/d/eng?path='",
    ],
    [
      { kind: "deck", domain: "eng", folder: "a/b c/d", section: 0 },
      "/%CF%80/d/eng?path=a%2Fb%20c%2Fd",
      "/d/eng?path=a%2Fb%20c%2Fd",
    ],
    [
      { kind: "deck", domain: "eng", folder: "x.md", section: 0 },
      "/%CF%80/d/eng?path=x.md",
      "/d/eng?path=x.md",
    ],
  ];

  it("round-trips every address kind through its route, awkward names included (Review Focus 1)", () => {
    // Mutation caught: a section written 0-based, `path` written with
    // URLSearchParams (a space as `+`), a domain called `e` read as an engram
    // route. The `?`, `%`, `'`, `a/b c/d` and `x.md` folders pin punctuation
    // `encodeURIComponent` treats differently from a browser's own query
    // serialiser (`'` above all, M3 fix wave): `url.pathname + url.search`
    // is what a real landing compares against, so that is what `game` names
    // here, while `fluidRouteOfStation`'s own raw builder output (`fluid`)
    // is compared unnormalised.
    expect(cases.length).toBeGreaterThan(0);
    for (const [address, game, fluid] of cases) {
      const url = new URL(gameRouteOf(address), "http://x");
      expect(url.pathname + url.search).toBe(game);
      expect(addressOfGameLocation(url.pathname, url.search)).toEqual(address);
      expect(fluidRouteOfStation(address)).toBe(fluid);
    }
  });

  it("reads the MANIFEST route as the bridge and an unknown game path as the airlock", () => {
    // Mutation caught: a nested `manifest` canonicalised, an unknown game
    // path answered null (the route would render nothing), a bad section
    // number passed through, or `=== "MANIFEST"` replacing the
    // case-insensitive compare (caught directly here rather than only
    // through interact.test.ts's arrivalSpawn test).
    expect(
      canonicalStation({
        kind: "engram",
        domain: "eng",
        permalink: "MANIFEST",
      }),
    ).toEqual({ kind: "bridge", domain: "eng" });
    expect(
      canonicalStation({
        kind: "engram",
        domain: "eng",
        permalink: "manifest",
      }),
    ).toEqual({ kind: "bridge", domain: "eng" });
    expect(
      canonicalStation({
        kind: "engram",
        domain: "eng",
        permalink: "Manifest",
      }),
    ).toEqual({ kind: "bridge", domain: "eng" });
    expect(
      canonicalStation({
        kind: "engram",
        domain: "eng",
        permalink: "notes/manifest",
      }).kind,
    ).toBe("engram");
    expect(addressOfGameLocation("/%CF%80/foo", "")).toEqual({
      kind: "airlock",
    });
    expect(addressOfGameLocation("/d/eng", "")).toBeNull();
    expect(addressOfGameLocation("/%CF%80/d/eng", "?path=x&section=0")).toEqual(
      { kind: "deck", domain: "eng", folder: "x", section: 0 },
    );
    expect(
      addressOfGameLocation("/%CF%80/d/eng", "?path=x&section=zz"),
    ).toEqual({ kind: "deck", domain: "eng", folder: "x", section: 0 });
  });

  it("never takes an unresolved deck for a resolved one", () => {
    // Mutation caught: `null` compared as 0.
    const deck = { kind: "deck", domain: "eng", folder: "x" } as const;
    expect(
      sameStation({ ...deck, section: null }, { ...deck, section: 0 }),
    ).toBe(false);
    expect(sameStation({ ...deck, section: 1 }, { ...deck, section: 1 })).toBe(
      true,
    );
    expect(sameStation(null, { kind: "airlock" })).toBe(false);
  });

  it("names the domain of every address kind, or null for the airlock", () => {
    // Mutation caught: the airlock's own domain read from a field it does
    // not carry, instead of the null the doc promises.
    expect(domainOf({ kind: "airlock" })).toBeNull();
    expect(domainOf({ kind: "bridge", domain: "eng" })).toBe("eng");
    expect(
      domainOf({ kind: "deck", domain: "eng", folder: "x", section: 0 }),
    ).toBe("eng");
    expect(domainOf({ kind: "engram", domain: "eng", permalink: "a" })).toBe(
      "eng",
    );
  });

  it("turns a PlaceAddress into its canonical station, MANIFEST included", () => {
    // Mutation caught: the address handed back as a plain engram room
    // without running C3's canonicalisation, so the MANIFEST case never
    // turns into the bridge.
    const engram: PlaceAddress = { domain: "eng", permalink: "notes/x" };
    expect(stationOfPlace(engram)).toEqual({
      kind: "engram",
      domain: "eng",
      permalink: "notes/x",
    });
    const manifest: PlaceAddress = { domain: "eng", permalink: "MANIFEST" };
    expect(stationOfPlace(manifest)).toEqual({
      kind: "bridge",
      domain: "eng",
    });
    const nested: PlaceAddress = { domain: "eng", permalink: "notes/manifest" };
    expect(stationOfPlace(nested)).toEqual({
      kind: "engram",
      domain: "eng",
      permalink: "notes/manifest",
    });
  });
});
