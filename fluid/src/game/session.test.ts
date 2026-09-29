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
import type { ChangeEvent, EngramChange } from "../api/events";
import { COALESCE_MS } from "../events/ChangeStreamProvider";
import type { Cue, SoundSink } from "./audio/cues";
import { ANSWER_WAIT_MS, answers } from "./audio/signature";
import type { Answer } from "../test/harness";
import { answersFor, domainsResponse } from "../test/harness";
import { CHEAT_GAP_TICKS } from "./core/cheat";
import { RELOCK_DELAY_MS } from "./core/input";
import { TICK_MS, type Clock } from "./core/loop";
import { prefetchPlace } from "./data/source";
import type { LoadedStation } from "./data/station";
import {
  addressOfGameLocation,
  domainOf,
  fluidRouteOfStation,
  gameRouteOf,
  stationOfPlace,
} from "./paths";
import { BLINK_CHANNELS, createBlink } from "./render/blink";
import { LOOK_ORDER, LOOKS } from "./render/looks";
import type { Camera, Renderer } from "./render/renderer";
import {
  INVERT_KEY,
  LIFT_RIDE_MS,
  LOOK_NOTICE_MS,
  LISTING_WAIT_MS,
  NOTICE_MS,
  RECONFIGURING,
  RESHAPE_MIN_MS,
  UP_LATCH_CLEAR,
  createSession,
  type HudSink,
  type PlaceLoader,
  type RendererFactory,
  type Session,
  type SessionOptions,
} from "./session";
import { boxFront, type DomainRow } from "./world/box";
import { airlockRoom } from "./world/airlock";
import { atConsoleExit, consoleRoom } from "./world/consoleRoom";
import {
  CANNED_BRIDGE,
  CANNED_BRIDGE_DATA,
  CANNED_DECK,
  CANNED_DOMAINS,
  CANNED_HANGAR,
  CANNED_HUB,
  CANNED_WORKSHOP,
  galleryRoom,
  heroHallRoom,
} from "./world/canned";
import { generateDeck, type DeckRow } from "./world/deck";
import { ACCESS_DENIED, NOT_FOUND, generateRoom } from "./world/generate";
import { LIFT_WORDS } from "./world/lifts";
import { REACH, wallFacingSpawn, wallPoint } from "./world/interact";
import { faultSeed, planRun, type FaultFrame } from "./world/malfunction";
import { MAX_PITCH, PLAYER_RADIUS, blockersFor } from "./world/move";
import {
  DIP_SWAP_MS,
  FLICKER_MS,
  darkened,
  diffRooms,
  settleSpot,
} from "./world/diff";
import { withPadHeroes } from "./world/hangar";
import { roomFor } from "./world/station";
import type {
  Fixture,
  Hero,
  PlaceInput,
  RoomSpec,
  StationAddress,
} from "./world/types";
import { CELL } from "./world/units";

vi.mock("../api/client", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../api/client")>();
  return { ...actual, api: vi.fn(), setCsrfToken: vi.fn() };
});

/**
 * The kind of fixture a room `roomFor` builds puts the player in front of,
 * or null for the room's own spawn: a seam for the live-change tests,
 * which need a terminal or a door in reach without walking there.
 */
const facing = vi.hoisted(() => ({
  kind: null as "terminal" | "door" | null,
}));

