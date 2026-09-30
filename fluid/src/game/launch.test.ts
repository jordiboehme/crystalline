/**
 * The launch hook the C64 screen imports (M4 C1, C4): where a Fluid page
 * launches into, and the sound context primed inside the launching gesture.
 */

import { describe, expect, it } from "vitest";

import { addressOfGameLocation, gameRouteOf } from "./paths";
import {
  GAME_PREFIX,
  gamePathOf,
  primeAudio,
  releasePrimedAudio,
  takePrimedAudio,
} from "./launch";

/** The module's own text, read the way `world/neighbours.test.ts` reads source (F26). */
const LAUNCH_SOURCE =
  Object.values(
    import.meta.glob<string>("./launch.ts", {
      query: "?raw",
      import: "default",
      eager: true,
    }),
  )[0] ?? "";

describe("the launch path (M4 C1)", () => {
  it("lands every Fluid route where the spec says", () => {
    // Mutation caught: the search dropped (a deck launches as its bridge),
    // "/" giving "/%CF%80/", a route outside the table not landing in the
    // airlock.
    const cases: [string, string, ReturnType<typeof addressOfGameLocation>][] =
      [
        ["/", "", { kind: "airlock" }],
        ["/d/eng", "", { kind: "bridge", domain: "eng" }],
        [
          "/d/eng",
          "?path=",
          { kind: "deck", domain: "eng", folder: "", section: 0 },
        ],
        [
          "/d/eng",
          "?path=My%20Notes",
          { kind: "deck", domain: "eng", folder: "My Notes", section: 0 },
        ],
        [
          "/d/eng/e/notes/deep/gamma",
          "",
          { kind: "engram", domain: "eng", permalink: "notes/deep/gamma" },
        ],
        ["/d/eng/edit/notes/x", "", { kind: "airlock" }],
        ["/d/eng/manifest/edit", "", { kind: "airlock" }],
        ["/draft/abc", "", { kind: "airlock" }],
        ["/search", "?q=x", { kind: "airlock" }],
        ["/graph", "", { kind: "airlock" }],
      ];
    expect(cases.length).toBeGreaterThan(0);
    for (const [pathname, search, expected] of cases) {
      const url = new URL(gamePathOf(pathname, search), "http://x");
      expect(addressOfGameLocation(url.pathname, url.search)).toEqual(expected);
    }
    expect(gamePathOf("/", "")).toBe(GAME_PREFIX);
    expect(gameRouteOf({ kind: "airlock" })).toBe(GAME_PREFIX);
  });

  it("imports nothing, so the main chunk carries no game module but this one", () => {
    // Mutation caught: an import added to launch.ts, a re-export
    // (`export { x } from "./paths"`) or a dynamic `import(...)` (each would
    // pull game code into the main chunk through ShatterGem).
    expect(LAUNCH_SOURCE.length).toBeGreaterThan(0);
    expect(LAUNCH_SOURCE).not.toMatch(/^\s*import\s/m);
    expect(LAUNCH_SOURCE).not.toMatch(/\bfrom\s*["']/);
    expect(LAUNCH_SOURCE).not.toMatch(/\bimport\s*\(/);
  });

  it("stays silent and hands nothing over without an AudioContext", () => {
    // Mutation caught: primeAudio throwing where the constructor is missing.
    expect(() => primeAudio()).not.toThrow();
    expect(takePrimedAudio()).toBeNull();
  });

  it("lends the same context until a new prime closes it (F29)", () => {
    // Mutation caught: the context cleared on the first take (StrictMode's
    // second mount would get none), an older context left open by a new
    // prime, release not closing it.
    const made: { closed: number }[] = [];
    class FakeContext {
      state = "running";
      closed = 0;
      resume() {
        return Promise.resolve();
      }
      close() {
        this.closed += 1;
        return Promise.resolve();
      }
      constructor() {
        made.push(this);
      }
    }
    const saved = (globalThis as { AudioContext?: unknown }).AudioContext;
    (globalThis as { AudioContext?: unknown }).AudioContext = FakeContext;
    try {
      primeAudio();
      expect(takePrimedAudio()).toBe(made[0]);
      expect(takePrimedAudio()).toBe(made[0]);
      primeAudio();
      expect(made[0]?.closed).toBe(1);
      expect(takePrimedAudio()).toBe(made[1]);
      releasePrimedAudio();
      expect(made[1]?.closed).toBe(1);
      expect(takePrimedAudio()).toBeNull();
    } finally {
      (globalThis as { AudioContext?: unknown }).AudioContext = saved;
    }
  });
});
