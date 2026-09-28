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

import App from "../App";
import { ApiProblem, api } from "../api/client";
import {
  answersFor,
  type Answer,
  domainsResponse,
  meResponse,
  userFixture,
} from "../test/harness";
import { primeAudio, releasePrimedAudio, takePrimedAudio } from "./launch";
import { INVERT_KEY, type Session, type SessionOptions } from "./session";
import type { LiftStop } from "./world/types";

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
  sessions: [] as { disposed: boolean }[],
  /** The options every session was created with, as the route passed them. */
  options: [] as SessionOptions[],
  /** The reader's "open in Fluid" handler the route last handed its view. */
  openFluid: null as (() => void) | null,
}));

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
        made.sessions.push({ disposed: false });
        return session;
      }
      const session = actual.createSession({
        ...opts,
        navigate: (path: string) => {
          made.navigations.push(path);
          opts.navigate(path);
        },
      });
      const entry = { disposed: false };
      made.sessions.push(entry);
      const dispose = session.dispose.bind(session);
      session.dispose = () => {
        entry.disposed = true;
        dispose();
      };
      return session;
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

/** Records the router's pathname and search each time they change. */
function LocationProbe() {
  const { pathname, search } = useLocation();
  useEffect(() => {
    location = pathname + search;
  }, [pathname, search]);
  return null;
}

let navigate: NavigateFunction | null = null;
let location = "";
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
    current: null,
    where: null,
  };
}

describe("GameRoute", () => {
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

    // A pick rides with its own index in the stops array (1: "Bridge"),
    // not a position in whatever the overlay currently shows.
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
    // a second journey to the deck it just entered).
    gl.available = true;
    serve({
      "/domains/eng/tree": TREE,
      "/domains/eng/engrams/manifest": () => detailResponse("manifest", "Eng"),
      "/domains/eng/inbound/manifest": () => EMPTY_INBOUND,
    });
    // `section=1` is the first section spelt out: the session lands there
    // and replaces the URL with the deck's own spelling, which leaves the
    // section out, so the location changes under the route.
    const view = renderAt("/%CF%80/d/eng?path=notes&section=1");
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
});