vi.mock("./world/station", async (importOriginal) => {
  const actual = await importOriginal<typeof import("./world/station")>();
  const { wallFacingSpawn } = await import("./world/interact");
  return {
    ...actual,
    roomFor: (...args: Parameters<typeof actual.roomFor>) => {
      const built = actual.roomFor(...args);
      const fixture = built.room.fixtures.find((f) => f.kind === facing.kind);
      if (fixture === undefined) return built;
      return {
        ...built,
        room: { ...built.room, spawn: wallFacingSpawn(fixture.slot) },
      };
    },
  };
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

/**
 * A sound sink that records every cue, and answers `toggleMute` from
 * `mutes` in turn (false once they run out).
 */
function recordSound(mutes: readonly boolean[] = []) {
  const cues: Cue[] = [];
  let next = 0;
  const sink = {
    cue: vi.fn<SoundSink["cue"]>((c) => {
      cues.push(c);
    }),
    toggleMute: vi.fn<SoundSink["toggleMute"]>(() => mutes[next++] ?? false),
  };
  return { cues, sink };
}

let client: QueryClient;
let renderer: ReturnType<typeof stubRenderer>;
let hud: ReturnType<typeof stubHud>;
let navigate: ReturnType<typeof vi.fn<(path: string) => void>>;
let openFluid: ReturnType<typeof vi.fn<(path: string) => void>>;
let sessions: Session[];
let now: number;
let pending: ((t: number) => void) | null;
/** The canvas of the newest session `start` made. */
let lastCanvas: HTMLCanvasElement | null = null;

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
    consoleRoom?: SessionOptions["consoleRoom"];
    onLift?: SessionOptions["onLift"];
    onPause?: SessionOptions["onPause"];
    sound?: SessionOptions["sound"];
  } = {},
): Session {
  const factory: RendererFactory =
    options.factory ??
    (() => ({
      renderer,
      color: "rgba8",
    }));
  lastCanvas = options.canvas ?? document.createElement("canvas");
  const session = createSession({
    canvas: lastCanvas,
    client: options.client === undefined ? client : options.client,
    hud,
    navigate,
    openFluid,
    forceRgba8: false,
    createRenderer: factory,
    clock,
    ...(options.load === undefined ? {} : { load: options.load }),
    ...(options.onLevels === undefined ? {} : { onLevels: options.onLevels }),
    ...(options.consoleRoom === undefined
      ? {}
      : { consoleRoom: options.consoleRoom }),
    ...(options.onLift === undefined ? {} : { onLift: options.onLift }),
    ...(options.onPause === undefined ? {} : { onPause: options.onPause }),
    ...(options.sound === undefined ? {} : { sound: options.sound }),
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

/**
 * Moves the mouse `dy` pixels (negative is forward, which looks up) with
 * the pointer locked to the newest session's canvas, then unlocks it.
 */
function mouseLook(dy: number) {
  Object.defineProperty(document, "pointerLockElement", {
    configurable: true,
    get: () => lastCanvas,
  });
  document.dispatchEvent(new MouseEvent("mousemove", { movementY: dy }));
  Reflect.deleteProperty(document, "pointerLockElement");
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
  facing.kind = null;
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

/**
 * `room` (the hero hall unless given) shown at `at` (the hall's own
 * address unless given) with the player at its first police box's front,
 * and that box's hero index.
 *
 * `spotView`'s own front spot stands about 2.48 m out (`FRAME_BASE` plus
 * `FRAME_SCALE` times the box's 1.3 m footprint), farther than `REACH`
 * (2.2 m), so the box would never come into focus there. The spawn is
 * put `REACH - 0.4` m out along `boxFront`'s own frame instead.
 */
function standAtBox(
  session: Session,
  room: RoomSpec = heroHallRoom(),
  at: StationAddress = HALL_ADDRESS,
): number {
  const index = room.heroes.findIndex((h) => h.kind === "police-box");
  if (index < 0) throw new Error("no police box in the hero hall");
  const front = boxFront(room.heroes[index]!);
  const dist = REACH - 0.4;
  const wx = front.x + front.inward[0] * dist;
  const wz = front.z + front.inward[1] * dist;
  const spawn = {
    x: wx / CELL - 0.5,
    y: wz / CELL - 0.5,
    yaw: Math.atan2(front.inward[0], front.inward[1]),
  };
  session.showRoom({ ...room, spawn }, { pitch: 0 }, at);
  frames(1);
  return index;
}

/** The hero hall's own address: the room the box tests stand a box in. */
const HALL_ADDRESS: StationAddress = {
  kind: "engram",
  domain: heroHallRoom().domain,
  permalink: heroHallRoom().permalink,
};

/** An engram's station address. */
const engramAt = (domain: string, permalink: string): StationAddress => ({
  kind: "engram",
  domain,
  permalink,
});

/** The domain of the address the session stands at, null for none. */
const domainNow = (session: Session): string | null =>
  session.current === null ? null : domainOf(session.current);

/** The door fractions the last frame was drawn with. */
const lastDoors = () =>
  renderer.draw.mock.calls.at(-1)?.[3] ?? new Map<string, number>();

describe("go", () => {
  it("loads the place and navigates to its game route once", async () => {
    serve();
    const session = start();
    session.go({ kind: "engram", domain: "eng", permalink: "alpha" });
    expect(hud.connector).toHaveBeenLastCalledWith(
      true,
      "alpha",
      expect.any(String),
    );
    await vi.waitFor(() => {
      expect(navigate).toHaveBeenCalledTimes(1);
    });
    expect(navigate).toHaveBeenCalledWith("/%CF%80/d/eng/e/alpha");
    expect(roomsSet()).toEqual(["alpha"]);
    expect(session.current).toEqual(engramAt("eng", "alpha"));
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
    session.go({ kind: "engram", domain: "eng", permalink: "alpha" });
    session.go({ kind: "engram", domain: "eng", permalink: "beta" });
    await vi.waitFor(() => {
      expect(navigate).toHaveBeenCalledTimes(1);
    });
    alpha.resolve(detailResponse("alpha", "Alpha"));
    await flush();
    expect(navigate).toHaveBeenCalledTimes(1);
    expect(navigate).toHaveBeenCalledWith("/%CF%80/d/eng/e/beta");
    expect(roomsSet()).toEqual(["beta"]);
    expect(session.current).toEqual(engramAt("eng", "beta"));
  });

  it("drops a load that settles after dispose", async () => {
    const alpha = deferred<unknown>();
    serve({ "/domains/eng/engrams/alpha": () => alpha.promise });
    const session = start();
    session.go({ kind: "engram", domain: "eng", permalink: "alpha" });
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
    session.go({ kind: "engram", domain: "eng", permalink: "alpha" });
    session.dispose();
    expect(hud.connector).toHaveBeenLastCalledWith(
      false,
      "alpha",
      expect.any(String),
    );
    expect(hud.reader).toHaveBeenLastCalledWith(null);
  });

  it("keeps the connector in the game's look while loading, whatever digit is pressed", () => {
    // Mutation caught: a look key read again while a place loads (the
    // connector would be redrawn in the day shift's or the third look's
    // colours).
    serve({ "/domains/eng/engrams/alpha": () => new Promise(() => {}) });
    const session = start();
    session.go({ kind: "engram", domain: "eng", permalink: "alpha" });
    expect(hud.connector).toHaveBeenLastCalledWith(true, "alpha", "aperture");
    const calls = hud.connector.mock.calls.length;
    for (const code of ["Digit1", "Digit2", "Digit3", "Digit4"]) {
      key("keydown", code);
      frames(1);
      key("keyup", code);
    }
    expect(hud.connector.mock.calls.length).toBe(calls);
    expect(hud.connector).toHaveBeenLastCalledWith(true, "alpha", "aperture");
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
      { kind: "engram", domain: "eng", permalink: "alpha" },
      {
        via: "hatch",
        from: { kind: "engram", domain: "eng", permalink: "beta" },
      },
    );
    await vi.waitFor(() => {
      expect(session.current).toEqual(engramAt("eng", "alpha"));
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
    session.go({ kind: "engram", domain: "eng", permalink: "alpha" });
    await vi.waitFor(() => {
      expect(session.current).toEqual(engramAt("eng", "alpha"));
    });
    session.go({ kind: "engram", domain: "eng", permalink: "beta" });
    await vi.waitFor(() => {
      expect(hud.notice).toHaveBeenCalledWith("ACCESS DENIED");
    });
    expect(session.current).toEqual(engramAt("eng", "alpha"));
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
    session.go({ kind: "engram", domain: "eng", permalink: "alpha" });
    await vi.waitFor(() => {
      expect(hud.notice).toHaveBeenCalledWith("?FILE NOT FOUND");
    });
    session.go({ kind: "engram", domain: "eng", permalink: "beta" });
    await vi.waitFor(() => {
      expect(hud.notice).toHaveBeenCalledWith("SIGNAL LOST");
    });
    expect(session.current).toBe(null);
    expect(renderer.setRoom).not.toHaveBeenCalled();
  });

  it("does not navigate to the address the URL already shows", async () => {
    serve();
    // Set with the raw character rather than its encoding: jsdom's own URL
    // parser normalizes it to `%CF%80` in `window.location.pathname` exactly
    // as a real browser would, which is the fact `gameRouteOf` is built
    // to agree with (see `paths.ts`). If it built from the raw character
    // instead, this comparison in `session.ts` would never see the two
    // sides as equal, and `navigate` would fire below when it must not.
    window.history.replaceState(null, "", "/π/d/eng/e/alpha");
    expect(window.location.pathname).toBe("/%CF%80/d/eng/e/alpha");
    const session = start();
    session.go({ kind: "engram", domain: "eng", permalink: "alpha" });
    await vi.waitFor(() => {
      expect(session.current).toEqual(engramAt("eng", "alpha"));
    });
    expect(roomsSet()).toEqual(["alpha"]);
    expect(navigate).not.toHaveBeenCalled();
  });

  it("goes nowhere without a client and says the signal is lost", () => {
    const session = start({ client: null });
    session.showCanned(CANNED_BRIDGE);
    session.go({ kind: "engram", domain: "station", permalink: "old-bridge" });
    expect(hud.notice).toHaveBeenCalledWith("SIGNAL LOST");
    expect(session.current).toEqual(engramAt("station", "manifest"));
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

  it("forgets a use pressed while loading instead of using it in the new room", async () => {
    const alpha = deferred<unknown>();
    serve({
      "/domains/eng/engrams/alpha": () => alpha.promise,
      "/domains/eng/inbound/alpha": () => FROM_BETA,
    });
    const session = start();
    // Through a door from Beta: the player arrives in front of the hatch
    // back to Beta, facing into the room.
    session.go(
      { kind: "engram", domain: "eng", permalink: "alpha" },
      {
        via: "door",
        from: { kind: "engram", domain: "eng", permalink: "beta" },
      },
    );
    frames(2);
    key("keydown", "Space");
    key("keyup", "Space");
    frames(2);
    alpha.resolve(detailResponse("alpha", "Alpha"));
    await vi.waitFor(() => {
      expect(session.current).toEqual(engramAt("eng", "alpha"));
    });
    // Turn round to the hatch: a stale use would crawl back the moment it is
    // in front of the player.
    const offered = () =>
      hud.prompt.mock.calls.at(-1)?.[0] === "SPACE CRAWL Beta relates_to";
    key("keydown", "ArrowLeft");
    for (let i = 0; i < 60 && !offered(); i++) frames(1);
    key("keyup", "ArrowLeft");
    frames(5);
    expect(hud.prompt).toHaveBeenLastCalledWith("SPACE CRAWL Beta relates_to");
    expect(hud.connector).toHaveBeenLastCalledWith(
      false,
      "alpha",
      expect.any(String),
    );
    expect(roomsSet()).toEqual(["alpha"]);

    // A fresh use in the new room still crawls back.
    key("keydown", "Space");
    frames(1);
    expect(hud.connector).toHaveBeenLastCalledWith(
      true,
      "Beta",
      expect.any(String),
    );
  });

  it("forgets a use pressed while loading when the load fails", async () => {
    const beta = deferred<unknown>();
    serve({ "/domains/eng/engrams/beta": () => beta.promise });
    const session = start();
    session.showCanned(CANNED_BRIDGE);
    frames(1);
    walkToScope();
    session.go({ kind: "engram", domain: "eng", permalink: "beta" });
    frames(2);
    key("keydown", "Space");
    key("keyup", "Space");
    frames(2);
    beta.resolve(Promise.reject(new ApiProblem(403, "no", "denied")));
    await vi.waitFor(() => {
      expect(hud.notice).toHaveBeenCalledWith("ACCESS DENIED");
    });
    // Still in front of the Scope terminal: the use pressed for the load's
    // room must not open the reader here.
    frames(5);
    expect(session.current).toEqual(engramAt("station", "manifest"));
    expect(hud.reader).not.toHaveBeenCalledWith(
      expect.objectContaining({ title: "Station Crystalline" }),
    );
    expect(hud.prompt).toHaveBeenLastCalledWith("SPACE READ Scope");
  });
});

describe("a room the renderer refuses", () => {
  it("keeps the player in the old room and says ?LOAD ERROR", async () => {
    serve();
    const session = start();
    session.go({ kind: "engram", domain: "eng", permalink: "alpha" });
    await vi.waitFor(() => {
      expect(session.current).toEqual(engramAt("eng", "alpha"));
    });
    frames(3);
    const before = eyeAt();
    renderer.setRoom.mockImplementation(() => {
      throw new Error("room needs 999 texture layers, the GPU holds 256");
    });
    session.go({ kind: "engram", domain: "eng", permalink: "beta" });
    await vi.waitFor(() => {
      expect(hud.notice).toHaveBeenCalledWith("?LOAD ERROR");
    });
    expect(session.current).toEqual(engramAt("eng", "alpha"));
    expect(navigate).toHaveBeenCalledTimes(1);
    expect(navigate).toHaveBeenLastCalledWith("/%CF%80/d/eng/e/alpha");
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

  it("never switches the look from a key: the room stays in Aperture grid and the status names no look", () => {
    // Mutation caught: the digit keys mapped to looks again (the room would
    // be handed to the renderer a second time, in another look), or the
    // look's name put back on the status line.
    const session = start({ client: null });
    session.showCanned(CANNED_BRIDGE);
    frames(1);
    const rooms = renderer.setRoom.mock.calls.length;
    expect(rooms).toBeGreaterThan(0);
    hud.notice.mockClear();
    for (const code of ["Digit1", "Digit2", "Digit3", "Digit4"]) {
      key("keydown", code);
      frames(1);
      key("keyup", code);
      frames(1);
    }
    expect(renderer.setRoom.mock.calls.length).toBe(rooms);
    for (const [, look] of renderer.setRoom.mock.calls)
      expect(look.id).toBe("aperture");
    expect(hud.notice).not.toHaveBeenCalled();
    expect(hud.status).toHaveBeenCalled();
    for (const [text] of hud.status.mock.calls)
      for (const id of LOOK_ORDER)
        expect(text).not.toContain(LOOKS[id].name.toUpperCase());
    expect(session.current).toEqual(engramAt("station", "manifest"));
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
    expect(session.current).toEqual(engramAt("station", "manifest"));
  });
});

describe("notices", () => {
  it("takes an in-room failure notice down after three seconds", () => {
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });
    try {
      const session = start({ client: null });
      session.showCanned(CANNED_BRIDGE);
      session.go({
        kind: "engram",
        domain: "station",
        permalink: "old-bridge",
      });
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

describe("flash", () => {
  // Mutation caught: the timer not set (the notice stays for good), or
  // the standing notice not brought back after it.
  it("shows a notice for NOTICE_MS, then the standing one or none (M4 C27)", () => {
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });
    try {
      const session = start({ client: null });
      session.showCanned(CANNED_BRIDGE);
      session.flash("NO CARRIER");
      expect(hud.notice).toHaveBeenLastCalledWith("NO CARRIER");
      vi.advanceTimersByTime(NOTICE_MS - 1);
      expect(hud.notice).toHaveBeenLastCalledWith("NO CARRIER");
      vi.advanceTimersByTime(1);
      expect(hud.notice).toHaveBeenLastCalledWith(null);
      session.dispose();

      // Before the first room, over a place that could not be entered:
      // that notice stands again after the flash.
      const dark = start({ client: null });
      dark.go({ kind: "engram", domain: "station", permalink: "old-bridge" });
      expect(hud.notice).toHaveBeenLastCalledWith("SIGNAL LOST");
      dark.flash("NO CARRIER");
      expect(hud.notice).toHaveBeenLastCalledWith("NO CARRIER");
      vi.advanceTimersByTime(NOTICE_MS);
      expect(hud.notice).toHaveBeenLastCalledWith("SIGNAL LOST");
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
    // No address behind it (M3 C5), unless the caller names one.
    expect(session.current).toBeNull();
    session.showRoom(built, undefined, engramAt("dev", "gallery"));
    expect(session.current).toEqual(engramAt("dev", "gallery"));
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
    hud.prompt.mock.calls.at(-1)?.[0]?.startsWith("SPACE READ") === true;
  for (let i = 0; i < 80 && !offered(); i++) frames(1);
  key("keyup", "KeyW");
  frames(15);
  expect(hud.prompt).toHaveBeenLastCalledWith("SPACE READ Scope");
}

describe("the reader", () => {
  it("opens at Space, takes the keys while open and gives them back on close", () => {
    const session = start({ client: null });
    session.showCanned(CANNED_BRIDGE);
    frames(1);
    walkToScope();
    key("keydown", "Space");
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
    key("keydown", "Space");
    frames(1);
    expect(hud.reader).toHaveBeenLastCalledWith(
      expect.objectContaining({ title: "Station Crystalline" }),
    );
    session.go({ kind: "engram", domain: "station", permalink: "old-bridge" });
    expect(hud.reader).toHaveBeenLastCalledWith(null);

    key("keydown", "Space");
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
    mouseLook(-48);
    frames(4);
    expect(lastCamera().pitch).toBeGreaterThan(0);

    key("keydown", "KeyI");
    frames(1);
    expect(hud.notice).toHaveBeenCalledWith("LOOK INVERTED");
    expect(window.localStorage.getItem(INVERT_KEY)).toBe("1");
    const before = lastCamera().pitch;
    mouseLook(-48);
    frames(4);
    expect(lastCamera().pitch).toBeLessThan(before);
    first.dispose();

    const second = start({ client: null });
    second.showCanned(CANNED_BRIDGE);
    frames(1);
    mouseLook(-48);
    frames(4);
    expect(lastCamera().pitch).toBeLessThan(0);

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
    mouseLook(-48);
    frames(4);
    expect(lastCamera().pitch).toBeGreaterThan(0);
  });
});

describe("the classic controls", () => {
  /** A session on the canned bridge, one tick in, at the entrance. */
  function onBridge(): Session {
    const session = start({ client: null });
    session.showCanned(CANNED_BRIDGE);
    frames(1);
    return session;
  }

  it("uses on Space, and E does nothing", () => {
    onBridge();
    walkToScope();
    key("keydown", "KeyE");
    frames(2);
    key("keyup", "KeyE");
    expect(hud.reader).not.toHaveBeenCalled();
    key("keydown", "Space");
    frames(1);
    key("keyup", "Space");
    expect(hud.reader).toHaveBeenLastCalledWith(
      expect.objectContaining({ title: "Station Crystalline" }),
    );
  });

  /**
   * What holding `codes` for `ticks` does on a session of its own, from a
   * spot a few steps into the bridge's hall: how far the eye moved along x
   * and z, how far the view turned, and the pitch it ends at.
   */
  function holding(codes: string[], ticks = 8) {
    // The clock starts over for each, so every session runs the same
    // ticks in the same frames and the distances compare exactly.
    now = 0;
    const session = onBridge();
    key("keydown", "KeyW");
    frames(6);
    key("keyup", "KeyW");
    frames(20);
    const [x, z] = eyeAt();
    const yaw = lastCamera().yaw;
    for (const code of codes) key("keydown", code);
    frames(ticks);
    for (const code of codes) key("keyup", code);
    const [x1, z1] = eyeAt();
    const out = {
      dx: x1 - x,
      dz: z1 - z,
      turned: lastCamera().yaw - yaw,
      pitch: lastCamera().pitch,
    };
    session.dispose();
    return out;
  }

  it("walks on Up and Down, as on W and S, and never pitches with them", () => {
    const up = holding(["ArrowUp"]);
    expect(up.dz).toBeLessThan(-0.5);
    expect(up.dz).toBeCloseTo(holding(["KeyW"]).dz, 9);
    expect(up.pitch).toBe(0);
    const down = holding(["ArrowDown"]);
    expect(down.dz).toBeGreaterThan(0.5);
    expect(down.dz).toBeCloseTo(holding(["KeyS"]).dz, 9);
    expect(down.pitch).toBe(0);
  });

  it("turns on Left and Right, and strafes on them while Alt is held", () => {
    const left = holding(["ArrowLeft"]);
    expect(left.turned).toBeGreaterThan(0.1);
    expect(Math.abs(left.dx)).toBeLessThan(1e-9);
    expect(holding(["ArrowRight"]).turned).toBeLessThan(-0.1);

    for (const alt of ["AltLeft", "AltRight"]) {
      const sideLeft = holding([alt, "ArrowLeft"]);
      expect(sideLeft.turned).toBe(0);
      expect(sideLeft.dx).toBeLessThan(-0.5);
      expect(sideLeft.dx).toBeCloseTo(holding(["KeyA"]).dx, 9);
      const sideRight = holding([alt, "ArrowRight"]);
      expect(sideRight.turned).toBe(0);
      expect(sideRight.dx).toBeGreaterThan(0.5);
      expect(sideRight.dx).toBeCloseTo(holding(["KeyD"]).dx, 9);
    }
  });

  it("strafes on comma and period, and no faster on two keys at once", () => {
    const comma = holding(["Comma"]);
    expect(comma.turned).toBe(0);
    expect(comma.dx).toBeCloseTo(holding(["KeyA"]).dx, 9);
    expect(comma.dx).toBeLessThan(-0.5);
    const period = holding(["Period"]);
    expect(period.dx).toBeCloseTo(holding(["KeyD"]).dx, 9);
    expect(period.dx).toBeGreaterThan(0.5);
    expect(holding(["KeyD", "Period"]).dx).toBeCloseTo(period.dx, 9);
    expect(holding(["AltLeft", "ArrowRight", "KeyD"]).dx).toBeCloseTo(
      period.dx,
      9,
    );
  });

  // Walking forward, a strafe of 2 would tilt the walk further to the
  // side than a strafe of 1, even after the wish is cut to length 1, so
  // this sees two strafe keys for one side summed instead of taken once.
  it("strafes on two keys for one side exactly as on one, while walking", () => {
    const one = holding(["KeyW", "KeyD"]);
    expect(one.dx).toBeGreaterThan(0.3);
    expect(one.dz).toBeLessThan(-0.3);
    for (const two of [
      ["KeyW", "KeyD", "AltLeft", "ArrowRight"],
      ["KeyW", "KeyD", "Period"],
      ["KeyW", "Period", "AltRight", "ArrowRight"],
    ]) {
      const both = holding(two);
      expect(both.dx).toBeCloseTo(one.dx, 9);
      expect(both.dz).toBeCloseTo(one.dz, 9);
    }
  });

  it("looks up and down with the mouse alone", () => {
    onBridge();
    mouseLook(-48);
    frames(4);
    expect(lastCamera().pitch).toBeGreaterThan(0);
  });

  /** Sends a cancelable key event and says whether it was prevented. */
  function prevented(
    type: "keydown" | "keyup",
    code: string,
    mods: KeyboardEventInit = {},
  ): boolean {
    const event = new KeyboardEvent(type, {
      code,
      cancelable: true,
      ...mods,
    });
    window.dispatchEvent(event);
    return event.defaultPrevented;
  }

  // Mutation caught: the session's own `onClaimedKey` added before the
  // input's listener, which then finds every claimed key cancelled and
  // drops it (M4 C26's guard in `input.ts`): no turning on the arrows, no
  // Space, in play.
  it("still hands the game the keys its own handler cancels", () => {
    const session = onBridge();
    const yaw = lastCamera().yaw;
    expect(prevented("keydown", "ArrowLeft")).toBe(true);
    frames(10);
    prevented("keyup", "ArrowLeft");
    frames(1);
    expect(lastCamera().yaw).not.toBeCloseTo(yaw, 3);
    session.dispose();
  });

  it("keeps the browser from scrolling or going back on the game's keys", () => {
    const session = onBridge();
    expect(prevented("keydown", "Space")).toBe(true);
    expect(prevented("keydown", "ArrowUp")).toBe(true);
    expect(prevented("keydown", "ArrowLeft", { altKey: true })).toBe(true);
    expect(prevented("keydown", "AltLeft", { altKey: true })).toBe(true);
    expect(prevented("keyup", "AltLeft")).toBe(true);
    expect(prevented("keyup", "AltRight")).toBe(true);
    expect(prevented("keydown", "Comma")).toBe(true);
    expect(prevented("keydown", "Period")).toBe(true);
    // Letters, and every browser shortcut with Ctrl or Cmd, stay the
    // browser's.
    expect(prevented("keydown", "KeyW")).toBe(false);
    expect(prevented("keydown", "ArrowLeft", { ctrlKey: true })).toBe(false);
    expect(prevented("keydown", "ArrowLeft", { metaKey: true })).toBe(false);
    session.dispose();
    expect(prevented("keydown", "Space")).toBe(false);
  });

  it("leaves every key to the level select while it is open", () => {
    const session = start({ client: null, onLevels: () => {} });
    session.showCanned(CANNED_BRIDGE);
    frames(1);
    type("idclev");
    frames(1);
    expect(prevented("keydown", "Space")).toBe(false);
    expect(prevented("keydown", "ArrowDown")).toBe(false);
    expect(prevented("keyup", "AltLeft")).toBe(false);
    session.closeLevels();
    expect(prevented("keydown", "Space")).toBe(true);
  });

  it("runs at about twice the walk while Shift is held", () => {
    const walked = holding(["KeyW"]).dz;
    expect(walked).toBeLessThan(-0.5);
    expect(holding(["ShiftLeft", "KeyW"]).dz).toBeLessThan(1.8 * walked);
    expect(holding(["ShiftRight", "ArrowUp"]).dz).toBeLessThan(1.8 * walked);
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
    mouseLook(-48);
    frames(4);
    expect(lastCamera().pitch).toBeGreaterThan(0);
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

  it("types the word in front of a terminal without reading it", () => {
    onBridge();
    walkToScope();
    hud.reader.mockClear();
    // E is no use any more: no E, inside the word or alone, reads.
    type("idcle");
    frames(2);
    type("e");
    frames(2);
    expect(hud.reader).not.toHaveBeenCalled();
    expect(hud.prompt).toHaveBeenLastCalledWith("SPACE READ Scope");

    type("idclev");
    frames(1);
    expect(levels).toHaveBeenLastCalledWith(true);
    expect(hud.reader).not.toHaveBeenCalled();
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
    expect(hud.reader).not.toHaveBeenCalled();
    key("keydown", "Space");
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
    // In front of the Scope, so a use that got through would read it. The
    // walk's momentum runs out before the select opens.
    walkToScope();
    frames(20);
    expect(hud.prompt).toHaveBeenLastCalledWith("SPACE READ Scope");
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
      "Space",
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
    // No key switches the look, so Digit1 does nothing here either, and
    // the status line names no look.
    expect(hud.status).toHaveBeenCalled();
    for (const [text] of hud.status.mock.calls) {
      for (const id of LOOK_ORDER)
        expect(text).not.toContain(LOOKS[id].name.toUpperCase());
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
    key("keydown", "Space");
    key("keyup", "Space");
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
      "/domains/eng/tree": () => ({
        domain: "eng",
        path: "",
        folders: [],
        engrams: [
          {
            permalink: "manifest",
            title: "Eng",
            type: "manifest",
            status: "stable",
          },
        ],
        truncated: false,
        total: 1,
      }),
      "/domains/eng/engrams/manifest": () => detailResponse("manifest", "Eng"),
      "/domains/eng/inbound/manifest": () => EMPTY_INBOUND,
    });
    const session = start({ onLevels: levels });
    session.go({ kind: "engram", domain: "eng", permalink: "alpha" });
    await vi.waitFor(() => {
      expect(navigate).toHaveBeenCalledTimes(1);
    });
    frames(1);
    type("idclev");
    frames(1);
    expect(levels).toHaveBeenLastCalledWith(true);

    session.jump("eng");
    expect(levels).toHaveBeenLastCalledWith(false);
    // The connector names the domain, not the bridge's permalink (C10).
    expect(hud.connector).toHaveBeenLastCalledWith(
      true,
      "eng",
      expect.any(String),
    );
    await vi.waitFor(() => {
      expect(navigate).toHaveBeenCalledTimes(2);
    });
    expect(navigate).toHaveBeenLastCalledWith("/%CF%80/d/eng");
    expect(session.current).toEqual({ kind: "bridge", domain: "eng" });
    expect(roomsSet()).toEqual(["alpha", "manifest"]);
    // The connector's closing call names the domain too, not just the
    // opening one.
    expect(hud.connector).toHaveBeenCalledWith(
      false,
      "eng",
      expect.any(String),
    );
  });

  it("closes the select on a go from outside and on dispose", () => {
    const session = onBridge();
    type("idclev");
    frames(1);
    session.go({ kind: "engram", domain: "station", permalink: "old-bridge" });
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
    session.go({ kind: "engram", domain: "eng", permalink: "alpha" });
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

  /** Presses Space, the use key, for one frame. */
  const pressUse = () => {
    key("keydown", "Space");
    frames(1);
    key("keyup", "Space");
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

  it("sends one fault cue per run of a stuttering door, placed (M4 C23, F15)", () => {
    // Mutation caught: the fault cues not sent, or sent every frame.
    const { cues, sink } = recordSound();
    const session = start({ client: null, sound: sink });
    session.showRoom(before(gallery, door4));
    frames(60);
    const runs = faultsSince().reduce(
      (n, m, i, all) => (m.has(door4) && !all[i - 1]?.has(door4) ? n + 1 : n),
      0,
    );
    expect(runs).toBeGreaterThanOrEqual(1);
    const faults = cues.filter(
      (c): c is Extract<Cue, { kind: "fault" }> => c.kind === "fault",
    );
    expect(faults).toHaveLength(runs);
    faults.forEach((c, i) => {
      expect(c).toMatchObject({ way: "door", run: i + 1 });
      expect(c.gain).toBeGreaterThan(0.3);
    });
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
    pressUse();
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

    // A use at the sealed door starts no travel and opens nothing.
    const ups = connectorUps();
    pressUse();
    frames(20);
    expect(connectorUps()).toBe(ups);
    expect(lastDoors().get(`door:${String(door1)}`)).toBe(0);
  });

  it("pops a hatch lid once when its crawl fails", async () => {
    const session = start({ client: null, load: failing("missing") });
    session.showRoom(before(gallery, hatch0));
    frames(2);
    expect(hud.prompt).toHaveBeenLastCalledWith(
      expect.stringMatching(/^SPACE CRAWL /),
    );
    pressUse();
    expect(connectorUps()).toBe(1);
    await flush();
    const mark = renderer.draw.mock.calls.length;
    frames(2);
    expect(faultsSince(mark).some((m) => m.has(hatch0))).toBe(true);
    expect(hud.prompt).toHaveBeenLastCalledWith("SEALED ?FILE NOT FOUND");

    // The failed hatch carries no one: hatchTravel reads the failed map.
    pressUse();
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
      session.go(stationOfPlace(door.address));
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
          Promise.resolve({
            kind: "engram" as const,
            place: CANNED_BRIDGE,
            folder: "",
          }),
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

  it("keeps a failure across a restored context, and clears it on re-entry", async () => {
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

    frames(4);
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
     * Walks from the entrance up to the blast door, opens it with Space, walks
     * in and lets the failed answer land.
     */
    const failBlast = async () => {
      key("keydown", "KeyW");
      const offered = () =>
        hud.prompt.mock.calls.at(-1)?.[0]?.startsWith("SPACE OPEN") === true;
      for (let i = 0; i < 120 && !offered(); i++) frames(1);
      key("keyup", "KeyW");
      expect(offered()).toBe(true);
      pressUse();
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
    session.go({ kind: "engram", domain: "dev", permalink: "elsewhere" });
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

describe("the police box's doors", () => {
  it("opens and closes on Space and draws the leaves by their fraction (2.6e C10)", () => {
    // Mutation caught: the box focus never read (the press drained by the
    // fixture branch), or doorOpen missing the box's key.
    const session = start();
    const i = standAtBox(session);
    expect(i).toBeGreaterThanOrEqual(0);
    expect(hud.prompt).toHaveBeenLastCalledWith("SPACE OPEN");
    key("keydown", "Space");
    key("keyup", "Space");
    frames(19);
    expect(lastDoors().get(`box:${String(i)}`)).toBe(1);
    expect(hud.prompt).toHaveBeenLastCalledWith("SPACE CLOSE");
    key("keydown", "Space");
    key("keyup", "Space");
    frames(19);
    expect(lastDoors().get(`box:${String(i)}`)).toBe(0);
  });

  it("never walks in without the console room option (2.6e C21)", () => {
    // Mutation caught: a walk-in on a dev route.
    const session = start();
    standAtBox(session);
    key("keydown", "Space");
    key("keyup", "Space");
    frames(19);
    const before = session.current;
    key("keydown", "ArrowUp");
    frames(60);
    key("keyup", "ArrowUp");
    expect(renderer.setRoom).toHaveBeenCalledTimes(1);
    expect(session.current).toEqual(before);
  });
});

const row = (
  name: string,
  canonicalName: string | null = null,
  aliases: string[] = [],
): DomainRow => ({ name, canonicalName, aliases });

/**
 * The hero hall's domain: the room left when the tests walk in from the hall.
 */
const HALL = heroHallRoom().domain;

/**
 * Answers every address as `stationLoad` does: a bridge (every jump and
 * every walk out of the console room goes to one) is the canned bridge
 * moved to its domain, with the canned bridge data.
 */
const okBridge: PlaceLoader = (a, signal) => stationLoad(a, signal);

/** The bridge room `okBridge` answers for `domain`, as `roomFor` builds it. */
function bridgeRoomOf(domain: string): RoomSpec {
  return roomFor(
    {
      kind: "bridge",
      place: { ...CANNED_BRIDGE, domain, permalink: "manifest" },
      bridge: { ...CANNED_BRIDGE_DATA, domain, display: domain },
    },
    null,
    null,
  ).room;
}

/**
 * A session with the console room switched on, its listing `rows` (a value
 * or a promise).
 */
function startWithConsole(
  rows: readonly DomainRow[] | null | Promise<readonly DomainRow[] | null>,
  load: PlaceLoader,
  onLevels?: (open: boolean) => void,
): Session {
  return start({
    load,
    consoleRoom: { domains: () => Promise.resolve(rows) },
    ...(onLevels === undefined ? {} : { onLevels }),
  });
}

/**
 * Opens the box in front and walks forward until the room changes (at most
 * 60 ticks).
 */
function walkIn() {
  key("keydown", "Space");
  key("keyup", "Space");
  frames(19);
  const before = renderer.setRoom.mock.calls.length;
  key("keydown", "ArrowUp");
  for (let t = 0; t < 60 && renderer.setRoom.mock.calls.length === before; t++)
    frames(1);
  key("keyup", "ArrowUp");
}

/** Backs from the console room's spawn into its inner doors. */
function backOut(ticks = 40) {
  key("keydown", "ArrowDown");
  frames(ticks);
  key("keyup", "ArrowDown");
}

/** The room the renderer was last handed. */
const lastRoom = () => renderer.setRoom.mock.calls.at(-1)?.[0];

describe("the console room", () => {
  it("cuts into the console room on walking in: no connector, no navigation, the room left kept as current (2.6e C12)", () => {
    // Mutation caught: the connector shown, navigate called, or current
    // moved to the console room; and the status line leading with an
    // empty title (its separator first).
    const session = startWithConsole([row(HALL), row("ops")], okBridge);
    standAtBox(session);
    const before = session.current;
    hud.connector.mockClear();
    walkIn();
    frames(1);
    expect(isConsole(lastRoom())).toBe(true);
    expect(hud.connector).not.toHaveBeenCalled();
    expect(navigate).not.toHaveBeenCalled();
    expect(session.current).toEqual(before);
    expect(lastCamera().yaw).toBe(0);
    const status = hud.status.mock.calls.at(-1)?.[0] ?? "";
    expect(status.startsWith("  |  ")).toBe(false);
    expect(
      status.startsWith(`${lastRoom()?.condition.toUpperCase() ?? "?"}  |  `),
    ).toBe(true);
  });

  it("walks out to another domain's bridge, the connector naming it, and steps out of a box there (2.6e C13, C14)", async () => {
    // Mutation caught: the exit going to the room left's own domain, the
    // connector showing the permalink, the landing without the box.
    const session = startWithConsole([row(HALL), row("ops")], okBridge);
    standAtBox(session);
    walkIn();
    await flush();
    backOut();
    expect(hud.connector).toHaveBeenCalledWith(true, "ops", expect.any(String));
    await vi.waitFor(() => {
      expect(navigate).toHaveBeenCalledWith("/%CF%80/d/ops");
    });
    const bridge = lastRoom();
    const plain = bridgeRoomOf("ops");
    expect(bridge?.heroes.filter((h) => h.kind === "police-box").length).toBe(
      plain.heroes.filter((h) => h.kind === "police-box").length + 1,
    );
    expect(session.current).toEqual({ kind: "bridge", domain: "ops" });
  });

  it("a failed exit leaves the player inside and fires again only after the doorway is left (Review Focus 2)", async () => {
    // Mutation caught: no latch (a load every tick in the doorway), or the
    // player thrown out of the room on a failure.
    let calls = 0;
    const load: PlaceLoader = (a, signal) => {
      calls++;
      return calls === 1
        ? Promise.resolve({ kind: "missing" })
        : okBridge(a, signal);
    };
    const session = startWithConsole([row(HALL), row("ops")], load);
    standAtBox(session);
    walkIn();
    await flush();
    backOut();
    await flush();
    frames(30); // still pressed against the doors
    await flush();
    expect(calls).toBe(1);
    expect(hud.notice).toHaveBeenCalledWith(NOT_FOUND);
    expect(isConsole(lastRoom())).toBe(true);
    key("keydown", "ArrowUp");
    frames(20);
    key("keyup", "ArrowUp");
    backOut();
    await vi.waitFor(() => {
      expect(calls).toBe(2);
    });
    await vi.waitFor(() => {
      expect(navigate).toHaveBeenCalledWith("/%CF%80/d/ops");
    });
    expect(domainNow(session)).toBe("ops");
  });

  it("never cuts in while an overlay has the keys or a load is in flight, and a jump from inside leaves the room (Review Focus 4)", async () => {
    // Mutation caught: the walk-in without its modal or loading guard, or
    // a jump that lands with the arrival box.
    const levels = vi.fn<(open: boolean) => void>();
    const slow = deferred<LoadedStation>();
    const load: PlaceLoader = (a, signal) =>
      a.kind === "engram" && a.permalink === "slow"
        ? slow.promise
        : okBridge(a, signal);
    const session = startWithConsole([row(HALL), row("ops")], load, levels);
    // Stand in the walk-in zone itself, so only the guards keep the cut out.
    const i = standAtBox(session);
    const h = heroHallRoom().heroes[i]!;
    const f = boxFront(h);
    const at = { x: f.x + f.inward[0] * 0.36, z: f.z + f.inward[1] * 0.36 };
    session.showRoom(
      {
        ...heroHallRoom(),
        spawn: {
          x: at.x / CELL - 0.5,
          y: at.z / CELL - 0.5,
          yaw: Math.atan2(f.inward[0], f.inward[1]),
        },
      },
      undefined,
      HALL_ADDRESS,
    );
    const shown = renderer.setRoom.mock.calls.length;
    type("idclev");
    frames(1);
    expect(levels).toHaveBeenLastCalledWith(true);
    frames(30); // the doors could not be opened: the select has the keys
    expect(renderer.setRoom.mock.calls.length).toBe(shown);
    session.closeLevels();
    key("keydown", "Space");
    key("keyup", "Space");
    frames(10); // half open
    session.go({ kind: "engram", domain: HALL, permalink: "slow" });
    frames(20); // open now, the player in the zone, a load in flight
    expect(renderer.setRoom.mock.calls.length).toBe(shown);
    slow.resolve({
      kind: "engram",
      place: { ...CANNED_BRIDGE, domain: HALL, permalink: "slow" },
      folder: "",
    });
    await flush();
    // Inside, a jump leaves like any go and lands without a box.
    standAtBox(session);
    walkIn();
    await flush();
    session.jump("ops");
    await vi.waitFor(() => {
      expect(navigate).toHaveBeenCalledWith("/%CF%80/d/ops");
    });
    expect(lastRoom()).toEqual(bridgeRoomOf("ops"));
  });

  it("never cuts in while the level select is open, even with the doors swinging wide under it (Review Focus 4)", () => {
    // Mutation caught: the walk-in without its modal guard. The brief's
    // own test cannot catch it, since the select takes the key that would
    // open the doors; here the doors are already opening when it opens.
    const levels = vi.fn<(open: boolean) => void>();
    const session = startWithConsole([row(HALL), row("ops")], okBridge, levels);
    const i = standAtBox(session);
    const f = boxFront(heroHallRoom().heroes[i]!);
    const at = { x: f.x + f.inward[0] * 0.36, z: f.z + f.inward[1] * 0.36 };
    session.showRoom(
      {
        ...heroHallRoom(),
        spawn: {
          x: at.x / CELL - 0.5,
          y: at.z / CELL - 0.5,
          yaw: Math.atan2(f.inward[0], f.inward[1]),
        },
      },
      undefined,
      HALL_ADDRESS,
    );
    const shown = renderer.setRoom.mock.calls.length;
    key("keydown", "Space");
    key("keyup", "Space");
    frames(2); // the doors start to open, far from open enough
    type("idclev");
    frames(1);
    expect(levels).toHaveBeenLastCalledWith(true);
    frames(30); // fully open now, the player in the zone, the select up
    expect(lastDoors().get(`box:${String(i)}`)).toBe(1);
    expect(renderer.setRoom.mock.calls.length).toBe(shown);
    session.closeLevels();
    frames(2);
    // The guard was all that kept it out: with the select closed, it cuts.
    expect(renderer.setRoom.mock.calls.length).toBe(shown + 1);
    expect(isConsole(lastRoom())).toBe(true);
  });

  it("keeps the console room and the arrival box through a restored context (Review Focus 5)", async () => {
    // Mutation caught: a restore that rebuilds the room (the box would
    // vanish, the console room would drop its player).
    const session = startWithConsole([row(HALL), row("ops")], okBridge);
    standAtBox(session);
    walkIn();
    const inside = lastRoom();
    frames(1);
    expect(lastRoom()).toBe(inside);
    await flush();
    backOut();
    await vi.waitFor(() => {
      expect(domainNow(session)).toBe("ops");
    });
    const bridge = lastRoom();
    lastCanvas?.dispatchEvent(new Event("webglcontextlost"));
    lastCanvas?.dispatchEvent(new Event("webglcontextrestored"));
    expect(lastRoom()).toBe(bridge);
    expect(bridge?.heroes.some((h) => h.kind === "police-box")).toBe(true);
  });

  it("steps out of the arrival box with its doors wide open and swinging shut (2.6e C14)", async () => {
    // Mutation caught: the arrival box's doors starting shut (no door
    // state set on landing), or left open (heading open, not shut).
    const session = startWithConsole([row(HALL), row("ops")], okBridge);
    standAtBox(session);
    walkIn();
    await flush();
    backOut();
    await vi.waitFor(() => {
      expect(domainNow(session)).toBe("ops");
    });
    const expected = roomFor(
      {
        kind: "bridge",
        place: { ...CANNED_BRIDGE, domain: "ops", permalink: "manifest" },
        bridge: { ...CANNED_BRIDGE_DATA, domain: "ops", display: "ops" },
      },
      null,
      "box",
    );
    expect(expected.box).not.toBeNull();
    expect(lastRoom()).toEqual(expected.room);
    const key = `box:${String(expected.box)}`;
    frames(1);
    expect(lastDoors().get(key)).toBeGreaterThan(0.9);
    frames(19);
    expect(lastDoors().get(key)).toBe(0);
  });

  it("does not walk back into the arrival box on the spot: only after a step out of its doorway (2.6e C11, C14)", async () => {
    // Mutation caught: the walk-in latch cleared by the doors swinging
    // shut, not by the player leaving the doorway, so turning round at the
    // arrival spot and opening the box walks straight back in.
    const session = startWithConsole([row(HALL), row("ops")], okBridge);
    standAtBox(session);
    walkIn();
    await flush();
    backOut();
    await vi.waitFor(() => {
      expect(domainNow(session)).toBe("ops");
    });
    frames(20); // the arrival box's doors swing shut
    key("keydown", "ArrowLeft");
    frames(37); // about half a turn at 3 rad/s
    key("keyup", "ArrowLeft");
    frames(1);
    expect(hud.prompt).toHaveBeenLastCalledWith("SPACE OPEN");
    const shown = renderer.setRoom.mock.calls.length;
    key("keydown", "Space");
    key("keyup", "Space");
    frames(19);
    expect(renderer.setRoom.mock.calls.length).toBe(shown);
    // A step back out of the doorway, then in again: now it walks in.
    key("keydown", "ArrowDown");
    frames(6);
    key("keyup", "ArrowDown");
    frames(10);
    key("keydown", "ArrowUp");
    for (let t = 0; t < 60 && renderer.setRoom.mock.calls.length === shown; t++)
      frames(1);
    key("keyup", "ArrowUp");
    expect(renderer.setRoom.mock.calls.length).toBe(shown + 1);
    expect(isConsole(lastRoom())).toBe(true);
  });

  it("walks back into the arrival box only after a step 1.2 m from its front, not a short one (2.6e C29)", async () => {
    // Mutation caught: the walk-in latch cleared once the player leaves
    // the 0.6 m doorway, or at any distance short of 1.2 m, so a short step
    // back and a push forward walks straight back in.
    const session = startWithConsole([row(HALL), row("ops")], okBridge);
    standAtBox(session);
    walkIn();
    await flush();
    backOut();
    await vi.waitFor(() => {
      expect(domainNow(session)).toBe("ops");
    });
    frames(20);
    key("keydown", "ArrowLeft");
    frames(37);
    key("keyup", "ArrowLeft");
    frames(1);
    expect(hud.prompt).toHaveBeenLastCalledWith("SPACE OPEN");
    key("keydown", "Space");
    key("keyup", "Space");
    frames(19);
    const room = lastRoom();
    const box = room?.heroes.find((h) => h.kind === "police-box");
    if (box === undefined) throw new Error("no arrival box");
    const away = () => {
      const f = boxFront(box);
      const e = lastCamera().eye;
      return Math.hypot(e[0] - f.x, e[2] - f.z);
    };
    const shown = renderer.setRoom.mock.calls.length;
    const stepBackTo = (d: number) => {
      key("keydown", "ArrowDown");
      for (let t = 0; t < 60 && away() < d; t++) frames(1);
      key("keyup", "ArrowDown");
      frames(15);
    };
    // Walks forward until the player stands against the box's front (or
    // cuts in), then pushes three ticks more: long enough to walk in, too
    // short to slide along the front out of the doorway.
    const pushIn = () => {
      key("keydown", "ArrowUp");
      for (
        let t = 0;
        t < 40 && away() > 0.4 && renderer.setRoom.mock.calls.length === shown;
        t++
      )
        frames(1);
      frames(3);
      key("keyup", "ArrowUp");
      frames(2);
    };
    // A short step: out of the doorway, short of 1.2 m.
    stepBackTo(0.8);
    expect(away()).toBeGreaterThan(0.6);
    expect(away()).toBeLessThan(1.2);
    pushIn();
    // In the open doorway, against the box, and still outside.
    expect(away()).toBeLessThan(0.45);
    expect(renderer.setRoom.mock.calls.length).toBe(shown);
    // A real step away: past 1.2 m, then in again.
    stepBackTo(1.4);
    expect(away()).toBeGreaterThanOrEqual(1.2);
    pushIn();
    expect(renderer.setRoom.mock.calls.length).toBe(shown + 1);
    expect(isConsole(lastRoom())).toBe(true);
  });

  it("ends the console room's visit on any other entry: its doorway's spot in another room leads nowhere (Review Focus 4)", async () => {
    // Mutation caught: an entry that does not clear the visit, so the
    // exit still fires at (6, 12) m in the next room.
    const loads: string[] = [];
    const load: PlaceLoader = (a, signal) => {
      loads.push(domainOf(a) ?? "");
      return okBridge(a, signal);
    };
    const session = startWithConsole([row(HALL), row("ops")], load);
    standAtBox(session);
    walkIn();
    await flush(); // the listing has landed: the exit could fire
    const hall = heroHallRoom();
    expect(hall.grid[5]?.[3]).toBe(".");
    session.showRoom({ ...hall, spawn: { x: 2.5, y: 5.25, yaw: 0 } });
    hud.connector.mockClear();
    frames(3);
    const eye = lastCamera().eye;
    expect(atConsoleExit({ x: eye[0], z: eye[2] })).toBe(true);
    await flush();
    expect(loads).toEqual([]);
    expect(hud.connector).not.toHaveBeenCalled();
  });

  it("tries a console room the renderer refuses once, with one notice, until the player steps out and back in", () => {
    // Mutation caught: the walk-in latched only after a successful cut, so
    // a refused room is rebuilt and refused on every tick in the doorway.
    renderer.setRoom.mockImplementation((room) => {
      if (isConsole(room)) throw new Error("too many layers");
    });
    const session = startWithConsole([row(HALL), row("ops")], okBridge);
    standAtBox(session);
    const tries = () =>
      renderer.setRoom.mock.calls.filter(([r]) => isConsole(r)).length;
    const errors = () =>
      hud.notice.mock.calls.filter(([t]) => t === "?LOAD ERROR").length;
    key("keydown", "Space");
    key("keyup", "Space");
    frames(19);
    key("keydown", "ArrowUp");
    frames(30); // pressed against the open box, in its doorway
    key("keyup", "ArrowUp");
    expect(tries()).toBe(1);
    expect(errors()).toBe(1);
    expect(domainNow(session)).toBe(HALL);
    key("keydown", "ArrowDown");
    frames(10);
    key("keyup", "ArrowDown");
    frames(10);
    key("keydown", "ArrowUp");
    frames(30);
    key("keyup", "ArrowUp");
    expect(tries()).toBe(2);
  });

  it("aborts the listing's signal when the console room is left and on dispose", async () => {
    // Mutation caught: an entry or a dispose that leaves the listing read
    // running after the visit it was for.
    const signals: AbortSignal[] = [];
    const hung = {
      domains: (signal: AbortSignal) => {
        signals.push(signal);
        return new Promise<readonly DomainRow[] | null>(() => undefined);
      },
    };
    const session = start({ load: okBridge, consoleRoom: hung });
    standAtBox(session);
    walkIn();
    expect(signals).toHaveLength(1);
    expect(signals[0]?.aborted).toBe(false);
    session.jump("ops");
    await vi.waitFor(() => {
      expect(domainNow(session)).toBe("ops");
    });
    expect(signals[0]?.aborted).toBe(true);

    const other = start({ load: okBridge, consoleRoom: hung });
    standAtBox(other);
    walkIn();
    expect(signals).toHaveLength(2);
    other.dispose();
    expect(signals[1]?.aborted).toBe(true);
  });

  it("never fires the exit while a jump is loading, even when the listing lands during it", async () => {
    // Mutation caught: the exit without its loading guard, which would
    // abort the jump in favour of a random bridge.
    const listing = deferred<readonly DomainRow[] | null>();
    const far = deferred<LoadedStation>();
    const loads: string[] = [];
    const load: PlaceLoader = (a, signal) => {
      loads.push(domainOf(a) ?? "");
      return domainOf(a) === "far" ? far.promise : okBridge(a, signal);
    };
    const session = startWithConsole(listing.promise, load);
    standAtBox(session);
    walkIn();
    backOut(); // in the doorway, the listing still being read
    session.jump("far");
    listing.resolve([row(HALL), row("ops")]);
    await flush();
    frames(5);
    await flush();
    expect(loads).toEqual(["far"]);
    far.resolve({
      kind: "bridge",
      place: { ...CANNED_BRIDGE, domain: "far", permalink: "manifest" },
      bridge: { ...CANNED_BRIDGE_DATA, domain: "far", display: "far" },
    });
    await vi.waitFor(() => {
      expect(domainNow(session)).toBe("far");
    });
    expect(loads).toEqual(["far"]);
  });

  it("gives up on a listing that has not answered in five seconds and leads to the room left's domain", async () => {
    // Mutation caught: no wait limit (the inner doors dead for good while
    // the listing hangs), or a limit that fires early.
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });
    try {
      const loads: string[] = [];
      const load: PlaceLoader = (a, signal) => {
        loads.push(domainOf(a) ?? "");
        return okBridge(a, signal);
      };
      const session = start({
        load,
        consoleRoom: {
          domains: () =>
            new Promise<readonly DomainRow[] | null>(() => undefined),
        },
      });
      standAtBox(session);
      walkIn();
      backOut(); // in the doorway
      await vi.advanceTimersByTimeAsync(LISTING_WAIT_MS - 100);
      frames(2);
      expect(loads).toEqual([]);
      await vi.advanceTimersByTimeAsync(100);
      frames(2);
      expect(loads).toEqual([HALL]);
      await vi.advanceTimersByTimeAsync(10);
      expect(session.current).toEqual({ kind: "bridge", domain: HALL });
    } finally {
      vi.useRealTimers();
    }
  });

  it("waits for the listing and falls back to the room left's domain when it fails (2.6e C13)", async () => {
    // Mutation caught: an exit fired (and latched) before the listing
    // landed, or a failed listing that strands the player.
    const listing = deferred<readonly DomainRow[] | null>();
    const loads: string[] = [];
    const load: PlaceLoader = (a, signal) => {
      loads.push(domainOf(a) ?? "");
      return okBridge(a, signal);
    };
    const session = startWithConsole(listing.promise, load);
    standAtBox(session);
    walkIn();
    backOut();
    expect(loads).toEqual([]);
    listing.resolve(null);
    await flush();
    frames(2); // still in the doorway, not latched: it fires now
    await vi.waitFor(() => {
      expect(loads).toEqual([HALL]);
    });
    await vi.waitFor(() => {
      expect(session.current).toEqual({ kind: "bridge", domain: HALL });
    });
  });
});

/** A notes deck's rows: `count` engrams in `notes`, in permalink order. */
function notesRows(count: number): DeckRow[] {
  return Array.from({ length: count }, (_, i) => {
    const n = String(i).padStart(2, "0");
    return {
      permalink: `notes/n${n}`,
      title: `Note ${n}`,
      type: "engram",
      status: "stable",
    };
  });
}

/**
 * Answers every station address kind at once: the airlock over the canned
 * listing, a bridge from the canned bridge moved to its domain, a deck of
 * 60 engrams (three sections) for `notes` and of two for the root, and an
 * engram from the canned bridge moved to its address.
 */
const stationLoad: PlaceLoader = (a) => {
  switch (a.kind) {
    case "airlock":
      return Promise.resolve({
        kind: "airlock",
        input: { domains: CANNED_DOMAINS, here: null },
      });
    case "bridge":
      return Promise.resolve({
        kind: "bridge",
        place: { ...CANNED_BRIDGE, domain: a.domain, permalink: "manifest" },
        bridge: { ...CANNED_BRIDGE_DATA, domain: a.domain, display: a.domain },
      });
    case "deck": {
      const rows = a.folder === "notes" ? notesRows(60) : notesRows(2);
      return Promise.resolve({
        kind: "deck",
        input: {
          domain: a.domain,
          folder: a.folder,
          rows,
          subfolders: [],
          total: rows.length,
          truncated: false,
        },
        section: a.section,
      });
    }
    case "engram":
      return Promise.resolve({
        kind: "engram",
        place: { ...CANNED_BRIDGE, domain: a.domain, permalink: a.permalink },
        folder: "",
      });
  }
};

/** Whether `room` is the console room: its own seed, not its fittings. */
const isConsole = (room: RoomSpec | undefined) =>
  room?.seed === consoleRoom().seed;

describe("station addresses (M3)", () => {
  it("lands on every address kind and replaces each URL at most once (Review Focus 3)", async () => {
    // Mutation caught: the landing compared by pathname alone (a deck's
    // `?path=` replaced on every landing), `current` read back from the
    // `RoomSpec` or from the address asked for instead of the one entered
    // (the MANIFEST route would stay an engram, an oversized or fractional
    // section would never be clamped), or the built route compared to the
    // location bar without itself being read back through `URL` first (M3
    // fix wave, M10b): the `it's` folder below has a `'` `encodeURIComponent`
    // leaves unescaped while `window.history.replaceState` normalises it to
    // `%27`, so an unnormalised compare would replace the URL on every
    // landing, this one included, instead of never.
    const starts: [string, StationAddress, string | null][] = [
      ["/π", { kind: "airlock" }, null],
      ["/π/d/eng", { kind: "bridge", domain: "eng" }, null],
      [
        "/π/d/eng?path=",
        { kind: "deck", domain: "eng", folder: "", section: 0 },
        null,
      ],
      [
        "/π/d/eng?path=notes&section=3",
        { kind: "deck", domain: "eng", folder: "notes", section: 2 },
        null,
      ],
      [
        "/π/d/eng/e/MANIFEST",
        { kind: "bridge", domain: "eng" },
        "/%CF%80/d/eng",
      ],
      // Past the last section: clamped to it when the deck is built (C8).
      [
        "/π/d/eng?path=notes&section=9",
        { kind: "deck", domain: "eng", folder: "notes", section: 2 },
        "/%CF%80/d/eng?path=notes&section=3",
      ],
      // Not a whole number: read as the first section.
      [
        "/π/d/eng?path=notes&section=1.5",
        { kind: "deck", domain: "eng", folder: "notes", section: 0 },
        "/%CF%80/d/eng?path=notes",
      ],
      // A folder with a `'`: normalised the same way on both sides, so it
      // never replaces the URL (M3 fix wave, M10b).
      [
        "/π/d/eng?path=it's",
        { kind: "deck", domain: "eng", folder: "it's", section: 0 },
        null,
      ],
    ];
    expect(starts.length).toBeGreaterThan(0);
    for (const [url, entered, replaced] of starts) {
      navigate.mockClear();
      window.history.replaceState(null, "", url);
      const asked = addressOfGameLocation(
        window.location.pathname,
        window.location.search,
      );
      if (asked === null) throw new Error(`not a game location: ${url}`);
      const session = start({ load: stationLoad });
      session.go(asked);
      await vi.waitFor(() => {
        expect(session.current, url).not.toBeNull();
      });
      expect(session.current, url).toEqual(entered);
      if (replaced === null) {
        expect(navigate, url).not.toHaveBeenCalled();
      } else {
        expect(navigate, url).toHaveBeenCalledTimes(1);
        expect(navigate, url).toHaveBeenCalledWith(replaced);
      }
      // Landing there again replaces nothing: the URL names it now.
      navigate.mockClear();
      session.go(entered);
      await flush();
      expect(navigate, url).not.toHaveBeenCalled();
      session.dispose();
    }
  });

  it("enters the police box from a deck and walks out to a bridge (M3 T10-2)", async () => {
    // Mutation caught: `cutIn` reading a permalink the address does not
    // have (a deck has none, so the console room would lose the room's own
    // key), or `current` moved off the deck while inside.
    const hangar = generateDeck(CANNED_HANGAR, 0);
    const pad = hangar.hangar?.pads[0];
    if (pad === undefined) throw new Error("the hangar has no pad");
    const box: Hero = {
      kind: "police-box",
      variant: 0,
      x: (pad.x0 + pad.x1) / 2,
      y: (pad.y0 + pad.y1) / 2,
      turn: 0,
      seed: pad.seed,
    };
    const room = withPadHeroes(hangar, [...hangar.heroes, box]);
    const at: StationAddress = {
      kind: "deck",
      domain: CANNED_HANGAR.domain,
      folder: CANNED_HANGAR.folder,
      section: 0,
    };
    const session = startWithConsole(
      [row(CANNED_HANGAR.domain), row("ops")],
      stationLoad,
    );
    standAtBox(session, room, at);
    walkIn();
    expect(isConsole(lastRoom())).toBe(true);
    expect(lastRoom()?.domain).toBe(CANNED_HANGAR.domain);
    expect(lastRoom()?.permalink).toBe(hangar.permalink);
    expect(hangar.permalink).toBe("cargo/flight-deck/");
    expect(session.current).toEqual(at);
    await flush();
    backOut();
    await vi.waitFor(() => {
      expect(session.current).toEqual({ kind: "bridge", domain: "ops" });
    });
    expect(navigate).toHaveBeenLastCalledWith("/%CF%80/d/ops");
  });

  it("shows a deck door's engram title on the connector (M3 T10)", () => {
    // Mutation caught: `labelFor` falling back to the permalink when the
    // room's own door names the place (a deck carries no place to ask).
    const deck = generateDeck(CANNED_DECK, 0);
    const i = deck.fixtures.findIndex(
      (f) => f.kind === "door" && f.address !== null,
    );
    const door = deck.fixtures[i];
    if (door?.kind !== "door" || door.address === null) {
      throw new Error("the deck has no door");
    }
    expect(door.label).not.toBe(door.address.permalink);
    const session = start({ load: () => new Promise(() => undefined) });
    session.showRoom(
      { ...deck, spawn: wallFacingSpawn(door.slot) },
      undefined,
      {
        kind: "deck",
        domain: deck.domain,
        folder: CANNED_DECK.folder,
        section: 0,
      },
    );
    key("keydown", "KeyW");
    for (let t = 0; t < 80 && hud.connector.mock.calls.length === 0; t++)
      frames(1);
    key("keyup", "KeyW");
    expect(hud.connector).toHaveBeenCalledWith(
      true,
      door.label,
      expect.any(String),
    );
  });

  it("says where the player is, as the status line does (M4's pause screen)", async () => {
    // Mutation caught: `where` answering the raw title or the permalink
    // instead of the status line's own upper-cased title, or a title for
    // the console room, which the status line leaves out.
    const session = startWithConsole([row(HALL), row("ops")], stationLoad);
    expect(session.where).toBeNull();
    session.showCanned(CANNED_BRIDGE);
    expect(session.where).toBe(CANNED_BRIDGE.title.toUpperCase());
    expect(session.where).not.toBe(CANNED_BRIDGE.title);
    const status = () => hud.status.mock.calls.at(-1)?.[0] ?? "";
    expect(status().startsWith(`${session.where ?? "?"}  |  `)).toBe(true);
    session.go({ kind: "airlock" });
    await vi.waitFor(() => {
      expect(session.current).toEqual({ kind: "airlock" });
    });
    expect(session.where).toBe("AIRLOCK");
    standAtBox(session);
    walkIn();
    expect(isConsole(lastRoom())).toBe(true);
    expect(session.where).toBeNull();
  });

  it("never takes the airlock for the console room, though both carry fittings", async () => {
    // Mutation caught: the console room told apart by `interior` (the
    // airlock has one too), so a renderer that refuses the console room
    // refuses the airlock with it.
    const airlock = airlockRoom({ domains: CANNED_DOMAINS, here: null });
    expect(airlock.interior?.length).toBeGreaterThan(0);
    expect(isConsole(airlock)).toBe(false);
    expect(isConsole(consoleRoom())).toBe(true);
    renderer.setRoom.mockImplementation((room) => {
      if (isConsole(room)) throw new Error("too many layers");
    });
    const session = start({ load: stationLoad });
    session.go({ kind: "airlock" });
    await vi.waitFor(() => {
      expect(session.current).toEqual({ kind: "airlock" });
    });
    expect(hud.notice).not.toHaveBeenCalledWith("?LOAD ERROR");
  });
});

describe("the lifts and the exit (M3 C26 to C29)", () => {
  const deck = generateDeck(CANNED_DECK, 0);
  const DECK_AT: StationAddress = {
    kind: "deck",
    domain: CANNED_DECK.domain,
    folder: CANNED_DECK.folder,
    section: 0,
  };

  /** The one lift of `room`, which must have one. */
  const liftOf = (room: RoomSpec): Extract<Fixture, { kind: "lift" }> => {
    const f = room.fixtures.find((x) => x.kind === "lift");
    if (f?.kind !== "lift") throw new Error("no lift");
    return f;
  };

  /** `room` with the player spawned in its lift's cell, facing the lift. */
  const facingLift = (room: RoomSpec): RoomSpec => ({
    ...room,
    spawn: wallFacingSpawn(liftOf(room).slot),
  });

  /** Presses Space, the use key, for one frame. */
  const pressUse = () => {
    key("keydown", "Space");
    frames(1);
    key("keyup", "Space");
  };

  type LiftCall = Parameters<NonNullable<SessionOptions["onLift"]>>[0];
  const liftSpy = () => vi.fn<(lift: LiftCall) => void>();
  /** The calls that opened the overlay. */
  const opened = (spy: ReturnType<typeof liftSpy>) =>
    spy.mock.calls.filter(([lift]) => lift !== null);

  /** The index of the deck lift's stop that rides to `kind`. */
  const stopTo = (kind: StationAddress["kind"]) => {
    const i = liftOf(deck).stops.findIndex((s) => s.to.kind === kind);
    expect(i).toBeGreaterThanOrEqual(0);
    return i;
  };

  /** A session with a lift spy, standing at the deck's lift. */
  const atDeckLift = (
    load: PlaceLoader,
    onLevels?: (open: boolean) => void,
    sound?: SessionOptions["sound"],
  ) => {
    const onLift = liftSpy();
    const session = start({
      load,
      onLift,
      ...(onLevels === undefined ? {} : { onLevels }),
      ...(sound === undefined ? {} : { sound }),
    });
    session.showRoom(facingLift(deck), undefined, DECK_AT);
    frames(1);
    return { session, onLift };
  };

  /** Cranks the clock by `ms`, a tick at a time. */
  const crank = (ms: number) => {
    const until = now + ms;
    while (now < until) frames(1);
  };

  /** How far in front of the exit's wall the eye of the last frame is. */
  const exitDepth = () => {
    const room = lastRoom();
    const exit = room?.fixtures.find((f) => f.kind === "exit");
    if (exit === undefined) throw new Error("no exit");
    const w = wallPoint(exit.slot);
    const eye = lastCamera().eye;
    return (eye[0] - w.x) * w.inward[0] + (eye[2] - w.z) * w.inward[1];
  };

  /** Holds `code` until `done` or `max` frames, then lets go. */
  const holdUntil = (code: string, done: () => boolean, max = 200) => {
    key("keydown", code);
    for (let t = 0; t < max && !done(); t++) frames(1);
    key("keyup", code);
  };

  const connectorUps = () =>
    hud.connector.mock.calls.filter(([active]) => active).length;

  it("opens the lift's stops on Space and holds the keys while they are open (C26)", () => {
    // Mutation caught: the lift overlay left out of `modal()` (a second
    // Space opens it again and the player walks off under it).
    const { session, onLift } = atDeckLift(() => new Promise(() => undefined));
    expect(hud.prompt).toHaveBeenLastCalledWith("SPACE LIFT");
    pressUse();
    const lift = liftOf(deck);
    expect(lift.stops.length).toBeGreaterThan(0);
    expect(opened(onLift)).toEqual([[{ stops: lift.stops, note: lift.note }]]);
    const eye = lastCamera().eye;
    pressUse();
    key("keydown", "KeyW");
    frames(10);
    key("keyup", "KeyW");
    expect(opened(onLift)).toHaveLength(1);
    expect(lastCamera().eye).toEqual(eye);
    session.closeLift();
    expect(onLift).toHaveBeenLastCalledWith(null);
    pressUse();
    expect(opened(onLift)).toHaveLength(2);
  });

  it("refuses the lift while busy and settles a failed stop or exit with a notice only (Review Focus 4)", async () => {
    // Mutation caught: the modal check dropped (the overlay opened under
    // a load in flight or the level select), the held result not dropped
    // on `leave`, a failed exit marking its way (it would never open
    // again). A failed stop marking the lift is not visible from outside:
    // a lift's offer and its doors ignore `failed`.
    // A load in flight.
    {
      const { session, onLift } = atDeckLift(
        () => new Promise(() => undefined),
      );
      session.go({ kind: "bridge", domain: "ops" });
      pressUse();
      frames(2);
      expect(opened(onLift)).toHaveLength(0);
      session.dispose();
    }
    // The level select.
    {
      const onLevels = vi.fn<(open: boolean) => void>();
      const { session, onLift } = atDeckLift(stationLoad, onLevels);
      type("idclev");
      frames(1);
      expect(onLevels).toHaveBeenLastCalledWith(true);
      pressUse();
      frames(2);
      expect(opened(onLift)).toHaveLength(0);
      session.dispose();
    }
    // A failed stop: the notice, and nothing else changes.
    const answers: Record<string, string> = {
      missing: NOT_FOUND,
      denied: ACCESS_DENIED,
      offline: "SIGNAL LOST",
    };
    let answer: "missing" | "denied" | "offline" = "missing";
    const failing: PlaceLoader = () => Promise.resolve({ kind: answer });
    const { session, onLift } = atDeckLift(failing);
    for (const kind of ["missing", "denied", "offline"] as const) {
      answer = kind;
      pressUse();
      const opens = opened(onLift).length;
      expect(opens).toBeGreaterThan(0);
      const rooms = renderer.setRoom.mock.calls.length;
      hud.notice.mockClear();
      session.ride(stopTo("bridge"));
      await flush();
      crank(1300);
      expect(hud.notice, kind).toHaveBeenCalledWith(answers[kind]);
      expect(session.current, kind).toEqual(DECK_AT);
      expect(renderer.setRoom.mock.calls.length, kind).toBe(rooms);
      pressUse();
      expect(opened(onLift).length, kind).toBe(opens + 1);
      session.closeLift();
    }
    session.dispose();

    // A ride's held result is dropped by a go from outside.
    {
      const load: PlaceLoader = (a, signal) =>
        a.kind === "engram"
          ? new Promise(() => undefined)
          : stationLoad(a, signal);
      const { session: s, onLift: spy } = atDeckLift(load);
      pressUse();
      expect(opened(spy)).toHaveLength(1);
      s.ride(stopTo("bridge"));
      await flush();
      s.go(engramAt("station", "elsewhere"));
      crank(1500);
      await flush();
      expect(s.current).toEqual(DECK_AT);
      s.dispose();
    }

    // A second ride aborts the first and lands only the second.
    {
      const signals: AbortSignal[] = [];
      const load: PlaceLoader = (a, signal) => {
        if (a.kind === "bridge") {
          signals.push(signal);
          return new Promise(() => undefined);
        }
        return stationLoad(a, signal);
      };
      const { session: s, onLift: spy } = atDeckLift(load);
      pressUse();
      expect(opened(spy)).toHaveLength(1);
      const rooms = renderer.setRoom.mock.calls.length;
      s.ride(stopTo("bridge"));
      const up = liftOf(deck).stops.findIndex((x) => x.label === "UP");
      expect(up).toBeGreaterThanOrEqual(0);
      s.ride(up);
      expect(signals).toHaveLength(1);
      expect(signals[0]?.aborted).toBe(true);
      await flush();
      crank(1300);
      await flush();
      expect(s.current).toEqual({
        ...liftOf(deck).stops[up]!.to,
        section: 0,
      });
      expect(renderer.setRoom.mock.calls.length).toBe(rooms + 1);
      s.dispose();
    }

    // A failed exit: the notice, and the exit opens and carries again.
    {
      const load: PlaceLoader = (a, signal) =>
        a.kind === "deck"
          ? Promise.resolve({ kind: "missing" })
          : stationLoad(a, signal);
      const s = start({ load, onLift: liftSpy() });
      s.go(engramAt("station", "alpha"));
      await vi.waitFor(() => {
        expect(s.current).toEqual(engramAt("station", "alpha"));
      });
      frames(1);
      holdUntil("ArrowUp", () => exitDepth() > 2.5);
      frames(10);
      const ups = connectorUps();
      hud.notice.mockClear();
      holdUntil("ArrowDown", () => connectorUps() > ups);
      expect(connectorUps()).toBe(ups + 1);
      await flush();
      expect(hud.notice).toHaveBeenCalledWith(NOT_FOUND);
      expect(s.current).toEqual(engramAt("station", "alpha"));
      holdUntil("ArrowUp", () => exitDepth() > 1.5);
      frames(10);
      holdUntil("ArrowDown", () => connectorUps() > ups + 1);
      expect(connectorUps()).toBe(ups + 2);
      s.dispose();
    }
  });

  it("lands a ride at LIFT_RIDE_MS after it started, its lift's doors open and heading shut (C27)", async () => {
    // Mutation caught: the result entered as soon as the load settles, or
    // the arrival's lift doors starting shut.
    const settled = deferred<LoadedStation>();
    const load: PlaceLoader = (a, signal) =>
      a.kind === "bridge" ? settled.promise : stationLoad(a, signal);
    const { session, onLift } = atDeckLift(load);
    pressUse();
    expect(opened(onLift)).toHaveLength(1);
    const i = stopTo("bridge");
    const stop = liftOf(deck).stops[i]!;
    const t0 = now;
    session.ride(i);
    expect(onLift).toHaveBeenLastCalledWith(null);
    expect(hud.connector).toHaveBeenLastCalledWith(
      true,
      stop.label,
      expect.any(String),
    );
    crank(200);
    settled.resolve(await stationLoad(stop.to, new AbortController().signal));
    await flush();
    let landedAt: number | null = null;
    for (let t = 0; t < 100 && landedAt === null; t++) {
      frames(1);
      if (session.current?.kind === "bridge") landedAt = now;
    }
    expect(landedAt).not.toBeNull();
    expect(landedAt ?? 0).toBeGreaterThanOrEqual(t0 + LIFT_RIDE_MS);
    expect(landedAt ?? 0).toBeLessThan(t0 + LIFT_RIDE_MS + 3 * TICK_MS);
    expect(hud.connector).toHaveBeenLastCalledWith(
      false,
      stop.label,
      expect.any(String),
    );
    const bridge = lastRoom();
    if (bridge === undefined) throw new Error("no room");
    const lift = bridge.fixtures.findIndex((f) => f.kind === "lift");
    expect(lift).toBeGreaterThanOrEqual(0);
    // The landing frame itself draws the lift open, never shut for a frame.
    expect(lastDoors().get(`door:${String(lift)}`)).toBe(1);
    frames(2);
    const open = lastDoors().get(`door:${String(lift)}`) ?? 0;
    expect(open).toBeGreaterThan(0.5);
    expect(open).toBeLessThan(1);
    frames(20);
    expect(lastDoors().get(`door:${String(lift)}`)).toBe(0);
  });

  it("sends the ride's depart, and its arrive whether the landing was held or not (M4 C23, F16)", async () => {
    // Mutation caught: the arrive sent only for a held landing (a load
    // slower than the ride keeps the hum going for good), or sent before
    // the landing.
    for (const lateMs of [200, LIFT_RIDE_MS + 500]) {
      const settled = deferred<LoadedStation>();
      const load: PlaceLoader = (a, signal) =>
        a.kind === "bridge" ? settled.promise : stationLoad(a, signal);
      const { cues, sink } = recordSound();
      const { session } = atDeckLift(load, undefined, sink);
      pressUse();
      // The lift's stops open with a terminal cue.
      expect(cues.at(-1)).toEqual({ kind: "terminal" });
      const i = stopTo("bridge");
      const stop = liftOf(deck).stops[i]!;
      cues.length = 0;
      session.ride(i);
      expect(cues).toEqual([
        { kind: "travel", via: "lift" },
        { kind: "ride", phase: "depart" },
      ]);
      crank(lateMs);
      settled.resolve(await stationLoad(stop.to, new AbortController().signal));
      await flush();
      expect(cues.some((c) => c.kind === "ride" && c.phase === "arrive")).toBe(
        session.current?.kind === "bridge",
      );
      for (let t = 0; t < 100 && session.current?.kind !== "bridge"; t++)
        frames(1);
      expect(session.current?.kind).toBe("bridge");
      const arrive = cues.findIndex(
        (c) => c.kind === "ride" && c.phase === "arrive",
      );
      expect(arrive).toBeGreaterThan(0);
      expect(
        cues.filter((c) => c.kind === "ride" && c.phase === "arrive"),
      ).toHaveLength(1);
      expect(cues.slice(arrive).some((c) => c.kind === "room")).toBe(true);
      session.dispose();
    }
  });

  it("ends the ride's sound once when the stop fails, the load errors or a new place drops the ride (M4 F16)", async () => {
    // Mutation caught: the arrive sent only for a landing that enters a
    // room (a failed stop keeps the hum), or dropped from the load's
    // error path or from `leave`.
    const arrives = (cues: readonly Cue[]) =>
      cues.filter((c) => c.kind === "ride" && c.phase === "arrive").length;
    const cases: {
      name: string;
      load: PlaceLoader;
      after: (session: Session) => void;
    }[] = [
      {
        name: "missing",
        load: (a, signal) =>
          a.kind === "bridge"
            ? Promise.resolve({ kind: "missing" })
            : stationLoad(a, signal),
        after: () => undefined,
      },
      {
        name: "error",
        load: (a, signal) =>
          a.kind === "bridge"
            ? Promise.reject(new Error("broken"))
            : stationLoad(a, signal),
        after: () => undefined,
      },
      {
        name: "leave",
        load: (a, signal) =>
          a.kind === "bridge"
            ? new Promise<LoadedStation>(() => undefined)
            : stationLoad(a, signal),
        after: (session) => {
          session.jump("eng");
        },
      },
    ];
    for (const c of cases) {
      const { cues, sink } = recordSound();
      const { session } = atDeckLift(c.load, undefined, sink);
      pressUse();
      session.ride(stopTo("bridge"));
      expect(cues.at(-1), c.name).toEqual({ kind: "ride", phase: "depart" });
      expect(arrives(cues), c.name).toBe(0);
      c.after(session);
      await flush();
      crank(LIFT_RIDE_MS + 200);
      await flush();
      expect(arrives(cues), c.name).toBe(1);
      expect(session.current?.kind, c.name).toBe("deck");
      session.dispose();
    }
  });

  it("rides to the stop the lift already stands at by only closing the overlay", () => {
    // Mutation caught: `stop.here` ignored, so picking the current section
    // starts a ride and reloads the deck the player is already standing in
    // instead of just closing the overlay.
    const { session, onLift } = atDeckLift(stationLoad);
    pressUse();
    expect(opened(onLift)).toHaveLength(1);
    const stops = liftOf(deck).stops;
    const here = stops.findIndex((s) => s.here);
    expect(here).toBeGreaterThanOrEqual(0);
    const rooms = renderer.setRoom.mock.calls.length;
    session.ride(here);
    expect(onLift).toHaveBeenLastCalledWith(null);
    expect(hud.connector).not.toHaveBeenCalledWith(
      true,
      stops[here]?.label,
      expect.any(String),
    );
    expect(renderer.setRoom.mock.calls.length).toBe(rooms);
    expect(session.current).toEqual(DECK_AT);
  });

  it("re-reads a failed airlock listing from its lift", () => {
    // Mutation caught: the overlay opened with no stops.
    const airlock = airlockRoom({ domains: null, here: null });
    expect(liftOf(airlock).note).toBe(LIFT_WORDS.domainError);
    const load = vi.fn<PlaceLoader>(() => new Promise(() => undefined));
    const onLift = liftSpy();
    const session = start({ load, onLift });
    session.showRoom(facingLift(airlock), undefined, { kind: "airlock" });
    frames(1);
    expect(hud.prompt).toHaveBeenLastCalledWith("SPACE LIFT");
    expect(load).not.toHaveBeenCalled();
    pressUse();
    expect(load).toHaveBeenCalledTimes(1);
    expect(load.mock.calls[0]?.[0]).toEqual({ kind: "airlock" });
    expect(onLift).not.toHaveBeenCalled();
  });

  it("latches the exit on entry: a step back does nothing, a walk in and back out goes up (C28)", async () => {
    // Mutation caught: the latch dropped (the exit, still open from the
    // walk in, carries the player straight back up) or never released.
    const door = deck.fixtures.findIndex(
      (f) => f.kind === "door" && f.address !== null,
    );
    const through = deck.fixtures[door];
    if (through?.kind !== "door" || through.address === null)
      throw new Error("the deck has no door");
    const session = start({ load: stationLoad, onLift: liftSpy() });
    session.showRoom(
      { ...deck, spawn: wallFacingSpawn(through.slot) },
      undefined,
      DECK_AT,
    );
    holdUntil("KeyW", () => connectorUps() > 0, 80);
    const target = stationOfPlace(through.address);
    await vi.waitFor(() => {
      expect(session.current).toEqual(target);
    });
    frames(2);
    const room = lastRoom();
    const exit = room?.fixtures.findIndex((f) => f.kind === "exit") ?? -1;
    expect(exit).toBeGreaterThanOrEqual(0);
    // Out of the deck's door means in through the exit: open, heading shut.
    const open = lastDoors().get(`door:${String(exit)}`) ?? 0;
    expect(open).toBeGreaterThan(0.5);
    // A step straight back reaches the doorway and goes nowhere.
    const ups = connectorUps();
    let nearest = Infinity;
    key("keydown", "ArrowDown");
    for (let t = 0; t < 40; t++) {
      frames(1);
      nearest = Math.min(nearest, exitDepth());
    }
    key("keyup", "ArrowDown");
    expect(nearest).toBeLessThan(0.6);
    expect(connectorUps()).toBe(ups);
    expect(session.current).toEqual(target);
    // In past UP_LATCH_CLEAR and back out: up to the deck.
    holdUntil("ArrowUp", () => exitDepth() > UP_LATCH_CLEAR + 0.5);
    expect(exitDepth()).toBeGreaterThan(UP_LATCH_CLEAR);
    frames(10);
    holdUntil("ArrowDown", () => connectorUps() > ups);
    expect(connectorUps()).toBe(ups + 1);
    await vi.waitFor(() => {
      expect(session.current?.kind).toBe("deck");
    });
  });
});

describe("the pause (M4 C6 to C9)", () => {
  /** Holds the pointer locked to the newest session's canvas. */
  function lock() {
    Object.defineProperty(document, "pointerLockElement", {
      configurable: true,
      get: () => lastCanvas,
    });
    document.dispatchEvent(new Event("pointerlockchange"));
  }

  /** Ends the lock, as the browser does on Esc or a lost focus. */
  function unlock() {
    Reflect.deleteProperty(document, "pointerLockElement");
    document.dispatchEvent(new Event("pointerlockchange"));
  }

  /**
   * An Esc keydown sent from the page's body, as a browser sends it, so a
   * listener on the body can cancel it before the window hears it.
   */
  function esc(options: { repeat?: boolean; prevented?: boolean } = {}) {
    const prevent = (e: Event) => {
      e.preventDefault();
    };
    if (options.prevented === true)
      document.body.addEventListener("keydown", prevent);
    document.body.dispatchEvent(
      new KeyboardEvent("keydown", {
        key: "Escape",
        code: "Escape",
        repeat: options.repeat ?? false,
        bubbles: true,
        cancelable: true,
      }),
    );
    document.body.removeEventListener("keydown", prevent);
  }

  const pauseSpy = () => vi.fn<(paused: boolean) => void>();
  const pauses = (spy: ReturnType<typeof pauseSpy>) =>
    spy.mock.calls.filter(([p]) => p).length;

  afterEach(() => {
    Reflect.deleteProperty(document, "pointerLockElement");
  });

  it("pauses on a lost lock and an unlocked Esc only when nothing is modal", () => {
    // Mutation caught: the modal check dropped (the reader's lock release
    // or a busy screen would pause), `repeat` or `defaultPrevented` not
    // checked, `!loading` added back (a lost lock during a load would not
    // pause).
    const onPause = pauseSpy();
    const session = start({ client: null, onPause });
    session.showCanned(CANNED_BRIDGE);
    frames(1);

    // (a) The lock ends with nothing open: paused.
    lock();
    unlock();
    expect(onPause).toHaveBeenLastCalledWith(true);
    expect(session.paused).toBe(true);
    session.resume();
    expect(onPause).toHaveBeenLastCalledWith(false);
    expect(session.paused).toBe(false);

    // (b) With the reader open, the same loss pauses nothing.
    walkToScope();
    key("keydown", "Space");
    frames(1);
    key("keyup", "Space");
    expect(hud.reader).toHaveBeenLastCalledWith(
      expect.objectContaining({ title: "Station Crystalline" }),
    );
    lock();
    unlock();
    esc();
    expect(pauses(onPause)).toBe(1);
    session.closeReader();

    // (c) An Esc while unlocked pauses.
    esc();
    expect(pauses(onPause)).toBe(2);
    session.resume();

    // (d) An auto-repeated one does not.
    esc({ repeat: true });
    expect(pauses(onPause)).toBe(2);

    // (e) Nor one something else already took.
    esc({ prevented: true });
    expect(pauses(onPause)).toBe(2);

    // (f) While busy, nothing pauses.
    session.setBusy(true);
    lock();
    unlock();
    esc();
    expect(pauses(onPause)).toBe(2);
    session.setBusy(false);
    esc();
    expect(pauses(onPause)).toBe(3);
    session.resume();
  });

  it("never pauses without onPause, as on the look demo and the gallery", () => {
    // Mutation caught: the pause listeners registered whatever the host
    // passed (a lost lock or an Esc would freeze a screen that shows no
    // pause and never resumes).
    const session = start({ client: null });
    session.showCanned(CANNED_BRIDGE);
    frames(1);
    lock();
    unlock();
    esc();
    expect(session.paused).toBe(false);
    const eye = lastCamera().eye;
    key("keydown", "KeyW");
    frames(5);
    key("keyup", "KeyW");
    expect(lastCamera().eye).not.toEqual(eye);
  });

  it("pauses on a lost lock while a load is in flight (C6a)", () => {
    const onPause = pauseSpy();
    const session = start({
      load: () => new Promise(() => undefined),
      onPause,
    });
    session.showCanned(CANNED_BRIDGE);
    frames(1);
    session.go({ kind: "airlock" });
    expect(hud.connector).toHaveBeenLastCalledWith(
      true,
      expect.any(String),
      expect.any(String),
    );
    lock();
    unlock();
    expect(onPause).toHaveBeenLastCalledWith(true);
  });

  it("freezes the station while paused", () => {
    // Mutation caught: the tick not returning early while paused (the W
    // walks, the door slides open).
    const deck = generateDeck(CANNED_DECK, 0);
    const i = deck.fixtures.findIndex(
      (f) => f.kind === "door" && f.address !== null,
    );
    const door = deck.fixtures[i];
    if (door?.kind !== "door") throw new Error("the deck has no door");
    const onPause = pauseSpy();
    const session = start({
      load: () => new Promise(() => undefined),
      onPause,
    });
    session.showRoom(
      { ...deck, spawn: wallFacingSpawn(door.slot) },
      undefined,
      {
        kind: "deck",
        domain: deck.domain,
        folder: CANNED_DECK.folder,
        section: 0,
      },
    );
    frames(1);
    const eye = lastCamera().eye;
    const shut = lastDoors().get(`door:${String(i)}`) ?? 0;
    expect(shut).toBeLessThan(0.5);
    session.pause();
    expect(onPause).toHaveBeenLastCalledWith(true);
    key("keydown", "KeyW");
    frames(35);
    expect(lastCamera().eye).toEqual(eye);
    expect(lastDoors().get(`door:${String(i)}`) ?? 0).toBe(shut);
    expect(hud.connector).not.toHaveBeenCalledWith(
      true,
      expect.any(String),
      expect.any(String),
    );

    session.resume();
    expect(onPause).toHaveBeenLastCalledWith(false);
    // The W held across the pause was forgotten; a new press walks.
    key("keyup", "KeyW");
    key("keydown", "KeyW");
    frames(5);
    key("keyup", "KeyW");
    expect(lastCamera().eye).not.toEqual(eye);
    expect(lastDoors().get(`door:${String(i)}`) ?? 0).toBeGreaterThan(shut);
  });

  it("closes the pause on a new place", () => {
    // Mutation caught: `leave` not closing the pause.
    const onPause = pauseSpy();
    const session = start({ client: null, onPause });
    session.showCanned(CANNED_BRIDGE);
    frames(1);
    session.pause();
    expect(onPause).toHaveBeenLastCalledWith(true);
    session.go({ kind: "airlock" });
    expect(onPause).toHaveBeenLastCalledWith(false);
    expect(session.paused).toBe(false);
  });

  it("pauses nothing while it is disposed", () => {
    // Mutation caught: the pause listeners removed after `input.dispose()`
    // and the handler not guarded by `disposed` (F18): `input.dispose()`
    // releases the lock, and that loss would pause a session going down.
    const onPause = pauseSpy();
    const session = start({ client: null, onPause });
    session.showCanned(CANNED_BRIDGE);
    frames(1);
    lock();
    const exit = vi.fn(() => {
      unlock();
    });
    Object.defineProperty(document, "exitPointerLock", {
      configurable: true,
      value: exit,
    });
    try {
      session.dispose();
    } finally {
      Reflect.deleteProperty(document, "exitPointerLock");
    }
    expect(exit).toHaveBeenCalledTimes(1);
    expect(pauses(onPause)).toBe(0);
  });

  it("releases the lock when paused from outside and is modal until resumed", () => {
    const onPause = pauseSpy();
    const session = start({ client: null, onPause });
    session.showCanned(CANNED_BRIDGE);
    frames(1);
    lock();
    const exit = vi.fn(() => {
      unlock();
    });
    Object.defineProperty(document, "exitPointerLock", {
      configurable: true,
      value: exit,
    });
    try {
      session.pause();
    } finally {
      Reflect.deleteProperty(document, "exitPointerLock");
    }
    expect(exit).toHaveBeenCalledTimes(1);
    // The lost lock that followed is the pause's own, not a second one.
    expect(onPause.mock.calls).toEqual([[true]]);
    // Resuming twice tells the host once.
    session.resume();
    session.resume();
    expect(onPause.mock.calls).toEqual([[true], [false]]);
  });

  it("asks for the lock again on resume, once (C8)", () => {
    // Mutation caught: `resume` ending the pause without asking for the
    // lock (CONT would leave the player unlocked, to click once more).
    const onPause = pauseSpy();
    const session = start({ client: null, onPause });
    session.showCanned(CANNED_BRIDGE);
    frames(1);
    const canvas = lastCanvas;
    if (canvas === null) throw new Error("no canvas");
    const request = vi.fn(() => Promise.resolve());
    Object.defineProperty(canvas, "requestPointerLock", {
      configurable: true,
      value: request,
    });
    lock();
    unlock();
    expect(session.paused).toBe(true);
    // Past the browser's wait for a new lock.
    const later = session.lockEndedAt + RELOCK_DELAY_MS + 1;
    vi.spyOn(performance, "now").mockReturnValue(later);
    session.resume();
    expect(request).toHaveBeenCalledTimes(1);
    session.resume();
    expect(request).toHaveBeenCalledTimes(1);
  });

  it("forgets a key pressed on the pause screen when it resumes", () => {
    // Mutation caught: `closePause` not clearing the input (a W pressed
    // while paused and still held would walk the player off on resume).
    const session = start({ client: null, onPause: pauseSpy() });
    session.showCanned(CANNED_BRIDGE);
    frames(1);
    session.pause();
    key("keydown", "KeyW");
    frames(2);
    const eye = lastCamera().eye;
    session.resume();
    frames(10);
    key("keyup", "KeyW");
    expect(lastCamera().eye).toEqual(eye);
  });

  it("draws nothing while paused and holds the shader's time across it (M4 C6)", () => {
    // Mutation caught: the draw not skipped while paused, or the paused
    // span not taken off the time (the effects would jump on resume).
    const session = start({ client: null, onPause: pauseSpy() });
    session.showCanned(CANNED_BRIDGE);
    frames(3);
    const drawn = renderer.draw.mock.calls.length;
    const before = renderer.draw.mock.calls.at(-1)?.[2] ?? NaN;
    session.pause();
    frames(35);
    now += 5000;
    frames(35);
    expect(renderer.draw.mock.calls.length).toBe(drawn);
    session.resume();
    frames(1);
    expect(renderer.draw.mock.calls.length).toBe(drawn + 1);
    const after = renderer.draw.mock.calls.at(-1)?.[2] ?? NaN;
    expect(after).toBeGreaterThan(before);
    expect(after - before).toBeLessThan(0.1);
  });

  it("stamps when the lock ended, for the pause screen's wait (C8)", () => {
    const session = start({ client: null });
    expect(session.lockEndedAt).toBe(-Infinity);
    lock();
    const before = performance.now();
    unlock();
    expect(session.lockEndedAt).toBeGreaterThanOrEqual(before);
    expect(session.lockEndedAt).toBeLessThanOrEqual(performance.now());
  });
});

describe("live changes (M4 C13 to C19)", () => {
  /** The engram room the live-change tests stand in. */
  const ROOM_AT = engramAt("station", "hall");

  /** The canned bridge's place moved to `station/hall`, with changes. */
  const placeOf = (o: Partial<PlaceInput> = {}): PlaceInput => ({
    ...CANNED_BRIDGE,
    domain: "station",
    permalink: "hall",
    ...o,
  });

  /** The place with one routing line changed: a text change. */
  const retexted = (word = "new") =>
    placeOf({
      content: CANNED_BRIDGE.content.replace(
        "Engineering questions go to the reactor deck.",
        `Engineering questions go to the ${word} reactor deck.`,
      ),
    });

  /** The place with one more relation: a new door, a shape change. */
  const redoored = (n = 1) =>
    placeOf({
      relations: [
        ...CANNED_BRIDGE.relations,
        ...Array.from({ length: n }, (_, i) => ({
          relType: "relates_to",
          target: { domain: null, target: `annex-${String(i)}` },
          resolved: true,
          address: { domain: "station", permalink: `annex-${String(i)}` },
          targetTitle: `Annex ${String(i)}`,
          targetSalience: 3,
        })),
      ],
    });

  const engramOf = (
    place: PlaceInput,
  ): Extract<LoadedStation, { kind: "engram" }> => ({
    kind: "engram",
    place,
    folder: "",
  });

  /** What the loader answers for an engram, by permalink. */
  let answers: Map<string, LoadedStation>;
  /** Every address the loader was asked for, with the clock at the time. */
  let asked: { address: StationAddress; at: number }[];
  /** A load held open for an engram, by permalink: answered once. */
  let holds: Map<string, Promise<LoadedStation>>;

  const liveLoad: PlaceLoader = (a, signal) => {
    asked.push({ address: a, at: now });
    if (a.kind !== "engram") return stationLoad(a, signal);
    const held = holds.get(a.permalink);
    if (held !== undefined) {
      holds.delete(a.permalink);
      return held;
    }
    const answer = answers.get(a.permalink);
    return Promise.resolve(answer ?? { kind: "missing" });
  };

  /** How many times the loader was asked for `permalink`. */
  const loadsOf = (permalink: string) =>
    asked.filter(
      (c) => c.address.kind === "engram" && c.address.permalink === permalink,
    ).length;

  /** An engram frame of `station/hall`, with changes. */
  const frameOf = (o: Partial<EngramChange> = {}): ChangeEvent => ({
    event: "engram",
    change: {
      domain: "station",
      permalink: "hall",
      path: "hall.md",
      kind: "modified",
      from: null,
      checksum: "1",
      actor: null,
      draftOf: null,
      ...o,
    },
  });

  /** Lets every settled promise run its callbacks, with no timer. */
  async function micro() {
    for (let i = 0; i < 10; i++) await Promise.resolve();
  }

  /**
   * Runs `ms` of the station: the loop's clock and the timers step
   * together, a tick at a time, and every settled load lands.
   */
  async function run(ms: number) {
    const until = now + ms;
    while (now < until) {
      vi.advanceTimersByTime(TICK_MS);
      await micro();
      frames(1);
    }
  }

  /** The room `roomFor` builds from `place`, as the session would. */
  const built = (place: PlaceInput) =>
    roomFor(engramOf(place), null, null).room;

  /** Starts a session standing in `station/hall`, its first answer `place`. */
  async function standIn(
    place: PlaceInput = placeOf(),
    extra: Parameters<typeof start>[0] = {},
  ): Promise<Session> {
    answers.set("hall", engramOf(place));
    const session = start({ load: liveLoad, ...extra });
    session.go(ROOM_AT);
    await micro();
    frames(1);
    expect(session.current).toEqual(ROOM_AT);
    return session;
  }

  /** Every level array drawn, copied as it was drawn. */
  let drawn: {
    at: number;
    levels: Float32Array;
    camera: Camera;
    doors: Map<string, number>;
  }[];
  /** The clock at every `setRoom`, with the room. */
  let rooms: { at: number; room: RoomSpec }[];
  /** The clock at every notice, with its text. */
  let notices: { at: number; text: string | null }[];

  beforeEach(() => {
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });
    answers = new Map();
    holds = new Map();
    asked = [];
    drawn = [];
    rooms = [];
    notices = [];
    renderer.draw.mockImplementation((camera, levels, _t, doors) => {
      drawn.push({
        at: now,
        levels: Float32Array.from(levels),
        camera,
        doors: new Map(doors),
      });
    });
    renderer.setRoom.mockImplementation((room) => {
      rooms.push({ at: now, room });
    });
    hud.notice.mockImplementation((text) => {
      notices.push({ at: now, text });
    });
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  /** The room the renderer was last handed. */
  const shown = (): RoomSpec => {
    const room = rooms.at(-1)?.room;
    if (room === undefined) throw new Error("no room");
    return room;
  };

  describe("holds a re-check while busy and applies the latest once (Review Focus 3)", () => {
    // Mutation caught: no pending flag (the change lost), no coalescing,
    // no reshape throttle.
    it("while a door's load is in flight", async () => {
      const session = await standIn();
      const other = deferred<LoadedStation>();
      holds.set("beta", other.promise);
      asked = [];
      session.go(engramAt("station", "beta"));
      answers.set("hall", engramOf(retexted()));
      session.changed(frameOf());
      await run(1000);
      expect(loadsOf("hall")).toBe(0);
      other.resolve({ kind: "offline" });
      await run(500);
      expect(loadsOf("hall")).toBe(1);
      expect(shown()).toEqual(built(retexted()));
    });

    it("while the reader is open", async () => {
      facing.kind = "terminal";
      const session = await standIn();
      key("keydown", "Space");
      frames(1);
      key("keyup", "Space");
      expect(hud.reader.mock.calls.at(-1)?.[0]).not.toBeNull();
      asked = [];
      answers.set("hall", engramOf(retexted()));
      session.changed(frameOf());
      await run(1000);
      expect(loadsOf("hall")).toBe(0);
      session.closeReader();
      await run(500);
      expect(loadsOf("hall")).toBe(1);
      expect(shown()).toEqual(built(retexted()));
    });

    it("while paused, and a re-check already in flight when the pause starts lands on resume", async () => {
      const onPause = vi.fn<(paused: boolean) => void>();
      const session = await standIn(placeOf(), { onPause });
      asked = [];
      answers.set("hall", engramOf(retexted()));
      session.pause();
      session.changed(frameOf());
      await run(1000);
      expect(loadsOf("hall")).toBe(0);
      const before = rooms.length;
      session.resume();
      await run(500);
      expect(loadsOf("hall")).toBe(1);
      expect(rooms.length).toBe(before + 1);
      expect(shown()).toEqual(built(retexted()));

      // A re-check whose answer lands under the pause changes nothing
      // there: the room under the pause screen stays the one it names,
      // and the latest answer is entered on resume.
      const slow = deferred<LoadedStation>();
      holds.set("hall", slow.promise);
      asked = [];
      session.changed(frameOf());
      await run(300);
      expect(loadsOf("hall")).toBe(1);
      session.pause();
      slow.resolve(engramOf(retexted("third")));
      const count = rooms.length;
      await run(1000);
      expect(rooms.length).toBe(count);
      answers.set("hall", engramOf(retexted("fourth")));
      session.resume();
      await run(500);
      expect(shown()).toEqual(built(retexted("fourth")));
    });

    it("during a lift ride's hold", async () => {
      const deckRoom = generateDeck(CANNED_DECK, 0);
      const lift = deckRoom.fixtures.find((f) => f.kind === "lift");
      if (lift?.kind !== "lift") throw new Error("no lift");
      const stop = lift.stops.findIndex((s) => s.to.kind === "bridge");
      expect(stop).toBeGreaterThanOrEqual(0);
      const onLift = vi.fn();
      const session = start({ load: liveLoad, onLift });
      const at: StationAddress = {
        kind: "deck",
        domain: CANNED_DECK.domain,
        folder: CANNED_DECK.folder,
        section: 0,
      };
      session.showRoom(
        { ...deckRoom, spawn: wallFacingSpawn(lift.slot) },
        undefined,
        at,
      );
      frames(1);
      key("keydown", "Space");
      frames(1);
      key("keyup", "Space");
      expect(onLift.mock.calls.at(-1)?.[0]).not.toBeNull();
      session.ride(stop);
      await micro();
      asked = [];
      session.changed({
        event: "domain",
        change: { domain: CANNED_DECK.domain, actor: null },
      });
      // The ride's load has settled; its landing is held for the ride.
      await run(LIFT_RIDE_MS - 2 * TICK_MS);
      expect(session.current?.kind).toBe("deck");
      expect(asked).toHaveLength(0);
      await run(500);
      expect(session.current?.kind).toBe("bridge");
      expect(asked).toHaveLength(1);
    });

    it("gives 20 frames within 2 s one re-check per coalesce window, at most one reshape per 3 s, and enters the last answer", async () => {
      const session = await standIn();
      // Twenty frames of a save that changed nothing: one re-check per
      // window, never one per frame.
      asked = [];
      for (let i = 0; i < 20; i++) {
        session.changed(frameOf());
        await run(100);
      }
      await run(1000);
      expect(asked.length).toBeGreaterThan(1);
      expect(asked.length).toBeLessThanOrEqual(
        Math.ceil(2000 / COALESCE_MS) + 1,
      );
      // Twenty frames of changes that each reshape the room.
      asked = [];
      for (let i = 0; i < 20; i++) {
        answers.set("hall", engramOf(redoored(1 + (i % 3))));
        session.changed(frameOf());
        await run(100);
      }
      const last = redoored(1 + (19 % 3));
      answers.set("hall", engramOf(last));
      await run(8000);
      // One re-check per window at most while the frames come, and the
      // ones they left owed after them.
      const starts = asked.map((c) => c.at);
      expect(starts.length).toBeGreaterThan(1);
      expect(starts.length).toBeLessThanOrEqual(Math.ceil(2000 / COALESCE_MS));
      const dips = notices.filter((n) => n.text === RECONFIGURING);
      expect(dips.length).toBeGreaterThanOrEqual(1);
      for (let i = 1; i < dips.length; i++) {
        expect(dips[i]!.at - dips[i - 1]!.at).toBeGreaterThanOrEqual(
          RESHAPE_MIN_MS,
        );
      }
      expect(shown()).toEqual(built(last));
    });
  });

  it("follows a move, a delete and a re-add (Review Focus 3)", async () => {
    // Mutation caught: the move not followed, the URL not replaced,
    // darkening that also changes the URL.
    const session = await standIn();
    expect(navigate).toHaveBeenCalledTimes(1);
    asked = [];
    const moved = placeOf({ permalink: "annex/hall" });
    answers.set("annex/hall", engramOf(moved));
    session.changed(
      frameOf({
        kind: "moved",
        permalink: "annex/hall",
        path: "annex/hall.md",
        from: { path: "hall.md", permalink: "hall" },
      }),
    );
    await run(2000);
    expect(loadsOf("annex/hall")).toBe(1);
    expect(notices.some((n) => n.text === RECONFIGURING)).toBe(true);
    expect(session.current).toEqual(engramAt("station", "annex/hall"));
    expect(navigate).toHaveBeenCalledTimes(2);
    expect(navigate).toHaveBeenLastCalledWith(
      gameRouteOf(engramAt("station", "annex/hall")),
    );
    const url = window.location.pathname + window.location.search;

    // Deleted: the room goes dark in place, the URL stays.
    const lit = shown();
    answers.delete("annex/hall");
    session.changed(
      frameOf({
        kind: "deleted",
        permalink: "annex/hall",
        path: "annex/hall.md",
      }),
    );
    await run(2000);
    const dark = shown();
    expect(dark.lights.map((z) => z.level)).toEqual(
      lit.lights.map((z) => Math.round(z.level / 4)),
    );
    const levels = drawn.at(-1)?.levels;
    if (levels === undefined) throw new Error("nothing drawn");
    dark.lights.forEach((z, i) => {
      expect(levels[i]).toBeLessThanOrEqual(z.level);
    });
    expect(
      dark.fixtures.some(
        (f) => f.kind === "hatch" && f.label === LIFT_WORDS.bridge,
      ),
    ).toBe(true);
    expect(hud.notice).toHaveBeenLastCalledWith(NOT_FOUND);
    expect(window.location.pathname + window.location.search).toBe(url);
    expect(navigate).toHaveBeenCalledTimes(2);
    expect(session.current).toEqual(engramAt("station", "annex/hall"));

    // A second delete while dark changes nothing.
    const darkCount = rooms.length;
    session.changed(
      frameOf({
        kind: "deleted",
        permalink: "annex/hall",
        path: "annex/hall.md",
      }),
    );
    await run(1000);
    expect(rooms.length).toBe(darkCount);

    // Added again: the room comes back through a dip.
    notices = [];
    answers.set("annex/hall", engramOf(moved));
    session.changed(
      frameOf({
        kind: "added",
        permalink: "annex/hall",
        path: "annex/hall.md",
      }),
    );
    await run(RESHAPE_MIN_MS + 2000);
    expect(notices.some((n) => n.text === RECONFIGURING)).toBe(true);
    expect(shown()).toEqual(built(moved));
  });

  it("flickers on a text change and keeps everything", async () => {
    // Mutation caught: a text change reshaping, the doors reset.
    facing.kind = "door";
    const session = await standIn();
    key("keydown", "Space");
    frames(1);
    key("keyup", "Space");
    await run(300);
    // A turn away from the entrance's heading, so an entry that put the
    // player back at the spawn would show.
    key("keydown", "ArrowLeft");
    frames(3);
    key("keyup", "ArrowLeft");
    await run(1000);
    const doorsBefore = new Map(lastDoors());
    expect([...doorsBefore.values()].some((v) => v > 0)).toBe(true);
    const cam = lastCamera();
    expect(diffRooms(shown(), built(retexted()))).toBe("text");
    notices = [];
    const drawnBefore = drawn.length;
    const base = Math.max(
      ...drawn.slice(-20).map((d) => d.levels.reduce((a, b) => a + b, 0)),
    );
    answers.set("hall", engramOf(retexted()));
    session.changed(frameOf());
    await run(COALESCE_MS + FLICKER_MS + 200);
    expect(notices.some((n) => n.text === RECONFIGURING)).toBe(false);
    expect(shown()).toEqual(built(retexted()));
    expect(lastCamera().eye).toEqual(cam.eye);
    expect(lastCamera().yaw).toBe(cam.yaw);
    // Every frame from the change on draws the doors as they stood.
    for (const d of drawn.slice(drawnBefore))
      expect(d.doors).toEqual(doorsBefore);
    const low = drawn
      .slice(drawnBefore)
      .filter((d) => d.levels.reduce((a, b) => a + b, 0) < 0.8 * base);
    expect(low.length).toBeGreaterThan(0);
    const first = low[0]!.at;
    expect(low.every((d) => d.at - first < FLICKER_MS)).toBe(true);
    expect(session.current).toEqual(ROOM_AT);
  });

  it("takes a changed lead paragraph as a text change: flicker, cue and the reader's new text", async () => {
    // Mutation caught: `same` returning whatever the markdown (the lead
    // paragraph before the first heading builds no room text, so the room
    // serialises equal: no flicker, no cue, and the reader keeps the old
    // text until a later change re-enters the room).
    const releaded = placeOf({
      content: CANNED_BRIDGE.content.replace(
        "# Station Crystalline\n",
        "# Station Crystalline\n\nA lead paragraph, written since.\n",
      ),
    });
    expect(releaded.content).not.toBe(CANNED_BRIDGE.content);
    expect(diffRooms(built(placeOf()), built(releaded))).toBe("same");
    facing.kind = "terminal";
    const { cues, sink } = recordSound();
    const session = await standIn(placeOf(), { sound: sink });
    const drawnBefore = drawn.length;
    const base = Math.max(
      ...drawn.slice(-20).map((d) => d.levels.reduce((a, b) => a + b, 0)),
    );
    cues.length = 0;
    answers.set("hall", engramOf(releaded));
    session.changed(frameOf());
    await run(COALESCE_MS + FLICKER_MS + 200);
    expect(cues).toContainEqual({ kind: "terminal" });
    const low = drawn
      .slice(drawnBefore)
      .filter((d) => d.levels.reduce((a, b) => a + b, 0) < 0.8 * base);
    expect(low.length).toBeGreaterThan(0);
    key("keydown", "Space");
    frames(1);
    key("keyup", "Space");
    expect(hud.reader.mock.calls.at(-1)?.[0]?.content).toBe(releaded.content);

    // The same markdown again: nothing to do.
    cues.length = 0;
    session.closeReader();
    session.changed(frameOf());
    await run(COALESCE_MS + FLICKER_MS + 200);
    expect(cues).not.toContainEqual({ kind: "terminal" });
  });

  it("sends a terminal cue on a text change and the dark room's ambience when the room goes dark (M4 C17, C19, C22)", async () => {
    // Mutation caught: the text change silent, the dark room entered with
    // its lit ambience.
    const { cues, sink } = recordSound();
    const session = await standIn(placeOf(), { sound: sink });
    cues.length = 0;
    answers.set("hall", engramOf(retexted()));
    session.changed(frameOf());
    await run(COALESCE_MS + FLICKER_MS + 200);
    expect(cues).toContainEqual({ kind: "terminal" });

    cues.length = 0;
    answers.set("hall", { kind: "denied" });
    session.changed(frameOf());
    await run(2000);
    const ambiences = cues.flatMap((c) =>
      c.kind === "room" ? [c.ambience] : [],
    );
    expect(ambiences).toEqual(["dark"]);
  });

  it("keeps the latches on a text change: a player left in a doorway is not carried through", async () => {
    // Mutation caught: the keep path resetting the way latch (the player
    // standing in the doorway of a travel that failed is sent again).
    facing.kind = "door";
    const session = await standIn();
    const door = shown().fixtures.find((f) => f.kind === "door");
    if (door?.kind !== "door" || door.address === null)
      throw new Error("no door with an address");
    const to = door.address.permalink;
    holds.set(to, Promise.resolve({ kind: "offline" }));
    key("keydown", "Space");
    frames(1);
    key("keyup", "Space");
    await run(500);
    key("keydown", "KeyW");
    for (let i = 0; i < 120 && loadsOf(to) === 0; i++) await run(TICK_MS);
    key("keyup", "KeyW");
    expect(loadsOf(to)).toBe(1);
    await run(500);
    expect(hud.notice).toHaveBeenCalledWith("SIGNAL LOST");
    expect(session.current).toEqual(ROOM_AT);
    answers.set("hall", engramOf(retexted()));
    session.changed(frameOf());
    await run(1500);
    expect(shown()).toEqual(built(retexted()));
    expect(loadsOf(to)).toBe(1);
  });

  describe("reshapes behind the dip and keeps the player on free floor", () => {
    // Mutation caught: the room swapped at 0 ms, the entrance spawn used.
    const next = () => built(redoored());

    /** A spot on free floor in both `a` and `b`, away from `a`'s spawn. */
    function freeInBoth(a: RoomSpec, b: RoomSpec): { x: number; z: number } {
      const clear = (r: RoomSpec, x: number, z: number) =>
        settleSpot(r, x, z).x === x && settleSpot(r, x, z).z === z;
      for (let cy = 0; cy < a.grid.length; cy++) {
        for (let cx = 0; cx < (a.grid[cy]?.length ?? 0); cx++) {
          const x = (cx + 0.3) * CELL;
          const z = (cy + 0.7) * CELL;
          if (cx === a.spawn.x && cy === a.spawn.y) continue;
          if (clear(a, x, z) && clear(b, x, z)) return { x, z };
        }
      }
      throw new Error("no free spot");
    }

    /** A spot on free floor in `a` that a blocker of `b` covers. */
    function blockedInB(a: RoomSpec, b: RoomSpec): { x: number; z: number } {
      for (const box of blockersFor(b)) {
        const x = (box.x0 + box.x1) / 2;
        const z = (box.z0 + box.z1) / 2;
        const s = settleSpot(a, x, z);
        if (s.x === x && s.z === z) return { x, z };
      }
      throw new Error("no spot blocked in the new room only");
    }

    function standAt(spot: { x: number; z: number }): Session {
      answers.set("hall", engramOf(placeOf()));
      const session = start({ load: liveLoad });
      const a = built(placeOf());
      session.showRoom(
        {
          ...a,
          spawn: { x: spot.x / CELL - 0.5, y: spot.z / CELL - 0.5, yaw: 0.5 },
        },
        { pitch: 0.3 },
        ROOM_AT,
      );
      frames(2);
      return session;
    }

    it("keeps the player's x and z on free floor, and swaps at 500 ms", async () => {
      const a = built(placeOf());
      const b = next();
      expect(diffRooms(a, b)).toBe("shape");
      const spot = freeInBoth(a, b);
      const session = standAt(spot);
      const pitch = lastCamera().pitch;
      expect(pitch).toBeCloseTo(0.3, 6);
      answers.set("hall", engramOf(redoored()));
      rooms = [];
      session.changed(frameOf());
      await run(2000);
      const dip = notices.find((n) => n.text === RECONFIGURING);
      if (dip === undefined) throw new Error("no dip notice");
      expect(rooms).toHaveLength(1);
      expect(rooms[0]!.room).toEqual(b);
      expect(rooms[0]!.at - dip.at).toBeGreaterThanOrEqual(DIP_SWAP_MS);
      expect(rooms[0]!.at - dip.at).toBeLessThan(DIP_SWAP_MS + 2 * TICK_MS);
      const [x, z] = eyeAt();
      expect(x).toBeCloseTo(spot.x, 6);
      expect(z).toBeCloseTo(spot.z, 6);
      expect(lastCamera().yaw).toBeCloseTo(0.5, 6);
      expect(lastCamera().pitch).toBe(pitch);
      // The lights fell to a tenth in the dip's dark stretch.
      const inDip = drawn.filter(
        (d) => d.at > dip.at + 420 && d.at < dip.at + 580,
      );
      expect(inDip.length).toBeGreaterThan(0);
      const lit = drawn.at(-1)!.levels.reduce((p, q) => p + q, 0);
      for (const d of inDip)
        expect(d.levels.reduce((p, q) => p + q, 0)).toBeLessThan(0.2 * lit);
      expect(session.current).toEqual(ROOM_AT);
    });

    it("moves the player to the nearest free cell when their spot is blocked", async () => {
      const a = built(placeOf());
      const b = next();
      const spot = blockedInB(a, b);
      const session = standAt(spot);
      answers.set("hall", engramOf(redoored()));
      session.changed(frameOf());
      await run(2000);
      expect(shown()).toEqual(b);
      const want = settleSpot(b, spot.x, spot.z);
      expect(want).not.toEqual(spot);
      // The first frame drawn in the new room stands at the spot; the
      // walk's own collision may nudge it afterwards.
      const swappedAt = rooms.at(-1)?.at ?? Infinity;
      const first = drawn.find((d) => d.at >= swappedAt);
      if (first === undefined) throw new Error("nothing drawn after the swap");
      expect(first.camera.eye[0]).toBeCloseTo(want.x, 6);
      expect(first.camera.eye[2]).toBeCloseTo(want.z, 6);
      expect(session.current).toEqual(ROOM_AT);
    });
  });

  it("closes a lift overlay opened during the dip when the room is swapped", async () => {
    // Mutation caught: the swap leaving the old room's stops open over the
    // new room, where a pick rides nowhere.
    const deckRoom = generateDeck(CANNED_DECK, 0);
    const lift = deckRoom.fixtures.find((f) => f.kind === "lift");
    if (lift?.kind !== "lift") throw new Error("no lift");
    const onLift = vi.fn();
    const session = start({ load: liveLoad, onLift });
    session.showRoom(
      { ...deckRoom, spawn: wallFacingSpawn(lift.slot) },
      undefined,
      {
        kind: "deck",
        domain: CANNED_DECK.domain,
        folder: CANNED_DECK.folder,
        section: 0,
      },
    );
    frames(1);
    session.changed({
      event: "domain",
      change: { domain: CANNED_DECK.domain, actor: null },
    });
    await run(COALESCE_MS + 2 * TICK_MS);
    const dip = notices.find((n) => n.text === RECONFIGURING);
    if (dip === undefined) throw new Error("no dip");
    key("keydown", "Space");
    frames(1);
    key("keyup", "Space");
    expect(onLift.mock.calls.at(-1)?.[0]).not.toBeNull();
    expect(now - dip.at).toBeLessThan(DIP_SWAP_MS);
    await run(DIP_SWAP_MS);
    expect(rooms.at(-1)?.at).toBeGreaterThanOrEqual(dip.at + DIP_SWAP_MS);
    expect(onLift).toHaveBeenLastCalledWith(null);
  });

  it("ignores frames that do not concern the room, and every frame inside the console room", async () => {
    // Mutation caught: `concerns` bypassed.
    const session = await standIn();
    asked = [];
    session.changed(frameOf({ domain: "far", permalink: "x", path: "x.md" }));
    await run(1000);
    expect(asked).toHaveLength(0);
    // The control: a frame of the room's own domain is read.
    session.changed(frameOf());
    await run(1000);
    expect(asked).toHaveLength(1);

    const inside = startWithConsole([row(HALL), row("ops")], liveLoad);
    standAtBox(inside);
    walkIn();
    frames(1);
    expect(isConsole(lastRoom())).toBe(true);
    asked = [];
    inside.changed(frameOf({ domain: HALL, permalink: "x", path: "x.md" }));
    inside.changed({ event: "domain", change: { domain: HALL, actor: null } });
    inside.changed({ event: "reset" });
    await run(1000);
    expect(asked).toHaveLength(0);
  });

  it("re-reads the airlock as it was entered, so a frame changes nothing there", async () => {
    // Mutation caught: the re-check built with no arrival (the stop the
    // player came from loses its mark: a shape change on every frame).
    // The precondition: the stop the player came from is marked, so a
    // build with no arrival differs from the room entered.
    const airlock: Extract<LoadedStation, { kind: "airlock" }> = {
      kind: "airlock",
      input: { domains: CANNED_DOMAINS, here: null },
    };
    const from: StationAddress = { kind: "bridge", domain: "orbit" };
    expect(
      diffRooms(
        roomFor(airlock, { from }, null).room,
        roomFor(airlock, null, null).room,
      ),
    ).not.toBe("same");
    const session = start({ load: liveLoad });
    session.go(from);
    await micro();
    frames(1);
    session.go({ kind: "airlock" }, { via: "lift", from: session.current! });
    await micro();
    frames(1);
    expect(session.current).toEqual({ kind: "airlock" });
    const count = rooms.length;
    notices = [];
    session.changed({
      event: "domain",
      change: { domain: "ops", actor: null },
    });
    await run(2000);
    expect(rooms.length).toBe(count);
    expect(notices.some((n) => n.text === RECONFIGURING)).toBe(false);
  });

  describe("fix round 1", () => {
    /** A promise that rejects once asked, as a load the server fails. */
    function failing(): { promise: Promise<LoadedStation>; fail(): void } {
      let reject!: (e: unknown) => void;
      const promise = new Promise<LoadedStation>((_, r) => {
        reject = r;
      });
      return {
        promise,
        fail: () => {
          reject(new Error("boom"));
        },
      };
    }

    it("never puts the player inside a prop when the room goes dark (I1)", async () => {
      // Mutation caught: the dark room entered with the player kept where
      // they stood, inside a prop its re-dressing moved.
      const cases: {
        place: PlaceInput;
        room: RoomSpec;
        x: number;
        z: number;
      }[] = [];
      for (const base of [CANNED_WORKSHOP, CANNED_HUB]) {
        for (let i = 0; i < 60 && cases.length < 4; i++) {
          const place = {
            ...base,
            domain: "station",
            permalink: `p${String(i)}`,
          };
          const room = built(place);
          const dark = darkened(room, "station");
          // A point a prop of the dark room covers is the only candidate:
          // cheap to find, then checked free before the dark with
          // `settleSpot`.
          const props = blockersFor(dark);
          const covered = (x: number, z: number) =>
            props.some((p) => x >= p.x0 && x <= p.x1 && z >= p.z0 && z <= p.z1);
          let found: { x: number; z: number } | null = null;
          for (
            let z = 0.125;
            z < room.grid.length * CELL && !found;
            z += 0.25
          ) {
            for (
              let x = 0.125;
              x < (room.grid[0]?.length ?? 0) * CELL;
              x += 0.25
            ) {
              if (!covered(x, z)) continue;
              const kept = settleSpot(room, x, z);
              if (kept.x === x && kept.z === z) {
                found = { x, z };
                break;
              }
            }
          }
          if (found) cases.push({ place, room, ...found });
        }
      }
      expect(cases.length).toBeGreaterThanOrEqual(2);
      for (const c of cases) {
        const session = start({ load: liveLoad });
        session.showRoom(
          {
            ...c.room,
            spawn: { x: c.x / CELL - 0.5, y: c.z / CELL - 0.5, yaw: 0.7 },
          },
          { pitch: -0.2 },
          engramAt("station", c.place.permalink),
        );
        frames(2);
        const count = rooms.length;
        session.changed(
          frameOf({ permalink: c.place.permalink, kind: "deleted" }),
        );
        await run(1000);
        expect(rooms.length).toBe(count + 1);
        const dark = shown();
        const want = settleSpot(dark, c.x, c.z);
        // The room went dark between two ticks: the first frame drawn
        // after it stands at the spot, or within the walk's own push off a
        // prop's edge of it, and inside no prop.
        const swappedAt = rooms.at(-1)!.at;
        const first = drawn.find((d) => d.at > swappedAt);
        if (first === undefined) throw new Error("nothing drawn");
        const [ex, , ez] = first.camera.eye;
        expect(Math.hypot(ex - want.x, ez - want.z)).toBeLessThanOrEqual(
          PLAYER_RADIUS + 1e-6,
        );
        for (const b of blockersFor(dark)) {
          const inside = ex > b.x0 && ex < b.x1 && ez > b.z0 && ez < b.z1;
          expect(inside).toBe(false);
        }
        expect(first.camera.yaw).toBeCloseTo(0.7, 6);
        expect(first.camera.pitch).toBeCloseTo(-0.2, 6);
        session.dispose();
      }
    });

    it("drops a live change a new place overtakes: its answer, its dip and its move (I2, M8)", async () => {
      // Mutation caught: `settled` without its generation and abort guard,
      // `leave` keeping the dip, `leave` keeping the move's target, the
      // dip's notice left up over the journey.
      const beta = placeOf({ permalink: "beta" });
      answers.set("beta", engramOf(beta));
      const session = await standIn();

      // (a) A move whose load a door's journey overtakes.
      const late = deferred<LoadedStation>();
      holds.set("annex/hall", late.promise);
      session.changed(
        frameOf({
          kind: "moved",
          permalink: "annex/hall",
          path: "annex/hall.md",
          from: { path: "hall.md", permalink: "hall" },
        }),
      );
      await run(COALESCE_MS + 2 * TICK_MS);
      expect(loadsOf("annex/hall")).toBe(1);
      session.go(engramAt("station", "beta"));
      await run(100);
      expect(session.current).toEqual(engramAt("station", "beta"));
      const navigations = navigate.mock.calls.length;
      late.resolve(engramOf(placeOf({ permalink: "annex/hall" })));
      await run(2000);
      expect(shown()).toEqual(built(beta));
      expect(navigate.mock.calls.length).toBe(navigations);
      // A frame for the room now shown reads that room, not the move.
      asked = [];
      session.changed(frameOf({ permalink: "beta", path: "beta.md" }));
      await run(1000);
      expect(loadsOf("beta")).toBe(1);
      expect(loadsOf("annex/hall")).toBe(0);
      expect(session.current).toEqual(engramAt("station", "beta"));

      // (b) A dip a journey overtakes before its swap.
      answers.set("beta", engramOf({ ...redoored(), permalink: "beta" }));
      session.changed(frameOf({ permalink: "beta", path: "beta.md" }));
      await run(COALESCE_MS + 2 * TICK_MS);
      const dip = notices.find((n) => n.text === RECONFIGURING);
      if (dip === undefined) throw new Error("no dip");
      const other = deferred<LoadedStation>();
      holds.set("gamma", other.promise);
      session.go(engramAt("station", "gamma"));
      // The dip's notice goes with it.
      expect(hud.notice).toHaveBeenLastCalledWith(null);
      const gamma = placeOf({ permalink: "gamma" });
      other.resolve(engramOf(gamma));
      await run(DIP_SWAP_MS + 500);
      expect(shown()).toEqual(built(gamma));
      expect(session.current).toEqual(engramAt("station", "gamma"));
    });

    it("runs a re-check held by a busy screen, the level select or a failed journey once each ends (I3)", async () => {
      // Mutation caught: `retryCheck` removed from `setBusy(false)`,
      // `closeLevels` or the travel's reject branch.
      const onLevels = vi.fn<(open: boolean) => void>();
      const session = await standIn(placeOf(), { onLevels });

      session.setBusy(true);
      asked = [];
      session.changed(frameOf());
      await run(1000);
      expect(loadsOf("hall")).toBe(0);
      session.setBusy(false);
      await run(200);
      expect(loadsOf("hall")).toBe(1);

      type("idclev");
      frames(1);
      expect(onLevels).toHaveBeenLastCalledWith(true);
      asked = [];
      session.changed(frameOf());
      await run(1000);
      expect(loadsOf("hall")).toBe(0);
      session.closeLevels();
      await run(200);
      expect(loadsOf("hall")).toBe(1);

      const broken = failing();
      holds.set("beta", broken.promise);
      session.go(engramAt("station", "beta"));
      asked = [];
      session.changed(frameOf());
      await run(1000);
      expect(loadsOf("hall")).toBe(0);
      broken.fail();
      await run(200);
      expect(hud.notice).toHaveBeenCalledWith("?LOAD ERROR");
      expect(loadsOf("hall")).toBe(1);
    });

    it("flashes SIGNAL LOST, darkens with ACCESS DENIED and never darkens the airlock (M4)", async () => {
      // Mutation caught: the offline flash removed, the denied notice
      // taken for the missing one, the airlock guard removed.
      const session = await standIn();
      const count = rooms.length;
      answers.set("hall", { kind: "offline" });
      session.changed(frameOf());
      await run(1000);
      expect(hud.notice).toHaveBeenCalledWith("SIGNAL LOST");
      expect(rooms.length).toBe(count);

      answers.set("hall", { kind: "denied" });
      session.changed(frameOf());
      await run(4000);
      expect(rooms.length).toBe(count + 1);
      expect(hud.notice).toHaveBeenLastCalledWith(ACCESS_DENIED);

      let asks = 0;
      const load: PlaceLoader = (a, signal) => {
        if (a.kind !== "airlock") return liveLoad(a, signal);
        asks++;
        return asks > 1
          ? Promise.resolve({ kind: "missing" })
          : liveLoad(a, signal);
      };
      const airlock = start({ load });
      airlock.go({ kind: "airlock" });
      await micro();
      frames(1);
      expect(airlock.current).toEqual({ kind: "airlock" });
      const before = rooms.length;
      notices = [];
      airlock.changed({
        event: "domain",
        change: { domain: "ops", actor: null },
      });
      await run(1000);
      expect(asks).toBe(2);
      expect(rooms.length).toBe(before);
      expect(notices.some((n) => n.text === NOT_FOUND)).toBe(false);
    });

    it("runs one re-check at a time and the one owed after it (M5)", async () => {
      // Mutation caught: `checking` dropped from the hold (two loads of
      // the same room at once).
      const session = await standIn();
      const slow = deferred<LoadedStation>();
      holds.set("hall", slow.promise);
      asked = [];
      session.changed(frameOf());
      await run(COALESCE_MS + 2 * TICK_MS);
      expect(loadsOf("hall")).toBe(1);
      session.changed(frameOf());
      await run(1000);
      expect(loadsOf("hall")).toBe(1);
      slow.resolve(engramOf(placeOf()));
      await run(200);
      expect(loadsOf("hall")).toBe(2);
    });

    it("opens the Fluid page of a move not yet followed on F, and ignores moves inside the console room (M6, M7)", async () => {
      // Mutation caught: F reading `current` alone (the old permalink's
      // page, a 404 after the move), a move taken inside the console room.
      const session = await standIn();
      const late = deferred<LoadedStation>();
      holds.set("annex/hall", late.promise);
      session.changed(
        frameOf({
          kind: "moved",
          permalink: "annex/hall",
          path: "annex/hall.md",
          from: { path: "hall.md", permalink: "hall" },
        }),
      );
      await run(COALESCE_MS + 2 * TICK_MS);
      expect(session.current).toEqual(ROOM_AT);
      expect(session.page).toEqual(engramAt("station", "annex/hall"));
      key("keydown", "KeyF");
      frames(1);
      key("keyup", "KeyF");
      expect(openFluid).toHaveBeenLastCalledWith(
        fluidRouteOfStation(engramAt("station", "annex/hall")),
      );
      session.dispose();

      const inside = startWithConsole([row(HALL), row("ops")], liveLoad);
      standAtBox(inside);
      walkIn();
      frames(1);
      expect(isConsole(lastRoom())).toBe(true);
      inside.changed(
        frameOf({
          domain: HALL,
          kind: "moved",
          permalink: "elsewhere",
          path: "elsewhere.md",
          from: { path: "x.md", permalink: heroHallRoom().permalink },
        }),
      );
      expect(inside.page).toEqual(HALL_ADDRESS);
      openFluid.mockClear();
      key("keydown", "KeyF");
      frames(1);
      key("keyup", "KeyF");
      expect(openFluid).toHaveBeenLastCalledWith(
        fluidRouteOfStation(HALL_ADDRESS),
      );
    });
  });
});

describe("sound cues (M4 Task 7)", () => {
  const gallery = galleryRoom();
  const GALLERY_AT = engramAt(gallery.domain, gallery.permalink);

  /** The gallery's bulkhead door that leads somewhere. */
  const bulkhead = gallery.fixtures.findIndex(
    (f) => f.kind === "door" && f.style === "bulkhead" && f.address !== null,
  );

  /**
   * The gallery with the player three cells back from the bulkhead's
   * cell, facing it, so the walk up to it takes a few strides.
   */
  const backFromBulkhead = (): RoomSpec => {
    const f = gallery.fixtures[bulkhead];
    if (f === undefined) throw new Error("no bulkhead");
    const spawn = wallFacingSpawn(f.slot);
    const w = wallPoint(f.slot);
    const back = {
      ...spawn,
      x: spawn.x + w.inward[0] * 3,
      y: spawn.y + w.inward[1] * 3,
    };
    expect(gallery.grid[back.y]?.[back.x]).toBe(".");
    return { ...gallery, spawn: back };
  };

  const kinds = (cues: readonly Cue[]) => cues.map((c) => c.kind);

  it("sends the cues of a walk through a door", async () => {
    // Mutation caught: a cue missing at any of these points, the entry
    // cue after the first step.
    expect(bulkhead).toBeGreaterThanOrEqual(0);
    const { cues, sink } = recordSound();
    const session = start({ client: null, load: stationLoad, sound: sink });
    session.showRoom(backFromBulkhead(), undefined, GALLERY_AT);
    expect(cues[0]).toEqual({
      kind: "room",
      ambience: gallery.condition,
      seed: gallery.seed,
    });

    // Up to the shut bulkhead: footsteps, feet alternating.
    key("keydown", "KeyW");
    frames(70);
    key("keyup", "KeyW");
    frames(10);
    const steps = cues.filter(
      (c): c is Extract<Cue, { kind: "step" }> => c.kind === "step",
    );
    expect(steps.length).toBeGreaterThanOrEqual(2);
    steps.forEach((s, i) => {
      expect(s.foot).toBe(i % 2);
      expect(s.run).toBe(false);
      if (i > 0) expect(s.n).toBe(steps[i - 1]!.n + 1);
    });
    expect(kinds(cues).indexOf("room")).toBeLessThan(
      kinds(cues).indexOf("step"),
    );
    expect(hud.prompt).toHaveBeenLastCalledWith(
      expect.stringMatching(/^SPACE /),
    );

    // Space opens it: one door cue, open, placed ahead.
    key("keydown", "Space");
    frames(1);
    key("keyup", "Space");
    frames(20);
    const doors = cues.filter((c) => c.kind === "door");
    expect(doors).toHaveLength(1);
    expect(doors[0]).toMatchObject({
      kind: "door",
      sound: "bulkhead",
      open: true,
    });
    const placed = doors[0] as Extract<Cue, { kind: "door" }>;
    expect(Math.abs(placed.pan)).toBeLessThan(0.5);
    expect(placed.gain).toBeGreaterThan(0.3);
    expect(placed.gain).toBeLessThanOrEqual(1);

    // Through it: the travel, then the room it lands in.
    const ups = () =>
      hud.connector.mock.calls.filter(([active]) => active).length;
    key("keydown", "KeyW");
    for (let t = 0; t < 80 && ups() === 0; t++) frames(1);
    key("keyup", "KeyW");
    expect(cues.at(-1)).toEqual({ kind: "travel", via: "door" });
    const travelAt = cues.length - 1;
    await flush();
    frames(1);
    const landed = cues.slice(travelAt).find((c) => c.kind === "room");
    expect(landed).toBeDefined();
    const order = kinds(cues);
    expect(order.indexOf("door")).toBeLessThan(order.indexOf("travel"));
    expect(order.lastIndexOf("room")).toBeGreaterThan(order.indexOf("travel"));
  });

  it("sends no step while anything is modal or a place loads", () => {
    // Mutation caught: steps sent while the player may not move (the
    // stride still drifts on after the keys are taken).
    const takes: { name: string; take: (s: Session) => void }[] = [
      {
        name: "busy",
        take: (s) => {
          s.setBusy(true);
        },
      },
      {
        name: "loading",
        take: (s) => {
          s.go(engramAt("eng", "beta"));
        },
      },
    ];
    for (const { name, take } of takes) {
      for (let walk = 6; walk <= 40; walk += 2) {
        const { cues, sink } = recordSound();
        const session = start({
          client: null,
          load: () => new Promise<LoadedStation>(() => undefined),
          sound: sink,
        });
        session.showRoom(backFromBulkhead(), undefined, GALLERY_AT);
        key("keydown", "ShiftLeft");
        key("keydown", "KeyW");
        frames(walk);
        const mark = cues.length;
        take(session);
        frames(20);
        key("keyup", "KeyW");
        key("keyup", "ShiftLeft");
        expect(
          cues.slice(mark).some((c) => c.kind === "step"),
          `${name} after ${String(walk)}`,
        ).toBe(false);
        session.dispose();
      }
    }
  });

  it("marks a running step, and a walking one not", () => {
    // Mutation caught: the run flag not carried.
    for (const running of [false, true]) {
      const { cues, sink } = recordSound();
      const session = start({ client: null, sound: sink });
      session.showRoom(backFromBulkhead(), undefined, GALLERY_AT);
      if (running) key("keydown", "ShiftLeft");
      key("keydown", "KeyW");
      frames(30);
      key("keyup", "KeyW");
      key("keyup", "ShiftLeft");
      const steps = cues.filter(
        (c): c is Extract<Cue, { kind: "step" }> => c.kind === "step",
      );
      expect(steps.length).toBeGreaterThanOrEqual(2);
      expect(steps.every((c) => c.run === running)).toBe(true);
      session.dispose();
    }
  });

  it("sends the police box's doors as they turn", () => {
    // Mutation caught: the boxes' doors left out of the cues.
    const { cues, sink } = recordSound();
    const session = start({ client: null, sound: sink });
    standAtBox(session);
    cues.length = 0;
    key("keydown", "Space");
    frames(1);
    key("keyup", "Space");
    frames(20);
    const doors = cues.filter((c) => c.kind === "door");
    expect(doors).toHaveLength(1);
    expect(doors[0]).toMatchObject({ sound: "box", open: true });
  });

  it("sends nothing once disposed", () => {
    // Mutation caught: the dispose guard on the cues removed.
    const { cues, sink } = recordSound();
    const session = start({ client: null, sound: sink });
    session.showCanned(CANNED_BRIDGE);
    frames(1);
    session.dispose();
    cues.length = 0;
    session.jump("eng");
    expect(cues).toEqual([]);
  });

  it("keeps the mute notice up for LOOK_NOTICE_MS", () => {
    // Mutation caught: the notice held for the failure notice's length.
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });
    try {
      const { sink } = recordSound([true]);
      const session = start({ client: null, sound: sink });
      session.showCanned(CANNED_BRIDGE);
      frames(1);
      key("keydown", "KeyM");
      frames(1);
      key("keyup", "KeyM");
      expect(hud.notice).toHaveBeenLastCalledWith("SOUND OFF");
      vi.advanceTimersByTime(LOOK_NOTICE_MS - 1);
      expect(hud.notice).toHaveBeenLastCalledWith("SOUND OFF");
      vi.advanceTimersByTime(1);
      expect(hud.notice).toHaveBeenLastCalledWith(null);
    } finally {
      vi.useRealTimers();
    }
  });

  it("mutes on M and flashes the state", () => {
    // Mutation caught: M read while modal; the reader's terminal cue
    // missing.
    const { cues, sink } = recordSound([true, false]);
    const session = start({ client: null, sound: sink });
    session.showCanned(CANNED_BRIDGE);
    frames(1);
    const pressM = () => {
      key("keydown", "KeyM");
      frames(1);
      key("keyup", "KeyM");
    };
    pressM();
    expect(sink.toggleMute).toHaveBeenCalledTimes(1);
    expect(hud.notice).toHaveBeenLastCalledWith("SOUND OFF");
    pressM();
    expect(sink.toggleMute).toHaveBeenCalledTimes(2);
    expect(hud.notice).toHaveBeenLastCalledWith("SOUND ON");

    // With the reader open, M is the reader's, and not replayed after.
    walkToScope();
    key("keydown", "Space");
    frames(1);
    key("keyup", "Space");
    expect(hud.reader).toHaveBeenLastCalledWith(
      expect.objectContaining({ title: "Station Crystalline" }),
    );
    expect(cues.at(-1)).toEqual({ kind: "terminal" });
    const notices = hud.notice.mock.calls.length;
    pressM();
    frames(2);
    session.closeReader();
    frames(3);
    expect(sink.toggleMute).toHaveBeenCalledTimes(2);
    expect(hud.notice.mock.calls.length).toBe(notices);
    session.dispose();

    // Without a sink, M is nothing at all.
    hud.notice.mockClear();
    const silent = start({ client: null });
    silent.showCanned(CANNED_BRIDGE);
    frames(1);
    pressM();
    frames(2);
    expect(
      hud.notice.mock.calls.some(([t]) => t !== null && t.startsWith("SOUND")),
    ).toBe(false);
  });

  it("sends a terminal cue when the level select opens", () => {
    // Mutation caught: the level select opening silently.
    const { cues, sink } = recordSound();
    const session = start({ client: null, onLevels: vi.fn(), sound: sink });
    session.showCanned(CANNED_BRIDGE);
    frames(1);
    cues.length = 0;
    type("idclev");
    frames(1);
    expect(cues).toEqual([{ kind: "terminal" }]);
  });

  /**
   * The canned bridge with its Scope terminal given the first seed that
   * answers (M4 C25), the player spawned facing it, and its address.
   */
  const answeringBridge = () => {
    const built = generateRoom(CANNED_BRIDGE);
    const index = built.fixtures.findIndex(
      (f) => f.kind === "terminal" && f.heading === "Scope",
    );
    const scope = built.fixtures[index];
    if (scope?.kind !== "terminal") throw new Error("no Scope terminal");
    let seed = -1;
    for (let s = 0; s < 10000 && seed < 0; s++) {
      if (answers({ ...scope, seed: s })) seed = s;
    }
    expect(seed).toBeGreaterThanOrEqual(0);
    const room: RoomSpec = {
      ...built,
      fixtures: built.fixtures.map((f, i) =>
        i === index ? { ...f, seed } : f,
      ),
      spawn: wallFacingSpawn(scope.slot),
    };
    const at = engramAt(CANNED_BRIDGE.domain, CANNED_BRIDGE.permalink);
    return { room, at };
  };

  /** Cranks until `ms` have passed since `from`. */
  const until = (from: number, ms: number) => {
    while (now - from < ms) frames(1);
  };

  const facingScope = () =>
    hud.prompt.mock.calls.at(-1)?.[0] === "SPACE READ Scope";

  it("answers once per visit after the wait (M4 C25)", () => {
    // Mutation caught: the timer not reset when the focus leaves, a cue
    // per tick, the visit's answer not cleared on entry.
    const { room, at } = answeringBridge();
    const { cues, sink } = recordSound();
    const session = start({ client: null, sound: sink });
    const answered = () => cues.filter((c) => c.kind === "answer");
    const facing = facingScope;

    session.showRoom(room, { pitch: 0 }, at);
    frames(1);
    expect(facing()).toBe(true);
    let t0 = now;

    // Faced for a while, then looked away and back: the wait starts over.
    until(t0, 1000);
    key("keydown", "ArrowLeft");
    let turned = 0;
    for (; turned < 60 && facing(); turned++) frames(1);
    key("keyup", "ArrowLeft");
    expect(facing()).toBe(false);
    expect(answered()).toEqual([]);
    // Back the same way, from the first tick it is in focus again.
    key("keydown", "ArrowRight");
    t0 = -1;
    for (let i = 0; i < turned; i++) {
      frames(1);
      if (t0 < 0 && facing()) t0 = now;
    }
    key("keyup", "ArrowRight");
    expect(facing()).toBe(true);
    until(t0, ANSWER_WAIT_MS - 100);
    expect(answered()).toEqual([]);
    until(t0, ANSWER_WAIT_MS);
    expect(answered()).toHaveLength(1);
    const cue = answered()[0] as Extract<Cue, { kind: "answer" }>;
    expect(Math.abs(cue.pan)).toBeLessThan(0.3);
    expect(cue.gain).toBeGreaterThan(0.5);

    // Standing on: no second.
    until(t0, ANSWER_WAIT_MS * 3);
    expect(answered()).toHaveLength(1);

    // Entered again: one more, after the wait.
    session.showRoom(room, { pitch: 0 }, at);
    frames(1);
    t0 = now;
    until(t0, ANSWER_WAIT_MS - 100);
    expect(answered()).toHaveLength(1);
    until(t0, ANSWER_WAIT_MS);
    expect(answered()).toHaveLength(2);
    until(t0, ANSWER_WAIT_MS * 3);
    expect(answered()).toHaveLength(2);
  });

  it("starts the answering console's wait over after a pause (M4 C25)", () => {
    // Mutation caught: the time before the pause counted towards the wait
    // (the console answering on the first tick after the resume).
    const { room, at } = answeringBridge();
    const { cues, sink } = recordSound();
    const session = start({ client: null, sound: sink });
    const answered = () => cues.filter((c) => c.kind === "answer");
    session.showRoom(room, { pitch: 0 }, at);
    frames(1);
    expect(facingScope()).toBe(true);
    until(now, 1000);
    session.pause();
    until(now, 3000);
    session.resume();
    frames(1);
    expect(facingScope()).toBe(true);
    const t0 = now;
    expect(answered()).toEqual([]);
    until(t0, ANSWER_WAIT_MS - 100);
    expect(answered()).toEqual([]);
    until(t0, ANSWER_WAIT_MS);
    expect(answered()).toHaveLength(1);
  });

  it("never answers while a place loads (M4 C25)", () => {
    // Mutation caught: the console timed while the room it stands in is
    // being left.
    const { room, at } = answeringBridge();
    const { cues, sink } = recordSound();
    const session = start({
      client: null,
      load: () => new Promise<LoadedStation>(() => undefined),
      sound: sink,
    });
    session.showRoom(room, { pitch: 0 }, at);
    frames(1);
    expect(facingScope()).toBe(true);
    until(now, 500);
    session.go(engramAt("eng", "beta"));
    until(now, ANSWER_WAIT_MS * 3);
    expect(cues.filter((c) => c.kind === "answer")).toEqual([]);
  });

  it("starts the answering console's wait over after a hidden tab (M4 C25)", () => {
    // Mutation caught: the time before the tab was hidden counted towards
    // the wait (the console answering on the first tick back).
    const { room, at } = answeringBridge();
    const { cues, sink } = recordSound();
    const session = start({ client: null, sound: sink });
    const answered = () => cues.filter((c) => c.kind === "answer");
    const visibility = (state: DocumentVisibilityState) => {
      Object.defineProperty(document, "visibilityState", {
        configurable: true,
        get: () => state,
      });
      document.dispatchEvent(new Event("visibilitychange"));
    };
    try {
      session.showRoom(room, { pitch: 0 }, at);
      frames(1);
      expect(facingScope()).toBe(true);
      until(now, 1000);
      // Hidden: no frames run while the clock goes on.
      visibility("hidden");
      now += 3000;
      visibility("visible");
      frames(1);
      expect(facingScope()).toBe(true);
      const t0 = now;
      expect(answered()).toEqual([]);
      until(t0, ANSWER_WAIT_MS - 100);
      expect(answered()).toEqual([]);
      until(t0, ANSWER_WAIT_MS);
      expect(answered()).toHaveLength(1);
    } finally {
      Reflect.deleteProperty(document, "visibilityState");
    }
  });

  it("sends the police box's take-off and landing, and the jump", async () => {
    // Mutation caught: the landing cue on any bridge arrival.
    const { cues, sink } = recordSound();
    const session = start({
      load: okBridge,
      consoleRoom: { domains: () => Promise.resolve([row(HALL), row("ops")]) },
      sound: sink,
    });
    standAtBox(session);
    walkIn();
    expect(isConsole(lastRoom())).toBe(true);
    expect(cues).toContainEqual({ kind: "box", phase: "takeoff" });
    // The cut in is an entry: the console room's hum.
    expect(cues).toContainEqual({
      kind: "room",
      ambience: "console",
      seed: consoleRoom().seed,
    });
    expect(cues).not.toContainEqual({ kind: "box", phase: "landing" });
    await flush();
    backOut();
    await vi.waitFor(() => {
      expect(navigate).toHaveBeenCalledWith("/%CF%80/d/ops");
    });
    expect(
      cues.filter((c) => c.kind === "box" && c.phase === "landing"),
    ).toHaveLength(1);

    cues.length = 0;
    session.jump("eng");
    await vi.waitFor(() => {
      expect(session.current).toEqual({ kind: "bridge", domain: "eng" });
    });
    expect(cues[0]).toEqual({ kind: "jump" });
    expect(kinds(cues)).toContain("room");
    expect(kinds(cues)).not.toContain("box");
    expect(kinds(cues)).not.toContain("answer");
  });
});
