/**
 * The session: loading places, travelling between them, and the keys that
 * belong to the session rather than to a single room.
 *
 * jsdom has no WebGL, so every session here gets a stub renderer through the
 * `createRenderer` seam and never asks the canvas for a context. The loop
 * runs on a hand-cranked clock (`frames`), so a tick happens only when a
 * test asks for one. The client is a real `QueryClient` with only `api`
 * stubbed, and `navigate` writes the URL the way the router would, so "the
 * URL already shows this place" is tested against a real location.
 */

import { QueryClient } from "@tanstack/react-query";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { ApiProblem, api } from "../api/client";
import type { Answer } from "../test/harness";
import { answersFor, domainsResponse } from "../test/harness";
import { CHEAT_GAP_TICKS } from "./core/cheat";
import { TICK_MS, type Clock } from "./core/loop";
import { prefetchPlace } from "./data/source";
import { BLINK_CHANNELS, createBlink } from "./render/blink";
import type { Camera, Renderer } from "./render/renderer";
import {
  INVERT_KEY,
  NOTICE_MS,
  createSession,
  type HudSink,
  type PlaceLoader,
  type RendererFactory,
  type Session,
} from "./session";
import { CANNED_BRIDGE, galleryRoom } from "./world/canned";
import { NOT_FOUND, generateRoom } from "./world/generate";
import { wallFacingSpawn, wallPoint } from "./world/interact";
import { faultSeed, planRun, type FaultFrame } from "./world/malfunction";
import { MAX_PITCH, PLAYER_RADIUS } from "./world/move";
import type { Fixture, RoomSpec } from "./world/types";

vi.mock("../api/client", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../api/client")>();
  return { ...actual, api: vi.fn(), setCsrfToken: vi.fn() };
});

vi.mock("./data/source", async (importOriginal) => {
  const actual = await importOriginal<typeof import("./data/source")>();
  return { ...actual, prefetchPlace: vi.fn(actual.prefetchPlace) };
});

const apiMock = vi.mocked(api);
const prefetchMock = vi.mocked(prefetchPlace);

/** A detail payload in the engine's own shape, with one section. */
function detailResponse(
  permalink: string,
  title: string,
  relations: unknown[] = [],
) {
  return {
    domain: "eng",
    permalink,
    title,
    path: `${permalink}.md`,
    url: `crystalline://eng/${permalink}`,
    content: `# ${title}\n\n## Notes\n\nSomething worth keeping.\n`,
    checksum: "c0ffee",
    frontmatter: {
      engram_type: "decision",
      title,
      status: "stable",
      tags: [],
      extra: {},
    },
    observations: [],
    relations,
    links: [],
    inbound: { count: 0, refs: [] },
  };
}

const EMPTY_INBOUND = {
  total: 0,
  page: 1,
  limit: 24,
  count: 0,
  types: [],
  hits: [],
};

/** A graph of the one node the room is about. */
function graphOf(permalink: string, title: string) {
  return {
    nodes: [{ id: 1, domain: "eng", permalink, title }],
    edges: [],
    truncated: false,
  };
}

/** Every route a load of alpha or beta asks for, overridable. */
function serve(routes: Record<string, Answer> = {}) {
  apiMock.mockImplementation(
    answersFor({
      "/domains": domainsResponse,
      "/graph": (path) =>
        path.includes("beta")
          ? graphOf("beta", "Beta")
          : graphOf("alpha", "Alpha"),
      "/domains/eng/engrams/alpha": () => detailResponse("alpha", "Alpha"),
      "/domains/eng/engrams/beta": () => detailResponse("beta", "Beta"),
      "/domains/eng/inbound/alpha": () => EMPTY_INBOUND,
      "/domains/eng/inbound/beta": () => EMPTY_INBOUND,
      ...routes,
    }),
  );
}

/** A promise and the function that settles it. */
function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((r) => {
    resolve = r;
  });
  return { promise, resolve };
}

/** A renderer that records what it was asked to do. */
function stubRenderer() {
  return {
    setRoom: vi.fn<Renderer["setRoom"]>(),
    resize: vi.fn<Renderer["resize"]>(),
    draw: vi.fn<Renderer["draw"]>(),
    dispose: vi.fn<Renderer["dispose"]>(),
  };
}

/** A HUD sink whose every writer is a spy. */
function stubHud() {
  return {
    prompt: vi.fn<HudSink["prompt"]>(),
    status: vi.fn<HudSink["status"]>(),
    frame: vi.fn<HudSink["frame"]>(),
    notice: vi.fn<HudSink["notice"]>(),
    connector: vi.fn<HudSink["connector"]>(),
    reader: vi.fn<HudSink["reader"]>(),
  };
}

let client: QueryClient;
let renderer: ReturnType<typeof stubRenderer>;
let hud: ReturnType<typeof stubHud>;
let navigate: ReturnType<typeof vi.fn<(path: string) => void>>;
let openFluid: ReturnType<typeof vi.fn<(path: string) => void>>;
let sessions: Session[];
let now: number;
let pending: ((t: number) => void) | null;

/** The hand-cranked clock the loop runs on. */
const clock: Clock = {
  now: () => now,
  request(cb) {
    pending = cb;
    return 1;
  },
  cancel() {
    pending = null;
  },
};

/** Runs `n` frames of one tick each. */
function frames(n: number) {
  for (let i = 0; i < n; i++) {
    now += TICK_MS;
    const cb = pending;
    pending = null;
    cb?.(now);
  }
}

/**
 * A new session with the stubs of this test, on a fresh canvas unless one
 * is given, and with a factory that hands out `renderer` unless one is.
 */
function start(
  options: {
    client?: QueryClient | null;
    canvas?: HTMLCanvasElement;
    factory?: RendererFactory;
    load?: PlaceLoader;
    onLevels?: (open: boolean) => void;
  } = {},
): Session {
  const factory: RendererFactory =
    options.factory ??
    (() => ({
      renderer,
      color: "rgba8",
    }));
  const session = createSession({
    canvas: options.canvas ?? document.createElement("canvas"),
    client: options.client === undefined ? client : options.client,
    hud,
    navigate,
    openFluid,
    forceRgba8: false,
    createRenderer: factory,
    clock,
    ...(options.load === undefined ? {} : { load: options.load }),
    ...(options.onLevels === undefined ? {} : { onLevels: options.onLevels }),
  });
  sessions.push(session);
  return session;
}

/** The rooms `setRoom` was handed, by permalink. */
function roomsSet(): string[] {
  return renderer.setRoom.mock.calls.map(([room]: [RoomSpec, unknown]) => {
    return room.permalink;
  });
}

/** The camera of the last frame drawn. */
function lastCamera(): Camera {
  const call = renderer.draw.mock.calls.at(-1);
  if (call === undefined) throw new Error("nothing drawn");
  return call[0];
}

function key(type: "keydown" | "keyup", code: string, repeat = false) {
  window.dispatchEvent(new KeyboardEvent(type, { code, repeat }));
}

/** Types a word by `KeyboardEvent.code`, each letter pressed and released. */
function type(word: string) {
  for (const ch of word) {
    const code = `Key${ch.toUpperCase()}`;
    key("keydown", code);
    key("keyup", code);
  }
}

/** Lets every settled promise run its callbacks. */
async function flush() {
  for (let i = 0; i < 10; i++) await Promise.resolve();
  await new Promise((r) => setTimeout(r, 0));
}

beforeEach(() => {
  apiMock.mockReset();
  prefetchMock.mockClear();
  client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  renderer = stubRenderer();
  hud = stubHud();
  navigate = vi.fn((path: string) => {
    window.history.replaceState(null, "", path);
  });
  openFluid = vi.fn();
  sessions = [];
  now = 0;
  pending = null;
  window.history.replaceState(null, "", "/");
  window.localStorage.clear();
});

