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
  it("mirrors the engram route under /game", () => {
    expect(gameEngramRoute("eng", "notes/deep/gamma")).toBe(
      "/game/d/eng/e/notes/deep/gamma",
    );
  });

  it("encodes segments the way engramRoute does", () => {
    expect(gameEngramRoute("my eng", "a b/c d")).toBe(
      "/game/d/my%20eng/e/a%20b/c%20d",
    );
    expect(gameEngramRoute("my eng", "a b/c d")).toBe(
      `/game${engramRoute("my eng", "a b/c d")}`,
    );
  });
});

describe("fluidRouteOf", () => {
  it("strips the /game prefix", () => {
    expect(fluidRouteOf("/game/d/eng/e/a")).toBe("/d/eng/e/a");
  });

  it("gives the root for the bare game path", () => {
    expect(fluidRouteOf("/game")).toBe("/");
    expect(fluidRouteOf("/game/")).toBe("/");
  });

  it("round-trips a game engram route", () => {
    expect(fluidRouteOf(gameEngramRoute("eng", "x y/z"))).toBe(
      engramRoute("eng", "x y/z"),
    );
  });

  it("leaves a path that is not under /game alone", () => {
    expect(fluidRouteOf("/gamer/x")).toBe("/gamer/x");
    expect(fluidRouteOf("/d/eng/e/a")).toBe("/d/eng/e/a");
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
      "/game/d/platform%20eng/e/manifest",
    );
  });
});
