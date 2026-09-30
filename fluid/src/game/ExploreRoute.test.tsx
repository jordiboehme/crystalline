/**
 * The game route, mounted in the whole app on an in-memory history.
 *
 * jsdom has no WebGL2, so the route first has to refuse the device the way
 * the look demo does; that pins that the route exists and
 * turns a device away before it draws a canvas. Then the probe is told
 * WebGL2 is there, and the context and the renderer are stubbed at their
 * modules, so the session's default factory runs without a GPU: the route
 * loads the station address in its URL (the airlock, a bridge, a deck or
 * an engram), loads the next one when the URL changes under it (history
 * included, also while a load is still in flight), and stops asking the
 * server anything once it is unmounted.
 */

import {
  act,
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import { StrictMode, useEffect } from "react";
import {
  MemoryRouter,
  useLocation,
  useNavigate,
  useNavigationType,
  type NavigateFunction,
} from "react-router";
import {
  afterEach,
  beforeEach,
  describe,
  expect,
  it,
  vi,
  type Mock,
  type MockInstance,
} from "vitest";

import { QueryClient } from "@tanstack/react-query";

import App from "../App";
import { ApiProblem, api } from "../api/client";
import { engramDetailKey } from "../api/engram";
import type { ChangeEvent } from "../api/events";
import { ME_QUERY_KEY } from "../auth/keys";
import {
  answersFor,
  type Answer,
  domainsResponse,
  meResponse,
  userFixture,
} from "../test/harness";
import type { Director } from "./audio/director";
import { CONNECTED_KEY, DTMF, dialNumber } from "./audio/modem";
import { FakeAudioContext } from "./audio/testContext";
import { primeAudio, releasePrimedAudio, takePrimedAudio } from "./launch";
import { INVERT_KEY, type Session, type SessionOptions } from "./session";
import type { LiftStop, StationAddress } from "./world/types";

vi.mock("../api/client", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../api/client")>();
  return { ...actual, api: vi.fn(), setCsrfToken: vi.fn() };
});

const gl = vi.hoisted(() => ({ available: false }));

vi.mock("./gl/context", async (importOriginal) => {
  const actual = await importOriginal<typeof import("./gl/context")>();
  return {
    ...actual,
    hasWebGL2: () => gl.available,
    createContext: () => ({
      gl: {},
      caps: { color: "rgba8", maxLayers: 256 },
    }),
  };
});

/** Every stub renderer made, and every path a session navigated to. */
const made = vi.hoisted(() => ({
  renderers: [] as {
    setRoom: ReturnType<typeof vi.fn>;
    dispose: ReturnType<typeof vi.fn>;
  }[],
  navigations: [] as string[],
  /** Every session made: disposed or not, and every `setBusy` it was told. */
  sessions: [] as { disposed: boolean; busy: boolean[] }[],
  /** The options every session was created with, as the route passed them. */
  options: [] as SessionOptions[],
  /** The reader's "open in Fluid" handler the route last handed its view. */
  openFluid: null as (() => void) | null,
  /** Every value the real sessions told the route's `onPause`. */
  pauses: [] as boolean[],
}));

/**
 * The kind of fixture a room the session builds puts the player in front
 * of, or null for the room's own spawn: a seam for the Esc precedence test,
 * which needs a terminal and a lift in reach of the route's real session
 * without walking there on a real-time loop.
 */