afterEach(() => {
  for (const s of sessions) s.dispose();
  vi.restoreAllMocks();
  client.clear();
  window.localStorage.clear();
});

describe("go", () => {
  it("loads the place and navigates to its game route once", async () => {
    serve();
    const session = start();
    session.go({ domain: "eng", permalink: "alpha" });
    expect(hud.connector).toHaveBeenLastCalledWith(
      true,
      "alpha",
      expect.any(String),
    );
    await vi.waitFor(() => {
      expect(navigate).toHaveBeenCalledTimes(1);
    });
    expect(navigate).toHaveBeenCalledWith("/game/d/eng/e/alpha");
    expect(roomsSet()).toEqual(["alpha"]);
    expect(session.current).toEqual({ domain: "eng", permalink: "alpha" });
    expect(hud.connector).toHaveBeenLastCalledWith(
      false,
      "alpha",
      expect.any(String),
    );
  });

  it("lets a second go win over one still in flight", async () => {
    const alpha = deferred<unknown>();
    serve({ "/domains/eng/engrams/alpha": () => alpha.promise });
    const session = start();
    session.go({ domain: "eng", permalink: "alpha" });
    session.go({ domain: "eng", permalink: "beta" });
    await vi.waitFor(() => {
      expect(navigate).toHaveBeenCalledTimes(1);
    });
    alpha.resolve(detailResponse("alpha", "Alpha"));
    await flush();
    expect(navigate).toHaveBeenCalledTimes(1);
    expect(navigate).toHaveBeenCalledWith("/game/d/eng/e/beta");
    expect(roomsSet()).toEqual(["beta"]);
    expect(session.current).toEqual({ domain: "eng", permalink: "beta" });
  });

  it("drops a load that settles after dispose", async () => {
    const alpha = deferred<unknown>();
    serve({ "/domains/eng/engrams/alpha": () => alpha.promise });
    const session = start();
    session.go({ domain: "eng", permalink: "alpha" });
    session.dispose();
    // Nothing reaches the HUD once the session is gone.
    for (const writer of Object.values(hud)) writer.mockClear();
    alpha.resolve(detailResponse("alpha", "Alpha"));
    await flush();
    frames(5);
    expect(navigate).not.toHaveBeenCalled();
    expect(renderer.setRoom).not.toHaveBeenCalled();
    for (const writer of Object.values(hud)) {
      expect(writer).not.toHaveBeenCalled();
    }
  });

  it("takes the connector down when disposed while loading", () => {
    serve({ "/domains/eng/engrams/alpha": () => new Promise(() => {}) });
    const session = start();
    session.go({ domain: "eng", permalink: "alpha" });
    session.dispose();
    expect(hud.connector).toHaveBeenLastCalledWith(
      false,
      "alpha",
      expect.any(String),
    );
    expect(hud.reader).toHaveBeenLastCalledWith(null);
  });

  it("redraws the connector in the new look while loading", () => {
    serve({ "/domains/eng/engrams/alpha": () => new Promise(() => {}) });
    const session = start();
    session.go({ domain: "eng", permalink: "alpha" });
    expect(hud.connector).toHaveBeenLastCalledWith(true, "alpha", "aperture");
    key("keydown", "Digit4");
    frames(1);
    expect(hud.connector).toHaveBeenLastCalledWith(true, "alpha", "freescape");
  });

  it("warms the cache once for the place behind a door the player walks up to", async () => {
    serve({
      "/domains/eng/engrams/alpha": () =>
        detailResponse("alpha", "Alpha", [
          {
            line: 3,
            rel_type: "depends_on",
            resolved: true,
            target: { domain: null, target: "Beta" },
          },
        ]),
      "/graph": () => ({
        nodes: [
          { id: 1, domain: "eng", permalink: "alpha", title: "Alpha" },
          { id: 2, domain: "eng", permalink: "beta", title: "Beta" },
        ],
        edges: [{ from: 1, to: 2, rel_type: "depends_on" }],
        truncated: false,
      }),
    });
    const session = start();
    // Back from beta through a hatch: the player arrives 1.6 m in front of
    // the door to beta, well inside the approach distance.
    session.go(
      { domain: "eng", permalink: "alpha" },
      { via: "hatch", from: { domain: "eng", permalink: "beta" } },
    );
    await vi.waitFor(() => {
      expect(session.current?.permalink).toBe("alpha");
    });
    expect(prefetchMock).not.toHaveBeenCalled();
    frames(20);
    expect(prefetchMock).toHaveBeenCalledTimes(1);
    expect(prefetchMock).toHaveBeenCalledWith(client, "eng", "beta");
  });

  it("stays in the current room with ACCESS DENIED on a 403", async () => {
    serve({
      "/domains/eng/engrams/beta": () => {
        throw new ApiProblem(403, "no", "denied");
      },
    });
    const session = start();
    session.go({ domain: "eng", permalink: "alpha" });
    await vi.waitFor(() => {
      expect(session.current?.permalink).toBe("alpha");
    });
    session.go({ domain: "eng", permalink: "beta" });
    await vi.waitFor(() => {
      expect(hud.notice).toHaveBeenCalledWith("ACCESS DENIED");
    });
    expect(session.current).toEqual({ domain: "eng", permalink: "alpha" });
    expect(roomsSet()).toEqual(["alpha"]);
    expect(navigate).toHaveBeenCalledTimes(1);
  });

  it("shows SIGNAL LOST and ?FILE NOT FOUND for the other failed answers", async () => {
    serve({
      "/domains/eng/engrams/alpha": () => {
        throw new ApiProblem(404, "no", "missing");
      },
      "/domains/eng/engrams/beta": () => {
        throw new ApiProblem(0, "no", "offline");
      },
    });
    const session = start();
    session.go({ domain: "eng", permalink: "alpha" });
    await vi.waitFor(() => {
      expect(hud.notice).toHaveBeenCalledWith("?FILE NOT FOUND");
    });
    session.go({ domain: "eng", permalink: "beta" });
    await vi.waitFor(() => {
      expect(hud.notice).toHaveBeenCalledWith("SIGNAL LOST");
    });
    expect(session.current).toBe(null);
    expect(renderer.setRoom).not.toHaveBeenCalled();
  });

  it("does not navigate to the address the URL already shows", async () => {
    serve();
    window.history.replaceState(null, "", "/game/d/eng/e/alpha");
    const session = start();
    session.go({ domain: "eng", permalink: "alpha" });
    await vi.waitFor(() => {
      expect(session.current?.permalink).toBe("alpha");
    });
    expect(roomsSet()).toEqual(["alpha"]);
    expect(navigate).not.toHaveBeenCalled();
  });

  it("goes nowhere without a client and says the signal is lost", () => {
    const session = start({ client: null });
    session.showCanned(CANNED_BRIDGE);
    session.go({ domain: "station", permalink: "old-bridge" });
    expect(hud.notice).toHaveBeenCalledWith("SIGNAL LOST");
    expect(session.current).toEqual({
      domain: "station",
      permalink: "manifest",
    });
    expect(apiMock).not.toHaveBeenCalled();
  });
});

