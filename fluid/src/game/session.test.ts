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
import { TICK_MS, type Clock } from "./core/loop";
import { prefetchPlace } from "./data/source";
import type { Camera, Renderer } from "./render/renderer";
import {
  INVERT_KEY,
  NOTICE_MS,
  createSession,
  type HudSink,
  type RendererFactory,
  type Session,
} from "./session";
import { CANNED_BRIDGE } from "./world/canned";
import { generateRoom } from "./world/generate";
import type { RoomSpec } from "./world/types";

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

/** A new session on a fresh canvas, with the stubs of this test. */
function start(options: { client?: QueryClient | null } = {}): Session {
  const factory: RendererFactory = () => ({
    renderer,
    color: "rgba8",
  });
  const session = createSession({
    canvas: document.createElement("canvas"),
    client: options.client === undefined ? client : options.client,
    hud,
    navigate,
    openFluid,
    forceRgba8: false,
    createRenderer: factory,
    clock,
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

function key(type: "keydown" | "keyup", code: string) {
  window.dispatchEvent(new KeyboardEvent(type, { code }));
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