const facing = vi.hoisted(() => ({
  kind: null as "terminal" | "lift" | null,
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

/**
 * A seam for the one test that needs to report a lift without walking the
 * player to one and facing it (M3 Task 12 fix round 1): when set, the
 * session mock below hands the route this stub instead of a real session,
 * so the test can call the route's own `onLift` and watch `ride` and
 * `closeLift` rather than drive the real fixture-facing input path. Every
 * other test leaves this null and gets the real session, as before.
 */
const sessionStub = vi.hoisted(() => ({
  factory: null as ((opts: SessionOptions) => Session) | null,
}));

// The real view, with the reader's F handler kept, so a test can press it
// without walking up to a terminal.
vi.mock("./ui/StationView", async (importOriginal) => {
  const actual = await importOriginal<typeof import("./ui/StationView")>();
  return {
    ...actual,
    StationView: (props: Parameters<typeof actual.StationView>[0]) => {
      made.openFluid = props.onOpenFluid;
      return actual.StationView(props);
    },
  };
});

vi.mock("./render/renderer", () => ({
  createRenderer: () => {
    const renderer = {
      setRoom: vi.fn(),
      resize: vi.fn(),
      draw: vi.fn(),
      dispose: vi.fn(),
    };
    made.renderers.push(renderer);
    return renderer;
  },
}));

// The real session, with every URL it asks the route for recorded.
vi.mock("./session", async (importOriginal) => {
  const actual = await importOriginal<typeof import("./session")>();
  return {
    ...actual,
    createSession: (opts: Parameters<typeof actual.createSession>[0]) => {
      made.options.push(opts);
      if (sessionStub.factory !== null) {
        const session = sessionStub.factory(opts);
        const entry = { disposed: false, busy: [] as boolean[] };
        made.sessions.push(entry);
        const setBusy = session.setBusy.bind(session);
        session.setBusy = (busy: boolean) => {
          entry.busy.push(busy);
          setBusy(busy);
        };
        return session;
      }
      const session = actual.createSession({
        ...opts,
        navigate: (path: string) => {
          made.navigations.push(path);
          opts.navigate(path);
        },
        onPause: (paused: boolean) => {
          made.pauses.push(paused);
          opts.onPause?.(paused);
        },
      });
      const entry = { disposed: false, busy: [] as boolean[] };
      made.sessions.push(entry);
      const setBusy = session.setBusy.bind(session);
      session.setBusy = (busy: boolean) => {
        entry.busy.push(busy);
        setBusy(busy);
      };
      const dispose = session.dispose.bind(session);
      session.dispose = () => {
        entry.disposed = true;
        dispose();
      };
      return session;
    },
  };
});

/**
 * Every director the route made, every audio context constructed, and
 * every number a director dialled.
 */
const sound = vi.hoisted(() => ({
  directors: [] as import("./audio/director").Director[],
  contexts: [] as import("./audio/testContext").FakeAudioContext[],
  dials: [] as {
    director: import("./audio/director").Director;
    number: string;
  }[],
}));

vi.mock("./audio/director", async (importOriginal) => {
  const actual = await importOriginal<typeof import("./audio/director")>();
  return {
    ...actual,
    createDirector: (...args: Parameters<typeof actual.createDirector>) => {
      const director = actual.createDirector(...args);
      sound.directors.push(director);
      const dial = director.dial.bind(director);
      director.dial = (number: string) => {
        sound.dials.push({ director, number });
        return dial(number);
      };
      return director;
    },
  };
});

/**
 * Stands a recording context in for the browser's `AudioContext`, for the
 * launch's prime and the mixer's own alike; every one made is kept.
 */
function stubAudio() {
  vi.stubGlobal(
    "AudioContext",
    class extends FakeAudioContext {
      constructor() {
        super();
        sound.contexts.push(this);
      }
    },
  );
}

/**
 * The change stream as the route subscribes to it: every listener with its
 * options, and how many unsubscribed.
 */
const stream = vi.hoisted(() => ({
  subs: [] as {
    listener: (event: import("../api/events").ChangeEvent) => void;
    options:
      import("../events/ChangeStreamProvider").StreamSubscription | undefined;
    unsubscribed: boolean;
  }[],
}));

vi.mock("../events/ChangeStreamProvider", async (importOriginal) => {
  const actual =
    await importOriginal<typeof import("../events/ChangeStreamProvider")>();
  return {
    ...actual,
    subscribeToChanges: (
      listener: (typeof stream.subs)[number]["listener"],
      options?: (typeof stream.subs)[number]["options"],
    ) => {
      const sub = { listener, options, unsubscribed: false };
      stream.subs.push(sub);
      return () => {
        sub.unsubscribed = true;
      };
    },
  };
});

const apiMock = vi.mocked(api);

/** A detail payload in the engine's own shape, with one section. */
function detailResponse(permalink: string, title: string) {
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
    relations: [],
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

function serve(routes: Record<string, Answer> = {}) {
  apiMock.mockImplementation(
    answersFor({
      "/auth/me": () => meResponse({ user: userFixture() }),
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

/** The paths the api was asked for, query strings dropped. */
function asked(): string[] {
  return apiMock.mock.calls.map(([path]) => path.split("?")[0] ?? path);
}

/** Hands the test the router's `navigate`, to change the URL under the app. */
function NavProbe({
  onNavigate,
}: {
  onNavigate: (n: NavigateFunction) => void;
}) {
  const navigate = useNavigate();
  useEffect(() => {
    onNavigate(navigate);
  }, [navigate, onNavigate]);
  return null;
}

/**
 * Records the router's pathname and search each time they change, and how
 * the router got there (a push, a replace or a pop).
 */
function LocationProbe() {
  const { pathname, search } = useLocation();
  const type = useNavigationType();
  useEffect(() => {
    location = pathname + search;
    navigationType = type;
  }, [pathname, search, type]);
  return null;
}

let navigate: NavigateFunction | null = null;
let location = "";
let navigationType = "";
const keepNavigate = (n: NavigateFunction) => {
  navigate = n;
};

function renderAt(path: string, strict = false) {
  const tree = (
    <MemoryRouter initialEntries={[path]}>
      <App />
      <NavProbe onNavigate={keepNavigate} />
      <LocationProbe />
    </MemoryRouter>
  );
  return render(strict ? <StrictMode>{tree}</StrictMode> : tree);
}

/** Changes the URL under the app, as a link or the history buttons would. */
function go(to: string | number) {
  const nav = navigate;
  if (nav === null) throw new Error("no navigate");
  act(() => {
    if (typeof to === "number") void nav(to);
    else void nav(to);
  });
}

/** The room the renderer was last handed. */
function lastRoomSpec(): { permalink: string; title: string } | undefined {
  const renderer = made.renderers.at(-1);
  const call = renderer?.setRoom.mock.calls.at(-1) as
    [{ permalink: string; title: string }, unknown] | undefined;
  return call?.[0];
}

/** The permalink of the room the renderer was last handed. */
function lastRoom(): string | undefined {
  return lastRoomSpec()?.permalink;
}

/** How many rooms the newest renderer was handed. */
const roomsEntered = () => made.renderers.at(-1)?.setRoom.mock.calls.length;

/** A tree level in the engine's own wire shape. */
function treeResponse(path: string, permalinks: string[]) {
  return {
    domain: "eng",
    path,
    folders: path === "" ? ["notes"] : [],
    engrams: permalinks.map((permalink) => ({
      permalink,
      title: permalink,
      type: "engram",
      status: "stable",
    })),
    truncated: false,
    total: permalinks.length,
  };
}

/** The tree of `eng`: its MANIFEST at the root, two engrams in `notes`. */
const TREE: Answer = (path) =>
  path.includes("path=notes")
    ? treeResponse("notes", ["notes/one", "notes/two"])
    : treeResponse("", ["manifest"]);

/** A promise and the function that settles it. */
function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((r) => {
    resolve = r;
  });
  return { promise, resolve };
}

/** The `console.error` calls React makes for an update outside `act`. */
function actWarnings() {
  return errors.mock.calls.filter((args) => String(args[0]).includes("act("));
}

/** Settles pending promises and timers the session and the HUD started. */
async function settle(ms = 0) {
  await act(async () => {
    await new Promise((r) => setTimeout(r, ms));
  });
}

let errors: MockInstance<typeof console.error>;

beforeEach(() => {
  apiMock.mockReset();
  navigate = null;
  location = "";
  navigationType = "";
  facing.kind = null;
  made.pauses.length = 0;
  stream.subs.length = 0;
  made.renderers.length = 0;
  made.navigations.length = 0;
  made.sessions.length = 0;
  made.options.length = 0;
  made.openFluid = null;
  gl.available = false;
  // jsdom has no `matchMedia`; the device check and the session's
  // pixel-ratio watch both ask it. A fine pointer and no coarse one: a
  // desktop.
  vi.stubGlobal(
    "matchMedia",
    (query: string) =>
      ({
        matches: query.includes("fine"),
        media: query,
        addEventListener: () => undefined,
        removeEventListener: () => undefined,
      }) as unknown as MediaQueryList,
  );
  errors = vi.spyOn(console, "error");
});

afterEach(() => {
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
  sessionStub.factory = null;
  // The launch keeps the primed context across tests: none leaks on.
  releasePrimedAudio();
  sound.directors.length = 0;
  sound.contexts.length = 0;
  sound.dials.length = 0;
  // The connecting screen shows once per tab session: every test starts a
  // new one.
  window.sessionStorage.clear();
});

/**
 * Types a word on the window by `KeyboardEvent.code`, then waits inside
 * `act` long enough for the loop to tick over it, so the session's
 * `onLevels` state update lands inside `act`.
 */
async function typeWord(word: string) {
  await act(async () => {
    for (const ch of word) {
      const code = `Key${ch.toUpperCase()}`;
      window.dispatchEvent(new KeyboardEvent("keydown", { code, key: ch }));
      window.dispatchEvent(new KeyboardEvent("keyup", { code, key: ch }));
    }
    await new Promise((r) => setTimeout(r, 150));
  });
}

const LEVELS = { name: "Jump to a domain" } as const;
const CONNECTING = { name: "Connecting" } as const;
const LIFT = { name: "Choose a stop" } as const;

/** Two stops, the second the one the lift stands at: enough to pick from. */
function liftStops(): LiftStop[] {
  return [
    { label: "Alpha", to: { kind: "airlock" }, key: false, here: false },
    { label: "Bridge", to: { kind: "airlock" }, key: false, here: true },
  ];
}

/**
 * A stub session for the lift-wiring test: every method not under test is a
 * plain spy, and `ride`/`closeLift` are the two spies the test itself holds
 * (declared before the route is rendered, so reading them back afterward
 * never runs into the narrowing a `let` reassigned only inside this factory
 * would). `closeLift`'s spy still calls `opts.onLift?.(null)` on its way
 * out, exactly what the real session's `closeLift` does, so the overlay
 * this test opened by hand also closes by hand.
 */
function stubSession(
  opts: SessionOptions,
  spies: {
    ride: Mock<(stop: number) => void>;
    closeLift: Mock<() => void>;
  },
): Session {
  return {
    go: vi.fn(),
    showCanned: vi.fn(),
    showRoom: vi.fn(),
    closeReader: vi.fn(),
    jump: vi.fn(),
    closeLevels: vi.fn(),
    ride: spies.ride,
    closeLift: () => {
      spies.closeLift();
      opts.onLift?.(null);
    },
    dispose: vi.fn(),
    pause: vi.fn(),
    resume: vi.fn(),
    setBusy: vi.fn(),
    flash: vi.fn(),
    changed: vi.fn(),
    current: null,
    page: null,
    where: null,
    paused: false,
    lockEndedAt: -Infinity,
  };
}

describe("ExploreRoute", () => {
  it("mounts on the raw π prefix too, not only its encoding", async () => {
    gl.available = true;
    serve();
    const view = renderAt("/π/d/eng/e/alpha");
    await waitFor(() => {
      expect(lastRoom()).toBe("alpha");
    });
    view.unmount();
  });

  it("shows the classic keys first in its key legend", async () => {
    gl.available = true;
    serve();
    const view = renderAt("/%CF%80/d/eng/e/alpha");
    await waitFor(() => {
      expect(lastRoom()).toBe("alpha");
    });
    expect(
      screen.getByText(/^ARROWS MOVE · ALT STRAFE · SPACE USE · SHIFT RUN · /),
    ).toBeInTheDocument();
    view.unmount();
  });

  it("refuses a device without WebGL2", async () => {
    serve();
    renderAt("/%CF%80/d/eng/e/alpha");
    expect(
      await screen.findByText("?DEVICE NOT PRESENT ERROR"),
    ).toBeInTheDocument();
    expect(asked()).not.toContain("/domains/eng/engrams/alpha");
  });

  it("releases the primed sound when it refuses the device (F38)", async () => {
    // Mutation caught: the refusal leaving the context the launch primed
    // open (nothing will ever borrow it).
    let closed = 0;
    class StubContext {
      state = "suspended";
      resume() {
        return Promise.resolve();
      }
      close() {
        closed += 1;
        return Promise.resolve();
      }
    }
    vi.stubGlobal("AudioContext", StubContext);
    try {
      primeAudio();
      serve();
      renderAt("/%CF%80/d/eng/e/alpha");
      await screen.findByText("?DEVICE NOT PRESENT ERROR");
      await waitFor(() => {
        expect(closed).toBe(1);
      });
      expect(takePrimedAudio()).toBeNull();
    } finally {
      vi.unstubAllGlobals();
      releasePrimedAudio();
    }
  });

  it("loads the place in its URL and follows the URL", async () => {
    gl.available = true;
    serve();
    const view = renderAt("/%CF%80/d/eng/e/alpha");

    await waitFor(() => {
      expect(lastRoom()).toBe("alpha");
    });
    expect(screen.queryByText("?DEVICE NOT PRESENT ERROR")).toBeNull();
    // The connector's minimum is up and hidden before the next journey.
    await settle(500);

    go("/%CF%80/d/eng/e/beta");
    await waitFor(() => {
      expect(lastRoom()).toBe("beta");
    });
    await settle(500);
    expect(
      asked().filter((p) => p === "/domains/eng/engrams/beta"),
    ).toHaveLength(1);
    expect(location).toBe("/%CF%80/d/eng/e/beta");
    view.unmount();
    expect(actWarnings()).toEqual([]);
  });

  it("stops for good when unmounted while a load is in flight", async () => {
    gl.available = true;
    const held = deferred<unknown>();
    serve({ "/domains/eng/engrams/alpha": () => held.promise });
    const view = renderAt("/%CF%80/d/eng/e/alpha");
    await waitFor(() => {
      expect(asked()).toContain("/domains/eng/engrams/alpha");
    });

    view.unmount();
    expect(made.renderers).toHaveLength(1);
    expect(made.renderers[0]?.dispose).toHaveBeenCalled();
    const calls = apiMock.mock.calls.length;
    held.resolve(detailResponse("alpha", "Alpha"));
    await settle(500);

    expect(apiMock.mock.calls.length).toBe(calls);
    expect(made.navigations).toEqual([]);
    expect(made.renderers[0]?.setRoom).not.toHaveBeenCalled();
    expect(actWarnings()).toEqual([]);
  });

  it("follows the history back to the room it is in while another loads", async () => {
    gl.available = true;
    const held = deferred<unknown>();
    serve({ "/domains/eng/engrams/beta": () => held.promise });
    const view = renderAt("/%CF%80/d/eng/e/alpha");
    await waitFor(() => {
      expect(lastRoom()).toBe("alpha");
    });
    await settle(500);

    go("/%CF%80/d/eng/e/beta");
    await waitFor(() => {
      expect(asked()).toContain("/domains/eng/engrams/beta");
    });
    go(-1);
    await settle(0);
    held.resolve(detailResponse("beta", "Beta"));
    await settle(500);

    // Beta's load was dropped: the player is in alpha, and so is the URL.
    expect(lastRoom()).toBe("alpha");
    expect(location).toBe("/%CF%80/d/eng/e/alpha");
    expect(made.navigations).not.toContain("/%CF%80/d/eng/e/beta");
    view.unmount();
  });

  it("follows the history forward to a room that failed to load before", async () => {
    gl.available = true;
    let betaMissing = true;
    serve({
      "/domains/eng/engrams/beta": () => {
        if (betaMissing) throw new ApiProblem(404, "not found", "no beta");
        return detailResponse("beta", "Beta");
      },
    });
    const view = renderAt("/%CF%80/d/eng/e/alpha");
    await waitFor(() => {
      expect(lastRoom()).toBe("alpha");
    });
    await settle(500);

    go("/%CF%80/d/eng/e/beta");
    await waitFor(() => {
      expect(asked()).toContain("/domains/eng/engrams/beta");
    });
    await settle(500);
    expect(lastRoom()).toBe("alpha");

    go(-1);
    await settle(500);
    betaMissing = false;
    go(1);
    await waitFor(() => {
      expect(lastRoom()).toBe("beta");
    });
    await settle(500);
    view.unmount();
  });

  it("does not travel again to the room the session itself put in the URL", async () => {
    gl.available = true;
    // The engram answers under its canonical permalink, so the session lands
    // in `alpha` and replaces the URL it was started on with alpha's route.
    serve({
      "/domains/eng/engrams/old-alpha": () => detailResponse("alpha", "Alpha"),
      "/domains/eng/inbound/old-alpha": () => EMPTY_INBOUND,
    });
    const view = renderAt("/%CF%80/d/eng/e/old-alpha");
    await waitFor(() => {
      expect(asked()).toContain("/domains/eng/engrams/old-alpha");
    });
    await settle(500);
    expect(asked()).not.toContain("/domains/eng/engrams/alpha");

    expect(location).toBe("/%CF%80/d/eng/e/alpha");
    expect(lastRoom()).toBe("alpha");

    // A URL the session did not put there is still followed.
    go("/%CF%80/d/eng/e/beta");
    await waitFor(() => {
      expect(lastRoom()).toBe("beta");
    });
    await settle(500);
    view.unmount();
  });
  it("runs exactly one live session under StrictMode's double mount", async () => {
    gl.available = true;
    serve();
    const view = renderAt("/%CF%80/d/eng/e/alpha", true);
    await waitFor(() => {
      expect(lastRoom()).toBe("alpha");
    });
    await settle(500);

    // StrictMode mounts, unmounts and mounts again: two sessions, each with
    // its own renderer, and only the second one still running.
    expect(made.sessions.length).toBeGreaterThanOrEqual(2);
    expect(made.renderers).toHaveLength(made.sessions.length);
    const live = made.sessions.filter((s) => !s.disposed);
    expect(live).toHaveLength(1);
    expect(made.sessions.at(-1)?.disposed).toBe(false);
    made.sessions.forEach((s, i) => {
      const renderer = made.renderers[i];
      if (s.disposed) {
        expect(renderer?.dispose).toHaveBeenCalled();
        expect(renderer?.setRoom).not.toHaveBeenCalled();
      } else {
        expect(renderer?.dispose).not.toHaveBeenCalled();
      }
    });
    // One room, entered once, and the URL replaced once.
    expect(made.renderers.at(-1)?.setRoom).toHaveBeenCalledTimes(1);
    expect(made.navigations).toEqual(["/%CF%80/d/eng/e/alpha"]);
    expect(location).toBe("/%CF%80/d/eng/e/alpha");

    view.unmount();
    expect(made.sessions.every((s) => s.disposed)).toBe(true);
    for (const renderer of made.renderers) {
      expect(renderer.dispose).toHaveBeenCalled();
    }
    expect(actWarnings()).toEqual([]);
  });

  it("hands the session the console room, its listing read from the domains (2.6e C13)", async () => {
    // Mutation caught: the route forgetting the consoleRoom option, or a
    // listing that does not read the domain listing.
    gl.available = true;
    serve();
    const view = renderAt("/%CF%80/d/eng/e/alpha");
    await waitFor(() => {
      expect(lastRoom()).toBe("alpha");
    });
    const consoleRoom = made.options.at(-1)?.consoleRoom;
    expect(consoleRoom).toBeDefined();
    const rows = await consoleRoom?.domains(new AbortController().signal);
    expect(rows).toEqual([{ name: "eng", canonicalName: null, aliases: [] }]);
    expect(rows?.map((r) => r.name)).toEqual(
      domainsResponse().domains.map((d) => d.name),
    );
    await settle(100);
    view.unmount();
  });

  it("opens the level select on idclev and jumps to a domain's bridge", async () => {
    gl.available = true;
    window.localStorage.clear();
    serve({
      "/domains/eng/tree": TREE,
      "/domains/eng/engrams/manifest": () => detailResponse("manifest", "eng"),
      "/domains/eng/inbound/manifest": () => EMPTY_INBOUND,
    });
    const view = renderAt("/%CF%80/d/eng/e/alpha");
    await waitFor(() => {
      expect(lastRoom()).toBe("alpha");
    });
    await settle(500);

    await typeWord("idclev");
    const dialog = await screen.findByRole("dialog", LEVELS);
    const row = await within(dialog).findByRole("option", { selected: true });
    expect(row.firstElementChild?.textContent).toBe("eng");
    expect(within(row).getByText("HERE")).toBeInTheDocument();
    // The word's I was taken back.
    expect(window.localStorage.getItem(INVERT_KEY)).not.toBe("1");

    const field = within(dialog).getByRole("textbox", { name: "Domain name" });
    fireEvent.change(field, { target: { value: "EN" } });
    fireEvent.keyDown(field, { key: "Enter" });
    expect(screen.queryByRole("dialog", LEVELS)).toBeNull();
    await waitFor(() => {
      expect(lastRoom()).toBe("manifest");
    });
    await settle(500);
    expect(location).toBe("/%CF%80/d/eng");
    expect(made.navigations).toEqual([
      "/%CF%80/d/eng/e/alpha",
      "/%CF%80/d/eng",
    ]);
    expect(
      asked().filter((p) => p === "/domains/eng/engrams/manifest"),
    ).toHaveLength(1);
    view.unmount();
    expect(actWarnings()).toEqual([]);
  });

  it("closes the level select on Esc and stays in the room", async () => {
    gl.available = true;
    serve();
    const view = renderAt("/%CF%80/d/eng/e/alpha");
    await waitFor(() => {
      expect(lastRoom()).toBe("alpha");
    });
    await settle(500);

    await typeWord("idclev");
    const dialog = await screen.findByRole("dialog", LEVELS);
    const field = within(dialog).getByRole("textbox", { name: "Domain name" });
    fireEvent.keyDown(field, { key: "Escape" });
    expect(screen.queryByRole("dialog", LEVELS)).toBeNull();
    await settle(300);
    expect(lastRoom()).toBe("alpha");
    expect(location).toBe("/%CF%80/d/eng/e/alpha");
    expect(asked()).not.toContain("/domains/eng/engrams/manifest");

    // The word opens it again.
    await typeWord("idclev");
    expect(await screen.findByRole("dialog", LEVELS)).toBeInTheDocument();
    view.unmount();
  });

  it("closes the level select when the URL changes under it", async () => {
    gl.available = true;
    serve();
    const view = renderAt("/%CF%80/d/eng/e/alpha");
    await waitFor(() => {
      expect(lastRoom()).toBe("alpha");
    });
    await settle(500);
    await typeWord("idclev");
    await screen.findByRole("dialog", LEVELS);

    go("/%CF%80/d/eng/e/beta");
    expect(screen.queryByRole("dialog", LEVELS)).toBeNull();
    await waitFor(() => {
      expect(lastRoom()).toBe("beta");
    });
    await settle(500);
    view.unmount();
  });

  it("mounts the lift overlay from the session's onLift, rides a pick with its stops index, and closes on Esc", async () => {
    // Mutation caught: `onLift: setLift` dropped from the session options
    // (the overlay never appears - `findByRole` below times out), the
    // route's `ride` callback not reaching `session.ride` (the spy sees no
    // call, or the wrong index), and the route's `closeLift` callback not
    // reaching `session.closeLift` (the spy sees no call, and the overlay
    // stays up past Esc). Driven through a stub session (`stubSession`)
    // rather than a real one facing a real lift fixture: reaching a real
    // lift needs the player walked and turned to face it through the
    // movement path, which `session.test.ts`'s `atDeckLift` reaches with a
    // test-only `showRoom` override not available at this route level; the
    // stub reports a lift the way the real session's `openLift` would,
    // through the very `onLift` option this test is proving is wired.
    gl.available = true;
    serve();
    const rideSpy = vi.fn<(stop: number) => void>();
    const closeLiftSpy = vi.fn<() => void>();
    sessionStub.factory = (opts) =>
      stubSession(opts, { ride: rideSpy, closeLift: closeLiftSpy });
    const view = renderAt("/%CF%80");
    await waitFor(() => {
      expect(made.options.length).toBeGreaterThan(0);
    });
    expect(screen.queryByRole("dialog", LIFT)).toBeNull();

    act(() => {
      made.options.at(-1)?.onLift?.({ stops: liftStops(), note: null });
    });
    const dialog = await screen.findByRole("dialog", LIFT);
    const options = within(dialog).getAllByRole("option");
    expect(options.map((o) => o.firstElementChild?.textContent)).toEqual([
      "Alpha",
      "Bridge",
    ]);
    expect(within(options[1]!).getByText("HERE")).toBeInTheDocument();

    // Rides with the clicked row's own index in the stops array (1:
    // "Bridge"); LiftSelect.test.tsx covers a filtered or windowed list's
    // index separately from its on-screen position, not repeated here.
    fireEvent.click(options[1]!);
    expect(rideSpy).toHaveBeenCalledTimes(1);
    expect(rideSpy).toHaveBeenCalledWith(1);

    const field = within(dialog).getByRole("textbox", { name: "Stop name" });
    fireEvent.keyDown(field, { key: "Escape" });
    expect(closeLiftSpy).toHaveBeenCalledTimes(1);
    expect(screen.queryByRole("dialog", LIFT)).toBeNull();
    view.unmount();
  });

  it("follows a location to another address, never to the one the session just entered (M3 C4, C5)", async () => {
    // Mutation caught: the address the URL follows compared by its domain
    // alone (the bridge, in the deck's own domain, would never be gone
    // to), or not compared at all (the session's own replace would start
    // a second journey to the deck it just entered). `section=999` is
    // built to also catch `requestedRef` dropped on the session's own
    // navigate (M3 fix wave, M10c): a merely redundant `section=1` lands
    // exactly where it was asked, so `requestedRef` never has to move for
    // the test to pass; `notes` holds one section, so `999` clamps to a
    // different one (0) when the deck is built, and only an updated
    // `requestedRef` stops the params effect from sending the session
    // there a second time once the URL changes under it.
    gl.available = true;
    serve({
      "/domains/eng/tree": TREE,
      "/domains/eng/engrams/manifest": () => detailResponse("manifest", "Eng"),
      "/domains/eng/inbound/manifest": () => EMPTY_INBOUND,
    });
    const view = renderAt("/%CF%80/d/eng?path=notes&section=999");
    await waitFor(() => {
      expect(lastRoom()).toBe("notes/");
    });
    await settle(500);
    // The route took the replaced URL for the address it had just entered.
    expect(made.navigations).toEqual(["/%CF%80/d/eng?path=notes"]);
    expect(location).toBe("/%CF%80/d/eng?path=notes");
    expect(roomsEntered()).toBe(1);

    go("/%CF%80/d/eng");
    await waitFor(() => {
      expect(lastRoom()).toBe("manifest");
    });
    await settle(500);
    expect(roomsEntered()).toBe(2);

    go(-1);
    await waitFor(() => {
      expect(lastRoom()).toBe("notes/");
    });
    await settle(500);
    expect(roomsEntered()).toBe(3);
    expect(location).toBe("/%CF%80/d/eng?path=notes");
    view.unmount();
  });

  it("starts in the airlock at /π and opens Fluid's front page on F there (M3 C25)", async () => {
    // Mutation caught: the route still only matching an engram's path (the
    // airlock would render Fluid's not-found page), or F reading an engram
    // route off an address that has none.
    gl.available = true;
    serve();
    const open = vi.spyOn(window, "open").mockImplementation(() => null);
    const view = renderAt("/%CF%80");
    await waitFor(() => {
      expect(lastRoomSpec()?.title).toBe("AIRLOCK");
    });
    await settle(300);
    await act(async () => {
      window.dispatchEvent(new KeyboardEvent("keydown", { code: "KeyF" }));
      window.dispatchEvent(new KeyboardEvent("keyup", { code: "KeyF" }));
      await new Promise((r) => setTimeout(r, 150));
    });
    expect(open).toHaveBeenCalledWith("/", "_blank", "noopener");
    view.unmount();
  });

  it("opens a deck's folder page from the reader's F (M3 C25)", async () => {
    // Mutation caught: the reader's F still building an engram route,
    // which a deck has no permalink for.
    gl.available = true;
    serve({ "/domains/eng/tree": TREE });
    const open = vi.spyOn(window, "open").mockImplementation(() => null);
    const view = renderAt("/%CF%80/d/eng?path=notes");
    await waitFor(() => {
      expect(lastRoom()).toBe("notes/");
    });
    await settle(300);
    expect(made.openFluid).not.toBeNull();
    act(() => {
      made.openFluid?.();
    });
    expect(open).toHaveBeenCalledWith(
      "/d/eng?path=notes",
      "_blank",
      "noopener",
    );
    view.unmount();
  });

  it("gives Esc to the overlay that is open and pauses only when none is (Review Focus 2)", async () => {
    // Mutation caught: the session's pause listener without its modal
    // check (every overlay's Esc would pause under it); the pause never
    // reaching the route (no screen on the last Esc). The overlays'
    // `preventDefault` on Esc is the second guard (C9): with the modal
    // check in place it cannot go red on its own, see the report. The
    // connecting screen's can (C9, M4 C26): it clears `busy` before the
    // session hears the Esc, so its `preventDefault` alone keeps that Esc
    // from pausing; mutation caught: the `preventDefault` dropped.
    gl.available = true;
    stubAudio();
    primeAudio();
    serve({ "/domains/eng/tree": TREE });
    const PAUSED = { name: "Paused" } as const;
    const esc = async (target: Element | Window = window) => {
      await act(async () => {
        fireEvent.keyDown(target, { key: "Escape", code: "Escape" });
        await new Promise((r) => setTimeout(r, 100));
      });
    };
    const space = async () => {
      await act(async () => {
        window.dispatchEvent(
          new KeyboardEvent("keydown", { code: "Space", key: " " }),
        );
        await new Promise((r) => setTimeout(r, 150));
        window.dispatchEvent(
          new KeyboardEvent("keyup", { code: "Space", key: " " }),
        );
      });
    };

    // The reader, at the room's terminal.
    facing.kind = "terminal";
    const view = renderAt("/%CF%80/d/eng/e/alpha");
    // The connecting screen, first: an Esc on it skips it and pauses
    // nothing.
    const connecting = await screen.findByRole("dialog", CONNECTING);
    await esc(connecting);
    expect(screen.queryByRole("dialog", CONNECTING)).toBeNull();
    expect(screen.queryByRole("dialog", PAUSED)).toBeNull();
    expect(made.pauses).toEqual([]);
    await waitFor(() => {
      expect(lastRoom()).toBe("alpha");
    });
    await settle(300);
    await space();
    const reader = await screen.findByRole("dialog", { name: /Alpha/ });
    expect(reader).toBeInTheDocument();
    await esc();
    expect(screen.queryByRole("dialog", { name: /Alpha/ })).toBeNull();
    expect(screen.queryByRole("dialog", PAUSED)).toBeNull();
    expect(made.pauses).toEqual([]);

    // The level select.
    await typeWord("idclev");
    const levels = await screen.findByRole("dialog", LEVELS);
    await esc(within(levels).getByRole("textbox", { name: "Domain name" }));
    expect(screen.queryByRole("dialog", LEVELS)).toBeNull();
    expect(screen.queryByRole("dialog", PAUSED)).toBeNull();
    expect(made.pauses).toEqual([]);

    // The lift, on a deck.
    facing.kind = "lift";
    go("/%CF%80/d/eng?path=notes");
    await waitFor(() => {
      expect(lastRoom()).toBe("notes/");
    });
    await settle(300);
    await space();
    const lift = await screen.findByRole("dialog", LIFT);
    await esc(within(lift).getByRole("textbox", { name: "Stop name" }));
    expect(screen.queryByRole("dialog", LIFT)).toBeNull();
    expect(screen.queryByRole("dialog", PAUSED)).toBeNull();
    expect(made.pauses).toEqual([]);

    // Nothing open: the same Esc pauses.
    await esc();
    expect(await screen.findByRole("dialog", PAUSED)).toBeInTheDocument();
    expect(made.pauses).toEqual([true]);
    view.unmount();
  });

  it.each<{
    name: string;
    url: string;
    current: StationAddress | null;
    where: string | null;
    to: string;
  }>([
    {
      name: "an engram's room",
      url: "/%CF%80/d/eng/e/notes/x",
      current: { kind: "engram", domain: "eng", permalink: "notes/x" },
      where: "X",
      to: "/d/eng/e/notes/x",
    },
    {
      name: "a deck",
      url: "/%CF%80/d/eng?path=notes",
      current: { kind: "deck", domain: "eng", folder: "notes", section: 0 },
      where: "NOTES",
      to: "/d/eng?path=notes",
    },
    {
      name: "the airlock",
      url: "/%CF%80",
      current: { kind: "airlock" },
      where: "AIRLOCK",
      to: "/",
    },
    {
      // Inside the console room `current` is the room walked in from,
      // and the room has no label.
      name: "the console room",
      url: "/%CF%80/d/eng/e/alpha",
      current: { kind: "engram", domain: "eng", permalink: "alpha" },
      where: null,
      to: "/d/eng/e/alpha",
    },
    {
      // The player walked on and the stub never replaced the URL: the
      // room the player is in wins over the one the URL names.
      name: "a room the URL has not caught up with",
      url: "/%CF%80/d/eng/e/alpha",
      current: { kind: "engram", domain: "eng", permalink: "beta" },
      where: "BETA",
      to: "/d/eng/e/beta",
    },
    {
      name: "the dark screen before the first room",
      url: "/%CF%80/d/eng/e/notes/x",
      current: null,
      where: null,
      to: "/d/eng/e/notes/x",
    },
  ])(
    "leaves $name to its Fluid page, replacing the entry (M4 C7)",
    async ({ url, current, where, to }) => {
      // Mutation caught: a push in place of the replace, the π prefix
      // kept (`gameRouteOf`), the URL's address preferred over `current`,
      // the `requestedRef` fallback dropped (the dark screen leaves to /).
      gl.available = true;
      serve({ "/domains/eng/tree": TREE });
      let options: SessionOptions | null = null;
      const resume = vi.fn<() => void>();
      sessionStub.factory = (opts) => {
        options = opts;
        return {
          ...stubSession(opts, { ride: vi.fn(), closeLift: vi.fn() }),
          resume,
          current,
          page: current,
          where,
          lockEndedAt: -Infinity,
        };
      };
      const view = renderAt(url);
      await waitFor(() => {
        expect(options).not.toBeNull();
      });
      act(() => {
        options?.onPause?.(true);
      });
      const dialog = await screen.findByRole("dialog", { name: "Paused" });
      expect(
        within(dialog).getByText(
          where === null ? "BREAK" : `BREAK IN ${where}`,
        ),
      ).toBeInTheDocument();
      fireEvent.click(within(dialog).getByRole("button", { name: "CONT" }));
      expect(resume).toHaveBeenCalledTimes(1);

      fireEvent.click(
        within(dialog).getByRole("button", { name: "RUN/STOP (ESC)" }),
      );
      await waitFor(() => {
        expect(location).toBe(to);
      });
      expect(navigationType).toBe("REPLACE");
      expect(screen.queryByRole("dialog", { name: "Paused" })).toBeNull();
      expect(made.sessions).toHaveLength(1);
      view.unmount();
    },
  );

  it("names the room a load landed in while paused (M4 C6a)", async () => {
    // Mutation caught: the screen's label kept from the moment of the
    // pause, while the way out already leads to the room landed in.
    gl.available = true;
    serve();
    let options: SessionOptions | null = null;
    const at = {
      current: { kind: "engram", domain: "eng", permalink: "alpha" },
      where: "ALPHA",
    } as { current: StationAddress; where: string };
    sessionStub.factory = (opts) => {
      options = opts;
      const base = stubSession(opts, { ride: vi.fn(), closeLift: vi.fn() });
      return {
        ...base,
        get current() {
          return at.current;
        },
        get page() {
          return at.current;
        },
        get where() {
          return at.where;
        },
      };
    };
    const view = renderAt("/%CF%80/d/eng/e/alpha");
    await waitFor(() => {
      expect(options).not.toBeNull();
    });
    act(() => {
      options?.onPause?.(true);
    });
    const dialog = await screen.findByRole("dialog", { name: "Paused" });
    expect(within(dialog).getByText("BREAK IN ALPHA")).toBeInTheDocument();
    // A load in flight lands under the pause and replaces the URL.
    at.current = { kind: "engram", domain: "eng", permalink: "beta" };
    at.where = "BETA";
    act(() => {
      options?.navigate("/%CF%80/d/eng/e/beta");
    });
    expect(within(dialog).getByText("BREAK IN BETA")).toBeInTheDocument();
    fireEvent.click(
      within(dialog).getByRole("button", { name: "RUN/STOP (ESC)" }),
    );
    await waitFor(() => {
      expect(location).toBe("/d/eng/e/beta");
    });
    view.unmount();
  });

  it("leaves by the new address of a move the room has not followed yet (M4 C18)", async () => {
    // Mutation caught: the way out reading `current` alone, the old
    // permalink's page, which the move made a 404.
    gl.available = true;
    serve();
    let options: SessionOptions | null = null;
    sessionStub.factory = (opts) => {
      options = opts;
      return {
        ...stubSession(opts, { ride: vi.fn(), closeLift: vi.fn() }),
        current: { kind: "engram", domain: "eng", permalink: "alpha" },
        page: { kind: "engram", domain: "eng", permalink: "notes/alpha" },
        where: "ALPHA",
      };
    };
    const view = renderAt("/%CF%80/d/eng/e/alpha");
    await waitFor(() => {
      expect(options).not.toBeNull();
    });
    act(() => {
      options?.onPause?.(true);
    });
    const dialog = await screen.findByRole("dialog", { name: "Paused" });
    fireEvent.click(
      within(dialog).getByRole("button", { name: "RUN/STOP (ESC)" }),
    );
    await waitFor(() => {
      expect(location).toBe("/d/eng/e/notes/alpha");
    });
    view.unmount();
  });

  it("opens the new address of a move from the reader's F (M4 C18)", async () => {
    // Mutation caught: the reader's F reading `current` alone, which opens
    // the old permalink's page, a 404 since the move.
    gl.available = true;
    serve();
    const open = vi.spyOn(window, "open").mockImplementation(() => null);
    sessionStub.factory = (opts) => ({
      ...stubSession(opts, { ride: vi.fn(), closeLift: vi.fn() }),
      current: { kind: "engram", domain: "eng", permalink: "alpha" },
      page: { kind: "engram", domain: "eng", permalink: "notes/alpha" },
    });
    const view = renderAt("/%CF%80/d/eng/e/alpha");
    await waitFor(() => {
      expect(made.options).toHaveLength(1);
      expect(made.openFluid).not.toBeNull();
    });
    act(() => {
      made.openFluid?.();
    });
    expect(open).toHaveBeenCalledWith(
      "/d/eng/e/notes/alpha",
      "_blank",
      "noopener",
    );
    view.unmount();
  });

  it("invalidates with Fluid's table and passes its identity (M4 C13)", async () => {
    // Mutation caught: no identity passed, the game inbound prefix not
    // invalidated, the mount's reset after the first go or refetching.
    gl.available = true;
    serve();
    const invalidate = vi.spyOn(QueryClient.prototype, "invalidateQueries");
    const go = vi.fn();
    const changed = vi.fn();
    let options: SessionOptions | null = null;
    sessionStub.factory = (opts) => {
      options = opts;
      return {
        ...stubSession(opts, { ride: vi.fn(), closeLift: vi.fn() }),
        go,
        changed,
      };
    };
    const view = renderAt("/%CF%80/d/eng/e/alpha");
    await waitFor(() => {
      expect(go).toHaveBeenCalledTimes(1);
    });
    const client = (options as SessionOptions | null)?.client;
    if (client === null || client === undefined) throw new Error("no client");
    // The mount's one reset, before the first go, marking only.
    const resets = invalidate.mock.calls
      .map((args, i) => ({
        args,
        order: invalidate.mock.invocationCallOrder[i]!,
      }))
      .filter(({ args }) => args[0]?.queryKey === undefined);
    expect(resets).toHaveLength(1);
    expect(resets[0]!.args[0]).toEqual({ refetchType: "none" });
    expect(resets[0]!.order).toBeLessThan(go.mock.invocationCallOrder[0]!);

    expect(stream.subs).toHaveLength(1);
    const sub = stream.subs[0]!;
    await waitFor(() => {
      expect(client.getQueryData(ME_QUERY_KEY)).toBeDefined();
    });
    expect(sub.options?.identity?.held()).toBe(
      client.getQueryData(ME_QUERY_KEY),
    );

    invalidate.mockClear();
    const frame: ChangeEvent = {
      event: "engram",
      change: {
        domain: "eng",
        permalink: "alpha",
        path: "alpha.md",
        kind: "modified",
        from: null,
        checksum: "2",
        actor: null,
        draftOf: null,
      },
    };
    act(() => {
      sub.listener(frame);
    });
    const keys = invalidate.mock.calls.map(([filters]) => filters);
    expect(keys).toContainEqual({
      queryKey: engramDetailKey("eng", "alpha"),
      refetchType: "none",
    });
    expect(keys).toContainEqual({
      queryKey: ["game", "inbound"],
      refetchType: "none",
    });
    for (const filters of keys) expect(filters?.refetchType).toBe("none");
    expect(changed).toHaveBeenCalledWith(frame);
    const lastInvalidation = Math.max(...invalidate.mock.invocationCallOrder);
    expect(changed.mock.invocationCallOrder[0]!).toBeGreaterThan(
      lastInvalidation,
    );

    // The identity's re-check asks the probe again.
    invalidate.mockClear();
    sub.options?.identity?.recheck();
    expect(invalidate).toHaveBeenCalledWith({ queryKey: ME_QUERY_KEY });

    expect(sub.unsubscribed).toBe(false);
    view.unmount();
    expect(sub.unsubscribed).toBe(true);
  });

  it.each(["/%CF%80/dev", "/%CF%80/dev/gallery"])(
    "ignores the word on %s",
    async (path) => {
      gl.available = true;
      window.localStorage.clear();
      serve();
      const view = renderAt(path);
      // The dev screen is a lazy chunk: wait for its session, not a guess.
      await waitFor(() => {
        expect(made.sessions.length).toBeGreaterThan(0);
      });
      await settle(300);
      await typeWord("idclev");
      await settle(200);
      // No dev route mounts LevelSelect, so this holds by construction; the
      // storage check below is the one that actually pins the word being
      // ignored (it fails if a dev host ever passes `onLevels`).
      expect(screen.queryByRole("dialog", LEVELS)).toBeNull();
      // The word's I stays a plain toggle there: nothing takes it back.
      expect(window.localStorage.getItem(INVERT_KEY)).toBe("1");
      view.unmount();
    },
  );

  // Mutation caught: the board's route missing or at another path (the
  // station mounts there instead, in the airlock). Its order among the
  // routes is not pinned: the router ranks by specificity, so it would
  // win over the splat declared anywhere; it sits above it by house style.
  it("serves the sound board at /π/dev/sounds in development", async () => {
    gl.available = true;
    serve();
    const view = renderAt("/%CF%80/dev/sounds");
    expect(
      await screen.findByRole("heading", { name: "SOUND BOARD" }),
    ).toBeInTheDocument();
    expect(made.sessions).toHaveLength(0);
    view.unmount();
  });

  describe("sound (M4 C28)", () => {
    /** Hides or shows the tab as the browser would. */
    function setHidden(hidden: boolean) {
      Object.defineProperty(document, "visibilityState", {
        configurable: true,
        get: () => (hidden ? "hidden" : "visible"),
      });
      document.dispatchEvent(new Event("visibilitychange"));
    }

    afterEach(() => {
      // Back to jsdom's own getter on the prototype.
      Reflect.deleteProperty(document, "visibilityState");
    });

    // Mutation caught: no sound sink handed to the session, no unlock on
    // the first click of a reload (no context ever made), or a context the
    // mixer made left open after the route went.
    it("hands the session the director, makes the sound on the first click and closes it on unmount", async () => {
      gl.available = true;
      stubAudio();
      serve();
      const view = renderAt("/%CF%80");
      await waitFor(() => {
        expect(made.options.length).toBeGreaterThan(0);
      });
      const director: Director | undefined = sound.directors.at(-1);
      expect(director).toBeDefined();
      expect(made.options.at(-1)?.sound).toBe(director);
      // Nothing primed (a reload): no context until a gesture.
      expect(sound.contexts).toHaveLength(0);

      const canvas = document.querySelector("canvas");
      expect(canvas).not.toBeNull();
      fireEvent.click(canvas!);
      expect(sound.contexts).toHaveLength(1);
      const ctx = sound.contexts[0]!;
      expect(ctx.calls).toEqual(["resume"]);
      expect(ctx.state).toBe("running");

      await settle(300);
      view.unmount();
      expect(ctx.calls.at(-1)).toBe("close");
    });

    // Mutation caught: the route's visibility listener left on the document
    // after the unmount (one more per mount, each holding a dead director).
    it("takes its visibility listener down on unmount", async () => {
      gl.available = true;
      stubAudio();
      serve();
      const added = vi.spyOn(document, "addEventListener");
      const removed = vi.spyOn(document, "removeEventListener");
      const view = renderAt("/%CF%80");
      await waitFor(() => {
        expect(made.options.length).toBeGreaterThan(0);
      });
      const listeners = added.mock.calls
        .filter(([type]) => type === "visibilitychange")
        .map(([, listener]) => listener);
      // The route's own and the session's: every one of them goes.
      expect(listeners.length).toBeGreaterThan(0);
      await settle(300);
      view.unmount();
      const gone = removed.mock.calls
        .filter(([type]) => type === "visibilitychange")
        .map(([, listener]) => listener);
      for (const listener of listeners) expect(gone).toContain(listener);
    });

    // Mutation caught: the suspend on the pause missing, the hidden tab
    // left playing, or a resume while one of the two still holds.
    it("suspends while paused or hidden and resumes after both", async () => {
      gl.available = true;
      stubAudio();
      primeAudio();
      const ctx = sound.contexts[0]!;
      expect(ctx.state).toBe("running");
      serve();
      sessionStub.factory = (opts) =>
        stubSession(opts, { ride: vi.fn(), closeLift: vi.fn() });
      const view = renderAt("/%CF%80");
      await waitFor(() => {
        expect(made.options.length).toBeGreaterThan(0);
      });
      const opts = made.options.at(-1)!;
      expect(ctx.state).toBe("running");

      act(() => opts.onPause?.(true));
      expect(ctx.state).toBe("suspended");
      // Shown again under the pause: still quiet.
      act(() => setHidden(true));
      act(() => setHidden(false));
      expect(ctx.state).toBe("suspended");
      act(() => opts.onPause?.(false));
      expect(ctx.state).toBe("running");

      act(() => setHidden(true));
      expect(ctx.state).toBe("suspended");
      // The pause ended while hidden: still quiet until shown.
      act(() => opts.onPause?.(true));
      act(() => opts.onPause?.(false));
      expect(ctx.state).toBe("suspended");
      act(() => setHidden(false));
      expect(ctx.state).toBe("running");
      view.unmount();
    });

    // Mutation caught: the key set when the screen was skipped (a later
    // launch in the tab would never dial in), the screen shown on every
    // mount, or the session not held busy under it.
    it("skips the connecting screen when the context is not running (Review Focus 5)", async () => {
      gl.available = true;
      stubAudio();
      primeAudio();
      const ctx = sound.contexts[0]!;
      // A context the browser has not let run yet.
      ctx.state = "suspended";
      ctx.deferred = true;
      serve();
      const mountAt = async () => {
        const view = renderAt("/%CF%80/d/eng/e/alpha");
        await waitFor(() => {
          expect(made.sessions.length).toBeGreaterThan(0);
        });
        await settle(50);
        return view;
      };
      const first = await mountAt();
      expect(screen.queryByRole("dialog", CONNECTING)).toBeNull();
      expect(window.sessionStorage.getItem(CONNECTED_KEY)).toBeNull();
      expect(sound.dials).toEqual([]);
      expect(made.sessions.at(-1)?.busy).not.toContain(true);
      first.unmount();

      // Running: the screen shows, the key is set, the session is busy
      // and the domain's number is dialled.
      ctx.deferred = false;
      ctx.state = "running";
      const second = await mountAt();
      const dialog = screen.getByRole("dialog", CONNECTING);
      expect(window.sessionStorage.getItem(CONNECTED_KEY)).toBe("1");
      expect(made.sessions.at(-1)?.busy.at(-1)).toBe(true);
      expect(sound.dials.map((d) => d.number)).toEqual([dialNumber("eng")]);
      expect(sound.dials[0]?.director).toBe(sound.directors.at(-1));
      await settle(1_000);
      // Typed out, and handed whole to the screen's live region.
      expect(within(dialog).getAllByText("ENG")).toHaveLength(2);
      // A click skips it and hands the session its keys back.
      fireEvent.click(dialog);
      expect(screen.queryByRole("dialog", CONNECTING)).toBeNull();
      expect(made.sessions.at(-1)?.busy.at(-1)).toBe(false);
      second.unmount();

      // Again in the same tab session: nothing.
      ctx.state = "running";
      const third = await mountAt();
      expect(screen.queryByRole("dialog", CONNECTING)).toBeNull();
      expect(sound.dials).toHaveLength(1);
      third.unmount();
    });

    // Mutation caught: the input taking a key the connecting screen
    // cancelled (the key that skips the screen also acts in the station:
    // F opening a Fluid tab, W walking).
    it("skips the connecting screen on a key sent the moment it appears", async () => {
      // Mutation caught: the skip listener added in a passive effect, a
      // task after the commit that shows the screen: a key in between is
      // lost and the screen stays up (the gap the Esc test above falls
      // into when the machine is loaded).
      gl.available = true;
      stubAudio();
      primeAudio();
      serve();
      let sent: KeyboardEvent | null = null;
      const probe = new MutationObserver(() => {
        if (sent !== null) return;
        if (!document.querySelector('[role="dialog"][aria-label="Connecting"]'))
          return;
        probe.disconnect();
        sent = new KeyboardEvent("keydown", {
          key: "a",
          code: "KeyA",
          bubbles: true,
          cancelable: true,
        });
        act(() => {
          document.body.dispatchEvent(sent!);
        });
      });
      probe.observe(document.body, { childList: true, subtree: true });
      try {
        const view = renderAt("/%CF%80");
        await waitFor(() => {
          expect(sent).not.toBeNull();
        });
        await settle(100);
        expect(sent!.defaultPrevented).toBe(true);
        expect(screen.queryByRole("dialog", CONNECTING)).toBeNull();
        view.unmount();
      } finally {
        probe.disconnect();
      }
    });

    it("takes the key that skips the connecting screen as no command (M4 C26)", async () => {
      gl.available = true;
      stubAudio();
      primeAudio();
      serve();
      const open = vi.spyOn(window, "open").mockImplementation(() => null);
      const view = renderAt("/%CF%80");
      const dialog = await screen.findByRole("dialog", CONNECTING);
      await waitFor(() => {
        expect(lastRoomSpec()?.title).toBe("AIRLOCK");
      });
      await settle(300);
      const pressF = async (target: Element | Window) => {
        await act(async () => {
          target.dispatchEvent(
            new KeyboardEvent("keydown", {
              code: "KeyF",
              key: "f",
              bubbles: true,
              cancelable: true,
            }),
          );
          target.dispatchEvent(
            new KeyboardEvent("keyup", {
              code: "KeyF",
              key: "f",
              bubbles: true,
              cancelable: true,
            }),
          );
          await new Promise((r) => setTimeout(r, 150));
        });
      };
      await pressF(dialog);
      expect(screen.queryByRole("dialog", CONNECTING)).toBeNull();
      expect(open).not.toHaveBeenCalled();
      // The next F is the station's again.
      await pressF(window);
      expect(open).toHaveBeenCalledWith("/", "_blank", "noopener");
      view.unmount();
    });

    // Mutation caught: the gate bypassed (every drop in a flapping minute
    // would hang up and flash), the notice or the hang-up missing, or the
    // carrier heard on a second subscription.
    // Mutation caught: a carrier change played over the dial-in (the
    // hang-up on the modem bus under the handshake, the notice under the
    // screen), or the state held back and never said once the dial ends.
    it("holds a carrier change until the dial-in ends, then says the state it is in (M4 C26, C27)", async () => {
      gl.available = true;
      stubAudio();
      primeAudio();
      const ctx = sound.contexts[0]!;
      serve();
      const flash = vi.fn<(text: string) => void>();
      sessionStub.factory = (opts) => ({
        ...stubSession(opts, { ride: vi.fn(), closeLift: vi.fn() }),
        flash,
      });
      const dialledWith = async (changes: boolean[]) => {
        const view = renderAt("/%CF%80");
        const dialog = await screen.findByRole("dialog", CONNECTING);
        const onCarrier = stream.subs.at(-1)?.options?.onCarrier;
        if (onCarrier === undefined) throw new Error("no carrier callback");
        const carrier = vi.spyOn(sound.directors.at(-1)!, "carrier");
        act(() => {
          for (const up of changes) onCarrier(up);
        });
        const during = { carrier: carrier.mock.calls.length, flash: 0 };
        during.flash = flash.mock.calls.length;
        fireEvent.click(dialog);
        return { view, carrier, during };
      };

      // Down, up and down again under the screen: nothing while it shows,
      // then the one drop it ended in.
      const down = await dialledWith([false, true, false]);
      expect(down.during).toEqual({ carrier: 0, flash: 0 });
      expect(down.carrier.mock.calls).toEqual([[false]]);
      expect(flash.mock.calls).toEqual([["NO CARRIER"]]);
      down.view.unmount();

      // Down and back up under the screen: it ended up, nothing to say.
      window.sessionStorage.clear();
      ctx.state = "running";
      flash.mockClear();
      const up = await dialledWith([false, true]);
      expect(up.during).toEqual({ carrier: 0, flash: 0 });
      expect(up.carrier).not.toHaveBeenCalled();
      expect(flash).not.toHaveBeenCalled();
      up.view.unmount();
    });

    it("says NO CARRIER at most once a minute when the stream drops (M4 C27)", async () => {
      gl.available = true;
      serve();
      const flash = vi.fn<(text: string) => void>();
      sessionStub.factory = (opts) => ({
        ...stubSession(opts, { ride: vi.fn(), closeLift: vi.fn() }),
        flash,
      });
      const view = renderAt("/%CF%80");
      await waitFor(() => {
        expect(stream.subs).toHaveLength(1);
      });
      const sub = stream.subs[0]!;
      expect(sub.options?.identity).toBeDefined();
      const onCarrier = sub.options?.onCarrier;
      if (onCarrier === undefined) throw new Error("no carrier callback");
      const carrier = vi.spyOn(sound.directors.at(-1)!, "carrier");
      act(() => {
        onCarrier(false);
      });
      expect(flash.mock.calls).toEqual([["NO CARRIER"]]);
      expect(carrier.mock.calls).toEqual([[false]]);
      act(() => {
        onCarrier(true);
      });
      expect(carrier.mock.calls).toEqual([[false], [true]]);
      // The second pair inside the minute: nothing.
      act(() => {
        onCarrier(false);
        onCarrier(true);
      });
      expect(flash).toHaveBeenCalledTimes(1);
      expect(carrier).toHaveBeenCalledTimes(2);
      expect(stream.subs).toHaveLength(1);
      view.unmount();
    });

    // Mutation caught: the primed context closed on unmount (StrictMode's
    // second mount would borrow a dead one, F29), or left suspended by the
    // first mount's cleanup so the second never plays (C26's check).
    it("borrows the primed context and never closes it, also under StrictMode (F29, F38)", async () => {
      gl.available = true;
      stubAudio();
      primeAudio();
      const ctx = sound.contexts[0]!;
      serve();
      const view = renderAt("/%CF%80/d/eng/e/alpha", true);
      await waitFor(() => {
        expect(lastRoom()).toBe("alpha");
      });
      await settle(300);
      // Two mounts, two directors, one context: the first mount's cleanup
      // suspended it and the second resumed it.
      expect(sound.directors.length).toBeGreaterThanOrEqual(2);
      expect(sound.contexts).toHaveLength(1);
      expect(ctx.calls).toContain("suspend");
      expect(ctx.calls).not.toContain("close");
      expect(ctx.state).toBe("running");
      expect(takePrimedAudio()).toBe(ctx);
      // The connecting screen stands over the second mount, and it is that
      // mount's session that is held busy and its director that dials (M4
      // C26, F41); mutation caught: the screen's state not carried over the
      // remount (the second mount finds the key set and neither holds the
      // session nor dials).
      expect(screen.getByRole("dialog", CONNECTING)).toBeInTheDocument();
      expect(made.sessions.at(-1)?.busy.at(-1)).toBe(true);
      expect(sound.dials.at(-1)?.director).toBe(sound.directors.at(-1));

      view.unmount();
      expect(ctx.calls).not.toContain("close");
      expect(ctx.state).toBe("suspended");
      expect(takePrimedAudio()).toBe(ctx);
    });

    it("plays the second mount's dial once its context runs again, under StrictMode (F29)", async () => {
      // Mutation caught: the dial played as a plain one-shot, which the
      // director drops while the context is not running: the first
      // mount's cleanup suspended it, the second mount's resume is still
      // settling, and the dial-in goes silent in development.
      gl.available = true;
      stubAudio();
      primeAudio();
      const ctx = sound.contexts[0]!;
      // As the browser traced it: the cleanup's suspend lands at once, the
      // second mount's resume settles later.
      const suspend = ctx.suspend.bind(ctx);
      ctx.suspend = () => {
        ctx.deferred = false;
        const done = suspend();
        ctx.deferred = true;
        return done;
      };
      const tones = new Set(Object.values(DTMF).flat());
      const dialTones = () =>
        ctx
          .ofKind("oscillator")
          .filter((o) => tones.has(o.frequency.events[0]?.[1] ?? -1)).length;
      serve();
      const view = renderAt("/%CF%80/d/eng/e/alpha", true);
      await waitFor(() => {
        expect(sound.dials).toHaveLength(2);
      });
      const first = dialTones();
      expect(first).toBeGreaterThan(0);
      expect(ctx.state).toBe("suspended");
      await act(async () => {
        ctx.settle();
        await Promise.resolve();
      });
      expect(ctx.state).toBe("running");
      expect(dialTones()).toBe(2 * first);
      view.unmount();
    });
  });
});
