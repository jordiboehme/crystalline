/**
 * The game route, mounted in the whole app on an in-memory history.
 *
 * jsdom has no WebGL2, so the route first has to refuse the device the way
 * the look demo does; that pins that the route exists in development and
 * turns a device away before it draws a canvas. Then the probe is told
 * WebGL2 is there, and the context and the renderer are stubbed at their
 * modules, so the session's default factory runs without a GPU: the route
 * loads the place in its URL, loads the next one when the URL changes
 * under it (history included, also while a load is still in flight), and
 * stops asking the server anything once it is unmounted.
 */

import { act, render, screen, waitFor } from "@testing-library/react";
import { useEffect } from "react";
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
}));

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
    createSession: (opts: Parameters<typeof actual.createSession>[0]) =>
      actual.createSession({
        ...opts,
        navigate: (path: string) => {
          made.navigations.push(path);
          opts.navigate(path);
        },
      }),
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

/** Records the router's pathname each time it changes. */
function LocationProbe() {
  const { pathname } = useLocation();
  useEffect(() => {
    location = pathname;
  }, [pathname]);
  return null;
}

let navigate: NavigateFunction | null = null;
let location = "";
const keepNavigate = (n: NavigateFunction) => {
  navigate = n;
};

function renderAt(path: string) {
  return render(
    <MemoryRouter initialEntries={[path]}>
      <App />
      <NavProbe onNavigate={keepNavigate} />
      <LocationProbe />
    </MemoryRouter>,
  );
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

/** The permalink of the room the renderer was last handed. */
function lastRoom(): string | undefined {
  const renderer = made.renderers.at(-1);
  const call = renderer?.setRoom.mock.calls.at(-1) as
    [{ permalink: string }, unknown] | undefined;
  return call?.[0].permalink;
}

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
});

describe("GameRoute", () => {
  it("refuses a device without WebGL2", async () => {
    serve();
    renderAt("/game/d/eng/e/alpha");
    expect(
      await screen.findByText("?DEVICE NOT PRESENT ERROR"),
    ).toBeInTheDocument();
    expect(asked()).not.toContain("/domains/eng/engrams/alpha");
  });

  it("loads the place in its URL and follows the URL", async () => {
    gl.available = true;
    serve();
    const view = renderAt("/game/d/eng/e/alpha");

    await waitFor(() => {
      expect(lastRoom()).toBe("alpha");
    });
    expect(screen.queryByText("?DEVICE NOT PRESENT ERROR")).toBeNull();
    // The connector's minimum is up and hidden before the next journey.
    await settle(500);

    go("/game/d/eng/e/beta");
    await waitFor(() => {
      expect(lastRoom()).toBe("beta");
    });
    await settle(500);
    expect(
      asked().filter((p) => p === "/domains/eng/engrams/beta"),
    ).toHaveLength(1);
    expect(location).toBe("/game/d/eng/e/beta");
    view.unmount();
    expect(actWarnings()).toEqual([]);
  });

  it("stops for good when unmounted while a load is in flight", async () => {
    gl.available = true;
    const held = deferred<unknown>();
    serve({ "/domains/eng/engrams/alpha": () => held.promise });
    const view = renderAt("/game/d/eng/e/alpha");
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
    const view = renderAt("/game/d/eng/e/alpha");
    await waitFor(() => {
      expect(lastRoom()).toBe("alpha");
    });
    await settle(500);

    go("/game/d/eng/e/beta");
    await waitFor(() => {
      expect(asked()).toContain("/domains/eng/engrams/beta");
    });
    go(-1);
    await settle(0);
    held.resolve(detailResponse("beta", "Beta"));
    await settle(500);

    // Beta's load was dropped: the player is in alpha, and so is the URL.
    expect(lastRoom()).toBe("alpha");
    expect(location).toBe("/game/d/eng/e/alpha");
    expect(made.navigations).not.toContain("/game/d/eng/e/beta");
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
    const view = renderAt("/game/d/eng/e/alpha");
    await waitFor(() => {
      expect(lastRoom()).toBe("alpha");
    });
    await settle(500);

    go("/game/d/eng/e/beta");
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
    const view = renderAt("/game/d/eng/e/old-alpha");
    await waitFor(() => {
      expect(asked()).toContain("/domains/eng/engrams/old-alpha");
    });
    await settle(500);
    expect(asked()).not.toContain("/domains/eng/engrams/alpha");

    expect(location).toBe("/game/d/eng/e/alpha");
    expect(lastRoom()).toBe("alpha");

    // A URL the session did not put there is still followed.
    go("/game/d/eng/e/beta");
    await waitFor(() => {
      expect(lastRoom()).toBe("beta");
    });
    await settle(500);
    view.unmount();
  });
});