describe("a load in flight", () => {
  /** Beta's one reference into Alpha: a hatch back to Beta in Alpha's room. */
  const FROM_BETA = {
    total: 1,
    page: 1,
    limit: 24,
    count: 1,
    types: [{ rel: "relates_to", count: 1 }],
    hits: [
      {
        domain: "eng",
        permalink: "beta",
        title: "Beta",
        path: "beta.md",
        rel: "relates_to",
        status: "stable",
      },
    ],
  };

  it("forgets an E pressed while loading instead of using it in the new room", async () => {
    const alpha = deferred<unknown>();
    serve({
      "/domains/eng/engrams/alpha": () => alpha.promise,
      "/domains/eng/inbound/alpha": () => FROM_BETA,
    });
    const session = start();
    // Through a door from Beta: the player arrives in front of the hatch
    // back to Beta, facing into the room.
    session.go(
      { domain: "eng", permalink: "alpha" },
      { via: "door", from: { domain: "eng", permalink: "beta" } },
    );
    frames(2);
    key("keydown", "KeyE");
    key("keyup", "KeyE");
    frames(2);
    alpha.resolve(detailResponse("alpha", "Alpha"));
    await vi.waitFor(() => {
      expect(session.current?.permalink).toBe("alpha");
    });
    // Turn round to the hatch: a stale E would crawl back the moment it is
    // in front of the player.
    const offered = () =>
      hud.prompt.mock.calls.at(-1)?.[0] === "E CRAWL Beta relates_to";
    key("keydown", "ArrowLeft");
    for (let i = 0; i < 60 && !offered(); i++) frames(1);
    key("keyup", "ArrowLeft");
    frames(5);
    expect(hud.prompt).toHaveBeenLastCalledWith("E CRAWL Beta relates_to");
    expect(hud.connector).toHaveBeenLastCalledWith(
      false,
      "alpha",
      expect.any(String),
    );
    expect(roomsSet()).toEqual(["alpha"]);

    // A fresh E in the new room still crawls back.
    key("keydown", "KeyE");
    frames(1);
    expect(hud.connector).toHaveBeenLastCalledWith(
      true,
      "Beta",
      expect.any(String),
    );
  });

  it("forgets an E pressed while loading when the load fails", async () => {
    const beta = deferred<unknown>();
    serve({ "/domains/eng/engrams/beta": () => beta.promise });
    const session = start();
    session.showCanned(CANNED_BRIDGE);
    frames(1);
    walkToScope();
    session.go({ domain: "eng", permalink: "beta" });
    frames(2);
    key("keydown", "KeyE");
    key("keyup", "KeyE");
    frames(2);
    beta.resolve(Promise.reject(new ApiProblem(403, "no", "denied")));
    await vi.waitFor(() => {
      expect(hud.notice).toHaveBeenCalledWith("ACCESS DENIED");
    });
    // Still in front of the Scope terminal: the E pressed for the load's
    // room must not open the reader here.
    frames(5);
    expect(session.current?.permalink).toBe("manifest");
    expect(hud.reader).not.toHaveBeenCalledWith(
      expect.objectContaining({ title: "Station Crystalline" }),
    );
    expect(hud.prompt).toHaveBeenLastCalledWith("E READ Scope");
  });
});

describe("a room the renderer refuses", () => {
  it("keeps the player in the old room and says ?LOAD ERROR", async () => {
    serve();
    const session = start();
    session.go({ domain: "eng", permalink: "alpha" });
    await vi.waitFor(() => {
      expect(session.current?.permalink).toBe("alpha");
    });
    frames(3);
    const before = eyeAt();
    renderer.setRoom.mockImplementation(() => {
      throw new Error("room needs 999 texture layers, the GPU holds 256");
    });
    session.go({ domain: "eng", permalink: "beta" });
    await vi.waitFor(() => {
      expect(hud.notice).toHaveBeenCalledWith("?LOAD ERROR");
    });
    expect(session.current).toEqual({ domain: "eng", permalink: "alpha" });
    expect(navigate).toHaveBeenCalledTimes(1);
    expect(navigate).toHaveBeenLastCalledWith("/game/d/eng/e/alpha");
    // The player stands where they stood, in Alpha, and still walks there.
    frames(3);
    expect(eyeAt()[0]).toBeCloseTo(before[0], 6);
    expect(eyeAt()[1]).toBeCloseTo(before[1], 6);
    expect(hud.status).toHaveBeenLastCalledWith(
      expect.stringMatching(/^ALPHA {2}\|/),
    );
    key("keydown", "KeyW");
    frames(5);
    key("keyup", "KeyW");
    expect(eyeAt()[1]).toBeLessThan(before[1] - 0.5);
  });

  it("keeps the look the renderer refuses the room in from being taken", () => {
    const session = start({ client: null });
    session.showCanned(CANNED_BRIDGE);
    frames(1);
    renderer.setRoom.mockImplementation(() => {
      throw new Error("no");
    });
    key("keydown", "Digit4");
    frames(1);
    expect(hud.notice).toHaveBeenLastCalledWith("?LOAD ERROR");
    expect(hud.status).toHaveBeenLastCalledWith(
      expect.stringContaining("APERTURE"),
    );
    expect(session.current?.permalink).toBe("manifest");
  });
});

describe("the blink banks", () => {
  it("hands the renderer one session-wide blink state, ticked once per tick", () => {
    start({ client: null }).showCanned(CANNED_BRIDGE);
    frames(1);
    const blinkOf = (i: number): Float32Array => {
      const call = renderer.draw.mock.calls.at(i);
      if (call === undefined) throw new Error("nothing drawn");
      return call[5];
    };
    const first = blinkOf(-1);
    expect(first).toBeInstanceOf(Float32Array);
    expect(first).toHaveLength(BLINK_CHANNELS);
    // A fresh state ticked in step with the session's reads the same
    // gains: find the session's tick count, then walk on together.
    const reference = createBlink();
    let ticks = 0;
    while (
      ticks < 50 &&
      Array.from(reference.gains).join() !== Array.from(first).join()
    ) {
      reference.tick();
      ticks++;
    }
    expect(ticks).toBeLessThan(50);
    frames(40);
    for (let i = 0; i < 40; i++) reference.tick();
    expect(Array.from(blinkOf(-1))).toEqual(Array.from(reference.gains));
    // The same array every frame, not a copy per room or per draw.
    expect(blinkOf(-1)).toBe(first);
  });
});

describe("the GPU context", () => {
  it("pauses on a lost context and rebuilds the renderer on restore", () => {
    const canvas = document.createElement("canvas");
    const factory = vi.fn<RendererFactory>(() => ({
      renderer,
      color: "rgba8",
    }));
    const session = start({ client: null, canvas, factory });
    session.showCanned(CANNED_BRIDGE);
    frames(2);
    expect(renderer.draw).toHaveBeenCalled();
    expect(factory).toHaveBeenCalledTimes(1);

    // Lost: the default is prevented (or the browser never restores it),
    // the loop stops and the notice says why the screen froze.
    const lost = new Event("webglcontextlost", { cancelable: true });
    canvas.dispatchEvent(lost);
    expect(lost.defaultPrevented).toBe(true);
    // The loop cancelled its next frame: nothing is waiting on the clock.
    expect(pending).toBeNull();
    expect(hud.notice).toHaveBeenLastCalledWith(
      "SIGNAL LOST - WAITING FOR THE GPU",
    );
    renderer.draw.mockClear();
    frames(5);
    expect(renderer.draw).not.toHaveBeenCalled();

    // Restored: a fresh renderer, handed the room the player is in, sized,
    // and drawing again; the old one is not touched.
    const first = renderer;
    const second = stubRenderer();
    factory.mockImplementation(() => ({ renderer: second, color: "rgba8" }));
    canvas.dispatchEvent(new Event("webglcontextrestored"));
    expect(factory).toHaveBeenCalledTimes(2);
    expect(second.setRoom).toHaveBeenCalledTimes(1);
    expect(second.setRoom.mock.calls[0]?.[0].permalink).toBe("manifest");
    expect(second.resize).toHaveBeenCalledTimes(1);
    expect(hud.notice).toHaveBeenLastCalledWith(null);
    expect(pending).not.toBeNull();
    frames(2);
    expect(second.draw).toHaveBeenCalled();
    expect(first.draw).not.toHaveBeenCalled();
    expect(session.current?.permalink).toBe("manifest");
  });
});

