/**
 * Every spelling of a domain, mapped to the local name the app routes by.
 *
 * The rules are the server's (`NameTable` in `core/src/names.rs`), and each
 * case below is one of them: a local name always wins, a canonical name maps
 * unless a local name holds it or two domains claim it, an alias maps unless
 * the spelling is taken, and matching is on the exact spelling.
 */

import { describe, expect, it } from "vitest";

import { domainSpellings, type DomainNameRow } from "./domainNames";

function row(
  name: string,
  canonicalName: string | null = null,
  aliases: string[] = [],
  shadowed = false,
): DomainNameRow {
  return { name, canonicalName, aliases, shadowed };
}

describe("domain spellings", () => {
  it("maps a local name to itself", () => {
    const names = domainSpellings([row("eng"), row("ops", "ops")]);
    expect(names.get("eng")).toBe("eng");
    expect(names.get("ops")).toBe("ops");
  });

  it("maps a canonical name to the domain that declares it", () => {
    const names = domainSpellings([row("moonbase", "moon")]);
    expect(names.get("moon")).toBe("moonbase");
    expect(names.get("moonbase")).toBe("moonbase");
  });

  it("maps an alias to the domain that answers to it", () => {
    const names = domainSpellings([row("moonbase", "moon", ["lunar"])]);
    expect(names.get("lunar")).toBe("moonbase");
  });

  it("gives a shadowed canonical name to the domain registered under it", () => {
    const names = domainSpellings([
      row("moon"),
      row("moonbase", "moon", [], true),
    ]);
    expect(names.get("moon")).toBe("moon");
  });

  it("keeps a local name ahead of a canonical name the listing did not flag", () => {
    // The same rule without the flag: the order of the listing and the flag
    // do not decide it, the local name does.
    const names = domainSpellings([row("moonbase", "moon"), row("moon")]);
    expect(names.get("moon")).toBe("moon");
  });

  it("resolves a canonical name two domains claim nowhere", () => {
    const names = domainSpellings([row("a", "moon"), row("b", "moon")]);
    expect(names.has("moon")).toBe(false);
  });

  it("drops an alias another domain holds as a local name", () => {
    const names = domainSpellings([row("moonbase", null, ["eng"]), row("eng")]);
    expect(names.get("eng")).toBe("eng");
  });

  it("drops an alias another domain holds as a canonical name", () => {
    const names = domainSpellings([
      row("moonbase", null, ["moon"]),
      row("lunar", "moon"),
    ]);
    expect(names.get("moon")).toBe("lunar");
  });

  it("drops an alias that is a contested canonical name", () => {
    const names = domainSpellings([
      row("a", "moon"),
      row("b", "moon"),
      row("c", null, ["moon"]),
    ]);
    expect(names.has("moon")).toBe(false);
  });

  it("drops an alias two domains list", () => {
    const names = domainSpellings([
      row("a", null, ["old"]),
      row("b", null, ["old"]),
    ]);
    expect(names.has("old")).toBe(false);
  });

  it("counts an alias a domain lists twice once", () => {
    const names = domainSpellings([row("moonbase", null, ["old", "old"])]);
    expect(names.get("old")).toBe("moonbase");
  });

  it("knows nothing of a name no domain answers to", () => {
    const names = domainSpellings([row("moonbase", "moon", ["lunar"])]);
    expect(names.has("mars")).toBe(false);
  });

  it("matches the exact spelling only", () => {
    const names = domainSpellings([row("moonbase", "moon", ["lunar"])]);
    expect(names.has("Moon")).toBe(false);
    expect(names.has("MoonBase")).toBe(false);
    expect(names.has("LUNAR")).toBe(false);
    expect(names.has(" moon")).toBe(false);
  });
});
