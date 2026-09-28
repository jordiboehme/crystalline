/**
 * The stops a lift lists (M3 C7, C12, C24): the airlock's domains, the
 * bridge's decks and a deck's way up, sections and subfolders, each list in
 * the order the panel and the overlay read it.
 */
import { describe, expect, it } from "vitest";

import { sortLevels } from "../ui/levels";
import { folderDeck } from "./folders";
import {
  airlockStops,
  bridgeStops,
  byLabel,
  deckLabel,
  deckStops,
  moreLine,
} from "./lifts";
import liftsSource from "./lifts.ts?raw";
import type { LiftStop } from "./types";

describe("the lift stops (M3 C7, C12, C24)", () => {
  it("lists the airlock's domains by label with keys on the private ones", () => {
    // Mutation caught: listing order kept, the key dropped, `here` on every stop.
    const stops = airlockStops(
      [
        { name: "zeta", private: false },
        { name: "Alpha", private: true },
      ],
      "zeta",
    );
    expect(stops.map((s) => s.label)).toEqual(["Alpha", "zeta"]);
    expect(stops.map((s) => s.key)).toEqual([true, false]);
    expect(stops.map((s) => s.here)).toEqual([false, true]);
    expect(stops[0]?.to).toEqual({ kind: "bridge", domain: "Alpha" });
  });

  it("puts the airlock and the root deck first on the bridge, then the folders by name", () => {
    // Mutation caught: the root deck listed when the root holds only the
    // MANIFEST, folders in the tree's order, AIRLOCK missing.
    const stops = bridgeStops("eng", ["zoo", "Alpha"], true);
    expect(stops.map((s) => s.label)).toEqual([
      "AIRLOCK",
      "DECK 1",
      deckLabel("eng", "Alpha"),
      deckLabel("eng", "zoo"),
    ]);
    expect(stops[1]?.to).toEqual({
      kind: "deck",
      domain: "eng",
      folder: "",
      section: 0,
    });
    expect(bridgeStops("eng", [], false).map((s) => s.label)).toEqual([
      "AIRLOCK",
    ]);
  });

  it("gives a nested deck the bridge, up, its sections and its subfolders", () => {
    // Mutation caught: UP to the wrong parent, UP on a top-level deck, a
    // one-section deck listing its section, a subfolder's path not joined.
    const stops = deckStops("eng", "a/b", ["A-F", "G-Z"], 1, ["c"]);
    expect(stops.map((s) => s.label)).toEqual([
      "BRIDGE",
      "UP",
      "SECTION A-F",
      "SECTION G-Z",
      deckLabel("eng", "a/b/c"),
    ]);
    expect(stops[1]?.to).toEqual({
      kind: "deck",
      domain: "eng",
      folder: "a",
      section: 0,
    });
    expect(stops[4]?.to).toEqual({
      kind: "deck",
      domain: "eng",
      folder: "a/b/c",
      section: 0,
    });
    expect(stops.filter((s) => s.here).map((s) => s.label)).toEqual([
      "SECTION G-Z",
    ]);
    expect(deckStops("eng", "a", ["A-Z"], 0, []).map((s) => s.label)).toEqual([
      "BRIDGE",
    ]);
  });

  it("pins every stop whole: its label, where it rides, its key and its mark", () => {
    // Mutation caught: a section stop riding to section 0 instead of its
    // own, the AIRLOCK stop riding to the bridge, the BRIDGE stop riding to
    // the airlock, `key: true` on a deck stop.
    const stop = (
      label: string,
      to: LiftStop["to"],
      key = false,
      here = false,
    ): LiftStop => ({ label, to, key, here });
    const deck = (folder: string, section = 0) =>
      ({ kind: "deck", domain: "eng", folder, section }) as const;
    expect(
      airlockStops(
        [
          { name: "zeta", private: false },
          { name: "Alpha", private: true },
        ],
        "zeta",
      ),
    ).toEqual([
      stop("Alpha", { kind: "bridge", domain: "Alpha" }, true),
      stop("zeta", { kind: "bridge", domain: "zeta" }, false, true),
    ]);
    expect(bridgeStops("eng", ["zoo", "Alpha"], true)).toEqual([
      stop("AIRLOCK", { kind: "airlock" }),
      stop("DECK 1", deck("")),
      stop(deckLabel("eng", "Alpha"), deck("Alpha")),
      stop(deckLabel("eng", "zoo"), deck("zoo")),
    ]);
    expect(deckStops("eng", "a/b", ["A-F", "G-Z"], 1, ["c"])).toEqual([
      stop("BRIDGE", { kind: "bridge", domain: "eng" }),
      stop("UP", deck("a")),
      stop("SECTION A-F", deck("a/b", 0)),
      stop("SECTION G-Z", deck("a/b", 1), false, true),
      stop(deckLabel("eng", "a/b/c"), deck("a/b/c")),
    ]);
  });

  it("sorts the bridge's folders and a deck's subfolders by name, not by their deck numbers", () => {
    // Mutation caught: the folders sorted by their `DECK <n> <NAME>` label,
    // which orders them by a hashed deck number. The names are picked so
    // that their deck numbers run the other way round, on the bridge and
    // one level down.
    const names = ["romeo", "Alpha"];
    const top = names.map((n) => deckLabel("eng", n));
    const nested = names.map((n) => deckLabel("eng", `a/${n}`));
    expect([...top].sort(byLabel)).toEqual(top);
    expect([...nested].sort(byLabel)).toEqual(nested);
    expect(bridgeStops("eng", names, false).map((s) => s.label)).toEqual([
      "AIRLOCK",
      deckLabel("eng", "Alpha"),
      deckLabel("eng", "romeo"),
    ]);
    expect(
      deckStops("eng", "a", ["A-Z"], 0, names).map((s) => s.label),
    ).toEqual([
      "BRIDGE",
      deckLabel("eng", "a/Alpha"),
      deckLabel("eng", "a/romeo"),
    ]);
  });

  it("labels a deck by its number and upper-cased folder name, and the overflow line", () => {
    // Mutation caught: the root deck labelled with a trailing space, the
    // full path in the label instead of the last segment, the name left
    // in its own case, the overflow line spelled apart from `+N MORE`.
    expect(deckLabel("eng", "")).toBe("DECK 1");
    expect(deckLabel("eng", "notes/My Stuff")).toBe(
      `DECK ${String(folderDeck("eng", "notes/My Stuff"))} MY STUFF`,
    );
    expect(moreLine(7)).toBe("+7 MORE");
  });

  it("sorts IDCLEV's rows with the same comparator", () => {
    // Mutation caught: `sortLevels` keeping a comparator of its own that drifts.
    const labels = ["b", "B", "a", "A"];
    expect(sortLevels(labels)).toEqual([...labels].sort(byLabel));
    expect([...labels].sort(byLabel)).toEqual(["A", "a", "B", "b"]);
  });

  it("keeps lifts.ts on the generator side: no ui, render or session imports", () => {
    // Mutation caught: an import of `ui/levels.ts` into the generator side.
    expect(liftsSource).not.toMatch(
      /\b(?:from|import)\s*\(?\s*["'](?:\.\.\/(?:ui|render)(?:\/[^"']*)?|\.\/(?:move|malfunction|interact|box|arrival|station))["']/,
    );
  });
});