describe("notices", () => {
  it("takes an in-room failure notice down after three seconds", () => {
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });
    try {
      const session = start({ client: null });
      session.showCanned(CANNED_BRIDGE);
      session.go({ domain: "station", permalink: "old-bridge" });
      expect(hud.notice).toHaveBeenLastCalledWith("SIGNAL LOST");
      vi.advanceTimersByTime(NOTICE_MS - 1);
      expect(hud.notice).toHaveBeenLastCalledWith("SIGNAL LOST");
      vi.advanceTimersByTime(1);
      expect(hud.notice).toHaveBeenLastCalledWith(null);
    } finally {
      vi.useRealTimers();
    }
  });
});

describe("showRoom", () => {
  it("shows a room built by hand with no load and no navigation", () => {
    const built: RoomSpec = {
      ...generateRoom(CANNED_BRIDGE),
      domain: "dev",
      permalink: "gallery",
      title: "Gallery",
    };
    const session = start();
    session.showRoom(built);
    expect(renderer.setRoom).toHaveBeenCalledTimes(1);
    expect(renderer.setRoom.mock.calls[0]?.[0]).toBe(built);
    expect(session.current).toEqual({ domain: "dev", permalink: "gallery" });
    frames(3);
    expect(lastCamera().eye[0]).toBeCloseTo((built.spawn.x + 0.5) * 2);
    expect(navigate).not.toHaveBeenCalled();
    expect(apiMock).not.toHaveBeenCalled();
  });

  it("takes the view's pitch after entering (the dev seams only)", () => {
    const built: RoomSpec = {
      ...generateRoom(CANNED_BRIDGE),
      domain: "dev",
      permalink: "gallery",
      title: "Gallery",
    };
    const session = start();
    session.showRoom(built, { pitch: -0.4 });
    frames(1);
    expect(lastCamera().pitch).toBeCloseTo(-0.4);
  });

  it("clamps the view's pitch to MAX_PITCH", () => {
    const built: RoomSpec = {
      ...generateRoom(CANNED_BRIDGE),
      domain: "dev",
      permalink: "gallery",
      title: "Gallery",
    };
    const session = start();
    session.showRoom(built, { pitch: -2 });
    frames(1);
    expect(lastCamera().pitch).toBeCloseTo(-MAX_PITCH);
  });
});

describe("build timing", () => {
  it("times the last renderer.setRoom and appends BUILD <ms> MS to the frame line", () => {
    renderer.setRoom.mockImplementation(() => {
      now += 7;
    });
    const session = start({ client: null });
    session.showCanned(CANNED_BRIDGE);
    frames(12);
    const last = hud.frame.mock.calls.at(-1)?.[0];
    expect(last).toContain("BUILD 7.0 MS");
  });
});

/** The eye's floor position in the last frame drawn. */
function eyeAt(): [number, number] {
  const eye = lastCamera().eye;
  return [eye[0], eye[2]];
}

/**
 * Walks the canned bridge's entrance to the Scope terminal on the west wall
 * (its wall point at x 0, z 9) with the keys alone: turn to face west, sidle
 * north to the terminal's row, walk up until the prompt offers it, and stand
 * still.
 */
function walkToScope() {
  key("keydown", "ArrowLeft");
  frames(18);
  key("keyup", "ArrowLeft");
  key("keydown", "KeyD");
  for (let i = 0; i < 60 && eyeAt()[1] > 9.2; i++) frames(1);
  key("keyup", "KeyD");
  frames(10);
  key("keydown", "KeyW");
  const offered = () =>
    hud.prompt.mock.calls.at(-1)?.[0]?.startsWith("E READ") === true;
  for (let i = 0; i < 80 && !offered(); i++) frames(1);
  key("keyup", "KeyW");
  frames(15);
  expect(hud.prompt).toHaveBeenLastCalledWith("E READ Scope");
}

describe("the reader", () => {
  it("opens at E, takes the keys while open and gives them back on close", () => {
    const session = start({ client: null });
    session.showCanned(CANNED_BRIDGE);
    frames(1);
    walkToScope();
    key("keydown", "KeyE");
    frames(1);
    expect(hud.reader).toHaveBeenLastCalledWith({
      title: "Station Crystalline",
      content: CANNED_BRIDGE.content,
      section: { heading: "Scope", occurrence: 0 },
      look: "aperture",
    });
    frames(10);

    // Open: no walking, and F and I belong to the reader.
    const still = eyeAt();
    key("keydown", "KeyW");
    frames(10);
    expect(eyeAt()[0]).toBeCloseTo(still[0], 6);
    expect(eyeAt()[1]).toBeCloseTo(still[1], 6);
    key("keydown", "KeyF");
    key("keydown", "KeyI");
    frames(2);
    expect(openFluid).not.toHaveBeenCalled();
    expect(window.localStorage.getItem(INVERT_KEY)).toBe(null);
    expect(hud.notice).not.toHaveBeenCalledWith("LOOK INVERTED");

    // Closed: the W still down from before is forgotten, a new press walks.
    session.closeReader();
    expect(hud.reader).toHaveBeenLastCalledWith(null);
    frames(5);
    expect(eyeAt()[0]).toBeCloseTo(still[0], 6);
    expect(openFluid).not.toHaveBeenCalled();
    key("keyup", "KeyW");
    key("keydown", "KeyW");
    frames(5);
    expect(eyeAt()[0]).not.toBeCloseTo(still[0], 2);
    key("keyup", "KeyW");
    key("keydown", "KeyF");
    frames(1);
    expect(openFluid).toHaveBeenCalledWith("/d/station/e/manifest");
  });

  it("is closed by travel and by showing a place", () => {
    const session = start({ client: null });
    session.showCanned(CANNED_BRIDGE);
    frames(1);
    walkToScope();
    key("keydown", "KeyE");
    frames(1);
    expect(hud.reader).toHaveBeenLastCalledWith(
      expect.objectContaining({ title: "Station Crystalline" }),
    );
    session.go({ domain: "station", permalink: "old-bridge" });
    expect(hud.reader).toHaveBeenLastCalledWith(null);

    key("keydown", "KeyE");
    frames(1);
    expect(hud.reader).toHaveBeenLastCalledWith(
      expect.objectContaining({ title: "Station Crystalline" }),
    );
    session.showCanned({ ...CANNED_BRIDGE, status: "archived" });
    expect(hud.reader).toHaveBeenLastCalledWith(null);
  });
});

describe("keys", () => {
  it("opens the current engram in Fluid on F", () => {
    const session = start({ client: null });
    session.showCanned(CANNED_BRIDGE);
    frames(1);
    key("keydown", "KeyF");
    frames(1);
    expect(openFluid).toHaveBeenCalledWith("/d/station/e/manifest");
  });

  it("inverts the vertical look on I and remembers it", () => {
    const first = start({ client: null });
    first.showCanned(CANNED_BRIDGE);
    frames(1);
    key("keydown", "ArrowUp");
    frames(4);
    expect(lastCamera().pitch).toBeGreaterThan(0);
    key("keyup", "ArrowUp");

    key("keydown", "KeyI");
    frames(1);
    expect(hud.notice).toHaveBeenCalledWith("LOOK INVERTED");
    expect(window.localStorage.getItem(INVERT_KEY)).toBe("1");
    const before = lastCamera().pitch;
    key("keydown", "ArrowUp");
    frames(4);
    expect(lastCamera().pitch).toBeLessThan(before);
    key("keyup", "ArrowUp");
    first.dispose();

    const second = start({ client: null });
    second.showCanned(CANNED_BRIDGE);
    frames(1);
    key("keydown", "ArrowUp");
    frames(4);
    expect(lastCamera().pitch).toBeLessThan(0);
    key("keyup", "ArrowUp");

    key("keydown", "KeyI");
    frames(1);
    expect(hud.notice).toHaveBeenCalledWith("LOOK NORMAL");
    expect(window.localStorage.getItem(INVERT_KEY)).toBe("0");
  });

  it("takes the normal look when the storage cannot be read", () => {
    window.localStorage.setItem(INVERT_KEY, "1");
    vi.spyOn(Storage.prototype, "getItem").mockImplementation(() => {
      throw new Error("blocked");
    });
    const session = start({ client: null });
    session.showCanned(CANNED_BRIDGE);
    frames(1);
    key("keydown", "ArrowUp");
    frames(4);
    expect(lastCamera().pitch).toBeGreaterThan(0);
    key("keyup", "ArrowUp");
  });
});

