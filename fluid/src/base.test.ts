import { afterEach, describe, expect, it, vi } from "vitest";

import { basePathOf, documentHrefFrom, joinBase, stripBaseFrom } from "./base";

function doc(baseURI: string, hasBase: boolean) {
  return {
    baseURI,
    querySelector: (selector: string) =>
      selector === "base[href]" && hasBase ? ({} as Element) : null,
  } as Pick<Document, "baseURI" | "querySelector">;
}

describe("basePathOf", () => {
  it("is empty at the root", () => {
    expect(basePathOf(doc("https://example.com/", true))).toBe("");
  });

  it("is the prefix without its slash under a path", () => {
    expect(basePathOf(doc("https://example.com/crystalline/", true))).toBe(
      "/crystalline",
    );
  });

  it("the base path is read from the base tag and not from the page address", () => {
    // A deep link without a base tag would otherwise read as a prefix.
    expect(basePathOf(doc("https://example.com/d/x/e/a/b", false))).toBe("");
  });
});

describe("joinBase and stripBaseFrom", () => {
  it("put the prefix in front and take it off again", () => {
    expect(joinBase("/crystalline", "/d/x")).toBe("/crystalline/d/x");
    expect(joinBase("", "/d/x")).toBe("/d/x");
    expect(stripBaseFrom("/crystalline", "/crystalline/d/x")).toBe("/d/x");
    expect(stripBaseFrom("/crystalline", "/crystalline")).toBe("/");
    expect(stripBaseFrom("/crystalline", "/crystallinex")).toBe(
      "/crystallinex",
    );
    expect(stripBaseFrom("", "/d/x")).toBe("/d/x");
  });
});

describe("documentHrefFrom", () => {
  const here = "https://example.com/crystalline/d/x/e/notes/a";

  it("a fragment link stays on the page it is written on", () => {
    expect(documentHrefFrom("/crystalline", "#observations", here)).toBe(
      "/crystalline/d/x/e/notes/a#observations",
    );
    expect(
      documentHrefFrom("", "#observations", "https://example.com/d/x/e/a?q=1"),
    ).toBe("/d/x/e/a?q=1#observations");
  });

  it("a relative link resolves against the page, as it did before the base tag", () => {
    expect(documentHrefFrom("/crystalline", "../b", here)).toBe(
      "/crystalline/d/x/e/b",
    );
    expect(documentHrefFrom("", "b", "https://example.com/d/x/e/notes/a")).toBe(
      "/d/x/e/notes/b",
    );
  });

  it("an app path gets the base and an absolute url is left alone", () => {
    expect(documentHrefFrom("/crystalline", "/d/y", here)).toBe(
      "/crystalline/d/y",
    );
    expect(documentHrefFrom("", "/d/y", here)).toBe("/d/y");
    for (const absolute of [
      "https://other.example/x",
      "mailto:a@b.example",
      "//cdn.example/x",
    ]) {
      expect(documentHrefFrom("/crystalline", absolute, here)).toBe(absolute);
    }
  });
});

describe("at the root", () => {
  afterEach(() => {
    vi.resetModules();
  });

  it("the router and the API sit at the root of the host", async () => {
    document.head.querySelector("base")?.remove();
    vi.resetModules();
    const mod = await import("./base");
    expect(mod.BASE_PATH).toBe("");
    expect(mod.ROUTER_BASENAME).toBe("/");
    expect(mod.API_BASE).toBe("/api/v1");
    expect(mod.withBase("/d/x")).toBe("/d/x");
  });
});

describe("under a prefix", () => {
  afterEach(() => {
    document.head.querySelector("base")?.remove();
    vi.resetModules();
  });

  it("every address that leaves the router carries the prefix", async () => {
    const base = document.createElement("base");
    base.href = "/crystalline/";
    document.head.prepend(base);
    vi.resetModules();
    const mod = await import("./base");
    expect(mod.BASE_PATH).toBe("/crystalline");
    expect(mod.ROUTER_BASENAME).toBe("/crystalline");
    expect(mod.API_BASE).toBe("/crystalline/api/v1");
    expect(mod.absoluteUrl("/draft/dl_x")).toBe(
      `${window.location.origin}/crystalline/draft/dl_x`,
    );
  });

  it("ssoSignInUrl puts the base path in front of return_to under a prefix", async () => {
    const base = document.createElement("base");
    base.href = "/crystalline/";
    document.head.prepend(base);
    vi.resetModules();
    const sso = await import("./api/sso");
    expect(sso.ssoSignInUrl("/authorize?request=r1")).toBe(
      "/crystalline/api/v1/auth/oidc/login?return_to=%2Fcrystalline%2Fauthorize%3Frequest%3Dr1",
    );
  });

  it("ssoSignInUrl keeps a root return_to as written", async () => {
    const sso = await import("./api/sso");
    expect(sso.ssoSignInUrl("/authorize?request=r1")).toBe(
      "/api/v1/auth/oidc/login?return_to=%2Fauthorize%3Frequest%3Dr1",
    );
  });
});