describe("the level cheat", () => {
  let levels: ReturnType<typeof vi.fn<(open: boolean) => void>>;
  beforeEach(() => {
    levels = vi.fn<(open: boolean) => void>();
  });

  /** A session on the canned bridge with the cheat on, one tick in. */
  function onBridge(): Session {
    const session = start({ client: null, onLevels: levels });
    session.showCanned(CANNED_BRIDGE);
    frames(1);
    return session;
  }

  it("opens the level select on idclev and takes the word's I back", () => {
    const session = onBridge();
    type("idclev");
    frames(1);
    expect(levels).toHaveBeenCalledTimes(1);
    expect(levels).toHaveBeenLastCalledWith(true);
    // The I toggled the look on its way through; the match took it back
    // and took its notice down with it (C6).
    expect(window.localStorage.getItem(INVERT_KEY)).toBe("0");
    expect(hud.notice).toHaveBeenLastCalledWith(null);

    session.closeLevels();
    expect(levels).toHaveBeenLastCalledWith(false);
    key("keydown", "ArrowUp");
    frames(4);
    expect(lastCamera().pitch).toBeGreaterThan(0);
    key("keyup", "ArrowUp");
  });

  it("reads the word across ticks, letter by letter", () => {
    onBridge();
    for (const ch of "idclev") {
      type(ch);
      frames(3);
    }
    expect(levels).toHaveBeenLastCalledWith(true);
    expect(window.localStorage.getItem(INVERT_KEY)).toBe("0");
    expect(hud.notice).toHaveBeenLastCalledWith(null);
  });

  it("forgets a word paused for longer than the gap", () => {
    onBridge();
    type("idc");
    frames(CHEAT_GAP_TICKS + 2);
    type("lev");
    frames(1);
    expect(levels).not.toHaveBeenCalled();
    // A plain I: its toggle stays.
    expect(window.localStorage.getItem(INVERT_KEY)).toBe("1");
  });

  it("swallows only the E right after i d c l, in front of a terminal", () => {
    const session = onBridge();
    walkToScope();
    hud.reader.mockClear();
    type("idcle");
    frames(2);
    expect(hud.reader).not.toHaveBeenCalled();
    expect(hud.prompt).toHaveBeenLastCalledWith("E READ Scope");

    type("e");
    frames(1);
    expect(hud.reader).toHaveBeenLastCalledWith(
      expect.objectContaining({ title: "Station Crystalline" }),
    );
    session.closeReader();
    hud.reader.mockClear();

    type("idcxe");
    frames(1);
    expect(hud.reader).toHaveBeenLastCalledWith(
      expect.objectContaining({ title: "Station Crystalline" }),
    );
    expect(levels).not.toHaveBeenCalled();
  });

  it("does nothing without onLevels, as on the look demo and the gallery", () => {
    const plain = start({ client: null });
    plain.showCanned(CANNED_BRIDGE);
    frames(1);
    type("idclev");
    frames(1);
    // No select, and the I is a plain toggle that nothing takes back.
    expect(window.localStorage.getItem(INVERT_KEY)).toBe("1");
    expect(hud.notice).toHaveBeenLastCalledWith("LOOK INVERTED");

    walkToScope();
    type("idcle");
    frames(1);
    expect(hud.reader).toHaveBeenLastCalledWith(
      expect.objectContaining({ title: "Station Crystalline" }),
    );
  });

  // Review Focus 1.
  it("reads the word through auto-repeat and a held key", () => {
    const session = onBridge();
    key("keydown", "KeyW");
    for (const ch of "idclev") {
      const code = `Key${ch.toUpperCase()}`;
      key("keydown", code);
      key("keydown", code, true);
      key("keydown", "KeyW", true);
      key("keydown", code, true);
      key("keyup", code);
      frames(2);
    }
    expect(levels).toHaveBeenCalledTimes(1);
    expect(levels).toHaveBeenLastCalledWith(true);
    expect(window.localStorage.getItem(INVERT_KEY)).toBe("0");

    // Open: the held W walks no further, its repeats included. The walk's
    // momentum runs out first, as in the reader test. It was walking at
    // full speed when the select opened and loses only 45 % a tick, so it
    // takes about twenty ticks to fall below the checks' precision.
    frames(30);
    const still = eyeAt();
    key("keydown", "KeyW", true);
    frames(10);
    expect(eyeAt()[0]).toBeCloseTo(still[0], 6);
    expect(eyeAt()[1]).toBeCloseTo(still[1], 6);

    // Closed: the W taken on the way in is forgotten until pressed again.
    session.closeLevels();
    frames(5);
    expect(eyeAt()[0]).toBeCloseTo(still[0], 6);
    expect(eyeAt()[1]).toBeCloseTo(still[1], 6);
    key("keyup", "KeyW");
  });

  // Review Focus 2.
  it("ignores every key typed while the level select is open", () => {
    const session = onBridge();
    // In front of the Scope, so an E that got through would read it. The
    // walk's momentum runs out before the select opens.
    walkToScope();
    frames(20);
    expect(hud.prompt).toHaveBeenLastCalledWith("E READ Scope");
    type("idclev");
    frames(1);
    expect(levels).toHaveBeenLastCalledWith(true);
    expect(hud.reader).not.toHaveBeenCalled();
    const still = eyeAt();
    const pitch = lastCamera().pitch;
    const yaw = lastCamera().yaw;
    const rooms = renderer.setRoom.mock.calls.length;
    hud.status.mockClear();
    for (const code of [
      "KeyW",
      "KeyA",
      "KeyS",
      "KeyD",
      "ArrowLeft",
      "ArrowUp",
      "KeyE",
      "KeyF",
      "KeyI",
      "Digit1",
    ]) {
      key("keydown", code);
      frames(2);
      key("keyup", code);
    }
    type("idclev");
    frames(5);
    expect(eyeAt()[0]).toBeCloseTo(still[0], 6);
    expect(eyeAt()[1]).toBeCloseTo(still[1], 6);
    expect(lastCamera().pitch).toBeCloseTo(pitch, 6);
    expect(lastCamera().yaw).toBeCloseTo(yaw, 6);
    expect(levels).toHaveBeenCalledTimes(1);
    expect(window.localStorage.getItem(INVERT_KEY)).toBe("0");
    expect(openFluid).not.toHaveBeenCalled();
    expect(hud.reader).not.toHaveBeenCalled();
    expect(renderer.setRoom.mock.calls.length).toBe(rooms);
    // Digit1 would switch to the day shift look; the status still names
    // the look the select opened over.
    expect(hud.status).toHaveBeenCalled();
    for (const [text] of hud.status.mock.calls) {
      expect(text).toContain("APERTURE GRID");
    }

    // Closed: nothing typed inside comes back as a command or a step.
    session.closeLevels();
    frames(5);
    expect(eyeAt()[0]).toBeCloseTo(still[0], 6);
    expect(eyeAt()[1]).toBeCloseTo(still[1], 6);
    expect(openFluid).not.toHaveBeenCalled();
    expect(window.localStorage.getItem(INVERT_KEY)).toBe("0");
  });

  // Review Focus 3.
  it("reads no word while the CRT reader is open", () => {
    const session = onBridge();
    walkToScope();
    key("keydown", "KeyE");
    key("keyup", "KeyE");
    frames(1);
    expect(hud.reader).toHaveBeenLastCalledWith(
      expect.objectContaining({ title: "Station Crystalline" }),
    );
    type("idclev");
    frames(2);
    expect(levels).not.toHaveBeenCalled();
    expect(window.localStorage.getItem(INVERT_KEY)).toBe(null);

    session.closeReader();
    type("idclev");
    frames(1);
    expect(levels).toHaveBeenLastCalledWith(true);
    expect(hud.reader).toHaveBeenLastCalledWith(null);
  });

  it("jumps to a domain's bridge with an ordinary go and closes the select", async () => {
    serve({
      "/domains/eng/engrams/manifest": () => detailResponse("manifest", "Eng"),
      "/domains/eng/inbound/manifest": () => EMPTY_INBOUND,
    });
    const session = start({ onLevels: levels });
    session.go({ domain: "eng", permalink: "alpha" });
    await vi.waitFor(() => {
      expect(navigate).toHaveBeenCalledTimes(1);
    });
    frames(1);
    type("idclev");
    frames(1);
    expect(levels).toHaveBeenLastCalledWith(true);

    session.jump("eng");
    expect(levels).toHaveBeenLastCalledWith(false);
    expect(hud.connector).toHaveBeenLastCalledWith(
      true,
      "manifest",
      expect.any(String),
    );
    await vi.waitFor(() => {
      expect(navigate).toHaveBeenCalledTimes(2);
    });
    expect(navigate).toHaveBeenLastCalledWith("/game/d/eng/e/manifest");
    expect(session.current).toEqual({ domain: "eng", permalink: "manifest" });
    expect(roomsSet()).toEqual(["alpha", "manifest"]);
  });

  it("closes the select on a go from outside and on dispose", () => {
    const session = onBridge();
    type("idclev");
    frames(1);
    session.go({ domain: "station", permalink: "old-bridge" });
    expect(levels).toHaveBeenLastCalledWith(false);

    type("idclev");
    frames(1);
    expect(levels).toHaveBeenLastCalledWith(true);
    session.dispose();
    expect(levels).toHaveBeenLastCalledWith(false);
  });

  it("opens on the dark screen before the first room (C5)", async () => {
    serve({
      "/domains/eng/engrams/alpha": () => {
        throw new ApiProblem(404, "not found", "no alpha");
      },
    });
    const session = start({ onLevels: levels });
    session.go({ domain: "eng", permalink: "alpha" });
    await vi.waitFor(() => {
      expect(hud.notice).toHaveBeenLastCalledWith(NOT_FOUND);
    });
    type("idclev");
    frames(1);
    expect(levels).toHaveBeenLastCalledWith(true);
    // The standing notice comes back once the look notice is taken down.
    expect(hud.notice).toHaveBeenLastCalledWith(NOT_FOUND);
    expect(session.current).toBe(null);
  });
});

describe("malfunctions", () => {
  /** The index of the `n`th fixture of `kind`, in fixture order. */
  const nth = (room: RoomSpec, kind: Fixture["kind"], n: number) => {
    let seen = -1;
    const i = room.fixtures.findIndex((f) => f.kind === kind && ++seen === n);
    if (i < 0) throw new Error(`no ${kind} ${String(n)}`);
    return i;
  };

  /** The fixture at `i`, which must exist. */
  const fixtureAt = (room: RoomSpec, i: number): Fixture => {
    const f = room.fixtures[i];
    if (f === undefined) throw new Error(`no fixture ${String(i)}`);
    return f;
  };

  /** `room` with the player spawned in fixture `i`'s cell, facing it. */
  const before = (room: RoomSpec, i: number): RoomSpec => ({
    ...room,
    spawn: wallFacingSpawn(fixtureAt(room, i).slot),
  });

  const gallery = galleryRoom();
  const door0 = nth(gallery, "door", 0);
  const door1 = nth(gallery, "door", 1);
  const door3 = nth(gallery, "door", 3);
  const door4 = nth(gallery, "door", 4);
  const hatch0 = nth(gallery, "hatch", 0);
  const portal0 = nth(gallery, "portal", 0);

  /** The fault frames of every draw from call `from` on. */
  const faultsSince = (from = 0): ReadonlyMap<number, FaultFrame>[] =>
    renderer.draw.mock.calls.slice(from).map((c) => c[4]);

  /** The fault frames of the last draw. */
  const lastFaults = (): ReadonlyMap<number, FaultFrame> => {
    const call = renderer.draw.mock.calls.at(-1);
    if (call === undefined) throw new Error("nothing drawn");
    return call[4];
  };

  /** The door fractions of the last draw. */
  const lastDoors = (): ReadonlyMap<string, number> => {
    const call = renderer.draw.mock.calls.at(-1);
    if (call === undefined) throw new Error("nothing drawn");
    return call[3];
  };

  /** A loader that answers every travel with `kind` at once. */
  const failing =
    (kind: "missing" | "denied" | "offline"): PlaceLoader =>
    () =>
      Promise.resolve({ kind });

  /** How often the connector went up. */
  const connectorUps = () =>
    hud.connector.mock.calls.filter(([active]) => active).length;

  /** The HUD prompts written from call `from` on. */
  const promptsSince = (from: number) =>
    hud.prompt.mock.calls.slice(from).map(([text]) => text);

  /** How far in front of fixture `i`'s wall the eye of the last frame is. */
  const depthAt = (room: RoomSpec, i: number) => {
    const w = wallPoint(fixtureAt(room, i).slot);
    const eye = lastCamera().eye;
    return (eye[0] - w.x) * w.inward[0] + (eye[2] - w.z) * w.inward[1];
  };

  /** Presses E for one frame. */
  const pressE = () => {
    key("keydown", "KeyE");
    frames(1);
    key("keyup", "KeyE");
  };

  /**
   * Holds W until a travel starts, then lets go: until the connector goes
   * up, or, with `loads` false (no client and no loader, where `go` fails
   * at once), until a notice goes up.
   */
  const walkIn = (loads = true) => {
    const ups = connectorUps();
    const notices = hud.notice.mock.calls.length;
    const started = () =>
      loads
        ? connectorUps() > ups
        : hud.notice.mock.calls.slice(notices).some(([t]) => t !== null);
    key("keydown", "KeyW");
    for (let i = 0; i < 80 && !started(); i++) frames(1);
    key("keyup", "KeyW");
    expect(started()).toBe(true);
    expect(connectorUps()).toBe(loads ? ups + 1 : ups);
  };

  it("stutters a ?FILE NOT FOUND door while the player stands near it", () => {
    const session = start({ client: null });
    session.showRoom(before(gallery, door4));
    frames(60);
    const opens = faultsSince().flatMap((m) => {
      const f = m.get(door4);
      return f === undefined ? [] : [f.open];
    });
    expect(opens.some((o) => o >= 0.3 && o <= 0.54)).toBe(true);
    expect(hud.prompt).toHaveBeenCalledWith("SEALED ?FILE NOT FOUND");
  });

  it("leaves a NO ROUTE door still", () => {
    const session = start({ client: null });
    session.showRoom(before(gallery, door3));
    frames(60);
    expect(faultsSince().some((m) => m.has(door3))).toBe(false);
    expect(hud.prompt).toHaveBeenCalledWith("SEALED NO ROUTE");
  });

  it("a broken door stays shut to collision and never carries the player", async () => {
    const sealed = start({ client: null });
    sealed.showRoom(before(gallery, door4));
    key("keydown", "KeyW");
    for (let i = 0; i < 175; i++) {
      frames(1);
      expect(depthAt(gallery, door4)).toBeGreaterThanOrEqual(
        PLAYER_RADIUS - 1e-6,
      );
    }
    key("keyup", "KeyW");
    expect(hud.connector).not.toHaveBeenCalledWith(
      true,
      expect.anything(),
      expect.anything(),
    );
    expect(faultsSince().some((m) => m.has(door4))).toBe(true);
    sealed.dispose();

    const open = start({ client: null, load: failing("missing") });
    open.showRoom(before(gallery, door0));
    key("keydown", "KeyW");
    for (let i = 0; i < 80 && connectorUps() === 0; i++) {
      frames(1);
      expect(depthAt(gallery, door0)).toBeGreaterThanOrEqual(
        PLAYER_RADIUS - 1e-6,
      );
    }
    expect(connectorUps()).toBe(1);
    await flush();
    expect(hud.notice).toHaveBeenCalledWith("?FILE NOT FOUND");
    const calls = hud.connector.mock.calls.length;
    for (let i = 0; i < 175; i++) {
      frames(1);
      expect(depthAt(gallery, door0)).toBeGreaterThanOrEqual(
        PLAYER_RADIUS - 1e-6,
      );
    }
    key("keyup", "KeyW");
    expect(hud.connector.mock.calls.length).toBe(calls);
    expect(connectorUps()).toBe(1);
  });

  it("shuts a door that failed on travel, runs it once, then keeps it sealed", async () => {
    const session = start({ client: null, load: failing("denied") });
    session.showRoom(before(gallery, door1));
    pressE();
    frames(15);
    expect(lastDoors().get(`door:${String(door1)}`)).toBe(1);
    walkIn();
    await flush();
    expect(hud.connector).toHaveBeenLastCalledWith(
      false,
      expect.any(String),
      expect.any(String),
    );
    expect(hud.notice).toHaveBeenCalledWith("ACCESS DENIED");

    const mark = renderer.draw.mock.calls.length;
    frames(60);
    const draws = renderer.draw.mock.calls.slice(mark);
    const shut = draws.findIndex(
      (c) => c[3].get(`door:${String(door1)}`) === 0,
    );
    expect(shut).toBeGreaterThanOrEqual(0);
    expect(shut).toBeLessThan(12);
    const first = draws.findIndex((c) => c[4].has(door1));
    expect(first).toBeGreaterThanOrEqual(shut);
    expect(first).toBeLessThanOrEqual(shut + 2);
    // No fault frame while the door was still shutting.
    expect(draws.slice(0, shut).some((c) => c[4].has(door1))).toBe(false);
    const peak = Math.max(
      ...draws.flatMap((c) => {
        const f = c[4].get(door1);
        return f === undefined ? [] : [f.open];
      }),
    );
    expect(peak).toBeGreaterThanOrEqual(0.3);
    expect(peak).toBeLessThanOrEqual(0.54);
    expect(hud.prompt).toHaveBeenCalledWith("SEALED ACCESS DENIED");

    // E at the sealed door starts no travel and opens nothing.
    const ups = connectorUps();
    pressE();
    frames(20);
    expect(connectorUps()).toBe(ups);
    expect(lastDoors().get(`door:${String(door1)}`)).toBe(0);
  });

  it("pops a hatch lid once when its crawl fails", async () => {
    const session = start({ client: null, load: failing("missing") });
    session.showRoom(before(gallery, hatch0));
    frames(2);
    expect(hud.prompt).toHaveBeenLastCalledWith(
      expect.stringMatching(/^E CRAWL /),
    );
    pressE();
    expect(connectorUps()).toBe(1);
    await flush();
    const mark = renderer.draw.mock.calls.length;
    frames(2);
    expect(faultsSince(mark).some((m) => m.has(hatch0))).toBe(true);
    expect(hud.prompt).toHaveBeenLastCalledWith("SEALED ?FILE NOT FOUND");

    // The failed hatch carries no one: hatchTravel reads the failed map.
    pressE();
    frames(5);
    expect(connectorUps()).toBe(1);
  });

  it("collapses a portal that fails on travel", async () => {
    const session = start({ client: null, load: failing("missing") });
    session.showRoom(before(gallery, portal0));
    walkIn();
    await flush();
    const mark = renderer.draw.mock.calls.length;
    frames(60);
    expect(faultsSince(mark).some((m) => m.get(portal0)?.scale === 0)).toBe(
      true,
    );
    expect(hud.prompt).toHaveBeenLastCalledWith("SEALED ?FILE NOT FOUND");

    // Back off and walk in again: the failed portal carries no one, even
    // once the latch of the first travel is gone.
    key("keydown", "KeyS");
    frames(20);
    key("keyup", "KeyS");
    expect(depthAt(gallery, portal0)).toBeGreaterThan(1);
    key("keydown", "KeyW");
    frames(40);
    key("keyup", "KeyW");
    expect(depthAt(gallery, portal0)).toBeLessThan(0.5);
    expect(connectorUps()).toBe(1);
  });

  describe("only a missing or denied travel seals its way", () => {
    /**
     * Walks into door 0 of the gallery, waits for the answer and checks the
     * notice, then that the door never says SEALED, never runs a fault and
     * still opens for the player.
     */
    const walksIntoDoor0 = async (
      session: Session,
      notice: string,
      loads = true,
    ): Promise<void> => {
      session.showRoom(before(gallery, door0));
      walkIn(loads);
      const from = hud.prompt.mock.calls.length;
      const mark = renderer.draw.mock.calls.length;
      await flush();
      expect(hud.notice).toHaveBeenCalledWith(notice);
      frames(60);
      expect(
        promptsSince(from).some((p) => p?.startsWith("SEALED") === true),
      ).toBe(false);
      expect(faultsSince(mark).some((m) => m.has(door0))).toBe(false);
      expect(lastDoors().get(`door:${String(door0)}`)).toBe(1);
    };

    it("not SIGNAL LOST from the loader", async () => {
      await walksIntoDoor0(
        start({ client: null, load: failing("offline") }),
        "SIGNAL LOST",
      );
    });

    it("not a thrown ?LOAD ERROR", async () => {
      await walksIntoDoor0(
        start({ client: null, load: () => Promise.reject(new Error("x")) }),
        "?LOAD ERROR",
      );
    });

    it("not SIGNAL LOST with no client and no loader", async () => {
      await walksIntoDoor0(start({ client: null }), "SIGNAL LOST", false);
    });

    it("not an outside go that answers missing", async () => {
      const session = start({ client: null, load: failing("missing") });
      session.showRoom(before(gallery, door0));
      frames(20);
      const from = hud.prompt.mock.calls.length;
      const mark = renderer.draw.mock.calls.length;
      const door = fixtureAt(gallery, door0);
      if (door.kind !== "door" || door.address === null) {
        throw new Error("door 0 leads somewhere");
      }
      session.go(door.address);
      await flush();
      expect(hud.notice).toHaveBeenCalledWith("?FILE NOT FOUND");
      frames(60);
      expect(
        promptsSince(from).some((p) => p?.startsWith("SEALED") === true),
      ).toBe(false);
      // Only the ways sealed from the start run faults.
      const faulted = new Set(faultsSince(mark).flatMap((m) => [...m.keys()]));
      expect(faulted.has(door0)).toBe(false);
      expect(lastDoors().get(`door:${String(door0)}`)).toBe(1);
    });

    it("not a room the renderer refuses", async () => {
      let setRooms = 0;
      const refusing = stubRenderer();
      refusing.setRoom.mockImplementation(() => {
        if (++setRooms > 1) throw new Error("no layers left");
      });
      renderer = refusing;
      const session = start({
        client: null,
        load: () =>
          Promise.resolve({ kind: "place" as const, place: CANNED_BRIDGE }),
      });
      session.showRoom(before(gallery, door0));
      const here = session.current;
      walkIn();
      const from = hud.prompt.mock.calls.length;
      const mark = renderer.draw.mock.calls.length;
      await flush();
      expect(hud.notice).toHaveBeenCalledWith("?LOAD ERROR");
      expect(session.current).toEqual(here);
      frames(60);
      expect(
        promptsSince(from).some((p) => p?.startsWith("SEALED") === true),
      ).toBe(false);
      expect(faultsSince(mark).some((m) => m.has(door0))).toBe(false);
      expect(lastDoors().get(`door:${String(door0)}`)).toBe(1);
    });
  });

  it("a stale failure seals nothing in the room entered since", async () => {
    const answer = deferred<{ kind: "missing" }>();
    const session = start({ client: null, load: () => answer.promise });
    session.showRoom(before(gallery, door0));
    walkIn();
    session.showRoom(before(gallery, door4));
    answer.resolve({ kind: "missing" });
    await flush();
    frames(60);
    expect(faultsSince().some((m) => m.has(door0))).toBe(false);

    session.showRoom(before(gallery, door0));
    const from = hud.prompt.mock.calls.length;
    const mark = renderer.draw.mock.calls.length;
    frames(60);
    expect(
      promptsSince(from).some((p) => p?.startsWith("SEALED") === true),
    ).toBe(false);
    expect(faultsSince(mark).some((m) => m.has(door0))).toBe(false);
    expect(lastDoors().get(`door:${String(door0)}`)).toBe(1);
  });

  it("keeps a failure across a look switch and a restored context, and clears it on re-entry", async () => {
    const canvas = document.createElement("canvas");
    const session = start({
      client: null,
      canvas,
      load: failing("missing"),
    });
    session.showRoom(before(gallery, door0));
    walkIn();
    await flush();
    // The door shuts over twelve ticks, then its run starts.
    for (let i = 0; i < 20 && !lastFaults().has(door0); i++) frames(1);
    expect(lastFaults().has(door0)).toBe(true);

    key("keydown", "Digit1");
    frames(1);
    key("keyup", "Digit1");
    expect(hud.status).toHaveBeenLastCalledWith(expect.stringContaining("DAY"));
    frames(3);
    expect(hud.prompt).toHaveBeenLastCalledWith("SEALED ?FILE NOT FOUND");
    const beforeLoss = lastFaults().get(door0);

    canvas.dispatchEvent(new Event("webglcontextlost", { cancelable: true }));
    canvas.dispatchEvent(new Event("webglcontextrestored"));
    frames(5);
    expect(hud.prompt).toHaveBeenLastCalledWith("SEALED ?FILE NOT FOUND");
    const afterRestore = lastFaults().get(door0);

    // The run picks up where it was: both frames are from the door's first
    // run, about five ticks apart, in its shudder where every frame differs.
    const door = fixtureAt(gallery, door0);
    if (door.kind !== "door") throw new Error("door 0 is a door");
    const run = planRun("door", faultSeed(door.slot, door.seed), 0);
    const at = (f: FaultFrame | undefined) =>
      run.findIndex((r) => JSON.stringify(r) === JSON.stringify(f));
    const was = at(beforeLoss);
    const is = at(afterRestore);
    expect(was).toBeGreaterThan(0);
    expect(is - was).toBeGreaterThanOrEqual(4);
    expect(is - was).toBeLessThanOrEqual(6);

    // A fresh entry of the same room forgets the failure.
    session.showRoom(before(gallery, door0));
    const mark = renderer.draw.mock.calls.length;
    frames(20);
    expect(hud.prompt).toHaveBeenLastCalledWith(null);
    expect(faultsSince(mark).some((m) => m.has(door0))).toBe(false);
    expect(lastDoors().get(`door:${String(door0)}`)).toBe(1);
  });

  describe("on the canned bridge", () => {
    const bridge = generateRoom(CANNED_BRIDGE);
    /** The blast door straight ahead of the bridge's entrance. */
    const blast = bridge.fixtures.findIndex(
      (f) => f.kind === "door" && f.style === "blast",
    );
    const blastKey = (() => {
      const f = bridge.fixtures[blast];
      if (f?.kind !== "door" || f.address === null) {
        throw new Error("the bridge has an open blast door");
      }
      return f.address.permalink;
    })();

    /**
     * Walks from the entrance up to the blast door, opens it with E, walks
     * in and lets the failed answer land.
     */
    const failBlast = async () => {
      key("keydown", "KeyW");
      const offered = () =>
        hud.prompt.mock.calls.at(-1)?.[0]?.startsWith("E OPEN") === true;
      for (let i = 0; i < 120 && !offered(); i++) frames(1);
      key("keyup", "KeyW");
      expect(offered()).toBe(true);
      pressE();
      frames(15);
      walkIn();
      await flush();
      expect(hud.notice).toHaveBeenCalledWith("?FILE NOT FOUND");
      frames(2);
      expect(hud.prompt).toHaveBeenLastCalledWith("SEALED ?FILE NOT FOUND");
    };

    it("keeps a failed way when the same place is shown again", async () => {
      expect(blast).toBeGreaterThanOrEqual(0);
      const session = start({ client: null, load: failing("missing") });
      session.showCanned(CANNED_BRIDGE);
      frames(1);
      await failBlast();

      session.showCanned(CANNED_BRIDGE);
      const mark = renderer.draw.mock.calls.length;
      frames(60);
      expect(hud.prompt).toHaveBeenLastCalledWith("SEALED ?FILE NOT FOUND");
      expect(faultsSince(mark).some((m) => m.has(blast))).toBe(true);
      expect(lastDoors().get(`door:${String(blast)}`)).toBe(0);
    });

    it("warms the cache for a failed way no more", async () => {
      serve();
      const session = start({ load: failing("missing") });
      session.showCanned(CANNED_BRIDGE);
      frames(1);
      await failBlast();
      const warmed = () =>
        prefetchMock.mock.calls.filter(([, , permalink]) => {
          return permalink === blastKey;
        }).length;
      expect(warmed()).toBe(1);

      // Shown again, the visit keeps its failure but forgets what it
      // warmed: the player still stands at the failed door, which is not
      // warmed a second time.
      session.showCanned(CANNED_BRIDGE);
      frames(20);
      expect(warmed()).toBe(1);
    });
  });

  it("seals nothing when an outside go fails while a travel is pending", async () => {
    const first = deferred<{ kind: "missing" }>();
    let calls = 0;
    const session = start({
      client: null,
      load: () =>
        ++calls === 1 ? first.promise : Promise.resolve({ kind: "missing" }),
    });
    session.showRoom(before(gallery, door0));
    walkIn();
    session.go({ domain: "dev", permalink: "elsewhere" });
    await flush();
    expect(hud.notice).toHaveBeenCalledWith("?FILE NOT FOUND");
    first.resolve({ kind: "missing" });
    await flush();
    const from = hud.prompt.mock.calls.length;
    const mark = renderer.draw.mock.calls.length;
    frames(60);
    expect(
      promptsSince(from).some((p) => p?.startsWith("SEALED") === true),
    ).toBe(false);
    expect(faultsSince(mark).some((m) => m.has(door0))).toBe(false);
    expect(lastDoors().get(`door:${String(door0)}`)).toBe(1);
  });

  it("gives the renderer the same frames for the same room on two sessions", () => {
    const seen: ReadonlyMap<number, FaultFrame>[] = [];
    for (let s = 0; s < 2; s++) {
      now = 0;
      renderer = stubRenderer();
      const session = start({ client: null });
      session.showRoom(before(gallery, door4));
      frames(20);
      seen.push(lastFaults());
      session.dispose();
    }
    expect(seen[0]?.size).toBeGreaterThan(0);
    expect(seen[1]).toEqual(seen[0]);
  });
});
